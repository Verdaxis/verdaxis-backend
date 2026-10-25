//! Caller-owned stock movements on an explicitly disposable PostgreSQL database.
#![cfg(feature = "contract-tests")]
#![allow(dead_code)]

// Compile the real private boundary, without opening another production entry.
#[path = "../src/order_write/command.rs"]
mod command;
#[path = "../src/order_write/decimal.rs"]
mod decimal;
#[path = "../src/order_write/error.rs"]
mod error;
#[path = "../src/order_write/expiry.rs"]
mod expiry;
#[path = "../src/order_write/locks.rs"]
mod locks;
#[path = "../src/order_write/transaction.rs"]
mod transaction;

pub use verdaxis_read::{ApiError, ApiErrorKind, AppState, auth, market};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

use decimal::StockQuantity;
use error::MarketErrorCode;
use transaction::stock_contract as stock;

fn d(value: &str) -> Decimal {
    Decimal::from_str_exact(value).unwrap()
}

fn q(value: &str) -> StockQuantity {
    StockQuantity::parse_exact(value).unwrap()
}

struct Fixture {
    conn: PgConnection,
    observer: PgConnection,
    schema: String,
    item: Uuid,
    order: Uuid,
    product: Uuid,
    organization: Uuid,
}

impl Fixture {
    async fn new(count: i32) -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable PostgreSQL URL required");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().ends_with("_test"));
        let mut conn = PgConnection::connect(&url).await.unwrap();
        let mut observer = PgConnection::connect(&url).await.unwrap();
        let schema = format!("stock_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
            .execute(&mut observer)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/market_write_stock.sql"))
            .execute(&mut conn)
            .await
            .unwrap();
        let item = Uuid::from_u128(1);
        let order = Uuid::from_u128(10);
        let product = Uuid::from_u128(1000);
        let organization = Uuid::from_u128(2000);
        sqlx::query("SELECT seed_stock($1,$2,$3,$4,$5)")
            .bind(item)
            .bind(order)
            .bind(product)
            .bind(organization)
            .bind(count)
            .execute(&mut conn)
            .await
            .unwrap();
        Self {
            conn,
            observer,
            schema,
            item,
            order,
            product,
            organization,
        }
    }

    async fn balances(&mut self, current: &str, reserved: &str) {
        let row = sqlx::query(
            "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1",
        )
        .bind(self.item)
        .fetch_one(&mut self.conn)
        .await
        .unwrap();
        assert_eq!(row.get::<Decimal, _>(0), d(current));
        assert_eq!(row.get::<Decimal, _>(1), d(reserved));
        stock::verify(&mut self.conn, &[self.item]).await.unwrap();
    }

    async fn stage(&mut self, action: &str, quantity: &str) {
        sqlx::query("SELECT stage_stock($1,$2,$3)")
            .bind(self.order)
            .bind(action)
            .bind(d(quantity))
            .execute(&mut self.conn)
            .await
            .unwrap();
    }

