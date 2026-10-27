#![cfg(feature = "contract-tests")]

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::amend::{AmendSettings, contract_router};

const KEY: &str = "synthetic-amend-signing-key-at-least-32-characters";
fn buyer() -> Uuid {
    Uuid::from_u128(0x1001)
}
fn seller() -> Uuid {
    Uuid::from_u128(0x2001)
}
fn staff() -> Uuid {
    Uuid::from_u128(0x3001)
}

/// The fixture produces buyer/seller organizations (REAL, approved) with one
/// admitted user each, one BID for the buyer and `asks` ASK rows for the
/// seller, all in the same slice.
struct Fixture {
    admin: PgConnection,
    observer: PgConnection,
    pool: PgPool,
    router: Router,
    bid: Uuid,
    asks: Vec<Uuid>,
    product: Uuid,
    point: Uuid,
}
impl Fixture {
    async fn new(asks: i32, quantity: i64) -> Self {
        let admin_url = std::env::var("RUST_AMEND_ADMIN_DATABASE_URL").unwrap();
        let runtime_url = std::env::var("RUST_AMEND_DATABASE_URL").unwrap();
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
        for sql in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_amend.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT create_fixture_seed($1)")
            .bind(asks)
            .execute(&mut admin)
            .await
            .unwrap();
        let bid = Uuid::from_u128(1);
        sqlx::query("SELECT matching_fixture_seed($1,$2,$3)")
            .bind(bid)
            .bind(asks)
            .bind(Decimal::from(quantity))
            .execute(&mut admin)
            .await
            .unwrap();
        let ask_rows = sqlx::query(
            "SELECT id FROM orderbook_orders WHERE side='ASK' ORDER BY acceptance_ordinal,id",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        let asks: Vec<Uuid> = ask_rows.iter().map(|row| row.get("id")).collect();
        let product = sqlx::query_scalar("SELECT product_id FROM orderbook_orders LIMIT 1")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        let point = sqlx::query_scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        let pool = PgPool::connect(&runtime_url).await.unwrap();
        let observer = PgConnection::connect(&runtime_url).await.unwrap();
        let router = contract_router(pool.clone(), KEY, AmendSettings::default());
        Self {
            admin,
            observer,
            pool,
            router,
            bid,
            asks,
            product,
            point,
        }
    }
    async fn count(&mut self, table: &str) -> i64 {
        let sql = match table {
            "orders" => "SELECT count(*) FROM orderbook_orders",
            "trades" => "SELECT count(*) FROM trades",
            "audit" => "SELECT count(*) FROM audit_logs",
            "events" => "SELECT count(*) FROM market_event_outbox",
            "receipts" => "SELECT count(*) FROM market_command_results",
            "notifications" => "SELECT count(*) FROM notifications",
            "watchlist" => "SELECT count(*) FROM watchlist_events",
            _ => panic!("unknown table"),
        };
        sqlx::query_scalar(sql)
            .fetch_one(&mut self.observer)
            .await
            .unwrap()
    }
    async fn order(&mut self, id: Uuid) -> serde_json::Value {
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(o)||jsonb_build_object('quantity_mt',o.quantity_mt::text,'remaining_quantity_mt',o.remaining_quantity_mt::text,'price_per_mt_usd',o.price_per_mt_usd::text,'carbon_intensity_gco2_mj',o.carbon_intensity_gco2_mj::text) FROM orderbook_orders o WHERE o.id=$1",
        )
        .bind(id)
        .fetch_one(&mut self.observer)
        .await
        .unwrap()
    }
}

