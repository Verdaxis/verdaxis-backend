//! The composed offboarding command on PostgreSQL: account, credential and
//! market changes in one transaction, through the served router with the
//! release switch on.
//!
//! Each case clones a fresh database from the runner's migrated template with
//! the converged runtime grants (`rs/read/tests/run_offboarding_composition.sh`).
//! Requests run as the runtime role through `verdaxis_read::router`; fixture
//! rows and observation use the administrator connection. Races use
//! independent connections, and each wait is observed in `pg_stat_activity`
//! with its blocking backend.
#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;
use verdaxis_read::{
    AppState, Config,
    auth::{VerificationSettings, offboarding::UnusablePasswordHash, verify_access},
    business::{
        admission::AuditContext,
        offboarding::{OffboardAccount, OffboardRequest},
        transaction::{CompletionControl, CompletionPoint, run_controlled_write},
    },
    compiled_sha,
};

const SECRET: &str = "synthetic-composition-key-at-least-32-characters";
const ISSUER: &str = "composition-test";
const AUDIENCE: &str = "composition-test";
const IGNORE: &str = "requires migrated disposable PostgreSQL";

const SUBJECT_ORGANIZATION: Uuid = Uuid::from_u128(0x5000);
const SUBJECT: Uuid = Uuid::from_u128(0x5001);
const COLLEAGUE: Uuid = Uuid::from_u128(0x5002);
const COUNTERPARTY_ORGANIZATION: Uuid = Uuid::from_u128(0x6000);
const COUNTERPARTY: Uuid = Uuid::from_u128(0x6001);
const OTHER_ORGANIZATION: Uuid = Uuid::from_u128(0x6500);
const ADMIN: Uuid = Uuid::from_u128(0x7001);
const JOIN_REQUEST: Uuid = Uuid::from_u128(0x7100);

// The standard market of the departing seller.
const ITEM: Uuid = Uuid::from_u128(0xa001);
/// Partly filled ASK, 4 of 5 t open, with 1 t in the pending trade.
const ASK_PARTLY: Uuid = Uuid::from_u128(0xa002);
/// Open ASK, 3 t.
const ASK_OPEN: Uuid = Uuid::from_u128(0xa003);
/// The counterparty BID filled by the pending trade.
const BID_FILLED: Uuid = Uuid::from_u128(0xa004);
const PENDING: Uuid = Uuid::from_u128(0xa010);
const CONFIRMED: Uuid = Uuid::from_u128(0xa011);
const DELIVERED: Uuid = Uuid::from_u128(0xa012);
/// The colleague's own open ASK and pending trade: outside the scope.
const COLLEAGUE_ASK: Uuid = Uuid::from_u128(0xa020);
const COLLEAGUE_PENDING: Uuid = Uuid::from_u128(0xa021);
/// A fully taken ASK, 1 t, whose whole quantity is in a second pending trade:
/// filled, not open, yet withdrawn when that trade is cancelled.
const ASK_TAKEN: Uuid = Uuid::from_u128(0xa005);
/// The counterparty BID filled by the second pending trade.
const BID_TAKEN: Uuid = Uuid::from_u128(0xa006);
const PENDING_TAKEN: Uuid = Uuid::from_u128(0xa013);

fn d(value: i64) -> Decimal {
    Decimal::from(value)
}

fn token(user: Uuid) -> String {
    let now = Utc::now().timestamp();
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({"sub": user, "type": "access", "iat": now - 1, "exp": now + 600,
                "iss": ISSUER, "aud": AUDIENCE}),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn offboard_body(expected: Option<Uuid>) -> Value {
    json!({
        "reason_code": "EMPLOYMENT_ENDED",
        "evidence_case_ref": "CASE-2026/0042",
        "urgent": false,
        "expected_organization_id": expected,
    })
}

/// The runtime role's pool. `gated` sessions find the fixture gate's advisory
/// lock first on their search path, so they pause at their first advisory
/// lock until the gate opens.
async fn runtime_pool(url: &str, gated: bool) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |connection, _| {
            Box::pin(async move {
                sqlx::raw_sql("SET TimeZone TO 'UTC'")
                    .execute(&mut *connection)
                    .await?;
                if gated {
                    sqlx::raw_sql("SET search_path TO offboarding_gate,public,pg_catalog")
                        .execute(&mut *connection)
                        .await?;
                }
                Ok(())
            })
        })
        .connect(url)
        .await
        .unwrap()
}

/// The served application with the release switch on.
fn application(runtime_url: &str, pool: PgPool) -> AppState {
    let config: Config = serde_json::from_value(json!({
        "kyc_execution_gate_enabled": false,
        "environment": "test",
        "release_sha": compiled_sha(),
        "migration_revision": ALLOWED_REVISIONS[0],
        "database_url": runtime_url,
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 4,
        "pool_max": 5,
        "python_budget": 52,
        "configured_max_connections": 100,
        "phase2": {
            "jwt_secret": SECRET, "jwt_issuer": ISSUER, "jwt_audience": AUDIENCE,
            "frontend_url": "https://app.example.test", "email_from": "team@example.test",
            "resend_api_key": "synthetic", "auth_domain_datasets": "null",
            "offboarding_enabled": true
        },
    }))
    .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(runtime_url)
        .unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read: pool, health },
        policy,
    )
    .unwrap()
}

struct World {
    database: market_database::MarketDatabase,
    admin: PgConnection,
    pool: PgPool,
    state: AppState,
    served: Router,
}

