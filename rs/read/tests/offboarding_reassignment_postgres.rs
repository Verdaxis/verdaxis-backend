//! PostgreSQL and cross-stack checks of the administrator obligation list and
//! the responsible-person reassignment.
//!
//! Each case clones a migrated template with converged runtime grants:
//!
//! - `RUST_OFFBOARDING_REASSIGNMENT_ADMIN_DATABASE_URL`: the fixture owner;
//! - `RUST_OFFBOARDING_REASSIGNMENT_DATABASE_URL`: the runtime role;
//! - `RUST_OFFBOARDING_REASSIGNMENT_TEMPLATE_DATABASE`: the template name;
//! - `RUST_TEMPLATE_ADMIN_DATABASE_URL`: the role that creates the clones.
//!
//! `python3`). Run with `--include-ignored --test-threads=1`.
#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::market_reassignment::{AUDIT_ACTION, EVENT_TYPE};
use verdaxis_read::routes::market_write::{freeze, trade_lifecycle};
use verdaxis_read::routes::reassignment;
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;

const KEY: &str = "synthetic-reassignment-signing-key-at-least-32";
const REASSIGNMENT_ISSUER: &str = "reassignment-tests";
const LIFECYCLE_ISSUER: &str = "lifecycle-tests";
const FREEZE_ISSUER: &str = "freeze-tests";
/// The Python settings require a distinct issuer and audience.
const CASE: &str = "CASE-2026/0042";

const BUYER_ORG: u128 = 0x1000;
const SELLER_ORG: u128 = 0x2000;
const OTHER_ORG: u128 = 0x3000;
const BUYER: u128 = 0x1001;
const MEMBER: u128 = 0x1002;
const SUPPLIER_IN_BUYER_ORG: u128 = 0x1003;
const SELLER: u128 = 0x2001;
const REPLACEMENT: u128 = 0x2002;
const BUYER_ROLE_IN_SELLER_ORG: u128 = 0x2003;
const PENDING: u128 = 0x2004;
const MUST_CHANGE_PASSWORD: u128 = 0x2005;
const UNVERIFIED: u128 = 0x2006;
const STRANGER: u128 = 0x3001;
const OTHER_SUPPLIER: u128 = 0x3002;
const ADMIN: u128 = 0x4001;
const SECOND_ADMIN: u128 = 0x4002;
const UNKNOWN: u128 = 0x9999;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn token(actor: u128, issuer: &str) -> String {
    token_for(actor, issuer, issuer)
}