    async fn finish(mut self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.conn)
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn reserved_identity_holds_after_each_movement() {
    let mut f = Fixture::new(1).await;
    let mut tx = f.conn.begin().await.unwrap();
    stock::reserve(&mut tx, f.item, q("10")).await.unwrap();
    stock::verify(&mut tx, &[f.item]).await.unwrap();
    tx.commit().await.unwrap();
    f.balances("0", "10").await;

    f.stage("pending", "4").await;
    f.balances("0", "10").await;
    let mut tx = f.conn.begin().await.unwrap();
    stock::withdraw(&mut tx, f.order, "CANCELLED")
        .await
        .unwrap();
    stock::verify(&mut tx, &[f.item]).await.unwrap();
    tx.commit().await.unwrap();
    f.balances("6", "4").await;
    let pending: Decimal =
        sqlx::query_scalar("SELECT quantity_mt FROM trades WHERE status='PENDING_CONFIRMATION'")
            .fetch_one(&mut f.conn)
            .await
            .unwrap();
    assert_eq!(pending, d("4"));
    f.finish().await;
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn ledger_five_movements_are_exact() {
    let mut f = Fixture::new(1).await;
    stock::reserve(&mut f.conn, f.item, q("10")).await.unwrap();
    f.balances("0", "10").await;
    f.stage("pending", "4").await;
    f.balances("0", "10").await;
    f.stage("confirm", "4").await;
    stock::consume(&mut f.conn, f.item, q("4")).await.unwrap();
    f.balances("0", "6").await;
    f.stage("pending", "2").await;
    f.balances("0", "6").await;
    f.stage("decline", "2").await;
    stock::restore(
        &mut f.conn,
        f.order,
        q("2"),
        "2026-10-25T10:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    f.balances("0", "6").await;
    stock::withdraw(&mut f.conn, f.order, "CANCELLED")
        .await
        .unwrap();
    f.balances("6", "0").await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn cancelled_trade_is_not_counted_as_executed() {
    let mut f = Fixture::new(1).await;
    stock::reserve(&mut f.conn, f.item, q("10")).await.unwrap();
    f.balances("0", "10").await;
    for action in ["cancel_trade", "decline"] {
        f.stage("pending", "2").await;
        f.balances("0", "10").await;
        f.stage(action, "2").await;
        stock::restore(
            &mut f.conn,
            f.order,
            q("2"),
            "2026-10-25T10:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        f.balances("0", "10").await;
    }
    stock::withdraw(&mut f.conn, f.order, "CANCELLED")
        .await
        .unwrap();
    f.balances("10", "0").await;
    // A terminal order's historical remainder grows, but liquidity stays withdrawn.
    f.finish().await;

    // Declining a completely taken order restores a live OPEN remainder.
    let mut full = Fixture::new(1).await;
    stock::reserve(&mut full.conn, full.item, q("10")).await.unwrap();
    full.balances("0", "10").await;
    full.stage("pending", "10").await;
    full.balances("0", "10").await;
    full.stage("decline", "10").await;
    stock::restore(&mut full.conn, full.order, q("10"), "2026-10-25T10:00:00Z".parse().unwrap()).await.unwrap();
    full.balances("0", "10").await;
    let status: String = sqlx::query_scalar("SELECT status FROM orderbook_orders WHERE id=$1")
        .bind(full.order).fetch_one(&mut full.conn).await.unwrap();
    assert_eq!(status, "OPEN");
    full.finish().await;
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn expiry_sweep_processes_at_most_one_hundred() {
    let mut f = Fixture::new(101).await;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM inventory_items ORDER BY id")
        .fetch_all(&mut f.conn)
        .await
        .unwrap();
    for id in &ids {
        stock::reserve(&mut f.conn, *id, q("10")).await.unwrap();
    }
    stock::verify(&mut f.conn, &ids).await.unwrap();
    let now: DateTime<Utc> = "2026-10-25T12:00:00Z".parse().unwrap();
    let effects = stock::expire(&mut f.conn, f.product, None, "SPOT", "ASK", now)
        .await
        .unwrap();
    assert_eq!(effects.len(), 100);
    assert!(
        effects
            .iter()
            .all(|effect| effect.released_quantity_mt.value() == d("10"))
    );
    assert!(
        effects
            .iter()
            .all(|effect| effect.organization_id == f.organization)
    );
    stock::verify(&mut f.conn, &ids).await.unwrap();
    let versions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM orderbook_orders WHERE status='EXPIRED' AND version=2",
    )
    .fetch_one(&mut f.conn)
    .await
    .unwrap();
    assert_eq!(versions, 100);
    let effects = stock::expire(&mut f.conn, f.product, None, "SPOT", "ASK", now)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    stock::verify(&mut f.conn, &ids).await.unwrap();
    assert!(
        stock::expire(&mut f.conn, f.product, None, "SPOT", "ASK", now)
            .await
            .unwrap()
            .is_empty()
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn expiry_keeps_the_pending_quantity_reserved() {
    let mut f = Fixture::new(1).await;
    stock::reserve(&mut f.conn, f.item, q("10")).await.unwrap();
    f.balances("0", "10").await;
    f.stage("pending", "4").await;
    f.balances("0", "10").await;
    let now: DateTime<Utc> = "2026-10-25T12:00:00Z".parse().unwrap();
    let mut tx = f.conn.begin().await.unwrap();
    let effects = stock::expire(&mut tx, f.product, None, "SPOT", "ASK", now)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].released_quantity_mt.value(), d("6"));
    stock::verify(&mut tx, &[f.item]).await.unwrap();
    // A separate connection cannot see the caller's uncommitted movement.
    let visible: Decimal =
        sqlx::query_scalar("SELECT reserved_stock_mt FROM inventory_items WHERE id=$1")
            .bind(f.item)
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(visible, d("10"));
    tx.rollback().await.unwrap();
    f.balances("0", "10").await;
    let mut tx = f.conn.begin().await.unwrap();
    stock::expire(&mut tx, f.product, None, "SPOT", "ASK", now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.balances("6", "4").await;
    f.stage("decline", "4").await;
    stock::restore(&mut f.conn, f.order, q("4"), now)
        .await
        .unwrap();
    f.balances("10", "0").await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "explicit disposable PostgreSQL required"]
async fn negative_or_overprecise_stock_is_refused() {
    let mut f = Fixture::new(1).await;
    for field in ["current_stock_mt", "reserved_stock_mt", "incoming_stock_mt"] {
        for bad in ["-0.01", "0.001", "100000.01"] {
            sqlx::query("SELECT corrupt_stock($1,$2,$3)")
                .bind(f.item)
                .bind(field)
                .bind(d(bad))
                .execute(&mut f.conn)
                .await
                .unwrap();
            let mut tx = f.conn.begin().await.unwrap();
            let error = stock::reserve(&mut tx, f.item, q("10")).await.unwrap_err();
            assert_eq!(error.code, MarketErrorCode::InventoryInconsistent);
            assert_eq!(error.status.as_u16(), 409);
            tx.rollback().await.unwrap();
            let current: Decimal =
                sqlx::query_scalar("SELECT current_stock_mt FROM inventory_items WHERE id=$1")
                    .bind(f.item)
                    .fetch_one(&mut f.observer)
                    .await
                    .unwrap();
            assert_eq!(
                current,
                if field == "current_stock_mt" {
                    d(bad)
                } else {
                    d("10")
                }
            );
            sqlx::query("SELECT reset_stock($1)")
                .bind(f.item)
                .execute(&mut f.conn)
                .await
                .unwrap();
        }
    }
    stock::reserve(&mut f.conn, f.item, q("10")).await.unwrap();
    f.balances("0", "10").await;
    // Two independent transactions serialize on the same inventory row. The
    // waiter must re-read, not apply the first transaction's stale reservation.
    let mut first = f.conn.begin().await.unwrap();
    stock::lock(&mut first, &[f.item]).await.unwrap();
    let item = f.item;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut f.observer).await.unwrap();
    let mut observer = f.observer;
    let waiter = tokio::spawn(async move {
        let mut second = observer.begin().await.unwrap();
        let error = stock::reserve(&mut second, item, q("1")).await.unwrap_err();
        second.rollback().await.unwrap();
        (observer, error.code)
    });
    // Prove the second independent backend is actually blocked on a row lock.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock')",
            ).bind(pid).fetch_one(&mut *first).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("second backend must reach the conflicting lock");
    first.commit().await.unwrap();
    let (observer, code) = waiter.await.unwrap();
    f.observer = observer;
    assert_eq!(code, MarketErrorCode::InventoryInconsistent);
    f.balances("0", "10").await;
    f.finish().await;
}