impl World {
    /// A fresh database with the members, the departing person's credentials
    /// and the standard market below.
    async fn new() -> Self {
        let database = market_database::MarketDatabase::new("RUST_OFFBOARDING_COMPOSITION").await;
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/offboarding_market.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/offboarding_composition.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        for (organization, kind, member, role) in [
            (
                Some(SUBJECT_ORGANIZATION),
                "FUEL_SUPPLIER",
                SUBJECT,
                "SUPPLIER",
            ),
            (
                Some(SUBJECT_ORGANIZATION),
                "FUEL_SUPPLIER",
                COLLEAGUE,
                "SUPPLIER",
            ),
            (
                Some(COUNTERPARTY_ORGANIZATION),
                "SHIPOWNER",
                COUNTERPARTY,
                "BUYER",
            ),
            (None, "", ADMIN, "ADMIN"),
        ] {
            sqlx::query("SELECT offboarding_market_member($1,$2,$3,$4)")
                .bind(organization)
                .bind(kind)
                .bind(member)
                .bind(role)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        sqlx::query("SELECT composition_credentials($1,$2,$3)")
            .bind(SUBJECT)
            .bind(OTHER_ORGANIZATION)
            .bind(JOIN_REQUEST)
            .execute(&mut admin)
            .await
            .unwrap();
        let pool = runtime_pool(&database.runtime_url, false).await;
        let state = application(&database.runtime_url, pool.clone());
        let served = verdaxis_read::router(state.clone());
        Self {
            database,
            admin,
            pool,
            state,
            served,
        }
    }

    async fn sql(&mut self, statement: &'static str) {
        sqlx::query(statement)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    /// The seller's market: a partly filled ASK whose filled tonne is in a
    /// pending trade, a fully taken ASK whose tonne is in a second pending
    /// trade, an open ASK, a confirmed and a delivered trade, and the
    /// colleague's ASK and pending trade. The item reserves the 9 t the three
    /// ASKs and the two pending tonnes hold.
    async fn standard_market(&mut self) {
        sqlx::query("SELECT offboarding_market_inventory($1,$2,2,9)")
            .bind(ITEM)
            .bind(SUBJECT)
            .execute(&mut self.admin)
            .await
            .unwrap();
        for (order, owner, side, quantity, remaining, status, item) in [
            (
                ASK_PARTLY,
                SUBJECT,
                "ASK",
                5,
                4,
                "PARTIALLY_FILLED",
                Some(ITEM),
            ),
            (ASK_OPEN, SUBJECT, "ASK", 3, 3, "OPEN", Some(ITEM)),
            (ASK_TAKEN, SUBJECT, "ASK", 1, 0, "FILLED", Some(ITEM)),
            (BID_FILLED, COUNTERPARTY, "BID", 1, 0, "FILLED", None),
            (BID_TAKEN, COUNTERPARTY, "BID", 1, 0, "FILLED", None),
            (COLLEAGUE_ASK, COLLEAGUE, "ASK", 1, 1, "OPEN", None),
        ] {
            self.order(order, owner, side, quantity, remaining, status, item)
                .await;
        }
        for (trade, bid, ask, buyer, seller, quantity, status) in [
            (
                PENDING,
                Some(BID_FILLED),
                Some(ASK_PARTLY),
                COUNTERPARTY,
                SUBJECT,
                1,
                "PENDING_CONFIRMATION",
            ),
            (
                PENDING_TAKEN,
                Some(BID_TAKEN),
                Some(ASK_TAKEN),
                COUNTERPARTY,
                SUBJECT,
                1,
                "PENDING_CONFIRMATION",
            ),
            (CONFIRMED, None, None, COUNTERPARTY, SUBJECT, 2, "CONFIRMED"),
            (DELIVERED, None, None, COUNTERPARTY, SUBJECT, 1, "DELIVERED"),
            (
                COLLEAGUE_PENDING,
                None,
                None,
                COUNTERPARTY,
                COLLEAGUE,
                1,
                "PENDING_CONFIRMATION",
            ),
        ] {
            sqlx::query("SELECT offboarding_market_trade($1,$2,$3,$4,$5,$6,$7)")
                .bind(trade)
                .bind(bid)
                .bind(ask)
                .bind(buyer)
                .bind(seller)
                .bind(d(quantity))
                .bind(status)
                .execute(&mut self.admin)
                .await
                .unwrap();
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn order(
        &mut self,
        order: Uuid,
        owner: Uuid,
        side: &str,
        quantity: i64,
        remaining: i64,
        status: &str,
        item: Option<Uuid>,
    ) {
        sqlx::query("SELECT offboarding_market_order($1,$2,$3,$4,$5,$6,$7)")
            .bind(order)
            .bind(owner)
            .bind(side)
            .bind(d(quantity))
            .bind(d(remaining))
            .bind(status)
            .bind(item)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT composition_snapshot($1)")
            .bind(SUBJECT)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn offboard(&self, router: &Router) -> (StatusCode, Value) {
        send(
            router,
            "POST",
            &format!("/api/auth/admin/users/{SUBJECT}/offboard"),
            Some(ADMIN),
            Some(offboard_body(Some(SUBJECT_ORGANIZATION))),
            &[],
        )
        .await
    }

    async fn preview(&self) -> (StatusCode, Value) {
        send(
            &self.served,
            "GET",
            &format!("/api/auth/admin/users/{SUBJECT}/offboarding-preview"),
            Some(ADMIN),
            None,
            &[],
        )
        .await
    }

    /// The serving command for the departing person, as the route builds it,
    /// for the owner's completion seam.
    fn command(
        &self,
        unusable_hash: UnusablePasswordHash,
    ) -> OffboardAccount<
        Arc<verdaxis_read::business::offboarding::UserMarketInvalidation>,
        Arc<impl verdaxis_read::business::admission::AdmissionEvents + use<>>,
    > {
        let service = verdaxis_read::runtime::offboarding::service(&self.state);
        OffboardAccount {
            subject_id: SUBJECT,
            request: OffboardRequest::parse(
                &serde_json::to_vec(&offboard_body(Some(SUBJECT_ORGANIZATION))).unwrap(),
            )
            .unwrap(),
            unusable_hash,
            audit: AuditContext::default(),
            market: service.market.clone(),
            events: service.events.clone(),
        }
    }

    fn access(&self, user: Uuid) -> verdaxis_read::auth::VerifiedAccess {
        verify_access(
            &token(user),
            &VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into()),
            Utc::now(),
        )
        .unwrap()
    }

    async fn observer(&self) -> PgConnection {
        PgConnection::connect(&self.database.admin_url)
            .await
            .unwrap()
    }
}

async fn send(
    router: &Router,
    method: &str,
    path: &str,
    caller: Option<Uuid>,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json");
    if let Some(caller) = caller {
        request = request.header("authorization", format!("Bearer {}", token(caller)));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

fn row<'a>(snapshot: &'a Value, table: &str, id: Uuid) -> &'a Value {
    let value = &snapshot[table][id.to_string()];
    assert!(!value.is_null(), "{table} row {id} is missing");
    value
}

fn decimal(value: &Value) -> Decimal {
    match value {
        Value::Number(number) => number.to_string().parse().unwrap(),
        Value::String(text) => text.parse().unwrap(),
        other => panic!("not a decimal: {other}"),
    }
}

/// The backend of the one runtime transaction that is idle while open: the
/// command paused before its commit.
async fn paused_transaction(observer: &mut PgConnection, role: &str) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let paused: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
             AND usename=$1 AND state='idle in transaction'",
        )
        .bind(role)
        .fetch_all(&mut *observer)
        .await
        .unwrap();
        if let [pid] = paused[..] {
            return pid;
        }
        assert!(tokio::time::Instant::now() < deadline, "{paused:?}");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ---------------------------------------------------------------------------
// One transaction.
// ---------------------------------------------------------------------------

/// One served command ends the account and the market scope together. The
/// preview, read first, counts exactly the orders the command withdraws (the
/// fully taken ASK of a pending trade included) and the trades it cancels.
/// The offboarding record links to those rows and states the released
/// tonnes. Every row the command wrote carries the same transaction ID, and
/// every market row records the marker's database time.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commit_contains_all_account_market_audit_and_outbox_changes() {
    let mut world = World::new().await;
    world.standard_market().await;
    let before = world.snapshot().await;

    let (status, preview) = world.preview().await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["open_owned_orders"], 3);
    assert_eq!(preview["pending_trades"], 2);
    assert_eq!(preview["frozen_pending_trades"], 0);
    assert_eq!(preview["confirmed_trades"], 1);
    assert_eq!(preview["delivered_trades"], 1);
    assert_eq!(preview["organization_id"], SUBJECT_ORGANIZATION.to_string());

    let (status, result) = world.offboard(&world.served.clone()).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["status"], "REJECTED");
    assert_eq!(result["user_id"], SUBJECT.to_string());
    assert_eq!(
        result["former_organization_id"],
        SUBJECT_ORGANIZATION.to_string()
    );
    let operation: Uuid = result["operation_id"].as_str().unwrap().parse().unwrap();
    let after = world.snapshot().await;