fn token_for(actor: u128, issuer: &str, audience: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    encode(
        &Header::default(),
        &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":issuer,"aud":audience}),
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap()
}

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

fn reassign_body(side: &str, replacement: u128, expected: u128) -> Value {
    json!({
        "side": side,
        "replacement_user_id": id(replacement),
        "evidence_case_ref": CASE,
        "expected_current_user_id": id(expected),
    })
}

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<String>,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = body.map(|value| value.to_string()).unwrap_or_default();
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A runtime pool whose sessions carry a name, so a test can see which
/// command waits for which lock.
async fn tagged_pool(url: &str, tag: &str) -> PgPool {
    let tag = tag.to_owned();
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            let tag = tag.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('application_name', $1, false)")
                    .bind(tag)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

struct Fixture {
    database: std::sync::Arc<market_database::MarketDatabase>,
    admin: PgConnection,
    observer: PgPool,
    trade: Uuid,
    /// The first administrator's command pool (`reassign-a`).
    reassign_a: Router,
    /// A second command pool (`reassign-b`) for a concurrent administrator.
    reassign_b: Router,
    lifecycle: Router,
    freeze: Router,
}

impl Fixture {
    async fn new() -> Self {
        let database = std::sync::Arc::new(
            market_database::MarketDatabase::new("RUST_OFFBOARDING_REASSIGNMENT").await,
        );
        let admin_url = url::Url::parse(&database.admin_url).unwrap();
        let runtime_url = url::Url::parse(&database.runtime_url).unwrap();
        assert_eq!(admin_url.host_str(), Some("127.0.0.1"));
        assert!(admin_url.path().ends_with("_test"));
        assert_eq!(
            (admin_url.host_str(), admin_url.port(), admin_url.path()),
            (
                runtime_url.host_str(),
                runtime_url.port(),
                runtime_url.path()
            )
        );
        assert_ne!(admin_url.username(), runtime_url.username());
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        for sql in [
            include_str!("fixtures/phase9_trades.sql"),
            include_str!("fixtures/phase9_confirm.sql"),
            include_str!("fixtures/phase9_settlement.sql"),
            include_str!("fixtures/offboarding_reassignment.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
        }
        let policy: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["qualification_profile"] == "scalar"
                    && p["execution_mode"] == "ORDERBOOK"
                    && p.get("specification_standard").is_none()
            })
            .unwrap();
        sqlx::query("SELECT trades_fixture_seed($1,$2)")
            .bind(product["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .bind(
                product["lanes"][0]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            )
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('confirm.policy',$1,false)")
            .bind(policy["products"].to_string())
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT confirm_fixture_pending('BUYER',false)")
            .execute(&mut admin)
            .await
            .unwrap();
        let trade = sqlx::query_scalar("SELECT settlement_fixture_confirmed()")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT reassignment_fixture_people()")
            .execute(&mut admin)
            .await
            .unwrap();
        let runtime = database.runtime_url.clone();
        let observer = PgPoolOptions::new()
            .max_connections(2)
            .connect(&database.admin_url)
            .await
            .unwrap();
        let reassign_a =
            reassignment::contract_router(tagged_pool(&runtime, "reassign-a").await, KEY);
        let reassign_b =
            reassignment::contract_router(tagged_pool(&runtime, "reassign-b").await, KEY);
        let lifecycle =
            trade_lifecycle::contract_router(tagged_pool(&runtime, "lifecycle").await, KEY, true);
        let freeze = freeze::contract_router(tagged_pool(&runtime, "freeze").await, KEY);
        Self {
            database,
            admin,
            observer,
            trade,
            reassign_a,
            reassign_b,
            lifecycle,
            freeze,
        }
    }

    async fn sql(&mut self, sql: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn assisted(&mut self) {
        self.sql("SELECT confirm_fixture_assisted(); SELECT confirm_fixture_pending('BUYER',false); SELECT settlement_fixture_confirmed()").await;
        self.trade = sqlx::query_scalar("SELECT id FROM trades")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
    }

    async fn clone_trade(&mut self, state: &str) -> Uuid {
        sqlx::query_scalar("SELECT reassignment_fixture_trade($1,$2)")
            .bind(self.trade)
            .bind(state)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn change(&mut self, user: u128, change: &str) {
        sqlx::query("SELECT trades_fixture_change($1,$2)")
            .bind(id(user))
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn force_side(&mut self, trade: Uuid, side: &str, person: u128) {
        sqlx::query("SELECT reassignment_fixture_force_side($1,$2,$3)")
            .bind(trade)
            .bind(side)
            .bind(id(person))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT reassignment_fixture_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn trade_row(&mut self, trade: Uuid) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(t) FROM trades t WHERE id=$1")
            .bind(trade)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn side_person(&mut self, trade: Uuid, column: &str) -> Option<Uuid> {
        self.trade_row(trade).await[column]
            .as_str()
            .map(|text| text.parse().unwrap())
    }

    async fn list(&self, actor: u128, subject: &str, query: &str) -> (StatusCode, Value) {
        send(
            &self.reassign_a,
            "GET",
            &format!("/api/auth/admin/users/{subject}/offboarding-trades{query}"),
            Some(token(actor, REASSIGNMENT_ISSUER)),
            None,
            &[],
        )
        .await
    }

    async fn reassign(&self, actor: u128, trade: Uuid, body: Value) -> (StatusCode, Value) {
        reassign_with(&self.reassign_a, actor, trade, body).await
    }

    async fn deliver(&self, trade: Uuid, actor: u128, key: Option<&str>) -> (StatusCode, Value) {
        let headers: Vec<(&str, &str)> = key
            .map(|k| vec![("idempotency-key", k)])
            .unwrap_or_default();
        send(
            &self.lifecycle,
            "PUT",
            &format!("/api/trades/{trade}/deliver"),
            Some(token(actor, LIFECYCLE_ISSUER)),
            Some(json!({"final_quantity_mt":"3","final_price_per_mt":"100"})),
            &headers,
        )
        .await
    }

    async fn pay(&self, trade: Uuid, actor: u128) -> (StatusCode, Value) {
        send(
            &self.lifecycle,
            "POST",
            &format!("/api/trades/{trade}/pay"),
            Some(token(actor, LIFECYCLE_ISSUER)),
            None,
            &[],
        )
        .await
    }

    async fn freeze_trade(&self, actor: u128, trade: Uuid) -> (StatusCode, Value) {
        freeze_with(&self.freeze, actor, trade).await
    }

    /// Wait until a named session is blocked on a lock in a statement that
    /// contains `fragment`.
    async fn wait_for(&self, application: &str, fragment: &str) {
        let observed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                     AND application_name=$1 AND wait_event_type='Lock' AND position($2 in query)>0)",
                )
                .bind(application)
                .bind(fragment)
                .fetch_one(&self.observer)
                .await
                .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(
            observed.is_ok(),
            "{application} never waited in a statement containing {fragment:?}"
        );
    }

    /// A separate owner session that closes and opens the fixture gate.
    async fn gatekeeper(&mut self, target: &str) -> PgConnection {
        sqlx::query("SELECT reassignment_fixture_gate_install($1)")
            .bind(target)
            .execute(&mut self.admin)
            .await
            .unwrap();
        let mut gate = PgConnection::connect(&self.database.admin_url)
            .await
            .unwrap();
        sqlx::query("SELECT reassignment_fixture_gate_close()")
            .execute(&mut gate)
            .await
            .unwrap();
        gate
    }

    async fn release(&mut self, mut gate: PgConnection) {
        let opened: bool = sqlx::query_scalar("SELECT reassignment_fixture_gate_open()")
            .fetch_one(&mut gate)
            .await
            .unwrap();
        assert!(opened);
        gate.close().await.unwrap();
    }

    async fn remove_gate(&mut self) {
        self.sql("SELECT reassignment_fixture_gate_remove()").await;
    }
}

async fn reassign_with(
    router: &Router,
    actor: u128,
    trade: Uuid,
    body: Value,
) -> (StatusCode, Value) {
    send(
        router,
        "POST",
        &format!("/api/auth/admin/trades/{trade}/reassign-responsible"),
        Some(token(actor, REASSIGNMENT_ISSUER)),
        Some(body),
        &[],
    )
    .await
}

async fn freeze_with(router: &Router, actor: u128, trade: Uuid) -> (StatusCode, Value) {
    send(
        router,
        "POST",
        &format!("/api/admin/trades/{trade}/freeze"),
        Some(token(actor, FREEZE_ISSUER)),
        Some(json!({"reason_code":"PAYMENT_DISPUTE","note":"Payment review"})),
        &[],
    )
    .await
}

fn without(mut value: Value, key: &str) -> Value {
    value.as_object_mut().unwrap().remove(key);
    value
}

fn entries(snapshot: &Value, key: &str) -> Vec<Value> {
    snapshot[key].as_array().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The unchanged Python application.
// ---------------------------------------------------------------------------

/// One Python settlement request: name, method, trade action and actor.
struct PythonCall {
    name: &'static str,
    trade: Uuid,
    action: &'static str,
    actor: u128,
}

/// Send the calls in order through the Python application and return each
/// call's `{status, body}` by name.
async fn python_settle(database_url: &str, calls: &[PythonCall]) -> Value {
    let mut results = serde_json::Map::new();
    for call in calls {
        let (method, body) = if call.action == "pay" {
            ("POST", None)
        } else {
            (
                "PUT",
                Some(json!({"final_quantity_mt":"3","final_price_per_mt":"100"})),
            )
        };
        let router = trade_lifecycle::contract_router(
            tagged_pool(database_url, "settlement").await,
            KEY,
            true,
        );
        let (status, body) = send(
            &router,
            method,
            &format!("/api/trades/{}/{}", call.trade, call.action),
            Some(token(call.actor, "lifecycle-tests")),
            body,
            &[],
        )
        .await;
        results.insert(
            call.name.to_owned(),
            json!({"status":status.as_u16(),"body":body}),
        );
    }
    Value::Object(results)
}

fn python_status(results: &Value, name: &str) -> u64 {
    results[name]["status"]
        .as_u64()
        .unwrap_or_else(|| panic!("no Python result {name}: {results}"))
}

// ---------------------------------------------------------------------------
// Cases.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn list_only_confirmed_or_delivered_for_subject_is_admin_only() {
    let mut f = Fixture::new().await;
    let confirmed = f.trade;
    let delivered = f.clone_trade("DELIVERED").await;
    let frozen = f.clone_trade("CONFIRMED").await;
    sqlx::query("SELECT reassignment_fixture_freeze($1)")
        .bind(frozen)
        .execute(&mut f.admin)
        .await
        .unwrap();
    for state in ["PENDING_CONFIRMATION", "PAID", "CANCELLED", "DECLINED"] {
        f.clone_trade(state).await;
    }
    let colleague = f.clone_trade("CONFIRMED").await;
    f.force_side(colleague, "SELLER", REPLACEMENT).await;
    let before = f.snapshot().await;

    let (status, body) = f.list(ADMIN, &id(SELLER).to_string(), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user_id"], id(SELLER).to_string());
    assert_eq!(body["total"], 3);
    assert_eq!(
        (body["skip"].clone(), body["limit"].clone()),
        (json!(0), json!(100))
    );
    let items = body["items"].as_array().unwrap();
    let mut listed: Vec<Uuid> = items
        .iter()
        .map(|item| item["trade_id"].as_str().unwrap().parse().unwrap())
        .collect();
    listed.sort();
    let mut expected = vec![confirmed, delivered, frozen];
    expected.sort();
    assert_eq!(
        listed, expected,
        "only confirmed and delivered sides of the person"
    );
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "frozen",
                "manual_resolution_required",
                "recorded_organization_id",
                "responsible_user_id",
                "side",
                "status",
                "trade_id"
            ],
            "the list carries no counterparty, case or reason field"
        );
        let trade: Uuid = item["trade_id"].as_str().unwrap().parse().unwrap();
        assert_eq!(item["side"], "SELLER");
        assert_eq!(item["recorded_organization_id"], id(SELLER_ORG).to_string());
        assert_eq!(item["responsible_user_id"], id(SELLER).to_string());
        assert_eq!(item["frozen"], trade == frozen);
        assert_eq!(item["manual_resolution_required"], false);
        let expected_status = if trade == delivered {
            "DELIVERED"
        } else {
            "CONFIRMED"
        };
        assert_eq!(item["status"], expected_status);
    }
    // The buyer's sides of the same trades, the colleague's trade included.
    let (status, body) = f.list(ADMIN, &id(BUYER).to_string(), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 4);
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["side"], "BUYER");
        assert_eq!(item["recorded_organization_id"], id(BUYER_ORG).to_string());
        assert_eq!(item["responsible_user_id"], id(BUYER).to_string());
    }
    // Paging is bounded and complete.
    let (_, page) = f
        .list(ADMIN, &id(SELLER).to_string(), "?limit=1&skip=1")
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 3);
    let (_, page) = f.list(ADMIN, &id(SELLER).to_string(), "?skip=3").await;
    assert_eq!(page["items"], json!([]));
    for query in ["?limit=0", "?limit=101", "?skip=-1"] {
        let (status, body) = f.list(ADMIN, &id(SELLER).to_string(), query).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query} {body}");
    }
    // Authority before validation and lookup: no caller learns whether a person exists.
    for actor in [SELLER, BUYER, STRANGER] {
        for subject in [
            id(SELLER).to_string(),
            id(UNKNOWN).to_string(),
            "not-a-uuid".into(),
        ] {
            let (status, body) = f.list(actor, &subject, "").await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(code(&body), "FORBIDDEN");
        }
    }
    let (status, body) = f.list(ADMIN, &id(UNKNOWN).to_string(), "").await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::NOT_FOUND, "USER_NOT_FOUND")
    );
    let (status, _) = f.list(ADMIN, "not-a-uuid", "").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let path = format!("/api/auth/admin/users/{}/offboarding-trades", id(SELLER));
    let (status, _) = send(&f.reassign_a, "GET", &path, None, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(
        &f.reassign_a,
        "GET",
        &path,
        Some(token(ADMIN, REASSIGNMENT_ISSUER)),
        None,
        &[(
            "x-verdaxis-market-support-context",
            "00000000-0000-0000-0000-000000006000",
        )],
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::FORBIDDEN, "MARKET_SUPPORT_MUTATION_NOT_ALLOWED")
    );
    // Reading the list writes nothing.
    assert_eq!(f.snapshot().await, before);
    // A rejected non-administrator is refused on both routes.
    f.change(STRANGER, "rejected").await;
    let (status, body) = f.list(STRANGER, &id(SELLER).to_string(), "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    f.change(SELLER, "detached").await;
    let (status, body) = f
        .reassign(
            STRANGER,
            confirmed,
            reassign_body("SELLER", REPLACEMENT, SELLER),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        f.side_person(confirmed, "seller_user_id").await,
        Some(id(SELLER))
    );
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn reassign_requires_same_recorded_org_and_role() {
    let mut f = Fixture::new().await;
    let trade = f.trade;
    let buyer_trade = f.clone_trade("CONFIRMED").await;
    let pending = f.clone_trade("PENDING_CONFIRMATION").await;
    let paid = f.clone_trade("PAID").await;
    // The seller has left the recorded organization.
    f.change(SELLER, "detached").await;
    let before = f.snapshot().await;
    let valid = reassign_body("SELLER", REPLACEMENT, SELLER);

    // Authentication, then authority, then validation, then lookup.
    let path = format!("/api/auth/admin/trades/{trade}/reassign-responsible");
    let (status, _) = send(&f.reassign_a, "POST", &path, None, Some(valid.clone()), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(
        &f.reassign_a,
        "POST",
        &path,
        Some(token(ADMIN, REASSIGNMENT_ISSUER)),
        Some(valid.clone()),
        &[(
            "x-verdaxis-market-support-context",
            "00000000-0000-0000-0000-000000006000",
        )],
    )
    .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::FORBIDDEN, "MARKET_SUPPORT_MUTATION_NOT_ALLOWED")
    );
    for actor in [SELLER, REPLACEMENT, BUYER, STRANGER] {
        for (target, body) in [
            (trade, valid.clone()),
            (id(UNKNOWN), valid.clone()),
            (trade, json!({"side":"SELLER"})),
        ] {
            let (status, response) = f.reassign(actor, target, body).await;
            assert_eq!(
                (status, code(&response)),
                (StatusCode::FORBIDDEN, "FORBIDDEN")
            );
        }
    }
    let (status, _) = send(
        &f.reassign_a,
        "POST",
        "/api/auth/admin/trades/not-a-uuid/reassign-responsible",
        Some(token(ADMIN, REASSIGNMENT_ISSUER)),
        Some(valid.clone()),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    for invalid in [
        without(valid.clone(), "evidence_case_ref"),
        without(valid.clone(), "expected_current_user_id"),
        json!({"side":"SELLER","replacement_user_id":id(REPLACEMENT),"evidence_case_ref":"Dismissed for misconduct","expected_current_user_id":id(SELLER)}),
        json!({"side":"SELLER","replacement_user_id":id(REPLACEMENT),"evidence_case_ref":CASE,"expected_current_user_id":id(SELLER),"reason":"left"}),
        reassign_body("SELLER", SELLER, SELLER),
    ] {
        let (status, body) = f.reassign(ADMIN, trade, invalid).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_FAILED")
        );
    }
    let (status, body) = f.reassign(ADMIN, id(UNKNOWN), valid.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::NOT_FOUND, "TRADE_NOT_FOUND")
    );

    // Only a current, approved, verified member of the recorded organization
    // with the role the side requires.
    for replacement in [
        BUYER_ROLE_IN_SELLER_ORG,
        PENDING,
        MUST_CHANGE_PASSWORD,
        UNVERIFIED,
        OTHER_SUPPLIER,
        STRANGER,
        SECOND_ADMIN,
        BUYER,
        UNKNOWN,
    ] {
        let (status, body) = f
            .reassign(ADMIN, trade, reassign_body("SELLER", replacement, SELLER))
            .await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE"),
            "replacement {replacement:#x}"
        );
    }
    // A detached former member is not a current member.
    f.change(REPLACEMENT, "detached").await;
    let (status, body) = f.reassign(ADMIN, trade, valid.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE")
    );
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002002'").await;
    // The person on the other side never takes over this side, even after
    // joining the recorded organization with the same role.
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000',role='SUPPLIER' WHERE id='00000000-0000-0000-0000-000000001001'").await;
    let (status, body) = f
        .reassign(ADMIN, trade, reassign_body("SELLER", BUYER, SELLER))
        .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE")
    );
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000001000',role='BUYER' WHERE id='00000000-0000-0000-0000-000000001001'").await;
    // The side names the recorded person: the seller is not the buyer side's person.
    let (status, body) = f
        .reassign(ADMIN, trade, reassign_body("BUYER", MEMBER, SELLER))
        .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_STALE")
    );
    // Only confirmed and delivered obligations change their person.
    for target in [pending, paid] {
        let (status, body) = f.reassign(ADMIN, target, valid.clone()).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::CONFLICT, "TRADE_NOT_REASSIGNABLE")
        );
    }
    assert_eq!(
        f.snapshot().await,
        before,
        "every refusal leaves every row unchanged"
    );

    let (status, body) = f.reassign(ADMIN, trade, valid.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["trade_id"], trade.to_string());
    assert_eq!(body["side"], "SELLER");
    assert_eq!(body["status"], "CONFIRMED");
    assert_eq!(body["recorded_organization_id"], id(SELLER_ORG).to_string());
    assert_eq!(body["previous_responsible_user_id"], id(SELLER).to_string());
    assert_eq!(body["responsible_user_id"], id(REPLACEMENT).to_string());
    assert!(body["reassigned_at"].is_string());
    assert_eq!(
        f.side_person(trade, "seller_user_id").await,
        Some(id(REPLACEMENT))
    );
    assert_eq!(f.side_person(trade, "buyer_user_id").await, Some(id(BUYER)));

    f.change(BUYER, "detached").await;
    let (status, body) = f
        .reassign(
            SECOND_ADMIN,
            buyer_trade,
            reassign_body("BUYER", MEMBER, BUYER),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["recorded_organization_id"], id(BUYER_ORG).to_string());
    assert_eq!(
        f.side_person(buyer_trade, "buyer_user_id").await,
        Some(id(MEMBER))
    );
    assert_eq!(
        f.side_person(buyer_trade, "seller_user_id").await,
        Some(id(SELLER))
    );
    // Repeating the same request is now stale, not a second change.
    let (status, body) = f.reassign(ADMIN, trade, valid).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_STALE")
    );
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn reassignment_preserves_terms_status_and_original_history() {
    let mut f = Fixture::new().await;
    let trade = f.trade;
    let python_trade = f.clone_trade("CONFIRMED").await;
    // The departing seller records a delivery with a key before leaving.
    let (status, body) = f.deliver(trade, SELLER, Some("handover")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DELIVERED");
    // The person leaves: rejected and detached, as account offboarding leaves them.
    f.change(SELLER, "rejected").await;
    f.change(SELLER, "detached").await;
    let (status, _) = f.pay(trade, SELLER).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a departed person cannot settle"
    );

    let before = f.snapshot().await;
    let (status, body) = f
        .reassign(ADMIN, trade, reassign_body("SELLER", REPLACEMENT, SELLER))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DELIVERED");
    let after = f.snapshot().await;

    // Only the side's person changes: terms, amounts, status and times stay.
    let row = |snapshot: &Value, wanted: Uuid| {
        entries(snapshot, "trades")
            .into_iter()
            .find(|row| row["id"] == wanted.to_string())
            .unwrap()
    };
    let (old, new) = (row(&before, trade), row(&after, trade));
    assert_eq!(old["seller_user_id"], id(SELLER).to_string());
    assert_eq!(new["seller_user_id"], id(REPLACEMENT).to_string());
    assert_eq!(
        without(old, "seller_user_id"),
        without(new, "seller_user_id")
    );
    assert_eq!(row(&before, python_trade), row(&after, python_trade));
    // Order authorship, stock, people, freezes, receipts and notices stay.
    for key in [
        "orders",
        "inventory",
        "users",
        "freezes",
        "receipts",
        "notifications",
    ] {
        assert_eq!(before[key], after[key], "{key} changed");
    }
    let receipts = entries(&after, "receipts");
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt["actor_user_id"] == id(SELLER).to_string()),
        "the departed person's receipt stays theirs"
    );
    // Earlier audit records stay; one new record names both people and the case.
    let old_audit = entries(&before, "audit");
    let new_audit = entries(&after, "audit");
    assert_eq!(new_audit.len(), old_audit.len() + 1);
    for record in &old_audit {
        assert!(
            new_audit.contains(record),
            "an earlier audit record changed"
        );
    }
    let record = new_audit
        .iter()
        .find(|record| !old_audit.contains(record))
        .unwrap();
    assert_eq!(record["action"], AUDIT_ACTION);
    assert_eq!(record["user_id"], id(ADMIN).to_string());
    assert_eq!(record["resource_type"], "trade");
    assert_eq!(record["resource_id"], trade.to_string());
    assert_eq!(
        record["changes"],
        json!({
            "side": "SELLER",
            "organization_id": id(SELLER_ORG),
            "trade_status": "DELIVERED",
            "responsible_user_id": {"before": id(SELLER), "after": id(REPLACEMENT)},
            "evidence_case_ref": CASE,
        })
    );
    assert!(record["ip_address"].is_string() && record["request_id"].is_string());
    // One event, for the recorded organization only, naming nobody.
    let old_events = entries(&before, "outbox");
    let new_events = entries(&after, "outbox");
    assert_eq!(new_events.len(), old_events.len() + 1);
    let event = new_events
        .iter()
        .find(|event| !old_events.contains(event))
        .unwrap();
    assert_eq!(event["event_type"], EVENT_TYPE);
    assert_eq!(event["aggregate_type"], "trade");
    assert_eq!(event["aggregate_id"], trade.to_string());
    assert_eq!(event["participants"], json!([id(SELLER_ORG)]));
    assert_eq!(event["payload"], json!({"id": trade, "side": "SELLER"}));
    let text = event.to_string();
    for hidden in [
        id(SELLER).to_string(),
        id(REPLACEMENT).to_string(),
        id(BUYER_ORG).to_string(),
        CASE.into(),
    ] {
        assert!(!text.contains(&hidden), "the event discloses {hidden}");
    }

    // A receipt never moves to the new person: the same key is a fresh,
    // refused command, not a replay of the departed person's delivery.
    let (status, body) = f.deliver(trade, REPLACEMENT, Some("handover")).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::BAD_REQUEST, "TRADE_NOT_CONFIRMED")
    );
    // The new person pays in Rust; the departed person still cannot.
    let (status, _) = f.pay(trade, SELLER).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = f.pay(trade, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "PAID");
    let paid = f.trade_row(trade).await;
    assert_eq!(
        paid["final_quantity_mt"],
        old_terms(&before, trade)["final_quantity_mt"]
    );
    assert_eq!(
        paid["price_per_mt_usd"],
        old_terms(&before, trade)["price_per_mt_usd"]
    );

    // The same handover in the Python stack: delivery and payment by the new
    // person on the second trade; the departed person is refused.
    let (status, body) = f
        .reassign(
            ADMIN,
            python_trade,
            reassign_body("SELLER", REPLACEMENT, SELLER),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = python_settle(
        &f.database.runtime_url,
        &[
            PythonCall {
                name: "departed-deliver",
                trade: python_trade,
                action: "deliver",
                actor: SELLER,
            },
            PythonCall {
                name: "deliver",
                trade: python_trade,
                action: "deliver",
                actor: REPLACEMENT,
            },
            PythonCall {
                name: "departed-pay",
                trade: python_trade,
                action: "pay",
                actor: SELLER,
            },
            PythonCall {
                name: "pay",
                trade: python_trade,
                action: "pay",
                actor: REPLACEMENT,
            },
        ],
    )
    .await;
    assert_eq!(
        python_status(&results, "departed-deliver"),
        403,
        "{results}"
    );
    assert_eq!(python_status(&results, "deliver"), 200, "{results}");
    assert_eq!(results["deliver"]["body"]["status"], "DELIVERED");
    assert_eq!(python_status(&results, "departed-pay"), 403, "{results}");
    assert_eq!(python_status(&results, "pay"), 200, "{results}");
    assert_eq!(results["pay"]["body"]["status"], "PAID");
    let settled = f.trade_row(python_trade).await;
    assert_eq!(settled["seller_user_id"], id(REPLACEMENT).to_string());
    assert_eq!(
        settled["quantity_mt"],
        old_terms(&before, python_trade)["quantity_mt"]
    );
    assert_eq!(
        settled["price_per_mt_usd"],
        old_terms(&before, python_trade)["price_per_mt_usd"]
    );
}

