#![cfg(feature = "contract-tests")]

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;
use verdaxis_read::MarketErrorCode;
use verdaxis_read::market_contract::matching::{apply, lock_and_plan, preview};
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