    // Account and credentials.
    let user = &after["user"];
    assert_eq!(user["status"], "REJECTED");
    assert!(user["organization_id"].is_null());
    assert!(!user["offboarded_at"].is_null());
    assert_ne!(user["password_hash"], before["user"]["password_hash"]);
    let marked: DateTime<Utc> = sqlx::query_scalar("SELECT offboarded_at FROM users WHERE id=$1")
        .bind(SUBJECT)
        .fetch_one(&mut world.admin)
        .await
        .unwrap();
    let live_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked",
    )
    .bind(SUBJECT)
    .fetch_one(&mut world.admin)
    .await
    .unwrap();
    assert_eq!(live_sessions, 0);
    assert_eq!(after["reset"], json!([]));
    assert_eq!(after["joins"][0]["status"], "REJECTED");
    assert_eq!(after["records"], 1);

    // Market scope: the three ASKs withdrawn, both pending trades cancelled,
    // their counterparty BIDs restored, the item released once; everything
    // else as it was.
    for order in [ASK_PARTLY, ASK_OPEN, ASK_TAKEN] {
        assert_eq!(row(&after, "orders", order)["status"], "CANCELLED");
    }
    assert_eq!(
        decimal(&row(&after, "orders", ASK_PARTLY)["remaining_quantity_mt"]),
        d(5)
    );
    assert_eq!(
        decimal(&row(&after, "orders", ASK_TAKEN)["remaining_quantity_mt"]),
        d(1)
    );
    for bid in [BID_FILLED, BID_TAKEN] {
        let bid = row(&after, "orders", bid);
        assert_eq!(
            (
                bid["status"].as_str(),
                decimal(&bid["remaining_quantity_mt"])
            ),
            (Some("OPEN"), d(1))
        );
    }
    for trade in [PENDING, PENDING_TAKEN] {
        assert_eq!(row(&after, "trades", trade)["status"], "CANCELLED");
    }
    let item = row(&after, "inventory", ITEM);
    assert_eq!(decimal(&item["current_stock_mt"]), d(11));
    assert_eq!(decimal(&item["reserved_stock_mt"]), Decimal::ZERO);
    for (table, id) in [
        ("trades", CONFIRMED),
        ("trades", DELIVERED),
        ("trades", COLLEAGUE_PENDING),
        ("orders", COLLEAGUE_ASK),
    ] {
        assert_eq!(
            row(&after, table, id),
            row(&before, table, id),
            "{table} {id}"
        );
    }
    for order in [ASK_PARTLY, ASK_OPEN, ASK_TAKEN, BID_FILLED, BID_TAKEN] {
        let updated: DateTime<Utc> =
            sqlx::query_scalar("SELECT updated_at FROM orderbook_orders WHERE id=$1")
                .bind(order)
                .fetch_one(&mut world.admin)
                .await
                .unwrap();
        assert_eq!(updated, marked, "market rows record the marker time");
    }

    // Audit: the offboarding record counts the market work it committed,
    // links to the changed rows and states the released tonnes.
    let record: Value = sqlx::query_scalar(
        "SELECT changes FROM audit_logs WHERE action='admin.user_offboarded' AND resource_id=$1",
    )
    .bind(SUBJECT.to_string())
    .fetch_one(&mut world.admin)
    .await
    .unwrap();
    assert_eq!(record["operation_id"], operation.to_string());
    assert_eq!(record["withdrawn_orders"], 3);
    assert_eq!(record["cancelled_trades"], 2);
    let ids = |key: &str| -> BTreeSet<String> {
        record[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} is a list"))
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        ids("withdrawn_order_ids"),
        BTreeSet::from([ASK_PARTLY, ASK_OPEN, ASK_TAKEN].map(|id| id.to_string()))
    );
    assert_eq!(
        ids("cancelled_trade_ids"),
        BTreeSet::from([PENDING, PENDING_TAKEN].map(|id| id.to_string()))
    );
    assert_eq!(record["released_quantity_mt"], "9", "2/9 t becomes 11/0 t");
    // The preview counted the same rows.
    assert_eq!(preview["open_owned_orders"], record["withdrawn_orders"]);
    assert_eq!(preview["pending_trades"], record["cancelled_trades"]);
    assert!(record["market_events"].as_u64().unwrap() >= 1);
    assert!(
        after["audit"].as_array().unwrap().len() >= 2,
        "the market work wrote its own audit beside the offboarding record"
    );

    // Outbox: the market scope event and the former organization's admission
    // event.
    let events: Vec<(String, String)> = sqlx::query_as(
        "SELECT aggregate_type::text, event_type::text FROM market_event_outbox ORDER BY 1, 2",
    )
    .fetch_all(&mut world.admin)
    .await
    .unwrap();
    let scope_events = events
        .iter()
        .filter(|(aggregate, event)| aggregate == "user" && event == "market_access_invalidated")
        .count();
    assert_eq!(
        scope_events as u64,
        record["market_events"].as_u64().unwrap(),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|(aggregate, event)| aggregate == "user" && event == "market_admission_changed"),
        "{events:?}"
    );
    assert_eq!(events.len(), scope_events + 1, "{events:?}");

    // One transaction: every written row has the same creating transaction.
    let transactions: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT xid FROM (\
           SELECT xmin::text AS xid FROM users WHERE id=$1 \
           UNION ALL SELECT xmin::text FROM refresh_sessions WHERE user_id=$1 \
           UNION ALL SELECT xmin::text FROM organization_join_requests WHERE user_id=$1 \
           UNION ALL SELECT xmin::text FROM user_status_transitions WHERE user_id=$1 \
           UNION ALL SELECT xmin::text FROM individual_offboardings \
           UNION ALL SELECT xmin::text FROM orderbook_orders WHERE id=ANY($2) \
           UNION ALL SELECT xmin::text FROM trades WHERE id=ANY($3) \
           UNION ALL SELECT xmin::text FROM inventory_items WHERE id=$4 \
           UNION ALL SELECT xmin::text FROM market_event_outbox \
           UNION ALL SELECT xmin::text FROM audit_logs \
         ) AS written",
    )
    .bind(SUBJECT)
    .bind(vec![ASK_PARTLY, ASK_OPEN, ASK_TAKEN, BID_FILLED, BID_TAKEN])
    .bind(vec![PENDING, PENDING_TAKEN])
    .bind(ITEM)
    .fetch_all(&mut world.admin)
    .await
    .unwrap();
    assert_eq!(transactions.len(), 1, "{transactions:?}");
    let before_audits = before["audit"].as_array().unwrap().len();
    assert_eq!(before_audits, 0, "the fixture writes no audit row");

    // The preview after the commit shows the marked account and an empty scope.
    let (status, preview) = world.preview().await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(!preview["offboarded_at"].is_null());
    assert_eq!(preview["open_owned_orders"], 0);
    assert_eq!(preview["pending_trades"], 0);
}