fn old_terms(snapshot: &Value, trade: Uuid) -> Value {
    entries(snapshot, "trades")
        .into_iter()
        .find(|row| row["id"] == trade.to_string())
        .unwrap()
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn frozen_trade_refuses_reassignment() {
    let mut f = Fixture::new().await;
    // The seller has left the recorded organization.
    f.change(SELLER, "detached").await;
    let static_trade = f.trade;
    let freeze_first = f.clone_trade("CONFIRMED").await;
    let reassign_first = f.clone_trade("DELIVERED").await;
    let valid = reassign_body("SELLER", REPLACEMENT, SELLER);

    // An active freeze refuses; nothing changes; clearing it allows the change.
    let (status, body) = f.freeze_trade(SECOND_ADMIN, static_trade).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let before = f.snapshot().await;
    let (status, body) = f.reassign(ADMIN, static_trade, valid.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "TRADE_FROZEN")
    );
    assert_eq!(f.snapshot().await, before);
    let (status, body) = send(
        &f.freeze,
        "POST",
        &format!("/api/admin/trades/{static_trade}/unfreeze"),
        Some(token(SECOND_ADMIN, FREEZE_ISSUER)),
        Some(json!({"note":"Review complete"})),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = f.reassign(ADMIN, static_trade, valid.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The freeze wins the trade lock: the reassignment waits, then refuses.
    let gate = f.gatekeeper("freeze").await;
    let router = f.freeze.clone();
    let freezing =
        tokio::spawn(async move { freeze_with(&router, SECOND_ADMIN, freeze_first).await });
    f.wait_for("freeze", "INSERT INTO trade_freezes").await;
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let reassigning =
        tokio::spawn(async move { reassign_with(&router, ADMIN, freeze_first, body).await });
    f.wait_for("reassign-a", "FROM trades WHERE id=$1 FOR UPDATE")
        .await;
    f.release(gate).await;
    let (status, body) = freezing.await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = reassigning.await.unwrap();
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "TRADE_FROZEN")
    );
    assert_eq!(
        f.side_person(freeze_first, "seller_user_id").await,
        Some(id(SELLER))
    );
    f.remove_gate().await;

    // The reassignment wins the trade lock: the freeze waits, then freezes
    // the trade with its new person, who cannot settle while it is frozen.
    let gate = f.gatekeeper("reassignment").await;
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let reassigning =
        tokio::spawn(async move { reassign_with(&router, ADMIN, reassign_first, body).await });
    f.wait_for("reassign-a", "SET seller_user_id").await;
    let router = f.freeze.clone();
    let freezing =
        tokio::spawn(async move { freeze_with(&router, SECOND_ADMIN, reassign_first).await });
    f.wait_for("freeze", "FROM trades WHERE id=$1 FOR UPDATE")
        .await;
    f.release(gate).await;
    let (status, body) = reassigning.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = freezing.await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    f.remove_gate().await;
    assert_eq!(
        f.side_person(reassign_first, "seller_user_id").await,
        Some(id(REPLACEMENT))
    );
    let (status, body) = f.pay(reassign_first, REPLACEMENT).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "TRADE_FROZEN")
    );
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn stale_or_moved_replacement_refused() {
    let mut f = Fixture::new().await;
    // The seller has left the recorded organization.
    f.change(SELLER, "detached").await;
    let stale = f.trade;
    let concurrent = f.clone_trade("CONFIRMED").await;
    let moved_first = f.clone_trade("CONFIRMED").await;
    let reassign_first = f.clone_trade("DELIVERED").await;
    let after_move = f.clone_trade("CONFIRMED").await;
    let valid = reassign_body("SELLER", REPLACEMENT, SELLER);
    let restore = "UPDATE users SET organization_id='00000000-0000-0000-0000-000000002000' WHERE id='00000000-0000-0000-0000-000000002002'";

    // A different current person than the administrator saw.
    f.force_side(stale, "SELLER", MUST_CHANGE_PASSWORD).await;
    let before = f.snapshot().await;
    let (status, body) = f.reassign(ADMIN, stale, valid.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_STALE")
    );
    assert_eq!(f.snapshot().await, before);

    // Two administrators: the second waits on the organization, then finds
    // the person changed and refuses instead of overwriting.
    let gate = f.gatekeeper("reassignment").await;
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let first = tokio::spawn(async move { reassign_with(&router, ADMIN, concurrent, body).await });
    f.wait_for("reassign-a", "SET seller_user_id").await;
    let router = f.reassign_b.clone();
    let body = valid.clone();
    let second =
        tokio::spawn(async move { reassign_with(&router, SECOND_ADMIN, concurrent, body).await });
    f.wait_for("reassign-b", "FROM organizations WHERE id=$1 FOR UPDATE")
        .await;
    f.release(gate).await;
    let (status, body) = first.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = second.await.unwrap();
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_STALE")
    );
    f.remove_gate().await;

    // The replacement moves first: the reassignment waits on the person's
    // row, then refuses the moved member.
    let mut mover = PgConnection::connect(&f.database.admin_url).await.unwrap();
    sqlx::query("SELECT set_config('application_name','mover',false)")
        .execute(&mut mover)
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut mover).await.unwrap();
    sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
        .bind(id(OTHER_ORG))
        .bind(id(REPLACEMENT))
        .execute(&mut mover)
        .await
        .unwrap();
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let waiting =
        tokio::spawn(async move { reassign_with(&router, ADMIN, moved_first, body).await });
    f.wait_for("reassign-a", "FROM users WHERE id=$1 FOR UPDATE")
        .await;
    sqlx::query("COMMIT").execute(&mut mover).await.unwrap();
    let (status, body) = waiting.await.unwrap();
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE")
    );
    assert_eq!(
        f.side_person(moved_first, "seller_user_id").await,
        Some(id(SELLER))
    );
    f.sql(restore).await;

    // The reassignment wins: the move waits for the committed change.
    let gate = f.gatekeeper("reassignment").await;
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let reassigning =
        tokio::spawn(async move { reassign_with(&router, ADMIN, reassign_first, body).await });
    f.wait_for("reassign-a", "SET seller_user_id").await;
    let moving = tokio::spawn(async move {
        sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
            .bind(id(OTHER_ORG))
            .bind(id(REPLACEMENT))
            .execute(&mut mover)
            .await
            .unwrap()
            .rows_affected()
    });
    f.wait_for("mover", "UPDATE users SET organization_id")
        .await;
    f.release(gate).await;
    let (status, body) = reassigning.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(moving.await.unwrap(), 1);
    f.remove_gate().await;
    assert_eq!(
        f.side_person(reassign_first, "seller_user_id").await,
        Some(id(REPLACEMENT))
    );
    // After the move, the moved person is no longer an eligible replacement.
    let (status, body) = f.reassign(ADMIN, after_move, valid).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE")
    );
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn assisted_support_side_reassignment_requires_manual_resolution() {
    let mut f = Fixture::new().await;
    f.assisted().await;
    let assisted = f.trade;
    let mut clones = Vec::new();
    for _ in 0..4 {
        let clone: Uuid = sqlx::query_scalar("SELECT settlement_fixture_clone()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        clones.push(clone);
    }
    let (python_buyer_trade, python_customer_trade, rust_customer_trade, rewritten) =
        (clones[0], clones[1], clones[2], clones[3]);
    // The support side's recorded person is the order owner and creator.
    assert_eq!(
        f.side_person(assisted, "seller_user_id").await,
        Some(id(ADMIN))
    );

    // The list flags the support side for manual resolution, not the ordinary side.
    let (status, body) = f.list(SECOND_ADMIN, &id(ADMIN).to_string(), "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 5);
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["side"], "SELLER");
        assert_eq!(item["manual_resolution_required"], true);
    }
    let (_, body) = f.list(SECOND_ADMIN, &id(BUYER).to_string(), "").await;
    assert_eq!(body["total"], 5);
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["manual_resolution_required"], false);
    }

    // The support side is refused and nothing changes, for every replacement.
    let before = f.snapshot().await;
    for replacement in [REPLACEMENT, SELLER] {
        let (status, body) = f
            .reassign(
                SECOND_ADMIN,
                assisted,
                reassign_body("SELLER", replacement, ADMIN),
            )
            .await;
        assert_eq!(
            (status, code(&body)),
            (
                StatusCode::CONFLICT,
                "REASSIGNMENT_MANUAL_RESOLUTION_REQUIRED"
            )
        );
    }
    assert_eq!(f.snapshot().await, before);

    // What the refusal prevents: with only the support side's person
    // rewritten, the owner and creator check refuses settlement on both
    // sides, in both stacks, while the intact support side still settles
    // through its customer member.
    f.force_side(rewritten, "SELLER", REPLACEMENT).await;
    let rewritten_before = f.trade_row(rewritten).await;
    for actor in [SELLER, REPLACEMENT, BUYER] {
        let (status, body) = f.deliver(rewritten, actor, None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "Rust deliver by {actor:#x}: {body}"
        );
    }
    let (status, body) = f
        .deliver(rust_customer_trade, SELLER, Some("assisted"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DELIVERED");
    let results = python_settle(
        &f.database.runtime_url,
        &[
            PythonCall {
                name: "rewritten-customer",
                trade: rewritten,
                action: "deliver",
                actor: SELLER,
            },
            PythonCall {
                name: "rewritten-replacement",
                trade: rewritten,
                action: "deliver",
                actor: REPLACEMENT,
            },
            PythonCall {
                name: "rewritten-buyer",
                trade: rewritten,
                action: "deliver",
                actor: BUYER,
            },
            PythonCall {
                name: "intact-customer",
                trade: python_customer_trade,
                action: "deliver",
                actor: SELLER,
            },
        ],
    )
    .await;
    for name in [
        "rewritten-customer",
        "rewritten-replacement",
        "rewritten-buyer",
    ] {
        assert_eq!(python_status(&results, name), 403, "{name}: {results}");
    }
    assert_eq!(python_status(&results, "intact-customer"), 200, "{results}");
    assert_eq!(results["intact-customer"]["body"]["status"], "DELIVERED");
    assert_eq!(f.trade_row(rewritten).await, rewritten_before);

    // The ordinary side of an assisted trade changes its departed person, and
    // the new person acts in both stacks; the departed person cannot.
    f.change(BUYER, "detached").await;
    for trade in [assisted, python_buyer_trade] {
        let (status, body) = f
            .reassign(SECOND_ADMIN, trade, reassign_body("BUYER", MEMBER, BUYER))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            f.side_person(trade, "seller_user_id").await,
            Some(id(ADMIN))
        );
    }
    let (status, _) = f.deliver(assisted, BUYER, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = f.deliver(assisted, MEMBER, Some("new-buyer")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DELIVERED");
    let results = python_settle(
        &f.database.runtime_url,
        &[
            PythonCall {
                name: "departed-buyer",
                trade: python_buyer_trade,
                action: "deliver",
                actor: BUYER,
            },
            PythonCall {
                name: "new-buyer",
                trade: python_buyer_trade,
                action: "deliver",
                actor: MEMBER,
            },
        ],
    )
    .await;
    // Python refuses a detached person before the party check (409, no
    // market organization) or at it (403); either way nothing changes.
    assert!(
        matches!(python_status(&results, "departed-buyer"), 403 | 409),
        "{results}"
    );
    assert_eq!(python_status(&results, "new-buyer"), 200, "{results}");
    assert_eq!(results["new-buyer"]["body"]["status"], "DELIVERED");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn reassignment_requires_a_departed_current_person() {
    let mut f = Fixture::new().await;
    let attached = f.trade;
    let detached = f.clone_trade("CONFIRMED").await;
    let moved = f.clone_trade("DELIVERED").await;
    let rejoined = f.clone_trade("CONFIRMED").await;
    let valid = reassign_body("SELLER", REPLACEMENT, SELLER);

    // An approved member, and a rejected member who is still attached, are
    // current members: no handover while they belong to the organization.
    for state in [None, Some("rejected")] {
        if let Some(state) = state {
            f.change(SELLER, state).await;
        }
        let before = f.snapshot().await;
        let (status, body) = f.reassign(ADMIN, attached, valid.clone()).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::CONFLICT, "REASSIGNMENT_CURRENT_STILL_MEMBER"),
            "{state:?}"
        );
        assert_eq!(f.snapshot().await, before);
    }

    // A detached person has left.
    f.change(SELLER, "detached").await;
    let (status, body) = f.reassign(ADMIN, detached, valid.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.side_person(detached, "seller_user_id").await,
        Some(id(REPLACEMENT))
    );
    // A person who moved to another organization has left.
    f.change(BUYER, "moved").await;
    let (status, body) = f
        .reassign(ADMIN, moved, reassign_body("BUYER", MEMBER, BUYER))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.side_person(moved, "buyer_user_id").await,
        Some(id(MEMBER))
    );

    // A departed person who rejoins while the reassignment waits is a member
    // again when the reassignment reads it. The rejoin's foreign key holds
    // the organization row, so the reassignment waits there first.
    let mut rejoin = PgConnection::connect(&f.database.admin_url).await.unwrap();
    sqlx::query("BEGIN").execute(&mut rejoin).await.unwrap();
    sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
        .bind(id(SELLER_ORG))
        .bind(id(SELLER))
        .execute(&mut rejoin)
        .await
        .unwrap();
    let router = f.reassign_a.clone();
    let body = valid.clone();
    let waiting = tokio::spawn(async move { reassign_with(&router, ADMIN, rejoined, body).await });
    f.wait_for("reassign-a", "FROM organizations WHERE id=$1 FOR UPDATE")
        .await;
    sqlx::query("COMMIT").execute(&mut rejoin).await.unwrap();
    let (status, body) = waiting.await.unwrap();
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_CURRENT_STILL_MEMBER")
    );
    assert_eq!(
        f.side_person(rejoined, "seller_user_id").await,
        Some(id(SELLER))
    );
}