async fn send(
    router: &Router,
    actor: Option<Uuid>,
    order: Uuid,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("PUT")
        .uri(format!("/api/orderbook/{order}"))
        .header("content-type", "application/json");
    if let Some(actor) = actor {
        let now = Utc::now().timestamp();
        let token = encode(
            &Header::default(),
            &json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":"amend-tests","aud":"amend-tests"}),
            &EncodingKey::from_secret(KEY.as_bytes()),
        )
        .unwrap();
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let reply = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = reply.status();
    let bytes = axum::body::to_bytes(reply.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// One plain price amendment returning 200 and the current projection.
async fn amend_price(f: &Fixture, order: Uuid, actor: Uuid, price: i64) -> (StatusCode, Value) {
    send(
        &f.router,
        Some(actor),
        order,
        &[],
        json!({"price_per_mt_usd": format!("{price}")}),
    )
    .await
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_rejects_each_nullable_economic_field() {
    let mut f = Fixture::new(0, 200).await;
    for field in [
        "quantity_mt",
        "price_per_mt_usd",
        "certifications",
        "certification_declared",
        "certification_scheme",
        "specification_standard",
        "msds_available",
        "carbon_intensity_gco2_mj",
        "carbon_intensity_method",
        "feedstock",
        "origin",
        "off_spec",
        "off_spec_notes",
    ] {
        let (status, body) =
            send(&f.router, Some(buyer()), f.bid, &[], json!({ field: null })).await;
        assert_eq!(status.as_u16(), 422, "{field}: {body}");
        assert_eq!(body["detail"]["code"], "ORDER_FIELD_NULL", "{field}");
        let row = f.order(f.bid).await;
        assert_eq!(row["version"].as_i64(), Some(1), "{field}");
    }
    // Expiry keeps its clearable meaning: null clears a dated expiry.
    sqlx::query("SELECT amend_fixture_expiring($1,now()+interval '1 hour')")
        .bind(f.bid)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, _) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"expires_at": null}),
    )
    .await;
    assert_eq!(status.as_u16(), 200);
    let row = f.order(f.bid).await;
    assert!(row["expires_at"].is_null());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_cannot_move_an_immutable_slice_field() {
    let mut f = Fixture::new(0, 200).await;
    // A non-null changed window is refused; an identical window is a no-op.
    let (status, body) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"availability_window": "2026-11"}),
    )
    .await;
    assert_eq!(status.as_u16(), 409, "{body}");
    // Undeclared identity fields are ignored, as the source request model does.
    for field in [
        "product_id",
        "delivery_point_id",
        "side",
        "provenance",
        "port_id",
    ] {
        let (status, _) = send(
            &f.router,
            Some(buyer()),
            f.bid,
            &[],
            json!({ field: Uuid::new_v4().to_string(), "price_per_mt_usd": "111" }),
        )
        .await;
        assert_eq!(status.as_u16(), 200, "{field}");
    }
    let row = f.order(f.bid).await;
    assert_eq!(row["version"].as_i64(), Some(2));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_below_filled_quantity_is_400() {
    let mut f = Fixture::new(0, 200).await;
    sqlx::query("SELECT amend_fixture_fill($1,$2)")
        .bind(f.bid)
        .bind(Decimal::from(50))
        .execute(&mut f.admin)
        .await
        .unwrap();
    let row = f.order(f.bid).await;
    let remaining = row["remaining_quantity_mt"].as_str().unwrap().to_owned();
    let (status, body) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "49"}),
    )
    .await;
    assert_eq!(status.as_u16(), 400, "{body}");
    let after = f.order(f.bid).await;
    assert_eq!(after["remaining_quantity_mt"].as_str().unwrap(), remaining);
    // Exactly the filled amount zeroes the remainder and fills the order.
    let (status, body) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "50"}),
    )
    .await;
    assert_eq!(status.as_u16(), 200, "{body}");
    let after = f.order(f.bid).await;
    assert_eq!(after["remaining_quantity_mt"].as_str().unwrap(), "0");
    assert_eq!(after["status"], "FILLED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_reserves_or_releases_the_exact_delta() {
    let mut f = Fixture::new(0, 200).await;
    let item = Uuid::new_v4();
    sqlx::query("SELECT matching_fixture_back_inventory($1,$2)")
        .bind(f.bid)
        .bind(item)
        .execute(&mut f.admin)
        .await
        .unwrap();
    // Increase: reserve the exact delta.
    let (status, _) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "250"}),
    )
    .await;
    assert_eq!(status.as_u16(), 200);
    let item_row: (Decimal, Decimal) = sqlx::query_as(
        "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1",
    )
    .bind(item)
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(item_row.0, Decimal::ZERO);
    assert_eq!(item_row.1, Decimal::from(250));
    // Reduction: release the exact delta back to current.
    let (status, _) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "220"}),
    )
    .await;
    assert_eq!(status.as_u16(), 200);
    let item_row: (Decimal, Decimal) = sqlx::query_as(
        "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1",
    )
    .bind(item)
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(item_row.0, Decimal::from(30));
    assert_eq!(item_row.1, Decimal::from(220));
    // Insufficient stock for a further increase is a 409 and changes nothing.
    sqlx::query("UPDATE inventory_items SET current_stock_mt=1 WHERE id=$1")
        .bind(item)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.order(f.bid).await;
    let (status, body) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "400"}),
    )
    .await;
    assert_eq!(status.as_u16(), 409, "{body}");
    let after = f.order(f.bid).await;
    assert_eq!(
        after["remaining_quantity_mt"],
        before["remaining_quantity_mt"]
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_rebuilds_both_sides_of_the_slice() {
    let mut f = Fixture::new(0, 200).await;
    // Seed one benchmark mirror on each side, then amend the BID price.
    for side in ["BID", "ASK"] {
        sqlx::query(
            "INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at) SELECT gen_random_uuid(),$1,'B30',$2,'SPOT',100,10,1,'live_slice_vwap',now(),now()",
        )
        .bind(side)
        .bind(f.point)
        .execute(&mut f.admin)
        .await
        .unwrap();
    }
    let (status, body) = amend_price(&f, f.bid, buyer(), 105).await;
    assert_eq!(status.as_u16(), 200, "{body}");
    // Both sides were rebuilt: the stale mirrors are gone and replaced by rows
    // derived from the eligible book.
    let mirrors: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM live_slice_benchmarks WHERE side='BID' AND benchmark_price_per_mt_usd=100",
    )
    .fetch_one(&mut f.observer)
    .await
    .unwrap();
    assert_eq!(mirrors, 0);
    let sides: i64 = sqlx::query_scalar("SELECT count(DISTINCT side) FROM live_slice_benchmarks")
        .fetch_one(&mut f.observer)
        .await
        .unwrap();
    assert_eq!(sides, 2);
    // Exactly one rebuild per side: one row per (side) after deduplication.
    let bid_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM live_slice_benchmarks WHERE side='BID'")
            .fetch_one(&mut f.observer)
            .await
            .unwrap();
    assert_eq!(bid_rows, 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn price_change_allocates_a_new_acceptance_ordinal() {
    let mut f = Fixture::new(0, 200).await;
    let before = f.order(f.bid).await;
    let ordinal = before["acceptance_ordinal"].as_i64().unwrap();
    let last = sqlx::query_scalar::<_, i64>("SELECT amend_fixture_last_ordinal()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert!(last >= ordinal);
    let (status, body) = amend_price(&f, f.bid, buyer(), 120).await;
    assert_eq!(status.as_u16(), 200, "{body}");
    let after = f.order(f.bid).await;
    assert!(after["acceptance_ordinal"].as_i64().unwrap() > ordinal);
    assert!(after["acceptance_ordinal"].as_i64().unwrap() > last);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn quantity_reduction_keeps_the_acceptance_ordinal() {
    let mut f = Fixture::new(0, 200).await;
    let (status, _) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"quantity_mt": "100"}),
    )
    .await;
    assert_eq!(status.as_u16(), 200);
    let after = f.order(f.bid).await;
    assert_eq!(after["acceptance_ordinal"].as_i64().unwrap(), 1);
    // A normalized no-op and an expiry shortening also keep the ordinal.
    sqlx::query("SELECT amend_fixture_expiring($1,now()+interval '2 hours')")
        .bind(f.bid)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (status, _) = send(
        &f.router,
        Some(buyer()),
        f.bid,
        &[],
        json!({"price_per_mt_usd": "110.00", "expires_at": (Utc::now() + Duration::hours(1)).to_rfc3339()}),
    )
    .await;
    assert_eq!(status.as_u16(), 200);
    let after = f.order(f.bid).await;
    assert_eq!(after["acceptance_ordinal"].as_i64().unwrap(), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn newly_qualified_order_gets_a_new_ordinal_and_cannot_jump_the_queue() {
    let mut f = Fixture::new(2, 200).await;
    // The seller's first ASK is executable; the second is inert because its
    // certification is undeclared. The buyer BID crosses both prices.
    let executable = f.asks[0];
    let inert = f.asks[1];
    sqlx::query("SELECT amend_fixture_inert_ask($1)")
        .bind(inert)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let earlier_ordinal: i64 =
        sqlx::query_scalar("SELECT acceptance_ordinal FROM orderbook_orders WHERE id=$1")
            .bind(executable)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    let inert_ordinal: i64 =
        sqlx::query_scalar("SELECT acceptance_ordinal FROM orderbook_orders WHERE id=$1")
            .bind(inert)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    let every_prior: i64 =
        sqlx::query_scalar("SELECT max(acceptance_ordinal) FROM orderbook_orders")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    // Declaring the certification makes the inert ASK executable: it must get
    // a fresh ordinal greater than every previously accepted order.
    let (status, body) = send(
        &f.router,
        Some(seller()),
        inert,
        &[],
        json!({"certification_declared": true}),
    )
    .await;
    assert_eq!(status.as_u16(), 200, "{body}");
    let after = f.order(inert).await;
    let new_ordinal = after["acceptance_ordinal"].as_i64().unwrap();
    assert!(new_ordinal > every_prior, "{new_ordinal} <= {every_prior}");
    assert_ne!(new_ordinal, inert_ordinal);
    // A later crossing BID fills the earlier executable ASK first.
    let crossing = Uuid::from_u128(0x99);
    sqlx::query("SELECT matching_fixture_second_aggressor($1,$2)")
        .bind(executable)
        .bind(crossing)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("SELECT amend_fixture_crossing($1,$2,$3)")
        .bind(crossing)
        .bind(f.bid)
        .bind(buyer())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let filled: Vec<Uuid> = sqlx::query_scalar(
        "SELECT ask_order_id FROM trades WHERE bid_order_id=$1 ORDER BY created_at",
    )
    .bind(crossing)
    .fetch_all(&f.pool)
    .await
    .unwrap();
    let _ = earlier_ordinal;
    assert!(!filled.is_empty());
    assert_eq!(
        filled[0], executable,
        "the earlier executable ASK must fill first"
    );
}