// ---------------------------------------------------------------------------
// Atomic refusal.
// ---------------------------------------------------------------------------

/// The market work refuses or fails after the account, credential and
/// membership changes: a frozen pending trade, a scope over the bound, a busy
/// trade row, an injected failure inside the market writes and one after
/// them. Each answer comes after a full rollback: every row is as before, so
/// the person keeps the membership, the sessions and the reset token. After
/// the busy answer the administrator repeats the command once the row is
/// free: it takes effect exactly once, and a further repeat changes nothing.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn market_failure_rolls_back_credentials_and_membership() {
    for case in [
        "frozen",
        "over the bound",
        "busy",
        "inside market work",
        "after market work",
    ] {
        let mut world = World::new().await;
        world.standard_market().await;
        let mut holder = None;
        match case {
            "frozen" => {
                sqlx::query("SELECT offboarding_market_freeze($1,$2)")
                    .bind(PENDING)
                    .bind(ADMIN)
                    .execute(&mut world.admin)
                    .await
                    .unwrap();
                let (status, preview) = world.preview().await;
                assert_eq!(status, StatusCode::OK, "{preview}");
                assert_eq!(
                    preview["frozen_pending_trades"], 1,
                    "the preview shows the blocker"
                );
            }
            "over the bound" => {
                sqlx::query("SELECT offboarding_market_bulk($1,$2,'orders',99)")
                    .bind(SUBJECT)
                    .bind(COUNTERPARTY)
                    .execute(&mut world.admin)
                    .await
                    .unwrap();
                let (status, preview) = world.preview().await;
                assert_eq!(
                    (status, code(&preview)),
                    (StatusCode::CONFLICT, "OFFBOARD_PREVIEW_LIMIT"),
                    "101 open orders"
                );
            }
            "busy" => {
                let mut session = world.observer().await;
                sqlx::raw_sql("BEGIN").execute(&mut session).await.unwrap();
                sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE")
                    .bind(PENDING)
                    .execute(&mut session)
                    .await
                    .unwrap();
                holder = Some(session);
            }
            "inside market work" => {
                world
                    .sql("INSERT INTO composition_failpoints VALUES('trades')")
                    .await
            }
            _ => {
                world
                    .sql("INSERT INTO composition_failpoints VALUES('individual_offboardings')")
                    .await
            }
        }
        let before = world.snapshot().await;
        let (status, body) = world.offboard(&world.served.clone()).await;
        let expected = match case {
            "frozen" => (StatusCode::CONFLICT, "MARKET_INVALIDATION_TRADE_FROZEN"),
            "over the bound" => (StatusCode::CONFLICT, "MARKET_INVALIDATION_LIMIT"),
            "busy" => (StatusCode::CONFLICT, "MARKET_BUSY"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
        };
        assert_eq!((status, code(&body)), expected, "{case}: {body}");
        if let Some(mut session) = holder {
            sqlx::raw_sql("ROLLBACK")
                .execute(&mut session)
                .await
                .unwrap();
        }
        let after = world.snapshot().await;
        assert_eq!(after, before, "{case}: nothing is kept");
        assert_eq!(after["user"]["status"], "APPROVED", "{case}");
        assert_eq!(
            after["user"]["organization_id"],
            SUBJECT_ORGANIZATION.to_string(),
            "{case}"
        );
        assert!(after["user"]["offboarded_at"].is_null(), "{case}");
        assert_eq!(after["sessions"].as_array().unwrap().len(), 2, "{case}");
        assert_eq!(after["reset"].as_array().unwrap().len(), 1, "{case}");
        assert_eq!(after["joins"][0]["status"], "PENDING", "{case}");
        // The person's access still works: nothing was cut off.
        let (status, body) = send(
            &world.served,
            "GET",
            "/api/auth/me",
            Some(SUBJECT),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        if case == "busy" {
            repeat_after_busy(&mut world).await;
        }
    }
}

/// The repeat after a busy answer: one offboarding, one record, each pending
/// trade cancelled once and the item released once.
async fn repeat_after_busy(world: &mut World) {
    let (status, body) = world.offboard(&world.served.clone()).await;
    assert_eq!(status, StatusCode::OK, "the repeat takes effect: {body}");
    let committed = world.snapshot().await;
    assert_eq!(committed["records"], 1);
    assert!(!committed["user"]["offboarded_at"].is_null());
    for trade in [PENDING, PENDING_TAKEN] {
        assert_eq!(row(&committed, "trades", trade)["status"], "CANCELLED");
    }
    let item = row(&committed, "inventory", ITEM);
    assert_eq!(decimal(&item["current_stock_mt"]), d(11));
    assert_eq!(decimal(&item["reserved_stock_mt"]), Decimal::ZERO);
    let records: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='admin.user_offboarded'")
            .fetch_one(&mut world.admin)
            .await
            .unwrap();
    assert_eq!(records, 1);
    let (status, body) = world.offboard(&world.served.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "OFFBOARD_ALREADY_COMPLETED"),
        "{body}"
    );
    assert_eq!(
        world.snapshot().await,
        committed,
        "a further repeat changes nothing"
    );
}

