//! The individual market scope of account offboarding, on a migrated
//! disposable database with the converged runtime grants.
//!
//! Each case clones a fresh database. The invalidation runs as the runtime
//! role inside a transaction that first holds the caller's prefix: the
//! organization rows, then the actor and subject user rows. Fixture rows are
//! written by the administrator connection.
#![cfg(feature = "contract-tests")]
#[path = "support/market_database.rs"]
mod market_database;

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;
use verdaxis_read::{
    Config, MarketPolicy,
    order_write::{
        InvalidateOrganization, InvalidateUser, InvalidationError, InvalidationResult,
        invalidate_organization, invalidate_user,
    },
};

const SUBJECT_ORGANIZATION: Uuid = Uuid::from_u128(0x5000);
const SUBJECT: Uuid = Uuid::from_u128(0x5001);
const COLLEAGUE: Uuid = Uuid::from_u128(0x5002);
const COUNTERPARTY_ORGANIZATION: Uuid = Uuid::from_u128(0x6000);
const COUNTERPARTY: Uuid = Uuid::from_u128(0x6001);
const ADMIN: Uuid = Uuid::from_u128(0x7001);

fn now() -> DateTime<Utc> {
    "2026-10-25T12:00:00Z".parse().unwrap()
}

fn d(value: i64) -> Decimal {
    Decimal::from(value)
}

fn id(n: u128) -> Uuid {
    Uuid::from_u128(0xa000 + n)
}

/// Which side of the market the departing person trades on.
#[derive(Clone, Copy, Debug)]
enum SubjectSide {
    Seller,
    Buyer,
}

impl SubjectSide {
    fn own_order_side(self) -> &'static str {
        match self {
            Self::Seller => "ASK",
            Self::Buyer => "BID",
        }
    }
    fn counter_order_side(self) -> &'static str {
        match self {
            Self::Seller => "BID",
            Self::Buyer => "ASK",
        }
    }
    fn role(self) -> &'static str {
        match self {
            Self::Seller => "SUPPLIER",
            Self::Buyer => "BUYER",
        }
    }
    fn counter_role(self) -> &'static str {
        match self {
            Self::Seller => "BUYER",
            Self::Buyer => "SUPPLIER",
        }
    }
    fn organization_type(self) -> &'static str {
        match self {
            Self::Seller => "FUEL_SUPPLIER",
            Self::Buyer => "SHIPOWNER",
        }
    }
    fn counter_organization_type(self) -> &'static str {
        match self {
            Self::Seller => "SHIPOWNER",
            Self::Buyer => "FUEL_SUPPLIER",
        }
    }
}

struct Fixture {
    database: Arc<market_database::MarketDatabase>,
    admin: PgConnection,
    runtime: PgConnection,
    policy: Arc<MarketPolicy>,
    config: Arc<Config>,
    side: SubjectSide,
}

