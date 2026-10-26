#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;
use verdaxis_read::MarketErrorCode;
use verdaxis_read::market_contract::matching::{apply, lock_and_plan, preview, probe_order_lock};
use verdaxis_read::market_matching::MatchBound;

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
