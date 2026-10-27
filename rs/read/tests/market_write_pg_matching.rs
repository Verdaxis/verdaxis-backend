#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;
use verdaxis_read::MarketErrorCode;
use verdaxis_read::market_contract::matching::{
    SealedMatchOptions, apply, lock_and_plan, lock_and_plan_as, lock_previewed_and_plan, preview,
    preview_parties, probe_order_lock, run_sealed_match, run_sealed_match_with,
};
use verdaxis_read::market_matching::{DemoOrderPair, MatchBound};

fn now() -> DateTime<Utc> {
    "2026-10-25T12:00:00Z".parse().unwrap()
}
fn d(raw: &str) -> Decimal {
    Decimal::from_str_exact(raw).unwrap()
}

struct Fixture {
    admin: PgConnection,
    writer: PgConnection,
    observer: PgConnection,
    aggressor: Uuid,
    resting: Vec<Uuid>,
}
impl Fixture {
    async fn new(count: i32, quantity: &str) -> Self {
        let admin_url = std::env::var("RUST_MATCHING_ADMIN_DATABASE_URL").unwrap();
        let runtime_url = std::env::var("RUST_MATCHING_DATABASE_URL").unwrap();
        let admin_target = url::Url::parse(&admin_url).unwrap();
        let runtime_target = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!(admin_target.host_str(), runtime_target.host_str());
        assert_eq!(admin_target.port(), runtime_target.port());
        assert_eq!(admin_target.path(), runtime_target.path());
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/market_write_matching.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let aggressor = Uuid::from_u128(1);
        sqlx::query("SELECT matching_fixture_seed($1,$2,$3)")
            .bind(aggressor)
            .bind(count)
            .bind(d(quantity))
            .execute(&mut admin)
            .await
            .unwrap();
        let resting = sqlx::query_scalar(
            "SELECT id FROM orderbook_orders WHERE side='ASK' ORDER BY acceptance_ordinal,id",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        let writer = PgConnection::connect(&runtime_url).await.unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        Self {
            admin,
            writer,
            observer,
            aggressor,
            resting,
        }
    }
    async fn unchanged(observer: &mut PgConnection, aggressor: Uuid, quantity: &str) {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trades")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let row =
            sqlx::query("SELECT remaining_quantity_mt,version FROM orderbook_orders WHERE id=$1")
                .bind(aggressor)
                .fetch_one(&mut *observer)
                .await
                .unwrap();
        assert_eq!(row.get::<Decimal, _>(0), d(quantity));
        assert_eq!(row.get::<i32, _>(1), 1);
        let versions: bool = sqlx::query_scalar("SELECT bool_and(version=1) FROM orderbook_orders")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
        assert!(versions);
    }
    async fn party(&mut self, user: Uuid, change: &str) {
        sqlx::query("SELECT matching_fixture_party($1,$2)")
            .bind(user)
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn order(&mut self, order: Uuid, change: &str) {
        sqlx::query("SELECT matching_fixture_order($1,$2)")
            .bind(order)
            .bind(change)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn assist(&mut self, order: Uuid, admin: Uuid) {
        sqlx::query("SELECT matching_fixture_assist($1,$2)")
            .bind(order)
            .bind(admin)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    /// Fills a locked plan would make, rolled back so the fixture is unchanged.
    async fn planned_fills(&mut self, bound: MatchBound<'_>) -> Vec<Uuid> {
        let mut tx = self.writer.begin().await.unwrap();
        let planned = lock_and_plan(&mut tx, self.aggressor, bound).await.unwrap();
        tx.rollback().await.unwrap();
        planned
            .plan
            .fills()
            .iter()
            .map(|fill| fill.resting_before.id)
            .collect()
    }
    async fn trades(&mut self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM trades")
            .fetch_one(&mut self.observer)
            .await
            .unwrap()
    }
    async fn conserved(&mut self) {
        let valid: bool = sqlx::query_scalar("SELECT bool_and(o.remaining_quantity_mt=o.quantity_mt-COALESCE(t.total,0)) FROM orderbook_orders o LEFT JOIN LATERAL (SELECT sum(quantity_mt) AS total FROM trades WHERE bid_order_id=o.id OR ask_order_id=o.id) t ON true").fetch_one(&mut self.observer).await.unwrap();
        assert!(valid);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn partial_fill_updates_both_remainders_and_versions() {
    let mut f = Fixture::new(2, "1.25").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills().len(), 2);
    let outcome = apply(&mut tx, planned).await.unwrap();
    assert_eq!(outcome.trades.len(), 2);
    assert_eq!(outcome.changed_orders.len(), 3);
    assert_eq!(outcome.changed_slices.len(), 2);
    Fixture::unchanged(&mut f.observer, f.aggressor, "1.25").await;
    tx.commit().await.unwrap();
    f.conserved().await;
    let rows = sqlx::query(
        "SELECT remaining_quantity_mt,version,status FROM orderbook_orders ORDER BY id",
    )
    .fetch_all(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(rows[0].get::<Decimal, _>(0), d("0"));
    assert_eq!(rows[0].get::<i32, _>(1), 3);
    assert_eq!(rows[0].get::<String, _>(2), "FILLED");
    assert_eq!(rows[2].get::<Decimal, _>(0), d("0.75"));
    assert_eq!(rows[2].get::<i32, _>(1), 2);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn resting_price_becomes_the_trade_price() {
    let mut f = Fixture::new(1, "1").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    let outcome = apply(&mut tx, planned).await.unwrap();
    assert_eq!(outcome.trades[0].price_per_mt_usd, d("100"));
    assert_eq!(outcome.trades[0].confirmed_at, now());
    tx.commit().await.unwrap();
    let row = sqlx::query("SELECT price_per_mt_usd,status,confirmed_at FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(row.get::<Decimal, _>(0), d("100"));
    assert_eq!(row.get::<String, _>(1), "CONFIRMED");
    assert_eq!(row.get::<DateTime<Utc>, _>(2), now());
    f.conserved().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn fanout_over_one_hundred_rejects_the_transaction() {
    let mut f = Fixture::new(101, "101").await;
    let mut tx = f.writer.begin().await.unwrap();
    let error = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap_err();
    assert_eq!(error.code, MarketErrorCode::MatchFanoutExceeded);
    assert_eq!(error.status.as_u16(), 409);
    tx.rollback().await.unwrap();
    Fixture::unchanged(&mut f.observer, f.aggressor, "101").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn exactly_one_hundred_that_finish_the_aggressor_is_allowed() {
    let mut f = Fixture::new(101, "100").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    let outcome = apply(&mut tx, planned).await.unwrap();
    assert_eq!(outcome.trades.len(), 100);
    tx.commit().await.unwrap();
    f.conserved().await;
    let last =
        sqlx::query("SELECT remaining_quantity_mt,version FROM orderbook_orders WHERE id=$1")
            .bind(f.resting[100])
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(last.get::<Decimal, _>(0), d("1"));
    assert_eq!(last.get::<i32, _>(1), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expired_and_terminal_rows_are_skipped() {
    let mut f = Fixture::new(3, "3").await;
    sqlx::query("SELECT matching_fixture_unavailable($1,$2,$3)")
        .bind(f.resting[0])
        .bind(f.resting[1])
        .bind(now())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills().len(), 1);
    assert_eq!(planned.plan.fills()[0].resting_before.id, f.resting[2]);
    apply(&mut tx, planned).await.unwrap();
    tx.commit().await.unwrap();
    f.conserved().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn caller_rollback_removes_every_fill_and_order_change() {
    let mut f = Fixture::new(3, "3").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    apply(&mut tx, planned).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trades")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 3);
    Fixture::unchanged(&mut f.observer, f.aggressor, "3").await;
    tx.rollback().await.unwrap();
    Fixture::unchanged(&mut f.observer, f.aggressor, "3").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn later_insert_failure_rolls_back_the_complete_prefix() {
    let mut f = Fixture::new(3, "3").await;
    sqlx::query("SELECT matching_fixture_reject_trade($1)")
        .bind(f.resting[1])
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert!(apply(&mut tx, planned).await.is_err());
    tx.rollback().await.unwrap();
    Fixture::unchanged(&mut f.observer, f.aggressor, "3").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn preview_and_locked_selection_keep_acceptance_priority() {
    let mut f = Fixture::new(3, "1.25").await;
    sqlx::query("SELECT matching_fixture_priority($1,$2,$3)")
        .bind(f.resting[0])
        .bind(f.resting[1])
        .bind(f.resting[2])
        .execute(&mut f.admin)
        .await
        .unwrap();
    let candidates = preview(&mut f.writer, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(
        candidates.iter().map(|o| o.id).collect::<Vec<_>>(),
        vec![f.resting[2], f.resting[1], f.resting[0]]
    );
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills()[0].resting_before.id, f.resting[2]);
    assert_eq!(planned.plan.fills()[1].resting_before.id, f.resting[1]);
    tx.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn lookahead_row_is_not_locked_or_changed() {
    let mut f = Fixture::new(101, "100").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    let mut probe = f.observer.begin().await.unwrap();
    let unlocked = probe_order_lock(&mut probe, f.resting[100]).await.unwrap();
    assert_eq!(unlocked, f.resting[100]);
    probe.rollback().await.unwrap();
    let mut probe = f.observer.begin().await.unwrap();
    let busy = probe_order_lock(&mut probe, f.resting[0])
        .await
        .unwrap_err();
    assert_eq!(
        busy.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    probe.rollback().await.unwrap();
    apply(&mut tx, planned).await.unwrap();
    tx.rollback().await.unwrap();
    Fixture::unchanged(&mut f.observer, f.aggressor, "100").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_matches_do_not_consume_the_same_remainder_twice() {
    let mut f = Fixture::new(1, "1").await;
    let mut tx = f.writer.begin().await.unwrap();
    let first = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    let aggressor = f.aggressor;
    let url = std::env::var("RUST_MATCHING_DATABASE_URL").unwrap();
    let mut second = PgConnection::connect(&url).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut second)
        .await
        .unwrap();
    let contender = tokio::spawn(async move {
        let mut tx = second.begin().await.unwrap();
        let result = lock_and_plan(&mut tx, aggressor, MatchBound::new(now())).await;
        tx.rollback().await.unwrap();
        result
    });
    let mut waiting = false;
    for _ in 0..100 {
        waiting = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND NOT granted)",
        )
        .bind(pid)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(
        waiting,
        "the independent transaction must reach a held lock"
    );
    apply(&mut tx, first).await.unwrap();
    tx.commit().await.unwrap();
    let result = contender.await.unwrap();
    match result {
        Ok(plan) => assert!(plan.plan.fills().is_empty()),
        Err(error) => assert!(matches!(
            error.code,
            MarketErrorCode::MarketSliceChanged | MarketErrorCode::MarketBusy
        )),
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.conserved().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn sealed_matching_composes_stock_and_before_effects() {
    let mut f = Fixture::new(1, "1").await;
    let item = Uuid::new_v4();
    sqlx::query("SELECT matching_fixture_back_inventory($1,$2)")
        .bind(f.resting[0])
        .bind(item)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUST_MATCHING_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let result =
        verdaxis_read::market_contract::matching::run_sealed_match(&pool, f.aggressor, now())
            .await
            .unwrap();
    assert_eq!(result.outcome.trades.len(), 1);
    assert_eq!(result.captured_orders.len(), 2);
    assert!(result.captured_orders.contains(&f.aggressor));
    assert!(result.captured_orders.contains(&f.resting[0]));
    assert_eq!(result.order_lock_order, vec![f.resting[0], f.aggressor]);
    assert!(result.inventory_after_orders);
    let reserved: Decimal =
        sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items WHERE id=$1")
            .bind(item)
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(reserved, Decimal::ZERO);
    let times: Vec<DateTime<Utc>> =
        sqlx::query_scalar("SELECT updated_at FROM orderbook_orders ORDER BY id")
            .fetch_all(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(times, vec![now(), now()]);
    f.conserved().await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn independent_sealed_commands_compete_for_one_resting_remainder() {
    let mut f = Fixture::new(1, "1").await;
    let other = Uuid::from_u128(900);
    sqlx::query("SELECT matching_fixture_second_aggressor($1,$2)")
        .bind(f.aggressor)
        .bind(other)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUST_MATCHING_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        verdaxis_read::market_contract::matching::run_sealed_match(&pool, f.aggressor, now()),
        verdaxis_read::market_contract::matching::run_sealed_match(&pool, other, now())
    );
    let mut fills = 0;
    for result in [left, right] {
        match result {
            Ok(result) => fills += result.outcome.trades.len(),
            Err(error) => assert!(matches!(
                error.code,
                MarketErrorCode::MarketBusy | MarketErrorCode::TransactionRetryExhausted
            )),
        }
    }
    assert_eq!(fills, 1);
    let orders = sqlx::query(
        "SELECT id,remaining_quantity_mt,version,updated_at FROM orderbook_orders ORDER BY id",
    )
    .fetch_all(&mut f.observer)
    .await
    .unwrap();
    let mut filled_bids = 0;
    for row in orders {
        let id: Uuid = row.get(0);
        let remainder: Decimal = row.get(1);
        let version: i32 = row.get(2);
        if id == f.resting[0] || remainder == Decimal::ZERO {
            assert_eq!(remainder, Decimal::ZERO);
            assert_eq!(version, 2);
            assert_eq!(row.get::<DateTime<Utc>, _>(3), now());
            if id != f.resting[0] {
                filled_bids += 1;
            }
        } else {
            assert_eq!(remainder, d("1"));
            assert_eq!(version, 1);
        }
    }
    assert_eq!(filled_bids, 1);
    let total: Decimal = sqlx::query_scalar("SELECT sum(quantity_mt) FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(total, d("1"));
    let effects: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM audit_logs)+(SELECT count(*) FROM notifications)+(SELECT count(*) FROM market_event_outbox)")
        .fetch_one(&mut f.observer).await.unwrap();
    assert_eq!(effects, 0, "capture does not emit completion descriptors");
    f.conserved().await;
    pool.close().await;
}

// ---------------------------------------------------------------------------
// Execution eligibility under the held party locks.
// ---------------------------------------------------------------------------

const BUYER_USER: Uuid = Uuid::from_u128(0x1001);
const SELLER_USER: Uuid = Uuid::from_u128(0x2001);
const SELLER_ORG: Uuid = Uuid::from_u128(0x2000);
const ADMIN: Uuid = Uuid::from_u128(0x7000);

async fn admin_connection() -> PgConnection {
    PgConnection::connect(&std::env::var("RUST_MATCHING_ADMIN_DATABASE_URL").unwrap())
        .await
        .unwrap()
}
async fn runtime_connection() -> PgConnection {
    PgConnection::connect(&std::env::var("RUST_MATCHING_DATABASE_URL").unwrap())
        .await
        .unwrap()
}
async fn runtime_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUST_MATCHING_DATABASE_URL").unwrap())
        .await
        .unwrap()
}
async fn backend_pid(conn: &mut PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(conn)
        .await
        .unwrap()
}
/// Whether `pid` reaches a lock another transaction holds.
async fn waits_on_a_lock(observer: &mut PgConnection, pid: i32) -> bool {
    for _ in 0..2000 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND NOT granted)",
        )
        .bind(pid)
        .fetch_one(&mut *observer)
        .await
        .unwrap();
        if waiting {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    false
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn owner_and_organization_are_revalidated_under_lock() {
    let mut f = Fixture::new(1, "1").await;
    // The resting owner may execute only through an unexpired grace.
    f.party(SELLER_USER, "kyc_pending").await;
    f.party(SELLER_USER, "grace").await;
    assert_eq!(
        f.planned_fills(MatchBound::new(now())).await,
        vec![f.resting[0]]
    );
    // An independent writer ends the grace and holds the resting row, so the
    // locked re-selection reads the grace before the writer commits.
    let mut holder = admin_connection().await;
    let mut holding = holder.begin().await.unwrap();
    sqlx::query(
        "UPDATE kyc_execution_grace SET expires_at='2026-10-25T11:00:00Z' WHERE user_id=$1",
    )
    .bind(SELLER_USER)
    .execute(&mut *holding)
    .await
    .unwrap();
    probe_order_lock(&mut holding, f.resting[0]).await.unwrap();
    let mut contender = runtime_connection().await;
    let pid = backend_pid(&mut contender).await;
    let aggressor = f.aggressor;
    let planning = tokio::spawn(async move {
        let mut tx = contender.begin().await.unwrap();
        let planned = lock_and_plan(&mut tx, aggressor, MatchBound::new(now())).await;
        tx.rollback().await.unwrap();
        planned.map(|planned| planned.plan.fills().len())
    });
    let mut observer = admin_connection().await;
    assert!(
        waits_on_a_lock(&mut observer, pid).await,
        "the locked re-selection must wait on the held row"
    );
    holding.commit().await.unwrap();
    let fills = planning.await.unwrap().unwrap();
    assert_eq!(
        fills, 0,
        "the owner decision reads the grace after the row lock"
    );
    Fixture::unchanged(&mut f.observer, f.aggressor, "1").await;

    // Organization approval is revalidated too.
    let mut f = Fixture::new(1, "1").await;
    f.party(SELLER_USER, "organization_pending").await;
    assert!(f.planned_fills(MatchBound::new(now())).await.is_empty());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn each_party_predicate_is_revalidated_with_the_gate_on_and_off() {
    // (changes to the resting owner, fills with the gate on, fills with it off)
    let cases: [(&[&str], usize, usize); 14] = [
        (&[], 1, 1),
        (&["role_admin"], 0, 0),
        (&["pending"], 0, 0),
        (&["rejected"], 0, 0),
        (&["email_unverified"], 0, 0),
        (&["password_change"], 0, 0),
        (&["moved"], 0, 0),
        (&["organization_pending"], 0, 0),
        (&["kyc_pending"], 0, 1),
        (&["kyc_rejected"], 0, 0),
        (&["kyc_other_organization"], 0, 0),
        (&["kyc_pending", "grace"], 1, 1),
        (&["kyc_rejected", "grace"], 0, 0),
        (&["kyc_pending", "grace", "grace_expired"], 0, 1),
    ];
    for (changes, gate_on, gate_off) in cases {
        let mut f = Fixture::new(1, "1").await;
        for change in changes {
            f.party(SELLER_USER, change).await;
        }
        let on = f.planned_fills(MatchBound::new(now())).await.len();
        let off = f
            .planned_fills(MatchBound::new(now()).with_kyc_gate(false))
            .await
            .len();
        assert_eq!((on, off), (gate_on, gate_off), "{changes:?}");
    }
    // The aggressor's owner is revalidated the same way.
    for change in ["role_admin", "email_unverified", "kyc_rejected", "moved"] {
        let mut f = Fixture::new(1, "1").await;
        f.party(BUYER_USER, change).await;
        assert!(
            f.planned_fills(MatchBound::new(now())).await.is_empty(),
            "{change}"
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn membership_and_kyc_changes_before_the_lock_are_seen_after_it() {
    for change in ["kyc_rejected", "moved"] {
        let mut f = Fixture::new(1, "1").await;
        let previewed = preview_parties(
            &mut f.writer,
            f.aggressor,
            BUYER_USER,
            MatchBound::new(now()),
        )
        .await
        .unwrap();
        assert!(previewed.users().contains(&SELLER_USER));
        f.party(SELLER_USER, change).await;
        let mut tx = f.writer.begin().await.unwrap();
        let planned = lock_previewed_and_plan(&mut tx, &previewed, MatchBound::new(now()))
            .await
            .unwrap();
        assert!(planned.plan.fills().is_empty(), "{change}");
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn kyc_writers_wait_for_the_held_party_locks() {
    let mut f = Fixture::new(1, "1").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills().len(), 1);
    let mut writer = admin_connection().await;
    let pid = backend_pid(&mut writer).await;
    let rejection = tokio::spawn(async move {
        sqlx::query("UPDATE users SET kyc_status='REJECTED' WHERE id=$1")
            .bind(SELLER_USER)
            .execute(&mut writer)
            .await
            .unwrap();
    });
    let mut observer = admin_connection().await;
    assert!(
        waits_on_a_lock(&mut observer, pid).await,
        "a KYC writer waits for the held owner lock"
    );
    apply(&mut tx, planned).await.unwrap();
    tx.commit().await.unwrap();
    rejection.await.unwrap();
    assert_eq!(f.trades().await, 1);
    f.conserved().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn assisted_resting_order_needs_one_locked_eligible_customer_member() {
    // Valid: the supplier member of the customer organization authorizes.
    let mut f = Fixture::new(1, "1").await;
    f.assist(f.resting[0], ADMIN).await;
    let pool = runtime_pool().await;
    let result = run_sealed_match(&pool, f.aggressor, now()).await.unwrap();
    assert_eq!(result.outcome.trades.len(), 1);
    let seller: Option<Uuid> = sqlx::query_scalar("SELECT seller_user_id FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(seller, Some(ADMIN));
    f.conserved().await;

    // Absent or rejected member: no customer authority.
    for change in ["absent", "kyc_rejected"] {
        let mut f = Fixture::new(1, "1").await;
        f.assist(f.resting[0], ADMIN).await;
        f.party(SELLER_USER, change).await;
        let result = run_sealed_match(&pool, f.aggressor, now()).await.unwrap();
        assert!(result.outcome.trades.is_empty(), "{change}");
        assert_eq!(f.trades().await, 0);
    }

    // Contended member: an open KYC writer holds the member row, so the
    // prefix cannot lock it and the command is refused without effect.
    let mut f = Fixture::new(1, "1").await;
    f.assist(f.resting[0], ADMIN).await;
    let mut holder = admin_connection().await;
    let mut holding = holder.begin().await.unwrap();
    sqlx::query("UPDATE users SET kyc_status=kyc_status WHERE id=$1")
        .bind(SELLER_USER)
        .execute(&mut *holding)
        .await
        .unwrap();
    let error = run_sealed_match(&pool, f.aggressor, now())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error.code,
            MarketErrorCode::MarketBusy | MarketErrorCode::MarketTimeout
        ),
        "{:?}",
        error.code
    );
    holding.rollback().await.unwrap();
    assert_eq!(f.trades().await, 0);
    Fixture::unchanged(&mut f.observer, f.aggressor, "1").await;
    let result = run_sealed_match(&pool, f.aggressor, now()).await.unwrap();
    assert_eq!(result.outcome.trades.len(), 1);

    // A member who joins after the preview is not locked and cannot authorize.
    let mut f = Fixture::new(1, "1").await;
    f.assist(f.resting[0], ADMIN).await;
    let previewed = preview_parties(
        &mut f.writer,
        f.aggressor,
        BUYER_USER,
        MatchBound::new(now()),
    )
    .await
    .unwrap();
    assert!(previewed.users().contains(&SELLER_USER));
    f.party(SELLER_USER, "kyc_rejected").await;
    let joined = Uuid::from_u128(0x2002);
    sqlx::query("SELECT matching_fixture_member($1,$2,'SUPPLIER')")
        .bind(SELLER_ORG)
        .bind(joined)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_previewed_and_plan(&mut tx, &previewed, MatchBound::new(now()))
        .await
        .unwrap();
    assert!(planned.plan.fills().is_empty());
    tx.rollback().await.unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn assisted_aggressor_executes_for_its_customer_organization() {
    let pool = runtime_pool().await;
    let mut f = Fixture::new(1, "1").await;
    f.assist(f.aggressor, ADMIN).await;
    let result = run_sealed_match(&pool, f.aggressor, now()).await.unwrap();
    assert_eq!(result.outcome.trades.len(), 1);
    let buyer: (Uuid, Option<Uuid>) = sqlx::query_as("SELECT buyer_id,buyer_user_id FROM trades")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(buyer, (Uuid::from_u128(0x1000), Some(ADMIN)));
    f.conserved().await;

    let mut f = Fixture::new(1, "1").await;
    f.assist(f.aggressor, ADMIN).await;
    f.party(BUYER_USER, "kyc_rejected").await;
    let result = run_sealed_match(&pool, f.aggressor, now()).await.unwrap();
    assert!(result.outcome.trades.is_empty());
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn ownerless_rows_never_fill_and_do_not_count_toward_the_bound() {
    let mut f = Fixture::new(101, "101").await;
    let ownerless = f.resting[0];
    f.order(ownerless, "ownerless").await;
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills().len(), 100);
    assert!(
        planned
            .plan
            .fills()
            .iter()
            .all(|fill| fill.resting_before.id != ownerless)
    );
    apply(&mut tx, planned).await.unwrap();
    tx.commit().await.unwrap();
    let remainder: Decimal =
        sqlx::query_scalar("SELECT remaining_quantity_mt FROM orderbook_orders WHERE id=$1")
            .bind(ownerless)
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(remainder, d("1"));
    f.conserved().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rows_skipped_after_the_cap_still_count_toward_the_bound() {
    let mut f = Fixture::new(101, "100").await;
    // The best-priority row is unqualified: it is in the previewed set, so the
    // first hundred leave a remainder while a look-ahead row exists.
    f.order(f.resting[0], "off_spec").await;
    let mut tx = f.writer.begin().await.unwrap();
    let error = lock_and_plan(&mut tx, f.aggressor, MatchBound::new(now()))
        .await
        .unwrap_err();
    assert_eq!(error.code, MarketErrorCode::MatchFanoutExceeded);
    tx.rollback().await.unwrap();
    Fixture::unchanged(&mut f.observer, f.aggressor, "100").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_pair_executes_only_its_exact_orders() {
    let mut f = Fixture::new(0, "1").await;
    let [bid, paired, unpaired, foreign] = [0xd1, 0xd2, 0xd3, 0xd4].map(Uuid::from_u128);
    sqlx::query("SELECT matching_fixture_demo($1,$2,$3,$4)")
        .bind(bid)
        .bind(paired)
        .bind(unpaired)
        .bind(foreign)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let plan = |pair: Option<DemoOrderPair>| {
        let bound = MatchBound::new(now());
        match pair {
            Some(pair) => bound.with_demo_pair(pair),
            None => bound,
        }
    };
    let mut tx = f.writer.begin().await.unwrap();
    let without = lock_and_plan_as(&mut tx, bid, BUYER_USER, plan(None))
        .await
        .unwrap();
    assert!(without.plan.fills().is_empty());
    tx.rollback().await.unwrap();
    let mut tx = f.writer.begin().await.unwrap();
    let other = DemoOrderPair::new(bid, unpaired).unwrap();
    let planned = lock_and_plan_as(&mut tx, bid, BUYER_USER, plan(Some(other)))
        .await
        .unwrap();
    assert_eq!(planned.plan.fills()[0].resting_before.id, unpaired);
    tx.rollback().await.unwrap();
    // The REAL order of the same slice never crosses DEMO liquidity.
    assert!(f.planned_fills(MatchBound::new(now())).await.is_empty());
    let pair = DemoOrderPair::new(bid, paired).unwrap();
    let mut tx = f.writer.begin().await.unwrap();
    let planned = lock_and_plan_as(&mut tx, bid, BUYER_USER, plan(Some(pair)))
        .await
        .unwrap();
    assert_eq!(
        planned
            .plan
            .fills()
            .iter()
            .map(|fill| fill.resting_before.id)
            .collect::<Vec<_>>(),
        vec![paired]
    );
    apply(&mut tx, planned).await.unwrap();
    tx.commit().await.unwrap();
    let parties: (Uuid, Uuid, String, String) = sqlx::query_as(
        "SELECT bid_order_id,ask_order_id,buyer_provenance,seller_provenance FROM trades",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(parties, (bid, paired, "DEMO".to_owned(), "DEMO".to_owned()));
    // The sealed owner reaches the same decision with the pair as an internal input.
    let pool = runtime_pool().await;
    let mut f = Fixture::new(0, "1").await;
    sqlx::query("SELECT matching_fixture_demo($1,$2,$3,$4)")
        .bind(bid)
        .bind(paired)
        .bind(unpaired)
        .bind(foreign)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let options = SealedMatchOptions {
        demo_pair: Some(pair),
        actor: Some(BUYER_USER),
        ..SealedMatchOptions::default()
    };
    let result = run_sealed_match_with(&pool, bid, now(), options)
        .await
        .unwrap();
    assert_eq!(result.outcome.trades.len(), 1);
    assert_eq!(result.outcome.trades[0].ask_order_id, paired);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn preview_overflow_survives_a_row_dropped_before_locking() {
    use std::sync::Arc;
    use verdaxis_read::market_contract::matching::{MatchPreviewBarrier, run_sealed_match_paused};

    let mut f = Fixture::new(101, "101").await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("RUST_MATCHING_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let barrier = Arc::new(MatchPreviewBarrier::default());
    let pending = tokio::spawn(run_sealed_match_paused(
        pool.clone(), f.aggressor, now(), barrier.clone(),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), barrier.previewed())
        .await.unwrap();
    // The SQL preview has 101 rows. Remove the first identity from the live
    // crossing set before locked re-selection takes its statement snapshot.
    f.order(f.resting[0], "ownerless").await;
    barrier.resume();
    let result = pending.await.unwrap();
    assert_eq!(result.unwrap_err().code, MarketErrorCode::MatchFanoutExceeded);
    Fixture::unchanged(&mut f.observer, f.aggressor, "101").await;
    let untouched: bool = sqlx::query_scalar(
        "SELECT bool_and(remaining_quantity_mt=quantity_mt) FROM orderbook_orders",
    ).fetch_one(&mut f.observer).await.unwrap();
    assert!(untouched);
    pool.close().await;
}