impl Fixture {
    /// The subject and a colleague in one organization, one counterparty
    /// member and one administrator without an organization.
    async fn new(side: SubjectSide) -> Self {
        let database =
            Arc::new(market_database::MarketDatabase::new("RUST_OFFBOARDING_MARKET").await);
        let admin_target = url::Url::parse(&database.admin_url).unwrap();
        let runtime_target = url::Url::parse(&database.runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!(admin_target.path(), runtime_target.path());
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/offboarding_market.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        for (organization, kind, member, role) in [
            (
                Some(SUBJECT_ORGANIZATION),
                side.organization_type(),
                SUBJECT,
                side.role(),
            ),
            (
                Some(SUBJECT_ORGANIZATION),
                side.organization_type(),
                COLLEAGUE,
                side.role(),
            ),
            (
                Some(COUNTERPARTY_ORGANIZATION),
                side.counter_organization_type(),
                COUNTERPARTY,
                side.counter_role(),
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
        let runtime = PgConnection::connect(&database.runtime_url).await.unwrap();
        let policy: MarketPolicy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let config: Config = serde_json::from_value(json!({
            "kyc_execution_gate_enabled":false,"environment":"test","release_sha":"test","migration_revision":"test","database_url":"unused","db_session_time_zone":"UTC","bind":"127.0.0.1:0","pool_max":3,"python_budget":0,"configured_max_connections":100
        }))
        .unwrap();
        Self {
            database,
            admin,
            runtime,
            policy: Arc::new(policy),
            config: Arc::new(config),
            side,
        }
    }

    fn command(&self) -> InvalidateUser {
        InvalidateUser {
            subject_id: SUBJECT,
            actor_id: ADMIN,
            now: now(),
            policy: Arc::clone(&self.policy),
            config: Arc::clone(&self.config),
        }
    }

    /// An order of `owner` on that person's side of the market.
    async fn order(
        &mut self,
        order: Uuid,
        owner: Uuid,
        quantity: i64,
        remaining: i64,
        status: &str,
        item: Option<Uuid>,
    ) {
        let side = if owner == COUNTERPARTY {
            self.side.counter_order_side()
        } else {
            self.side.own_order_side()
        };
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

    /// A trade between `member` of the subject's organization and the
    /// counterparty, linked to the two orders where given.
    async fn trade(
        &mut self,
        trade: Uuid,
        member: Uuid,
        member_order: Option<Uuid>,
        counter_order: Option<Uuid>,
        quantity: i64,
        status: &str,
    ) {
        let (bid, ask, buyer, seller) = match self.side {
            SubjectSide::Seller => (counter_order, member_order, COUNTERPARTY, member),
            SubjectSide::Buyer => (member_order, counter_order, member, COUNTERPARTY),
        };
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

    async fn inventory(&mut self, item: Uuid, owner: Uuid, current: i64, reserved: i64) {
        sqlx::query("SELECT offboarding_market_inventory($1,$2,$3,$4)")
            .bind(item)
            .bind(owner)
            .bind(d(current))
            .bind(d(reserved))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn bulk(&mut self, kind: &str, n: i32) {
        sqlx::query("SELECT offboarding_market_bulk($1,$2,$3,$4)")
            .bind(SUBJECT)
            .bind(COUNTERPARTY)
            .bind(kind)
            .bind(n)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT offboarding_market_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    /// Run the user scope as the runtime role behind the caller's prefix.
    /// The transaction is committed only when `commit` is set.
    async fn invalidate(&mut self, commit: bool) -> Result<InvalidationResult, InvalidationError> {
        let command = self.command();
        let mut tx = self.runtime.begin().await.unwrap();
        hold_prefix(&mut tx, &[SUBJECT_ORGANIZATION], &[ADMIN, SUBJECT]).await;
        let result = invalidate_user(&mut tx, &command).await;
        if commit && result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    }
}

/// The business owner's canonical prefix: organizations, then users, each
/// sorted, each `FOR UPDATE`.
async fn hold_prefix(conn: &mut PgConnection, organizations: &[Uuid], users: &[Uuid]) {
    let mut organizations = organizations.to_vec();
    organizations.sort();
    let mut users = users.to_vec();
    users.sort();
    for organization in organizations {
        sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(organization)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    }
    for user in users {
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    }
}

/// Wait until another session of this database waits on a row lock.
async fn await_lock_wait(observer: &mut PgConnection) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock'",
        )
        .fetch_one(&mut *observer)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no session waited on the held lock"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
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

fn refusal_code(error: &InvalidationError) -> Option<String> {
    match error {
        InvalidationError::Application(error) if error.status == 409 => {
            error.detail["code"].as_str().map(str::to_owned)
        }
        _ => None,
    }
}

fn sqlstate(error: &InvalidationError) -> Option<String> {
    match error {
        InvalidationError::Database(error) => error
            .as_database_error()
            .and_then(|error| error.code())
            .map(|code| code.into_owned()),
        InvalidationError::Application(_) => None,
    }
}

/// Every row of `table` other than `changed` is byte-identical.
fn others_unchanged(before: &Value, after: &Value, table: &str, changed: &[Uuid]) {
    let before = before[table].as_object().unwrap();
    let after = after[table].as_object().unwrap();
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "{table} identities"
    );
    for (key, value) in before {
        if changed.iter().any(|id| id.to_string() == *key) {
            continue;
        }
        assert_eq!(&after[key], value, "{table} row {key} changed");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn only_subject_orders_and_pending_trades_are_invalidated() {
    for side in [SubjectSide::Seller, SubjectSide::Buyer] {
        let mut f = Fixture::new(side).await;
        // The subject: one open order, one partly filled order with a pending
        // trade, one filled order with a confirmed trade.
        let (open, partial, filled) = (id(1), id(2), id(3));
        f.order(open, SUBJECT, 10, 10, "OPEN", None).await;
        f.order(partial, SUBJECT, 10, 6, "PARTIALLY_FILLED", None)
            .await;
        f.order(filled, SUBJECT, 5, 0, "FILLED", None).await;
        // The counterparty order of the pending trade, and one unrelated.
        let (counter, counter_open) = (id(4), id(5));
        f.order(counter, COUNTERPARTY, 10, 6, "PARTIALLY_FILLED", None)
            .await;
        f.order(counter_open, COUNTERPARTY, 8, 8, "OPEN", None)
            .await;
        // The colleague's open order and pending trade stay with the company.
        let (colleague_open, colleague_partial) = (id(6), id(7));
        f.order(colleague_open, COLLEAGUE, 10, 10, "OPEN", None)
            .await;
        f.order(
            colleague_partial,
            COLLEAGUE,
            10,
            7,
            "PARTIALLY_FILLED",
            None,
        )
        .await;
        let (pending, confirmed, colleague_pending) = (id(10), id(11), id(12));
        f.trade(
            pending,
            SUBJECT,
            Some(partial),
            Some(counter),
            4,
            "PENDING_CONFIRMATION",
        )
        .await;
        f.trade(confirmed, SUBJECT, Some(filled), None, 5, "CONFIRMED")
            .await;
        f.trade(
            colleague_pending,
            COLLEAGUE,
            Some(colleague_partial),
            None,
            3,
            "PENDING_CONFIRMATION",
        )
        .await;
        let before = f.snapshot().await;

        let result = f.invalidate(true).await.unwrap();
        let mut withdrawn = vec![open, partial];
        withdrawn.sort();
        assert_eq!(result.withdrawn_orders, withdrawn, "{side:?}");
        assert_eq!(result.cancelled_trade_ids, vec![pending], "{side:?}");
        assert_eq!(result.released_uncommitted_quantity, Decimal::ZERO);
        assert_eq!(result.outbox_ids.len(), 1);

        let after = f.snapshot().await;
        others_unchanged(&before, &after, "orders", &[open, partial, counter]);
        others_unchanged(&before, &after, "trades", &[pending]);
        for (order, status, remaining) in [
            (open, "CANCELLED", 10),
            (partial, "CANCELLED", 10),
            (counter, "OPEN", 10),
        ] {
            let current = row(&after, "orders", order);
            let previous = row(&before, "orders", order);
            assert_eq!(current["status"], status, "{side:?} {order}");
            assert_eq!(decimal(&current["remaining_quantity_mt"]), d(remaining));
            assert_eq!(current["quantity_mt"], previous["quantity_mt"]);
            assert_eq!(current["owner_user_id"], previous["owner_user_id"]);
            assert_eq!(
                current["version"].as_i64().unwrap(),
                previous["version"].as_i64().unwrap() + 1
            );
        }
        let cancelled = row(&after, "trades", pending);
        let mut expected = row(&before, "trades", pending).clone();
        expected["status"] = json!("CANCELLED");
        assert_eq!(*cancelled, expected, "only the status of the trade changes");

        let audits = after["audit"].as_array().unwrap();
        let scope = audits
            .iter()
            .find(|a| a["action"] == "market.access_invalidated")
            .unwrap();
        assert_eq!(scope["resource_type"], "user");
        assert_eq!(scope["resource_id"], SUBJECT.to_string());
        assert_eq!(scope["user_id"], ADMIN.to_string());
        assert_eq!(scope["changes"]["reason"], "account_offboarded");
        assert_eq!(scope["changes"]["cancelled_trade_ids"], json!([pending]));
        for audit in audits {
            assert_ne!(audit["changes"]["reason"], "organization_rejected");
        }
        let cancelled_orders: Vec<_> = audits
            .iter()
            .filter(|a| a["action"] == "order.cancelled")
            .map(|a| a["resource_id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(cancelled_orders.len(), 2);
        assert!(cancelled_orders.contains(&open.to_string()));
        assert!(cancelled_orders.contains(&partial.to_string()));

        let outbox = &after["outbox"][0];
        assert_eq!(outbox["id"], result.outbox_ids[0].to_string());
        assert_eq!(outbox["event_type"], "market_access_invalidated");
        assert_eq!(outbox["aggregate_type"], "user");
        assert_eq!(
            outbox["participant_org_ids"],
            json!([SUBJECT_ORGANIZATION, COUNTERPARTY_ORGANIZATION])
        );
        assert_eq!(
            outbox["payload"],
            json!({"schema_version":1,"cancelled_trade_count":1,"affected_order_count":3,"_public_market_invalidation":true})
        );
        assert!(!outbox["payload"].to_string().contains(&SUBJECT.to_string()));
        assert!(outbox["dispatched_at"].is_null());
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn pending_cancellation_releases_reservations_once() {
    let mut f = Fixture::new(SubjectSide::Seller).await;
    // One inventory item backs two subject ASKs: a 6 t remainder plus a 4 t
    // pending trade, and a 3 t remainder, so 13 t are reserved.
    let (item, partial, open) = (id(20), id(21), id(22));
    f.inventory(item, SUBJECT, 50, 13).await;
    f.order(partial, SUBJECT, 10, 6, "PARTIALLY_FILLED", Some(item))
        .await;
    f.order(open, SUBJECT, 3, 3, "OPEN", Some(item)).await;
    let pending = id(23);
    f.trade(
        pending,
        SUBJECT,
        Some(partial),
        None,
        4,
        "PENDING_CONFIRMATION",
    )
    .await;
    // The subject as buyer of a colleague-free counterparty ASK: the
    // counterparty keeps its 9 t reservation for its restored remainder.
    let (counter_item, counter_ask) = (id(24), id(25));
    f.inventory(counter_item, COUNTERPARTY, 20, 9).await;
    sqlx::query("SELECT offboarding_market_order($1,$2,'ASK',10,5,'PARTIALLY_FILLED',$3)")
        .bind(counter_ask)
        .bind(COUNTERPARTY)
        .bind(counter_item)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let bought = id(26);
    sqlx::query("SELECT offboarding_market_trade($1,NULL,$2,$3,$4,4,'PENDING_CONFIRMATION')")
        .bind(bought)
        .bind(counter_ask)
        .bind(SUBJECT)
        .bind(COUNTERPARTY)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.snapshot().await;

    let result = f.invalidate(true).await.unwrap();
    let mut cancelled = vec![pending, bought];
    cancelled.sort();
    assert_eq!(result.cancelled_trade_ids, cancelled);
    assert_eq!(result.released_uncommitted_quantity, d(13));
    let after = f.snapshot().await;
    let stock = row(&after, "inventory", item);
    assert_eq!(decimal(&stock["current_stock_mt"]), d(63));
    assert_eq!(decimal(&stock["reserved_stock_mt"]), Decimal::ZERO);
    assert_eq!(
        row(&after, "inventory", counter_item),
        row(&before, "inventory", counter_item)
    );
    let counter = row(&after, "orders", counter_ask);
    assert_eq!(counter["status"], "PARTIALLY_FILLED");
    assert_eq!(decimal(&counter["remaining_quantity_mt"]), d(9));
    for order in [partial, open] {
        let withdrawn = row(&after, "orders", order);
        assert_eq!(withdrawn["status"], "CANCELLED");
        assert_eq!(
            withdrawn["quantity_mt"],
            row(&before, "orders", order)["quantity_mt"]
        );
    }

    // A second run finds nothing to change: the stock is released once.
    let settled = f.snapshot().await;
    let again = f.invalidate(true).await.unwrap();
    assert!(again.withdrawn_orders.is_empty());
    assert!(again.cancelled_trade_ids.is_empty());
    assert_eq!(again.released_uncommitted_quantity, Decimal::ZERO);
    assert!(again.outbox_ids.is_empty());
    let repeated = f.snapshot().await;
    for table in ["orders", "trades", "inventory"] {
        assert_eq!(repeated[table], settled[table], "{table}");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn confirmed_and_delivered_obligations_unchanged() {
    for side in [SubjectSide::Seller, SubjectSide::Buyer] {
        let mut f = Fixture::new(side).await;
        // A partly filled order whose filled part is a confirmed obligation,
        // and a filled order with delivered and paid trades.
        let (partial, filled) = (id(30), id(31));
        f.order(partial, SUBJECT, 10, 4, "PARTIALLY_FILLED", None)
            .await;
        f.order(filled, SUBJECT, 6, 0, "FILLED", None).await;
        let (confirmed, delivered, paid) = (id(32), id(33), id(34));
        f.trade(confirmed, SUBJECT, Some(partial), None, 6, "CONFIRMED")
            .await;
        f.trade(delivered, SUBJECT, Some(filled), None, 4, "DELIVERED")
            .await;
        f.trade(paid, SUBJECT, Some(filled), None, 2, "PAID").await;
        // Earlier evidence that names the subject as actor.
        sqlx::query("INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,changes,timestamp) VALUES($1,$2,'trade.confirmed','trade',$3,'{}'::jsonb,'2026-10-20T00:00:00Z')")
            .bind(id(35))
            .bind(SUBJECT)
            .bind(confirmed.to_string())
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;

        let result = f.invalidate(true).await.unwrap();
        assert_eq!(result.withdrawn_orders, vec![partial], "{side:?}");
        assert!(result.cancelled_trade_ids.is_empty());
        let after = f.snapshot().await;
        // Status, quantity, price, commission and recorded people of every
        // obligation stay exactly as they were.
        assert_eq!(after["trades"], before["trades"], "{side:?}");
        others_unchanged(&before, &after, "orders", &[partial]);
        let withdrawn = row(&after, "orders", partial);
        assert_eq!(withdrawn["status"], "CANCELLED");
        assert_eq!(decimal(&withdrawn["remaining_quantity_mt"]), d(4));
        assert_eq!(decimal(&withdrawn["quantity_mt"]), d(10));
        let earlier = before["audit"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id(35).to_string())
            .unwrap();
        assert!(after["audit"].as_array().unwrap().contains(earlier));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn over_100_refuses_without_partial_effects() {
    // (rows, kind, refused): the exact boundary is accepted, one more is not.
    // Confirmed trades and the colleague's orders are outside the bound.
    for (count, kind, refused) in [
        (100, "pending", false),
        (101, "pending", true),
        (100, "orders", false),
        (101, "orders", true),
    ] {
        let mut f = Fixture::new(SubjectSide::Seller).await;
        f.bulk(kind, count).await;
        f.bulk("confirmed", 101).await;
        sqlx::query("SELECT offboarding_market_order(gen_random_uuid(),$1,'ASK',1,1,'OPEN',NULL)")
            .bind(COLLEAGUE)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;
        let result = f.invalidate(false).await;
        if refused {
            let error = result.unwrap_err();
            assert_eq!(
                refusal_code(&error).as_deref(),
                Some("MARKET_INVALIDATION_LIMIT"),
                "{count} {kind}: {error:?}"
            );
        } else {
            let result = result.unwrap();
            let changed = if kind == "pending" {
                result.cancelled_trade_ids.len()
            } else {
                result.withdrawn_orders.len()
            };
            assert_eq!(changed, 100, "{count} {kind}");
        }
        assert_eq!(f.snapshot().await, before, "{count} {kind}");
        // A refused scope is not truncated by a committed attempt either.
        if refused {
            assert!(f.invalidate(true).await.is_err());
            assert_eq!(f.snapshot().await, before, "{count} {kind} committed");
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn new_row_after_discovery_refuses_atomically() {
    // 1 + 1 rows: the late row changes the scope, so the attempt aborts with a
    // retryable conflict. 100 + 1 rows: the late row overflows the bound.
    for (seeded, expected) in [(1, "40001"), (100, "MARKET_INVALIDATION_LIMIT")] {
        let mut f = Fixture::new(SubjectSide::Seller).await;
        f.bulk("pending", seeded).await;
        // A subject ASK gives the scope a slice to lock.
        f.order(id(40), SUBJECT, 2, 2, "OPEN", None).await;
        sqlx::query("SELECT offboarding_market_install_late_row($1,$2)")
            .bind(SUBJECT)
            .bind(COUNTERPARTY)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let before = f.snapshot().await;
        let command = f.command();
        let mut tx = f.runtime.begin().await.unwrap();
        hold_prefix(&mut tx, &[SUBJECT_ORGANIZATION], &[ADMIN, SUBJECT]).await;
        sqlx::raw_sql("SET LOCAL search_path TO offboarding_late_row,public,pg_catalog")
            .execute(&mut *tx)
            .await
            .unwrap();
        let error = invalidate_user(&mut tx, &command).await.unwrap_err();
        tx.rollback().await.unwrap();
        let observed = sqlstate(&error).or_else(|| refusal_code(&error));
        assert_eq!(observed.as_deref(), Some(expected), "{error:?}");
        assert_eq!(f.snapshot().await, before, "{seeded}");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn matching_race_cannot_recreate_exposure() {
    use verdaxis_read::market_contract::matching::{
        MatchPreviewBarrier, run_sealed_match, run_sealed_match_paused,
    };
    for offboarding_first in [true, false] {
        let mut f = Fixture::new(SubjectSide::Seller).await;
        // Three one-tonne subject ASKs at 100 and a 2 t counterparty BID at
        // 110 that crosses them.
        let asks = [id(50), id(51), id(52)];
        for ask in asks {
            f.order(ask, SUBJECT, 1, 1, "OPEN", None).await;
        }
        let bid = id(53);
        f.order(bid, COUNTERPARTY, 2, 2, "OPEN", None).await;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&f.database.runtime_url)
            .await
            .unwrap();
        let mut observer = PgConnection::connect(&f.database.admin_url).await.unwrap();
        let command = f.command();

        if offboarding_first {
            // The offboarding transaction holds the prefix and has withdrawn
            // the ASKs; matching waits on the subject's organization row.
            let mut tx = f.runtime.begin().await.unwrap();
            hold_prefix(&mut tx, &[SUBJECT_ORGANIZATION], &[ADMIN, SUBJECT]).await;
            let result = invalidate_user(&mut tx, &command).await.unwrap();
            assert_eq!(result.withdrawn_orders, asks.to_vec());
            let matcher = pool.clone();
            let pending = tokio::spawn(async move { run_sealed_match(&matcher, bid, now()).await });
            await_lock_wait(&mut observer).await;
            tx.commit().await.unwrap();
            let matched = pending.await.unwrap().expect("matching after commit");
            assert!(matched.outcome.trades.is_empty());
        } else {
            // Matching holds its prefix and slice; the offboarding prefix waits.
            let barrier = Arc::new(MatchPreviewBarrier::default());
            let pending = tokio::spawn(run_sealed_match_paused(
                pool.clone(),
                bid,
                now(),
                barrier.clone(),
            ));
            tokio::time::timeout(Duration::from_secs(10), barrier.previewed())
                .await
                .unwrap();
            let runtime_url = f.database.runtime_url.clone();
            let offboarding = tokio::spawn(async move {
                let mut conn = PgConnection::connect(&runtime_url).await.unwrap();
                let mut tx = conn.begin().await.unwrap();
                hold_prefix(&mut tx, &[SUBJECT_ORGANIZATION], &[ADMIN, SUBJECT]).await;
                let result = invalidate_user(&mut tx, &command).await.unwrap();
                tx.commit().await.unwrap();
                result
            });
            await_lock_wait(&mut observer).await;
            barrier.resume();
            let matched = pending.await.unwrap().expect("matching wins the lock");
            assert_eq!(matched.outcome.trades.len(), 2);
            let filled: Vec<_> = matched
                .outcome
                .trades
                .iter()
                .map(|trade| trade.ask_order_id)
                .collect();
            let result = offboarding.await.unwrap();
            // Only the ASK that matching left open is withdrawn.
            assert_eq!(result.withdrawn_orders.len(), 1);
            assert!(asks.contains(&result.withdrawn_orders[0]));
            assert!(!filled.contains(&result.withdrawn_orders[0]));
            assert!(result.cancelled_trade_ids.is_empty());
        }

        let after = f.snapshot().await;
        // No live subject order and no pending subject trade remain, and
        // every ASK tonne is either in a confirmed trade or withdrawn.
        for ask in asks {
            let order = row(&after, "orders", ask);
            assert!(
                matches!(order["status"].as_str(), Some("CANCELLED" | "FILLED")),
                "{offboarding_first}: {order}"
            );
            let traded: Decimal = after["trades"]
                .as_object()
                .unwrap()
                .values()
                .filter(|t| t["ask_order_id"] == ask.to_string())
                .map(|t| decimal(&t["quantity_mt"]))
                .sum();
            let withdrawn = if order["status"] == "CANCELLED" {
                decimal(&order["remaining_quantity_mt"])
            } else {
                Decimal::ZERO
            };
            assert_eq!(traded + withdrawn, d(1), "{offboarding_first} {ask}");
        }
        let trades = after["trades"].as_object().unwrap();
        for trade in trades.values() {
            assert_eq!(trade["status"], "CONFIRMED");
            assert_eq!(trade["seller_user_id"], SUBJECT.to_string());
        }
        assert_eq!(trades.len(), if offboarding_first { 0 } else { 2 });
        let aggressor = row(&after, "orders", bid);
        let expected = if offboarding_first { 2 } else { 0 };
        assert_eq!(
            decimal(&aggressor["remaining_quantity_mt"]),
            d(expected),
            "{offboarding_first}"
        );
        pool.close().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn empty_scope_writes_no_event_and_unknown_subject_is_refused() {
    let mut f = Fixture::new(SubjectSide::Seller).await;
    // A subject with no market rows writes its scope audit and no event.
    let before = f.snapshot().await;
    let result = f.invalidate(true).await.unwrap();
    assert!(result.withdrawn_orders.is_empty() && result.cancelled_trade_ids.is_empty());
    assert!(result.outbox_ids.is_empty());
    let after = f.snapshot().await;
    assert_eq!(after["outbox"], before["outbox"]);
    assert_eq!(
        after["audit"].as_array().unwrap().len(),
        before["audit"].as_array().unwrap().len() + 1
    );
    // An unknown subject is refused before any write.
    let mut command = f.command();
    command.subject_id = Uuid::from_u128(0xdead);
    let mut tx = f.runtime.begin().await.unwrap();
    let error = invalidate_user(&mut tx, &command).await.unwrap_err();
    tx.rollback().await.unwrap();
    assert_eq!(refusal_code(&error).as_deref(), Some("USER_NOT_FOUND"));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn company_rejection_cancels_pending_trades_and_releases_reservations() {
    let mut f = Fixture::new(SubjectSide::Seller).await;
    // Both members of the rejected company have an inventory-backed ASK with
    // a pending trade; one ASK also carries a confirmed obligation.
    let item = id(60);
    f.inventory(item, SUBJECT, 30, 17).await;
    let (subject_ask, colleague_ask) = (id(61), id(62));
    f.order(subject_ask, SUBJECT, 10, 5, "PARTIALLY_FILLED", Some(item))
        .await;
    f.order(
        colleague_ask,
        COLLEAGUE,
        10,
        6,
        "PARTIALLY_FILLED",
        Some(item),
    )
    .await;
    let counter_bid = id(63);
    f.order(counter_bid, COUNTERPARTY, 10, 4, "PARTIALLY_FILLED", None)
        .await;
    let (subject_pending, colleague_pending, confirmed) = (id(64), id(65), id(66));
    f.trade(
        subject_pending,
        SUBJECT,
        Some(subject_ask),
        Some(counter_bid),
        2,
        "PENDING_CONFIRMATION",
    )
    .await;
    f.trade(
        colleague_pending,
        COLLEAGUE,
        Some(colleague_ask),
        Some(counter_bid),
        4,
        "PENDING_CONFIRMATION",
    )
    .await;
    f.trade(confirmed, SUBJECT, Some(subject_ask), None, 3, "CONFIRMED")
        .await;
    let before = f.snapshot().await;

    let command = InvalidateOrganization {
        organization_id: SUBJECT_ORGANIZATION,
        actor_id: ADMIN,
        now: now(),
        policy: Arc::clone(&f.policy),
        config: Arc::clone(&f.config),
    };
    let mut tx = f.runtime.begin().await.unwrap();
    hold_prefix(
        &mut tx,
        &[SUBJECT_ORGANIZATION],
        &[ADMIN, SUBJECT, COLLEAGUE],
    )
    .await;
    let result = invalidate_organization(&mut tx, &command).await.unwrap();
    tx.commit().await.unwrap();

    let mut cancelled = vec![subject_pending, colleague_pending];
    cancelled.sort();
    assert_eq!(result.cancelled_trade_ids, cancelled);
    let mut withdrawn = vec![subject_ask, colleague_ask];
    withdrawn.sort();
    assert_eq!(result.withdrawn_orders, withdrawn);
    // 5 + 2 and 6 + 4 tonnes return to stock, once.
    assert_eq!(result.released_uncommitted_quantity, d(17));
    let after = f.snapshot().await;
    let stock = row(&after, "inventory", item);
    assert_eq!(decimal(&stock["current_stock_mt"]), d(47));
    assert_eq!(decimal(&stock["reserved_stock_mt"]), Decimal::ZERO);
    for trade in [subject_pending, colleague_pending] {
        assert_eq!(row(&after, "trades", trade)["status"], "CANCELLED");
    }
    // The confirmed obligation stays for settlement.
    assert_eq!(
        row(&after, "trades", confirmed),
        row(&before, "trades", confirmed)
    );
    let counter = row(&after, "orders", counter_bid);
    assert_eq!(counter["status"], "OPEN");
    assert_eq!(decimal(&counter["remaining_quantity_mt"]), d(10));
    let outbox = &after["outbox"][0];
    assert_eq!(outbox["aggregate_type"], "organization");
    assert_eq!(
        outbox["payload"]["organization_id"],
        SUBJECT_ORGANIZATION.to_string()
    );
    assert_eq!(outbox["payload"]["cancelled_trade_count"], 2);
    let earlier = before["audit"].as_array().unwrap();
    let written: Vec<_> = after["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| !earlier.contains(a))
        .collect();
    assert!(!written.is_empty());
    assert!(
        written
            .iter()
            .all(|a| a["changes"]["reason"] == "organization_rejected")
    );
}
