#![cfg(feature = "contract-tests")]

use std::io::Write;
use std::process::{Command, Stdio};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::orders::{order_etag, served_contract_router};

const KEY: &str = "synthetic-cancel-test-signing-key-at-least-32";
const CONTEXT_HEADER: &str = "X-Verdaxis-Market-Support-Context";
struct Fixture {
    admin: PgConnection,
    pool: PgPool,
    router: Router,
    order: Uuid,
    actor: Uuid,
    member: Uuid,
    stranger: Uuid,
    organization: Uuid,
    context: Uuid,
    product_code: String,
}

/// One response from the served router.
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}
impl Reply {
    fn code(&self) -> &str {
        self.body["detail"]["code"].as_str().unwrap_or_default()
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

fn token(actor: Uuid) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"cancel-tests","aud":"cancel-tests"});
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap()
}

/// Send one request with exactly these headers through `router`.
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    actor: Option<Uuid>,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(actor) = actor {
        request = request.header("authorization", format!("Bearer {}", token(actor)));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let data = body.map(|b| b.to_string()).unwrap_or_default();
    if !data.is_empty() {
        request = request.header("content-type", "application/json");
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(data)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    Reply {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// Python's response-schema dump of stored order terms (the client contract).
fn python_order_terms(model: &str, terms: &Value) -> Value {
    let script = format!(
        "import json, sys\nfrom pydantic import TypeAdapter\nfrom app.schemas.fame_order import FameOrderTerms\nfrom app.schemas.lng_order import LngOrderTerms\nadapter = TypeAdapter({model})\nprint(json.dumps(adapter.dump_python(adapter.validate_python(json.load(sys.stdin)), mode='json')))"
    );
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "venv/bin/python".into());
    let mut child = Command::new(python)
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
        .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
        .args(["-c", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the project Python interpreter runs");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(terms.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

impl Fixture {
    async fn new() -> Self {
        let admin_url =
            std::env::var("RUST_CANCEL_ADMIN_DATABASE_URL").expect("disposable administrator URL");
        let runtime_url =
            std::env::var("RUST_CANCEL_DATABASE_URL").expect("disposable runtime URL");
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.path().ends_with("_test"));
        assert_eq!(
            (a.host_str(), a.port(), a.path()),
            (r.host_str(), r.port(), r.path())
        );
        assert_ne!(a.username(), r.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_effects.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_cancel.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT effect_fixture_reset()")
            .execute(&mut admin)
            .await
            .unwrap();
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::new_v4()).collect();
        let policy: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let p = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
            .unwrap();
        sqlx::query("SELECT cancel_fixture_seed($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(ids[0])
            .bind(ids[1])
            .bind(ids[2])
            .bind(ids[3])
            .bind(ids[4])
            .bind(ids[5])
            .bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .bind(p["lanes"][0].as_str().unwrap().parse::<Uuid>().unwrap())
            .bind(p["code"].as_str().unwrap())
            .bind(p["specification_standard"].as_str().unwrap_or("ISO 8217"))
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            admin,
            router: served_contract_router(pool.clone(), KEY, true),
            pool,
            order: ids[0],
            actor: ids[1],
            member: ids[2],
            stranger: ids[3],
            organization: ids[4],
            context: ids[5],
            product_code: p["code"].as_str().unwrap().to_owned(),
        }
    }
    async fn change(&mut self, change: &str) {
        sqlx::query("SELECT cancel_fixture_change($1,$2,$3)")
            .bind(self.order)
            .bind(self.context)
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn state(&mut self) -> Value {
        sqlx::query_scalar("SELECT cancel_fixture_state($1)")
            .bind(self.order)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    #[allow(clippy::too_many_arguments)]
    async fn request(
        &self,
        method: &str,
        path: &str,
        actor: Uuid,
        body: Option<Value>,
        key: Option<&str>,
        etag: Option<&str>,
        context: Option<Uuid>,
    ) -> (StatusCode, Value, usize) {
        let context = context.map(|context| context.to_string());
        let mut headers = Vec::new();
        if let Some(key) = key {
            headers.push(("Idempotency-Key", key));
        }
        if let Some(etag) = etag {
            headers.push(("If-Match", etag));
        }
        if let Some(context) = context.as_deref() {
            headers.push((CONTEXT_HEADER, context));
        }
        let reply = send(&self.router, method, path, Some(actor), &headers, body).await;
        let length = if reply.body.is_null() {
            0
        } else {
            reply.body.to_string().len()
        };
        (reply.status, reply.body, length)
    }
    async fn cancel(&self, actor: Uuid, key: Option<&str>) -> (StatusCode, Value, usize) {
        self.request(
            "DELETE",
            &format!("/api/orderbook/{}", self.order),
            actor,
            None,
            key,
            None,
            None,
        )
        .await
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_releases_only_the_unfilled_remainder() {
    let mut f = Fixture::new().await;
    let before = f.state().await;
    let result = f.cancel(f.actor, None).await;
    assert_eq!(result.0, StatusCode::NO_CONTENT);
    assert_eq!(result.2, 0);
    let after = f.state().await;
    assert_eq!(after["current"], "6");
    assert_eq!(after["reserved"], "4");
    assert_eq!(after["remaining"], "6");
    assert_eq!(after["status"], "CANCELLED");
    assert_eq!(after["version"], 1);
    assert_eq!(after["audit"]["version"], 1);
    assert_eq!(after["trade"], before["trade"]);
    assert_eq!(after["audit_count"], 1);
    assert_eq!(after["event_count"], 1);
    assert_eq!(after["notification_count"], 0);
    assert_eq!(after["audit"]["reason"], Value::Null);
    assert_eq!(
        after["audit"]["effective_organization_id"],
        f.organization.to_string()
    );
    assert_eq!(after["participants"], json!([f.organization]));
    assert_eq!(after["pin_status"], "CANCELLED");
    assert_eq!(after["before_status"], "PARTIALLY_FILLED");
    assert_eq!(after["after_status"], "CANCELLED");
    assert_eq!(after["observed_at_matches_update"], true);
    assert_eq!(after["payload"]["market_product"], f.product_code);
    assert_eq!(after["payload"]["scope"], "DELIVERY_POINT");
    assert!(after["payload"]["delivery_point_id"].is_string());
    assert!(after["payload"]["delivery_point"].is_string());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_owner_and_etag_matrix() {
    for (change, actor_kind, method, etag_kind, expected) in [
        ("ordinary", "member", "DELETE", "none", 403),
        ("ordinary", "stranger", "DELETE", "none", 403),
        ("legacy", "member", "DELETE", "none", 204),
        ("detached", "owner", "DELETE", "none", 204),
        ("assisted", "member", "POST", "none", 428),
        ("assisted", "member", "POST", "stale", 412),
        ("assisted", "member", "POST", "current", 204),
        ("assisted", "member", "POST", "unknown", 412),
        ("assisted", "member", "POST", "any", 400),
        ("assisted", "member", "POST", "blank", 400),
        ("assisted", "stranger", "POST", "current", 403),
        ("ordinary", "owner", "DELETE", "any", 204),
        ("ordinary", "owner", "DELETE", "unknown", 204),
    ] {
        let mut f = Fixture::new().await;
        f.change(change).await;
        let actor = match actor_kind {
            "member" => f.member,
            "stranger" => f.stranger,
            _ => f.actor,
        };
        let etag = match etag_kind {
            "current" => Some(order_etag(f.order, 1)),
            "stale" => Some(order_etag(f.order, 9)),
            "unknown" => Some("W/\"x\"".to_owned()),
            "any" => Some("*".to_owned()),
            "blank" => Some(" ".to_owned()),
            _ => None,
        };
        let before = f.state().await;
        let path = format!(
            "/api/orderbook/{}{}",
            f.order,
            if method == "POST" { "/cancel" } else { "" }
        );
        let got = f
            .request(
                method,
                &path,
                actor,
                (method == "POST").then(|| json!({"reason":"Customer request"})),
                None,
                etag.as_deref(),
                None,
            )
            .await;
        assert_eq!(
            got.0.as_u16(),
            expected,
            "{change}/{actor_kind}/{etag_kind}: {:?}",
            got.1
        );
        if expected != 204 {
            assert_eq!(f.state().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_replay_returns_stored_204() {
    let mut f = Fixture::new().await;
    assert_eq!(
        f.cancel(f.actor, Some("same-command")).await.0,
        StatusCode::NO_CONTENT
    );
    f.change("rejected").await;
    let committed = f.state().await;
    assert_eq!(
        f.cancel(f.actor, Some("same-command")).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.state().await, committed);
    let changed = f
        .request(
            "DELETE",
            &format!("/api/orderbook/{}", f.order),
            f.actor,
            Some(json!({"reason":"Different request"})),
            Some("same-command"),
            None,
            None,
        )
        .await;
    assert_eq!(changed.0, StatusCode::CONFLICT);
    assert_eq!(f.state().await, committed);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_of_a_filled_order_is_400() {
    let mut f = Fixture::new().await;
    f.change("filled").await;
    let before = f.state().await;
    assert_eq!(f.cancel(f.actor, None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(f.state().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_orders_are_private_and_uuid_stable() {
    let mut f = Fixture::new().await;
    f.change("owner_rows").await;
    let response = f
        .request(
            "GET",
            "/api/orderbook/my?skip=0&limit=100",
            f.actor,
            None,
            None,
            None,
            None,
        )
        .await;
    assert_eq!(response.0, StatusCode::OK, "{:?}", response.1);
    let rows = response.1.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let mut ids: Vec<_> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    let actual = ids.clone();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(actual, ids);
    for row in rows {
        assert_eq!(row["organization_id"], f.organization.to_string());
        assert!(row["etag"].is_string());
        assert!(row["terms_digest"].is_string());
    }
    let owned = rows
        .iter()
        .find(|r| r["id"] == f.order.to_string())
        .unwrap();
    assert_eq!(owned["trade_count"], 1);
    assert_eq!(owned["creation_method"], "SELF_SERVICE");
    assert_eq!(owned["off_spec_notes"], "Private owner instructions");
    assert_eq!(owned["fame_terms"], Value::Null);
    assert_eq!(owned["port_id"], Value::Null);
    for row in rows {
        assert_eq!(row["tier_label"], "INDEPENDENT");
    }
    let page = f
        .request(
            "GET",
            "/api/orderbook/my?skip=1&limit=1",
            f.actor,
            None,
            None,
            None,
            None,
        )
        .await;
    assert_eq!(page.1, json!([rows[1]]));
    f.change("tier").await;
    let tiered = f
        .request("GET", "/api/orderbook/my", f.actor, None, None, None, None)
        .await;
    for row in tiered.1.as_array().unwrap() {
        assert_eq!(row["tier_label"], "MAJOR_TRADER");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn latest_ask_template_excludes_expired() {
    let mut f = Fixture::new().await;
    f.change("templates").await;
    let result = f
        .request(
            "GET",
            "/api/orderbook/my/latest-ask-template",
            f.actor,
            None,
            None,
            None,
            None,
        )
        .await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["price_per_mt_usd"], "100");
    assert_eq!(result.1["off_spec"], false);
    assert_eq!(result.1["off_spec_notes"], Value::Null);
    f.change("all_expired").await;
    let result = f
        .request(
            "GET",
            "/api/orderbook/my/latest-ask-template",
            f.actor,
            None,
            None,
            None,
            None,
        )
        .await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1, Value::Null);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cleanup_and_owner_read_admission_matrix() {
    for change in [
        "rejected",
        "pending",
        "unverified",
        "password_required",
        "kyc_rejected",
        "organization_rejected",
        "detached",
    ] {
        let mut f = Fixture::new().await;
        f.change(change).await;
        let read = f
            .request("GET", "/api/orderbook/my", f.actor, None, None, None, None)
            .await;
        let expected = match change {
            "rejected" => 401,
            "pending" | "password_required" => 403,
            "detached" => 400,
            _ => 200,
        };
        assert_eq!(read.0.as_u16(), expected, "{change}: {:?}", read.1);
        assert_eq!(
            f.cancel(f.actor, None).await.0,
            StatusCode::NO_CONTENT,
            "{change}"
        );
    }
    for change in ["password_cutoff", "revocation_cutoff"] {
        let mut f = Fixture::new().await;
        f.change(change).await;
        let before = f.state().await;
        assert_eq!(f.cancel(f.actor, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(f.state().await, before);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn assisted_context_is_actor_bound_and_cleanup_uses_no_customer_kyc() {
    for (change, expected, code) in [
        ("context_valid", 204, ""),
        ("context_customer_rejected", 204, ""),
        ("context_foreign", 404, "MARKET_SUPPORT_CONTEXT_INVALID"),
        ("context_expired", 409, "MARKET_SUPPORT_CONTEXT_INVALID"),
        ("context_revoked", 409, "MARKET_SUPPORT_CONTEXT_INVALID"),
        ("context_capability_revoked", 404, "NOT_FOUND"),
        ("context_other_org", 403, "ORDER_OWNER_FORBIDDEN"),
    ] {
        let mut f = Fixture::new().await;
        f.change("assisted").await;
        f.change(change).await;
        let before = f.state().await;
        let got = f
            .request(
                "POST",
                &format!("/api/orderbook/{}/cancel", f.order),
                f.actor,
                Some(json!({"reason":"Customer request"})),
                None,
                Some(&order_etag(f.order, 1)),
                Some(f.context),
            )
            .await;
        assert_eq!(got.0.as_u16(), expected, "{change}: {:?}", got.1);
        if expected != 204 {
            assert_eq!(got.1["detail"]["code"], code, "{change}");
            assert_eq!(f.state().await, before);
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_cancel_releases_once_and_same_key_replays() {
    for key in [None, Some("parallel-command")] {
        let mut f = Fixture::new().await;
        let (a, b) = tokio::join!(f.cancel(f.actor, key), f.cancel(f.actor, key));
        let mut outcomes = [a, b];
        outcomes.sort_by_key(|outcome| outcome.0.as_u16());
        // Exactly one request commits the cancellation. The racing twin either
        // replays the stored receipt, reads the already terminal order, or
        // reaches the 500 ms lock timeout while the winner still holds the
        // party and slice locks; every outcome still releases the order once.
        assert_eq!(outcomes[0].0, StatusCode::NO_CONTENT);
        let twin = &outcomes[1];
        let code = twin.1["detail"]["code"].as_str().unwrap_or_default();
        let expected: &[(u16, &str)] = if key.is_some() {
            &[(204, ""), (409, "MARKET_BUSY")]
        } else {
            &[(400, "ORDER_TERMINAL"), (409, "MARKET_BUSY")]
        };
        assert!(
            expected
                .iter()
                .any(|(status, expected_code)| twin.0.as_u16() == *status && code == *expected_code),
            "unexpected concurrent twin outcome {} {}",
            twin.0.as_u16(),
            code,
        );
        let state = f.state().await;
        assert_eq!(state["current"], "6");
        assert_eq!(state["reserved"], "4");
        assert_eq!(state["audit_count"], 1);
        assert_eq!(state["event_count"], 1);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancellation_effect_failure_rolls_back_stock_and_receipt() {
    let mut f = Fixture::new().await;
    f.change("audit_failure").await;
    let before = f.state().await;
    assert_eq!(
        f.cancel(f.actor, Some("atomic-command")).await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(f.state().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_waits_for_the_declared_slice_on_an_independent_connection() {
    let mut f = Fixture::new().await;
    let mut holder =
        PgConnection::connect(&std::env::var("RUST_CANCEL_ADMIN_DATABASE_URL").unwrap())
            .await
            .unwrap();
    let key: i64 = sqlx::query_scalar("SELECT cancel_fixture_slice_key($1)")
        .bind(f.order)
        .fetch_one(&mut holder)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT cancel_fixture_hold($1)")
        .bind(key)
        .execute(&mut holder)
        .await
        .unwrap();
    let router = f.router.clone();
    let actor = f.actor;
    let order = f.order;
    let task = tokio::spawn(async move {
        let now = chrono::Utc::now().timestamp();
        let token=encode(&Header::default(),&json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"cancel-tests","aud":"cancel-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/orderbook/{order}"))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });
    let mut observed = false;
    for _ in 0..100 {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND usename='rust_app' AND wait_event_type='Lock' AND pid<>pg_backend_pid())").fetch_one(&mut f.admin).await.unwrap();
        if waiting {
            observed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(
        observed,
        "the independent request must wait on the held slice"
    );
    assert_eq!(f.state().await["status"], "PARTIALLY_FILLED");
    sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
    assert_eq!(task.await.unwrap(), StatusCode::NO_CONTENT);
    assert_eq!(f.state().await["current"], "6");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_receipt_resource_and_actor_are_bound() {
    let mut f = Fixture::new().await;
    assert_eq!(
        f.cancel(f.actor, Some("bound-command")).await.0,
        StatusCode::NO_CONTENT
    );
    f.change("owner_rows").await;
    let other: Uuid = sqlx::query_scalar(
        "SELECT id FROM orderbook_orders WHERE organization_id=$1 AND id<>$2 LIMIT 1",
    )
    .bind(f.organization)
    .bind(f.order)
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    let before = f.state().await;
    let response = f
        .request(
            "DELETE",
            &format!("/api/orderbook/{other}"),
            f.actor,
            None,
            Some("bound-command"),
            None,
            None,
        )
        .await;
    assert_eq!(response.0, StatusCode::CONFLICT);
    assert_eq!(response.1["detail"]["code"], "RECEIPT_RESOURCE_MISMATCH");
    assert_eq!(
        f.cancel(f.member, Some("bound-command")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.state().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_read_keeps_private_typed_declarations() {
    let mut f = Fixture::new().await;
    f.change("typed_owner").await;
    let response = f
        .request("GET", "/api/orderbook/my", f.actor, None, None, None, None)
        .await;
    assert_eq!(response.0, StatusCode::OK);
    let row = &response.1[0];
    assert_eq!(
        row["fame_terms"]["certificate_reference"],
        "PRIVATE-CERTIFICATE-ID"
    );
    assert_eq!(row["fame_terms"]["batch_reference"], "PRIVATE-BATCH-001");
    assert_eq!(
        row["fame_terms"]["quality_evidence"]["laboratory"],
        "PRIVATE-LABORATORY"
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn served_support_mode_classification_matches_python() {
    let mut f = Fixture::new().await;
    f.change("assisted").await;
    let before = f.state().await;
    let context = f.context.to_string();
    let ctx = context.as_str();
    let order = f.order;
    let upper = order.to_string().to_uppercase();
    // Python classifies any non-empty header value; validity is checked later.
    for (method, path, header, refused) in [
        ("POST", format!("/api/orderbook/{upper}/cancel"), ctx, false),
        ("POST", format!("/api/orderbook/{order}/cancel/"), ctx, false),
        ("POST", format!("/api/admin/market-support/contexts/{context}/exit"), ctx, false),
        ("POST", format!("/api/admin/market-support/contexts/{context}/exit/"), ctx, false),
        ("POST", "/api/orderbook".to_owned(), ctx, false),
        ("POST", "/api/orderbook/".to_owned(), ctx, false),
        ("POST", "/orderbook".to_owned(), ctx, false),
        ("GET", "/api/orderbook/my".to_owned(), ctx, false),
        ("DELETE", format!("/api/orderbook/{order}"), ctx, true),
        ("DELETE", format!("/api/orderbook/{order}"), "not-a-context", true),
        ("PUT", format!("/api/orderbook/{order}"), ctx, true),
        ("POST", format!("/api/orderbook/{order}/cancel/extra"), ctx, true),
        ("POST", "/api/trades/".to_owned(), ctx, true),
        ("PATCH", "/api/notifications/read-all".to_owned(), ctx, true),
    ] {
        let reply = send(&f.router, method, &path, Some(f.actor), &[(CONTEXT_HEADER, header)], None).await;
        let classified_refusal = reply.status == StatusCode::FORBIDDEN
            && reply.code() == "MARKET_SUPPORT_MUTATION_NOT_ALLOWED";
        assert_eq!(classified_refusal, refused, "{method} {path}: {:?}", reply.body);
    }
    assert_eq!(f.state().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn served_assisted_cancel_reaches_the_handler() {
    let mut f = Fixture::new().await;
    f.change("assisted").await;
    let before = f.state().await;
    let context = f.context.to_string();
    let etag = order_etag(f.order, 1);
    let headers = [(CONTEXT_HEADER, context.as_str()), ("If-Match", etag.as_str())];
    let reason = json!({"reason": "Customer request"});
    let deleted = send(
        &f.router,
        "DELETE",
        &format!("/api/orderbook/{}", f.order),
        Some(f.actor),
        &headers,
        Some(reason.clone()),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::FORBIDDEN);
    assert_eq!(deleted.code(), "MARKET_SUPPORT_MUTATION_NOT_ALLOWED");
    assert_eq!(f.state().await, before);
    let posted = send(
        &f.router,
        "POST",
        &format!("/api/orderbook/{}/cancel", f.order),
        Some(f.actor),
        &headers,
        Some(reason),
    )
    .await;
    assert_eq!(posted.status, StatusCode::NO_CONTENT, "{:?}", posted.body);
    let after = f.state().await;
    assert_eq!(after["status"], "CANCELLED");
    assert_eq!(after["version"], 2);
    assert_eq!(after["audit"]["version"], 2);
    assert_eq!(after["audit"]["support_context_id"], context);
    assert_eq!(after["audit"]["reason"], "Customer request");
    assert_eq!(after["audit_count"], 1);
    assert_eq!(after["event_count"], 1);
    assert_eq!(after["participants"], json!([f.organization]));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_bumps_the_version_only_where_python_does() {
    let mut f = Fixture::new().await;
    assert_eq!(f.cancel(f.actor, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(f.state().await["version"], 1);
    let mut b100 = Fixture::new().await;
    b100.change("typed_owner").await;
    assert_eq!(
        b100.cancel(b100.actor, None).await.0,
        StatusCode::NO_CONTENT
    );
    let after = b100.state().await;
    assert_eq!(after["version"], 2);
    assert_eq!(after["audit"]["version"], 2);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_reads_take_no_market_write_locks() {
    let mut f = Fixture::new().await;
    let mut holder =
        PgConnection::connect(&std::env::var("RUST_CANCEL_ADMIN_DATABASE_URL").unwrap())
            .await
            .unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(f.organization)
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM orderbook_orders WHERE organization_id=$1 FOR UPDATE")
        .bind(f.organization)
        .execute(&mut holder)
        .await
        .unwrap();
    let (list, template) = tokio::join!(
        f.request("GET", "/api/orderbook/my", f.actor, None, None, None, None),
        f.request(
            "GET",
            "/api/orderbook/my/latest-ask-template",
            f.actor,
            None,
            None,
            None,
            None
        ),
    );
    assert_eq!(list.0, StatusCode::OK, "{:?}", list.1);
    assert_eq!(template.0, StatusCode::OK, "{:?}", template.1);
    sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
    assert_eq!(f.state().await["status"], "PARTIALLY_FILLED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_of_an_order_without_a_delivery_point_is_204() {
    let mut f = Fixture::new().await;
    f.change("no_point").await;
    let result = f.cancel(f.actor, None).await;
    assert_eq!(result.0, StatusCode::NO_CONTENT, "{:?}", result.1);
    let after = f.state().await;
    assert_eq!(after["status"], "CANCELLED");
    assert_eq!(after["current"], "6");
    assert_eq!(after["reserved"], "4");
    assert_eq!(after["audit_count"], 1);
    assert_eq!(after["event_count"], 1);
    assert_eq!(after["payload"]["delivery_point_id"], Value::Null);
    assert_eq!(after["payload"]["delivery_point"], Value::Null);
    assert_eq!(after["payload"]["region"], "");
    assert_eq!(after["payload"]["scope"], "UNKNOWN");
    assert_eq!(after["payload"]["market_product"], f.product_code);
    assert_eq!(after["observed_at_matches_update"], true);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_refusals_follow_python_status_and_order() {
    let missing = format!("/api/orderbook/{}", Uuid::new_v4());
    // Python validates the token and account before the order lookup.
    let mut f = Fixture::new().await;
    f.change("password_cutoff").await;
    let cut_off = send(&f.router, "DELETE", &missing, Some(f.actor), &[], None).await;
    assert_eq!(cut_off.status, StatusCode::UNAUTHORIZED);
    // The order lookup comes before the POST reason rule.
    let f = Fixture::new().await;
    let reasonless = send(&f.router, "POST", &format!("{missing}/cancel"), Some(f.actor), &[], None).await;
    assert_eq!(reasonless.status, StatusCode::NOT_FOUND);
    assert_eq!(reasonless.code(), "ORDER_NOT_FOUND");
    // A malformed path identifier is a 422 validation error, as FastAPI's.
    let malformed = send(&f.router, "DELETE", "/api/orderbook/not-a-uuid", Some(f.actor), &[], None).await;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(malformed.code(), "VALIDATION_FAILED");
    // An invalid Idempotency-Key is a 400.
    let order = format!("/api/orderbook/{}", f.order);
    let key = send(&f.router, "DELETE", &order, Some(f.actor), &[("Idempotency-Key", " ")], None).await;
    assert_eq!(key.status, StatusCode::BAD_REQUEST);
    // A blank context header is no context: the owner cancels self-service.
    let mut f = Fixture::new().await;
    let blank = send(
        &f.router,
        "POST",
        &format!("/api/orderbook/{}/cancel", f.order),
        Some(f.actor),
        &[(CONTEXT_HEADER, " ")],
        Some(json!({"reason": "Owner request"})),
    )
    .await;
    assert_eq!(blank.status, StatusCode::NO_CONTENT, "{:?}", blank.body);
    assert_eq!(f.state().await["status"], "CANCELLED");
    // An administrator without a context meets the retired workspace route.
    let mut f = Fixture::new().await;
    f.change("admin_owner").await;
    let before = f.state().await;
    let admin = f.cancel(f.actor, None).await;
    assert_eq!(admin.0, StatusCode::GONE);
    assert_eq!(admin.1["detail"]["code"], "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED");
    assert_eq!(f.state().await, before);
    // Assisted orders: the reason rule precedes ownership, and the context
    // header must name a context.
    let mut f = Fixture::new().await;
    f.change("assisted").await;
    let before = f.state().await;
    let stranger = f.cancel(f.stranger, None).await;
    assert_eq!(stranger.0, StatusCode::BAD_REQUEST, "{:?}", stranger.1);
    let cancel_path = format!("/api/orderbook/{}/cancel", f.order);
    let etag = order_etag(f.order, 1);
    let invalid = send(
        &f.router,
        "POST",
        &cancel_path,
        Some(f.actor),
        &[(CONTEXT_HEADER, "not-a-context"), ("If-Match", etag.as_str())],
        Some(json!({"reason": "Customer request"})),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.code(), "MARKET_SUPPORT_CONTEXT_INVALID");
    assert_eq!(
        invalid.header("X-Verdaxis-Market-Support-Context-Invalid"),
        Some("true")
    );
    assert_eq!(
        invalid.header("X-Verdaxis-Market-Support-Context-Code"),
        Some("MARKET_SUPPORT_CONTEXT_INVALID")
    );
    let disabled = served_contract_router(f.pool.clone(), KEY, false);
    let context = f.context.to_string();
    let off = send(
        &disabled,
        "POST",
        &cancel_path,
        Some(f.actor),
        &[(CONTEXT_HEADER, context.as_str()), ("If-Match", etag.as_str())],
        Some(json!({"reason": "Customer request"})),
    )
    .await;
    assert_eq!(off.status, StatusCode::NOT_FOUND);
    assert_eq!(off.code(), "MARKET_SUPPORT_CONTEXT_INVALID");
    assert_eq!(f.state().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_reads_validate_like_python() {
    let f = Fixture::new().await;
    for query in ["skip=abc", "limit=0", "limit=101", "skip=-1"] {
        let reply = send(&f.router, "GET", &format!("/api/orderbook/my?{query}"), Some(f.actor), &[], None).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(reply.code(), "VALIDATION_FAILED", "{query}");
    }
    // Owner reads do not read command headers.
    let headers = [("Idempotency-Key", " "), ("If-Match", "*")];
    for path in ["/api/orderbook/my", "/api/orderbook/my/latest-ask-template"] {
        let reply = send(&f.router, "GET", path, Some(f.actor), &headers, None).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}: {:?}", reply.body);
    }
    let unauthenticated = send(&f.router, "GET", "/api/orderbook/my?skip=abc", None, &[], None).await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_rows_use_the_python_response_schema() {
    let mut f = Fixture::new().await;
    f.change("owner_rows").await;
    f.change("legacy_window").await;
    f.change("typed_owner").await;
    let stored: Value = sqlx::query_scalar("SELECT fame_terms::jsonb FROM orderbook_orders WHERE id=$1")
        .bind(f.order)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let expected_terms = python_order_terms("FameOrderTerms", &stored);
    let response = f
        .request("GET", "/api/orderbook/my", f.actor, None, None, None, None)
        .await;
    assert_eq!(response.0, StatusCode::OK, "{:?}", response.1);
    let rows = response.1.as_array().unwrap();
    let owned = rows
        .iter()
        .find(|r| r["id"] == f.order.to_string())
        .unwrap();
    // Private declarations include every declared optional field.
    assert_eq!(owned["fame_terms"], expected_terms);
    assert_eq!(owned["fame_terms"]["astm_grade"], Value::Null);
    assert_eq!(owned["lng_terms"], Value::Null);
    for row in rows {
        for field in ["created_at", "updated_at"] {
            let text = row[field].as_str().unwrap();
            assert!(text.ends_with('Z'), "{field}: {text}");
            chrono::DateTime::parse_from_rfc3339(text).unwrap();
        }
        if row["id"] != f.order.to_string() {
            assert_eq!(row["availability_window"], "2026-Q3");
        }
    }
    // The owner row's digest is the public projection's digest of the same order.
    let sql = format!(
        "SELECT {} FROM orderbook_orders o WHERE o.id=$1",
        verdaxis_read::market::visibility::private_digest_input_sql("o")
    );
    let input: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(f.order)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let digest = verdaxis_read::digest_terms(&verdaxis_read::canonical_terms_from_row(&input).unwrap()).unwrap();
    assert_eq!(owned["terms_digest"], digest);
    let template = f
        .request("GET", "/api/orderbook/my/latest-ask-template", f.actor, None, None, None, None)
        .await;
    assert_eq!(template.0, StatusCode::OK, "{:?}", template.1);
    assert_eq!(template.1["fame_terms"], expected_terms);
}