// ---------------------------------------------------------------------------
// Concurrency.
// ---------------------------------------------------------------------------

/// The owner's organization lock, the first lock the offboarding command
/// waits for. It differs from every market command's organization lock, so a
/// waiting backend running it is the offboarding.
const OFFBOARDING_ORGANIZATION_LOCK: &str =
    "SELECT id FROM organizations WHERE id = ANY($1) ORDER BY id FOR UPDATE";

/// One backend waiting for a row lock held by another.
struct Waiter {
    pid: i32,
    relation: String,
    query: String,
}

/// Wait until a backend other than `holder` waits for a row lock that
/// `holder` holds, and return it. A row-lock waiter holds the tuple lock of
/// the row it waits for, which names the relation.
async fn await_row_waiter(observer: &mut PgConnection, holder: i32) -> Waiter {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let waiter: Option<(i32, String, String)> = sqlx::query_as(
            "SELECT a.pid, c.relname::text, a.query FROM pg_stat_activity a \
             JOIN pg_locks l ON l.pid=a.pid AND l.locktype='tuple' \
             JOIN pg_class c ON c.oid=l.relation \
             WHERE a.datname=current_database() AND a.pid<>pg_backend_pid() AND a.pid<>$1 \
             AND a.wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(a.pid)) LIMIT 1",
        )
        .bind(holder)
        .fetch_optional(&mut *observer)
        .await
        .unwrap();
        if let Some((pid, relation, query)) = waiter {
            return Waiter {
                pid,
                relation,
                query,
            };
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no backend waited for a row held by backend {holder}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// A counterparty takes the departing seller's ASK while the offboarding runs.
/// The two transactions overlap: the test observes the second one, by its
/// backend, waiting for an organization row the first one holds, so it cannot
/// pass with the two run one after the other. Whichever holds the rows first
/// commits first, and the other is ordered after it: no pending trade on the
/// person's order survives the offboarding. Afterwards the person's own
/// command, and a repeat of a command already committed with the same
/// idempotency key, are refused instead of taking effect or replaying the
/// stored receipt.
///
/// With the take first, the offboarding waits under its 500 ms lock bound. A
/// host too slow to commit the take inside it gets `MARKET_BUSY`; that attempt
/// must have kept nothing, and the whole case runs again on a fresh database
/// (at most three attempts). A passing attempt never repeats the command.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_offboard_and_mutation_has_one_ordered_winner() {
    for offboarding_first in [true, false] {
        let mut attempts = 0;
        loop {
            attempts += 1;
            if ordered_race(offboarding_first, attempts).await {
                break;
            }
            assert!(
                attempts < 3,
                "offboarding first {offboarding_first}: busy three times"
            );
        }
        eprintln!("offboarding first {offboarding_first}: passed on attempt {attempts}");
    }
}

/// One attempt of the race. False when the waiting offboarding answered
/// `MARKET_BUSY` and kept nothing.
async fn ordered_race(offboarding_first: bool, attempt: u32) -> bool {
    const TAKEN: Uuid = Uuid::from_u128(0xb001);
    const CANCELLED_BEFORE: Uuid = Uuid::from_u128(0xb002);
    const STOCK: Uuid = Uuid::from_u128(0xb003);
    let case = format!("offboarding first {offboarding_first}, attempt {attempt}");
    let mut world = World::new().await;
    sqlx::query("SELECT offboarding_market_inventory($1,$2,0,3)")
        .bind(STOCK)
        .bind(SUBJECT)
        .execute(&mut world.admin)
        .await
        .unwrap();
    world
        .order(TAKEN, SUBJECT, "ASK", 2, 2, "OPEN", Some(STOCK))
        .await;
    world
        .order(CANCELLED_BEFORE, SUBJECT, "ASK", 1, 1, "OPEN", Some(STOCK))
        .await;
    // The person's own committed command, with a stored receipt.
    let cancel = format!("/api/orderbook/{CANCELLED_BEFORE}");
    let key = [("Idempotency-Key", "cancel-before-offboarding")];
    let (status, body) = send(&world.served, "DELETE", &cancel, Some(SUBJECT), None, &key).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{case}: {body}");
    let (status, _) = send(&world.served, "DELETE", &cancel, Some(SUBJECT), None, &key).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "{case}: the receipt replays before"
    );
    sqlx::query("SELECT offboarding_market_install_gate()")
        .execute(&mut world.admin)
        .await
        .unwrap();
    let mut observer = world.observer().await;
    let take_body = take_request(&mut world.admin, TAKEN).await;

    let (take, waiter, holder) = if offboarding_first {
        // The command holds every lock and pauses before its commit.
        let control = CompletionControl::new(CompletionPoint::BeforeCommit, false);
        let command = tokio::spawn(run_controlled_write(
            world.pool.clone(),
            Arc::new(Semaphore::new(4)),
            world.access(ADMIN),
            world.command(UnusablePasswordHash::prepare().await.unwrap()),
            control.clone(),
        ));
        control.reached().await;
        let role = url::Url::parse(&world.database.runtime_url)
            .unwrap()
            .username()
            .to_owned();
        let holder = paused_transaction(&mut observer, &role).await;
        let served = world.served.clone();
        let taker = tokio::spawn(async move {
            send(
                &served,
                "POST",
                "/api/trades/",
                Some(COUNTERPARTY),
                Some(take_body),
                &[],
            )
            .await
        });
        // The take waits for the organization row the paused command holds.
        let waiter = await_row_waiter(&mut observer, holder).await;
        assert_ne!(waiter.query, OFFBOARDING_ORGANIZATION_LOCK, "{case}");
        control.release();
        let offboarded = command.await.unwrap();
        assert!(
            offboarded.is_ok(),
            "{case}: {:?}",
            offboarded.err().map(|e| e.detail)
        );
        (taker.await.unwrap(), waiter, holder)
    } else {
        // The take holds its party rows and pauses at its slice lock; the
        // offboarding waits for those rows, then runs after its commit.
        let gated = runtime_pool(&world.database.runtime_url, true).await;
        let taker_router = verdaxis_read::router(application(&world.database.runtime_url, gated));
        let taker = tokio::spawn(async move {
            send(
                &taker_router,
                "POST",
                "/api/trades/",
                Some(COUNTERPARTY),
                Some(take_body),
                &[],
            )
            .await
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let holder: i32 = loop {
            let paused: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datname=current_database() \
                 AND wait_event='PgSleep' LIMIT 1",
            )
            .fetch_optional(&mut observer)
            .await
            .unwrap();
            if let Some(pid) = paused {
                break pid;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{case}: the take did not pause"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        let served = world.served.clone();
        let offboarding = tokio::spawn(async move {
            send(
                &served,
                "POST",
                &format!("/api/auth/admin/users/{SUBJECT}/offboard"),
                Some(ADMIN),
                Some(offboard_body(Some(SUBJECT_ORGANIZATION))),
                &[],
            )
            .await
        });
        // The offboarding, identified by its own lock statement, waits for
        // the organization row the paused take holds.
        let waiter = await_row_waiter(&mut observer, holder).await;
        assert_eq!(waiter.query, OFFBOARDING_ORGANIZATION_LOCK, "{case}");
        sqlx::query("SELECT offboarding_market_open_gate()")
            .execute(&mut world.admin)
            .await
            .unwrap();
        let take = taker.await.unwrap();
        let (status, body) = offboarding.await.unwrap();
        if status == StatusCode::CONFLICT && code(&body) == "MARKET_BUSY" {
            let kept = world.snapshot().await;
            assert!(kept["user"]["offboarded_at"].is_null(), "{case}");
            assert_eq!(kept["records"], 0, "{case}");
            assert_eq!(kept["sessions"].as_array().unwrap().len(), 2, "{case}");
            assert_eq!(kept["user"]["status"], "APPROVED", "{case}");
            eprintln!("{case}: busy while the take committed; nothing kept");
            return false;
        }
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        (take, waiter, holder)
    };
    assert_eq!(waiter.relation, "organizations", "{case}");
    eprintln!(
        "{case}: backend {} waited for an organization row held by backend {holder}",
        waiter.pid
    );

    let after = world.snapshot().await;
    assert!(!after["user"]["offboarded_at"].is_null(), "{case}");
    assert_eq!(after["records"], 1, "{case}");
    let (status, body) = take;
    if offboarding_first {
        assert_ne!(status, StatusCode::OK, "{case}: {body}");
        assert!(after["trades"].as_object().unwrap().is_empty(), "{case}");
    } else {
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        let trade: Uuid = body["id"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            row(&after, "trades", trade)["status"],
            "CANCELLED",
            "{case}"
        );
    }
    // Either order: the ASK is withdrawn with all 2 t and the stock is
    // released once.
    let order = row(&after, "orders", TAKEN);
    assert_eq!(order["status"], "CANCELLED", "{case}");
    assert_eq!(decimal(&order["remaining_quantity_mt"]), d(2), "{case}");
    let stock = row(&after, "inventory", STOCK);
    assert_eq!(decimal(&stock["current_stock_mt"]), d(3), "{case}");
    assert_eq!(
        decimal(&stock["reserved_stock_mt"]),
        Decimal::ZERO,
        "{case}"
    );

    // After the commit: no effect and no replay for the person.
    let (status, body) = send(&world.served, "DELETE", &cancel, Some(SUBJECT), None, &key).await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{case}: the stored receipt is not replayed: {status} {body}"
    );
    let fresh = [("Idempotency-Key", "cancel-after-offboarding")];
    let (status, body) = send(
        &world.served,
        "DELETE",
        &format!("/api/orderbook/{TAKEN}"),
        Some(SUBJECT),
        None,
        &fresh,
    )
    .await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{case}: {status} {body}"
    );
    assert_eq!(world.snapshot().await, after, "{case}: no later effect");
    true
}

/// The body of one take of `order`, with the terms digest of its stored row.
async fn take_request(admin: &mut PgConnection, order: Uuid) -> Value {
    let terms: Value = sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
        .bind(order)
        .fetch_one(admin)
        .await
        .unwrap();
    let digest = verdaxis_read::canonical_terms_from_row(&terms)
        .and_then(|value| verdaxis_read::digest_terms(&value))
        .unwrap();
    json!({"order_id": order, "quantity_mt": "1", "expected_terms_digest": digest})
}

// ---------------------------------------------------------------------------
// Unknown outcome.
// ---------------------------------------------------------------------------

/// The commit succeeds but its acknowledgement is lost, so the administrator
/// gets `COMMIT_OUTCOME_UNKNOWN`. Reading the user through the preview shows
/// the committed marker and the empty market scope. Sending the command again
/// is refused as already completed and changes nothing: no response is stored
/// or replayed, and the work happened once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn unknown_outcome_can_be_resolved_by_admin_user_read() {
    let mut world = World::new().await;
    world.standard_market().await;
    let control = CompletionControl::new(CompletionPoint::AfterCommit, true);
    let command = tokio::spawn(run_controlled_write(
        world.pool.clone(),
        Arc::new(Semaphore::new(4)),
        world.access(ADMIN),
        world.command(UnusablePasswordHash::prepare().await.unwrap()),
        control.clone(),
    ));
    control.reached().await;
    control.release();
    let error = command.await.unwrap().unwrap_err();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.detail["code"], "COMMIT_OUTCOME_UNKNOWN");

    let (status, preview) = world.preview().await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["status"], "REJECTED");
    assert!(!preview["offboarded_at"].is_null(), "the commit is visible");
    assert!(preview["organization_id"].is_null());
    assert_eq!(preview["open_owned_orders"], 0);
    assert_eq!(preview["pending_trades"], 0);

    let committed = world.snapshot().await;
    let (status, body) = world.offboard(&world.served.clone()).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "OFFBOARD_ALREADY_COMPLETED"),
        "{body}"
    );
    assert_eq!(
        world.snapshot().await,
        committed,
        "the repeat changes nothing"
    );
    assert_eq!(committed["records"], 1);
    let offboarding_audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='admin.user_offboarded'")
            .fetch_one(&mut world.admin)
            .await
            .unwrap();
    assert_eq!(offboarding_audits, 1);
    let cancelled: BTreeSet<String> = committed["trades"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, trade)| trade["status"] == "CANCELLED")
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        cancelled,
        BTreeSet::from([PENDING, PENDING_TAKEN].map(|id| id.to_string()))
    );
    let _ = IGNORE;
}

