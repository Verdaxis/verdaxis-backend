#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::orders::{contract_router, order_etag};

const KEY: &str = "synthetic-cancel-test-signing-key-at-least-32";
struct Fixture {
    admin: PgConnection,
    router: Router,
    order: Uuid,
    actor: Uuid,
    member: Uuid,
    stranger: Uuid,
    organization: Uuid,
    context: Uuid,
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
            router: contract_router(pool, KEY),
            order: ids[0],
            actor: ids[1],
            member: ids[2],
            stranger: ids[3],
            organization: ids[4],
            context: ids[5],
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
        let now = chrono::Utc::now().timestamp();
        let token = encode(&Header::default(), &json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"cancel-tests","aud":"cancel-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"));
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(etag) = etag {
            request = request.header("If-Match", etag);
        }
        if let Some(context) = context {
            request = request.header("X-Verdaxis-Market-Support-Context", context.to_string());
        }
        let data = body.map(|b| b.to_string()).unwrap_or_default();
        if !data.is_empty() {
            request = request.header("content-type", "application/json");
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(data)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            bytes.len(),
        )
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
    assert_eq!(after["version"], 2);
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
        ("assisted", "stranger", "POST", "current", 403),
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
    assert_eq!(
        owned["fame_terms"]["certificate_number"],
        "private-fixture-reference"
    );
    assert_eq!(owned["port_id"], Value::Null);
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
            "rejected" | "pending" | "password_required" => 403,
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
    for (change, expected) in [
        ("context_valid", 204),
        ("context_customer_rejected", 204),
        ("context_foreign", 404),
        ("context_expired", 409),
        ("context_revoked", 409),
        ("context_capability_revoked", 409),
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
        let mut statuses = vec![a.0.as_u16(), b.0.as_u16()];
        statuses.sort_unstable();
        assert_eq!(
            statuses,
            if key.is_some() {
                vec![204, 204]
            } else {
                vec![204, 400]
            }
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