/// Mark `users` as offboarded although they stay approved and attached. The
/// database forbids that state, so only the marker check can decide these
/// cases; the constraints come back `NOT VALID`, so later writes are still
/// checked, and are validated again once the markers are cleared.
const MARK_DESPITE_CONSTRAINTS: &str = "ALTER TABLE users \
     DROP CONSTRAINT ck_users_offboarded_requires_rejected, \
     DROP CONSTRAINT ck_users_offboarded_requires_detached; \
     UPDATE users SET offboarded_at=now() WHERE id IN \
     ('00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002002'); \
     ALTER TABLE users \
     ADD CONSTRAINT ck_users_offboarded_requires_rejected \
     CHECK (offboarded_at IS NULL OR status = 'REJECTED') NOT VALID, \
     ADD CONSTRAINT ck_users_offboarded_requires_detached \
     CHECK (offboarded_at IS NULL OR organization_id IS NULL) NOT VALID";

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn offboarding_marker_decides_departure_and_eligibility() {
    let mut f = Fixture::new().await;
    let attached = f.trade;
    let second = f.clone_trade("CONFIRMED").await;
    let valid = reassign_body("SELLER", REPLACEMENT, SELLER);
    f.sql(MARK_DESPITE_CONSTRAINTS).await;

    // A marked replacement is never eligible, even while approved and
    // attached.
    let before = f.snapshot().await;
    let (status, body) = f.reassign(ADMIN, second, valid.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE")
    );
    assert_eq!(f.snapshot().await, before);

    // A marked current person has departed, even while the membership row
    // still names the recorded organization.
    f.sql("UPDATE users SET offboarded_at=NULL WHERE id='00000000-0000-0000-0000-000000002002'")
        .await;
    let (status, body) = f.reassign(ADMIN, attached, valid).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.side_person(attached, "seller_user_id").await,
        Some(id(REPLACEMENT))
    );

    f.sql(
        "UPDATE users SET offboarded_at=NULL WHERE id='00000000-0000-0000-0000-000000002001'; \
         ALTER TABLE users VALIDATE CONSTRAINT ck_users_offboarded_requires_rejected; \
         ALTER TABLE users VALIDATE CONSTRAINT ck_users_offboarded_requires_detached",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn replacement_role_follows_the_side() {
    let mut f = Fixture::new().await;
    let seller_trade = f.trade;
    let python_seller_trade = f.clone_trade("CONFIRMED").await;
    let buyer_trade = f.clone_trade("CONFIRMED").await;
    let python_buyer_trade = f.clone_trade("CONFIRMED").await;
    // The seller left and became a buyer elsewhere; the buyer left and
    // became a supplier elsewhere.
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000003000',role='BUYER' WHERE id='00000000-0000-0000-0000-000000002001'").await;
    f.sql("UPDATE users SET organization_id='00000000-0000-0000-0000-000000003000',role='SUPPLIER' WHERE id='00000000-0000-0000-0000-000000001001'").await;

    // The side decides the role, never the departed person's present role:
    // the seller side needs a supplier, the buyer side a buyer.
    let before = f.snapshot().await;
    for (trade, side, replacement, current) in [
        (seller_trade, "SELLER", BUYER_ROLE_IN_SELLER_ORG, SELLER),
        (buyer_trade, "BUYER", SUPPLIER_IN_BUYER_ORG, BUYER),
    ] {
        let (status, body) = f
            .reassign(ADMIN, trade, reassign_body(side, replacement, current))
            .await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::CONFLICT, "REASSIGNMENT_REPLACEMENT_INELIGIBLE"),
            "{side}"
        );
    }
    assert_eq!(f.snapshot().await, before);
    for trade in [seller_trade, python_seller_trade] {
        let (status, body) = f
            .reassign(ADMIN, trade, reassign_body("SELLER", REPLACEMENT, SELLER))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    for trade in [buyer_trade, python_buyer_trade] {
        let (status, body) = f
            .reassign(ADMIN, trade, reassign_body("BUYER", MEMBER, BUYER))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // Rust: the new supplier delivers and pays; the new buyer delivers.
    let (status, body) = f.deliver(seller_trade, REPLACEMENT, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = f.pay(seller_trade, REPLACEMENT).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "PAID");
    let (status, body) = f.deliver(buyer_trade, MEMBER, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "DELIVERED");
    // Python: the same people act on the second pair of trades.
    let results = python_settle(
        &f.database.runtime_url,
        &[
            PythonCall {
                name: "supplier-deliver",
                trade: python_seller_trade,
                action: "deliver",
                actor: REPLACEMENT,
            },
            PythonCall {
                name: "supplier-pay",
                trade: python_seller_trade,
                action: "pay",
                actor: REPLACEMENT,
            },
            PythonCall {
                name: "buyer-deliver",
                trade: python_buyer_trade,
                action: "deliver",
                actor: MEMBER,
            },
        ],
    )
    .await;
    for name in ["supplier-deliver", "supplier-pay", "buyer-deliver"] {
        assert_eq!(python_status(&results, name), 200, "{name}: {results}");
    }
    assert_eq!(results["supplier-pay"]["body"]["status"], "PAID");
    assert_eq!(results["buyer-deliver"]["body"]["status"], "DELIVERED");
}

#[tokio::test]
#[ignore = "requires an attested disposable PostgreSQL"]
async fn disabled_reassignment_is_not_routed() {
    let f = Fixture::new().await;
    const ISSUER: &str = "reassignment-serving";
    let config: verdaxis_read::Config = serde_json::from_value(json!({
        "environment": "test",
        "release_sha": verdaxis_read::compiled_sha(),
        "migration_revision": ALLOWED_REVISIONS[0],
        "database_url": f.database.runtime_url,
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 4,
        "pool_max": 5,
        "python_budget": 52,
        "configured_max_connections": 100,
        "phase2": {
            "jwt_secret": KEY, "jwt_issuer": ISSUER, "jwt_audience": ISSUER,
            "frontend_url": "https://app.example.test", "email_from": "team@example.test",
            "resend_api_key": "synthetic", "auth_domain_datasets": "null"
        }
    }))
    .unwrap();
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let state = verdaxis_read::AppState::new(config, pools, policy).unwrap();
    assert!(verdaxis_read::runtime::market_write::mounts(&state));
    let before = sqlx::query_scalar::<_, Value>("SELECT reassignment_fixture_snapshot()")
        .fetch_one(&f.observer)
        .await
        .unwrap();
    let list = format!("/api/auth/admin/users/{}/offboarding-trades", id(SELLER));
    let reassign = format!("/api/auth/admin/trades/{}/reassign-responsible", f.trade);
    let body = reassign_body("SELLER", REPLACEMENT, SELLER);
    let host = [("host", "localhost")];

    // The served router has no such route.
    let serving = verdaxis_read::router(state.clone());
    let (status, _) = send(
        &serving,
        "GET",
        &list,
        Some(token(ADMIN, ISSUER)),
        None,
        &host,
    )
    .await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "{status}"
    );
    let (status, _) = send(
        &serving,
        "POST",
        &reassign,
        Some(token(ADMIN, ISSUER)),
        Some(body.clone()),
        &host,
    )
    .await;
    assert!(
        matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ),
        "{status}"
    );
    let after = sqlx::query_scalar::<_, Value>("SELECT reassignment_fixture_snapshot()")
        .fetch_one(&f.observer)
        .await
        .unwrap();
    assert_eq!(after, before);

    // The dormant composition exists and answers with the same credentials,
    // so the refusal above is the missing route, not the token.
    let dormant = verdaxis_read::runtime::market_write::reassignment_routes(&state)
        .expect("the composition has Phase 2 settings");
    let (status, body) = send(
        &dormant,
        "GET",
        &list,
        Some(token(ADMIN, ISSUER)),
        None,
        &host,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