/// The command's connection ends while its COMMIT is in flight: the server
/// terminated the backend, so the COMMIT gets the termination notice instead
/// of a result. The outcome is unknown to the client, so the answer is
/// `503 COMMIT_OUTCOME_UNKNOWN` without `Retry-After`. Reading the user
/// through the preview decides it: here the account is not offboarded and the
/// market scope is as before, and repeating the command takes effect exactly
/// once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn commit_time_termination_is_unknown_until_the_user_is_read() {
    let mut world = World::new().await;
    world.standard_market().await;
    let before = world.snapshot().await;
    let control = CompletionControl::new(CompletionPoint::BeforeCommit, false);
    let command = tokio::spawn(run_controlled_write(
        world.pool.clone(),
        Arc::new(Semaphore::new(4)),
        world.access(ADMIN),
        world.command(UnusablePasswordHash::prepare().await.unwrap()),
        control.clone(),
    ));
    control.reached().await;
    let role = url::Url::parse(&world.database.runtime_url)
        .unwrap()
        .username()
        .to_owned();
    let mut observer = world.observer().await;
    let paused = paused_transaction(&mut observer, &role).await;
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(paused)
        .fetch_one(&mut observer)
        .await
        .unwrap();
    assert!(terminated);
    control.release();
    let error = command.await.unwrap().unwrap_err();
    let answer = (
        error.status,
        error.detail["code"].as_str().unwrap_or_default().to_owned(),
    );
    assert_eq!(
        (answer.0, answer.1.as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, "COMMIT_OUTCOME_UNKNOWN")
    );
    assert_eq!(error.retry_after, None, "the client re-reads, not waits");

    let (status, preview) = world.preview().await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(preview["offboarded_at"].is_null(), "not offboarded");
    assert_eq!(preview["status"], "APPROVED");
    assert_eq!(preview["organization_id"], SUBJECT_ORGANIZATION.to_string());
    assert_eq!(preview["open_owned_orders"], 3);
    assert_eq!(preview["pending_trades"], 2);
    assert_eq!(world.snapshot().await, before, "nothing committed");

    let (status, body) = world.offboard(&world.served.clone()).await;
    assert_eq!(status, StatusCode::OK, "the repeat takes effect: {body}");
    let committed = world.snapshot().await;
    assert_eq!(committed["records"], 1);
    for trade in [PENDING, PENDING_TAKEN] {
        assert_eq!(row(&committed, "trades", trade)["status"], "CANCELLED");
    }
    let item = row(&committed, "inventory", ITEM);
    assert_eq!(decimal(&item["current_stock_mt"]), d(11));
}

/// The command's connection ends while a statement before the COMMIT is
/// running: here it waits for a pending trade row another session holds. The
/// error comes before any COMMIT is sent, so it keeps its classification,
/// `500 DATABASE_ERROR`, and nothing is kept. Once the row is free, the repeat
/// takes effect exactly once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn termination_before_commit_keeps_its_classification() {
    let mut world = World::new().await;
    world.standard_market().await;
    let before = world.snapshot().await;
    let mut holder = world.observer().await;
    sqlx::raw_sql("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE")
        .bind(PENDING)
        .execute(&mut holder)
        .await
        .unwrap();
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut holder)
        .await
        .unwrap();
    let served = world.served.clone();
    let command = tokio::spawn(async move {
        send(
            &served,
            "POST",
            &format!("/api/auth/admin/users/{SUBJECT}/offboard"),
            Some(ADMIN),
            Some(offboard_body(Some(SUBJECT_ORGANIZATION))),
            &[],
        )
        .await
    });
    let mut observer = world.observer().await;
    let waiter = await_row_waiter(&mut observer, holder_pid).await;
    assert_eq!(waiter.relation, "trades");
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(waiter.pid)
        .fetch_one(&mut observer)
        .await
        .unwrap();
    assert!(terminated);
    let (status, body) = command.await.unwrap();
    assert_eq!(
        (status, code(&body)),
        (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
        "{body}"
    );
    sqlx::raw_sql("ROLLBACK")
        .execute(&mut holder)
        .await
        .unwrap();
    assert_eq!(world.snapshot().await, before, "nothing is kept");

    let (status, body) = world.offboard(&world.served.clone()).await;
    assert_eq!(status, StatusCode::OK, "the repeat takes effect: {body}");
    let committed = world.snapshot().await;
    assert_eq!(committed["records"], 1);
    for trade in [PENDING, PENDING_TAKEN] {
        assert_eq!(row(&committed, "trades", trade)["status"], "CANCELLED");
    }
}
