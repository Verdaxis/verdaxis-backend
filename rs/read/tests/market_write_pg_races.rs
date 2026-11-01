#![cfg(feature = "contract-tests")]

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    AppState,
    auth::VerificationSettings,
    routes::market_write::{amend, inventory, order_create, orders, trade_lifecycle, trades},
};

const KEY: &str = "synthetic-races-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const ITEM: u128 = 0x4000;
fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}
fn verification() -> VerificationSettings {
    VerificationSettings::new(KEY.into(), None, "races-tests".into(), "races-tests".into())
}

#[derive(Clone, Debug)]
struct Command {
    method: &'static str,
    path: String,
    actor: u128,
    key: Option<String>,
    body: Value,
}
impl Command {
    fn new(method: &'static str, path: String, actor: u128, body: Value) -> Self {
        Self {
            method,
            path,
            actor,
            key: None,
            body,
        }
    }
    fn keyed(mut self, key: &str) -> Self {
        self.key = Some(key.into());
        self
    }
}
async fn send(router: Router, command: Command) -> (StatusCode, Value) {
    let now = chrono::Utc::now().timestamp();
    let token = encode(&Header::default(), &json!({"sub":id(command.actor),"type":"access","iat":now,"exp":now+600,"iss":"races-tests","aud":"races-tests"}), &EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let mut request = Request::builder()
        .method(command.method)
        .uri(command.path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    if let Some(key) = command.key {
        request = request.header("idempotency-key", key);
    }
    let mut body = command.body;
    if let Some(context) = body
        .as_object_mut()
        .and_then(|o| o.remove("__fixture_context"))
    {
        request = request.header(
            "x-verdaxis-market-support-context",
            context.as_str().unwrap(),
        );
    }
    if let Some(object) = body.as_object_mut() {
        object.remove("__fixture_wait_expiry");
    }
    let response = router
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
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
struct Fixture {
    admin: PgConnection,
    admin_url: String,
    runtime_url: String,
    product: Uuid,
    point: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url =
            std::env::var("RUST_RACES_ADMIN_DATABASE_URL").expect("disposable administrator URL");
        let runtime_url = std::env::var("RUST_RACES_DATABASE_URL").expect("disposable runtime URL");
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.port().is_some());
        assert!(a.path().ends_with("_test"));
        assert_eq!(
            (a.host_str(), a.port(), a.path()),
            (r.host_str(), r.port(), r.path())
        );
        assert_ne!(a.username(), r.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT version_num FROM alembic_version")
                .fetch_one(&mut admin)
                .await
                .unwrap(),
            "p89_20261022_trade_freeze"
        );
        for source in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_publish.sql"),
            include_str!("fixtures/market_write_create.sql"),
            include_str!("fixtures/phase9_races.sql"),
        ] {
            sqlx::raw_sql(source).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT races_seed()")
            .execute(&mut admin)
            .await
            .unwrap();
        let product = "c4cecebc-3d2d-5840-8021-57a9a11bc673".parse().unwrap();
        let point = sqlx::query_scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        Self {
            admin,
            admin_url,
            runtime_url,
            product,
            point,
        }
    }
    async fn router(&self, name: &'static str) -> (Router, PgPool) {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |c, _| {
                Box::pin(async move {
                    sqlx::query("SELECT set_config('application_name',$1,false)")
                        .bind(name)
                        .execute(c)
                        .await?;
                    Ok(())
                })
            })
            .connect(&self.runtime_url)
            .await
            .unwrap();
        let config = serde_json::from_value(json!({"environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":"p89_20261022_trade_freeze","database_url":self.runtime_url,"db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":true})).unwrap();
        let mut pools = verdaxis_read::connect(&config).await.unwrap();
        pools.read = pool.clone();
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let app = AppState::new(config, pools, policy).unwrap();
        let support = verdaxis_read::market_support::router(
            verdaxis_read::market_support::SupportState::new(
                pool.clone(),
                std::sync::Arc::new(verification()),
                verdaxis_read::market_support::SupportSettings::new(true, &id(0x3001).to_string()),
                serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json"))
                    .unwrap(),
                (*app.config).clone(),
            ),
        );
        let rejection_pool = pool.clone();
        let rejection_policy = app.policy.clone();
        let rejection_config = app.config.clone();
        let rejection = Router::new().route(
            "/component/organizations/reject",
            axum::routing::put(move |request: Request<Body>| {
                let pool = rejection_pool.clone();
                let policy = rejection_policy.clone();
                let config = rejection_config.clone();
                async move {
                    let now = chrono::Utc::now();
                    let access = verdaxis_read::auth::verify_authorization(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok()),
                        &verification(),
                        now,
                    )
                    .unwrap();
                    let result = verdaxis_read::business::transaction::run_ordered_write(
                        pool,
                        std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
                        access,
                        verdaxis_read::business::organizations::ReviewOrganization {
                            organization_id: id(0x2000),
                            approve: false,
                            reason: Some("Concurrent review".into()),
                            now,
                            policy,
                            config,
                        },
                    )
                    .await;
                    use axum::response::IntoResponse;
                    match result {
                        Ok(value) => axum::Json(value).into_response(),
                        Err(error) => error.into_response(),
                    }
                }
            }),
        );
        let router = orders::router_with_create(
            app.clone(),
            verification(),
            verification(),
            order_create::CreateSettings::default(),
        )
        .merge(amend::router(
            app.clone(),
            verification(),
            amend::AmendSettings::default(),
        ))
        .merge(inventory::router_with_publication(
            app.clone(),
            verification(),
            order_create::CreateSettings::default(),
        ))
        .merge(trades::router(
            app.clone(),
            verification(),
            true,
            Default::default(),
            None,
        ))
        .merge(verdaxis_read::crud::watchlists::router(
            app.clone(),
            verdaxis_read::crud::watchlists::WatchlistSettings::new(
                verification(),
                chrono::Utc::now(),
            ),
        ))
        .merge(trade_lifecycle::router(app, verification()))
        .merge(support)
        .merge(rejection);
        (router, pool)
    }
    async fn call(&self, command: Command) -> (StatusCode, Value) {
        let (router, pool) = self.router("races-serial").await;
        let result = send(router, command).await;
        pool.close().await;
        result
    }
    async fn ledger(&mut self) {
        let invariant: Value = sqlx::query_scalar("SELECT races_invariants()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        assert_eq!(
            invariant,
            json!({"stock":true,"orders":true,"money":true,"freeze":true}),
            "conservation after committed command: {invariant}"
        );
        let total: Decimal = sqlx::query_scalar("SELECT COALESCE(sum(current_stock_mt+reserved_stock_mt),0)+(SELECT COALESCE(sum(t.quantity_mt),0) FROM trades t JOIN orderbook_orders o ON o.id=t.ask_order_id WHERE o.inventory_item_id IS NOT NULL AND t.status::text IN ('CONFIRMED','DELIVERED','PAID')) FROM inventory_items").fetch_one(&mut self.admin).await.unwrap();
        assert!(total >= Decimal::ZERO);
    }
    fn publish(&self) -> Command {
        Command::new(
            "POST",
            format!("/api/inventory/{}/publish", id(ITEM)),
            SELLER,
            Value::Null,
        )
    }
    fn cancel(&self, order: Uuid) -> Command {
        Command::new(
            "DELETE",
            format!("/api/orderbook/{order}"),
            SELLER,
            Value::Null,
        )
    }
    fn amend(&self, order: Uuid, quantity: &str) -> Command {
        Command::new(
            "PUT",
            format!("/api/orderbook/{order}"),
            SELLER,
            json!({"quantity_mt":quantity}),
        )
    }
    fn create(&self, quantity: &str, price: &str) -> Command {
        Command::new(
            "POST",
            "/api/orderbook".into(),
            BUYER,
            json!({"side":"BID","product_id":self.product,"delivery_point_id":self.point,"quantity_mt":quantity,"price_per_mt_usd":price}),
        )
    }
    async fn listing(&self) -> Uuid {
        let result = self.call(self.publish()).await;
        assert_eq!(result.0, StatusCode::OK, "{result:?}");
        result.1["listing_id"].as_str().unwrap().parse().unwrap()
    }
    async fn take(&mut self, order: Uuid, quantity: &str) -> Command {
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
                .bind(order)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        let digest = verdaxis_read::canonical_terms_from_row(&row)
            .and_then(|v| verdaxis_read::digest_terms(&v))
            .unwrap();
        Command::new(
            "POST",
            "/api/trades/".into(),
            BUYER,
            json!({"order_id":order,"quantity_mt":quantity,"expected_terms_digest":digest}),
        )
    }
    async fn stock(&mut self) -> (Decimal, Decimal) {
        let row = sqlx::query(
            "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1",
        )
        .bind(id(ITEM))
        .fetch_one(&mut self.admin)
        .await
        .unwrap();
        (row.get(0), row.get(1))
    }
    async fn effects(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('audit',(SELECT count(*) FROM audit_logs),'outbox',(SELECT count(*) FROM market_event_outbox),'notifications',(SELECT count(*) FROM notifications),'receipts',(SELECT count(*) FROM market_command_results))").fetch_one(&mut self.admin).await.unwrap()
    }
    async fn reader_check(&mut self, order: Uuid) {
        assert_pin(&mut self.admin, order).await;
        let watch: Uuid =
            sqlx::query_scalar("SELECT watchlist_id FROM watchlist_targets WHERE order_id=$1")
                .bind(order)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        let rust = self
            .call(Command::new(
                "GET",
                format!("/api/watchlists/{watch}"),
                BUYER,
                Value::Null,
            ))
            .await;
        ok(&rust, StatusCode::OK);
        let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
        let script = r#"import asyncio,json,sys
from uuid import UUID
from sqlalchemy.ext.asyncio import create_async_engine,async_sessionmaker
from sqlalchemy import select
from sqlalchemy.orm import selectinload
from app.models.watchlist import WatchlistTarget
from app.services.watchlists import _pin_response
async def main():
 engine=create_async_engine(sys.argv[1].replace('postgresql://','postgresql+asyncpg://').replace('postgres://','postgresql+asyncpg://'))
 async with async_sessionmaker(engine)() as db:
  target=(await db.execute(select(WatchlistTarget).options(selectinload(WatchlistTarget.delivery_point)).where(WatchlistTarget.order_id==UUID(sys.argv[2])))).scalar_one()
  print(_pin_response(target,unread_event_count=0,latest_event_at=None).model_dump_json())
 await engine.dispose()
asyncio.run(main())"#;
        let output = std::process::Command::new(python)
            .args(["-c", script, &self.runtime_url, &order.to_string()])
            .env("ENVIRONMENT", "test")
            .env("RELEASE_SHA", "test")
            .env("JWT_SECRET", KEY)
            .current_dir(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap(),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let py: Value = serde_json::from_slice(&output.stdout).unwrap();
        fn find(value: &Value, order: Uuid) -> Option<&Value> {
            match value {
                Value::Object(o) => {
                    if o.get("order_id") == Some(&json!(order)) {
                        Some(value)
                    } else {
                        o.values().find_map(|v| find(v, order))
                    }
                }
                Value::Array(a) => a.iter().find_map(|v| find(v, order)),
                _ => None,
            }
        }
        let expected_status: String =
            sqlx::query_scalar("SELECT status::text FROM orderbook_orders WHERE id=$1")
                .bind(order)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        let target = if let Some(target) = find(&rust.1, order) {
            target
        } else {
            assert!(
                matches!(expected_status.as_str(), "CANCELLED" | "FILLED"),
                "live PIN missing: {}",
                rust.1
            );
            assert_eq!(py["snapshot_status"], expected_status);
            println!("TERMINAL_PIN_FILTERED order={order} status={expected_status}");
            return;
        };
        for key in [
            "snapshot_price_per_mt_usd",
            "snapshot_quantity_mt",
            "snapshot_remaining_quantity_mt",
            "snapshot_status",
        ] {
            assert_eq!(target[key], py[key], "{key}");
        }
    }
    async fn refusal_overlap(
        &mut self,
        first: Command,
        second: Command,
        resource: Uuid,
        trade: bool,
    ) -> ((StatusCode, Value), (StatusCode, Value)) {
        let (r1, p1) = self.router("races-first").await;
        let (r2, p2) = self.router("races-second").await;
        let mut holder = PgConnection::connect(&self.admin_url).await.unwrap();
        sqlx::raw_sql("BEGIN; SELECT set_config('application_name','races-holder',false)")
            .execute(&mut holder)
            .await
            .unwrap();
        sqlx::query("SELECT races_hold_resource($1,$2)")
            .bind(resource)
            .bind(trade)
            .execute(&mut holder)
            .await
            .unwrap();
        let a = tokio::spawn(send(r1, first));
        wait(&mut self.admin, "races-first", "races-holder").await;
        let b = tokio::spawn(send(r2, second));
        sqlx::raw_sql("SELECT races_wait_second(); COMMIT")
            .execute(&mut holder)
            .await
            .unwrap();
        let result = (a.await.unwrap(), b.await.unwrap());
        p1.close().await;
        p2.close().await;
        self.ledger().await;
        println!(
            "OBSERVED_REFUSAL_WAIT resource={resource} trade={trade} statuses={},{}",
            result.0.0, result.1.0
        );
        result
    }
    async fn overlap(
        &mut self,
        first: Command,
        second: Command,
    ) -> ((StatusCode, Value), (StatusCode, Value)) {
        let (r1, p1) = self.router("races-first").await;
        let (r2, p2) = self.router("races-second").await;
        let mut barrier = PgConnection::connect(&self.admin_url).await.unwrap();
        sqlx::query("SELECT set_config('application_name','races-barrier',false)")
            .execute(&mut barrier)
            .await
            .unwrap();
        sqlx::query("SELECT races_hold()")
            .execute(&mut barrier)
            .await
            .unwrap();
        let a = tokio::spawn(send(r1, first.clone()));
        let first_pid = wait(&mut self.admin, "races-first", "races-barrier").await;
        if let Some(order) = first
            .body
            .get("__fixture_wait_expiry")
            .and_then(Value::as_str)
        {
            sqlx::query("SELECT races_wait_expiry($1)")
                .bind(order.parse::<Uuid>().unwrap())
                .execute(&mut self.admin)
                .await
                .unwrap();
        }
        let b = tokio::spawn(send(r2, second.clone()));
        let observation: Value = sqlx::query_scalar("SELECT races_observe_release()")
            .fetch_one(&mut barrier)
            .await
            .unwrap();
        let second_pid = observation["pid"].as_i64().unwrap();
        let release_ms = observation["release_ms"].as_f64().unwrap();
        assert!(
            release_ms < 350.0,
            "release must retain the economic lock budget"
        );
        let mut results = (a.await.unwrap(), b.await.unwrap());
        if matches!(
            results.1.1["detail"]["code"].as_str(),
            Some(
                "MARKET_BUSY"
                    | "MARKET_TIMEOUT"
                    | "TRANSACTION_RETRY_EXHAUSTED"
                    | "MARKET_SLICE_CHANGED"
            )
        ) {
            println!(
                "BOUNDED_CONTENDER status={} body={}",
                results.1.0, results.1.1
            );
            self.ledger().await;
            results.1 = send(self.router("races-completed-order").await.0, second.clone()).await;
        }
        println!(
            "OBSERVED_ROUTE_WAIT first={first:?} second={second:?} first_pid={first_pid} second_pid={second_pid} observation_ms={} statuses={},{}",
            release_ms, results.0.0, results.1.0
        );
        p1.close().await;
        p2.close().await;
        self.ledger().await;
        results
    }
}
async fn wait(admin: &mut PgConnection, name: &str, blocker: &str) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let pid:Option<i32>=sqlx::query_scalar("SELECT a.pid FROM pg_stat_activity a WHERE a.datname=current_database() AND a.application_name=$1 AND a.wait_event_type='Lock' AND EXISTS(SELECT 1 FROM pg_stat_activity b WHERE b.pid=ANY(pg_blocking_pids(a.pid)) AND b.application_name=$2)").bind(name).bind(blocker).fetch_optional(&mut *admin).await.unwrap();
        if let Some(pid) = pid {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name} did not wait for {blocker}"
        );
        tokio::task::yield_now().await;
    }
}
fn ok(result: &(StatusCode, Value), status: StatusCode) {
    assert_eq!(result.0, status, "{result:?}");
}
fn lifecycle(trade: Uuid, action: &str) -> Command {
    Command::new(
        if action == "pay" { "POST" } else { "PUT" },
        format!("/api/trades/{trade}/{action}"),
        SELLER,
        if action == "deliver" {
            json!({"final_quantity_mt":"300","final_price_per_mt":"100"})
        } else {
            Value::Null
        },
    )
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_and_take_release_only_the_unfilled_remainder() {
    for take_first in [true, false] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let cancel = f.cancel(order);
        let (a, b) = if take_first {
            f.overlap(take, cancel).await
        } else {
            f.overlap(cancel, take).await
        };
        ok(
            &a,
            if take_first {
                StatusCode::OK
            } else {
                StatusCode::NO_CONTENT
            },
        );
        ok(
            &b,
            if take_first {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::BAD_REQUEST
            },
        );
        assert_eq!(
            f.stock().await,
            if take_first {
                (Decimal::from(700), Decimal::from(300))
            } else {
                (Decimal::from(1000), Decimal::ZERO)
            },
            "cancel releases the unfilled remainder, not original quantity"
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_twins_replay_or_refuse_without_second_effects() {
    for keyed in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let c = if keyed {
            f.cancel(order).keyed("cancel")
        } else {
            f.cancel(order)
        };
        let (a, b) = f.overlap(c.clone(), c).await;
        ok(&a, StatusCode::NO_CONTENT);
        ok(
            &b,
            if keyed {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::BAD_REQUEST
            },
        );
        assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
        let counts = f.effects().await;
        ok(&f.call(f.cancel(order)).await, StatusCode::BAD_REQUEST);
        assert_eq!(f.effects().await, counts);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_and_amend_revalidate_in_both_orders() {
    for amend_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let (a, b) = if amend_first {
            f.overlap(f.amend(order, "800"), f.cancel(order)).await
        } else {
            f.overlap(f.cancel(order), f.amend(order, "800")).await
        };
        ok(
            &a,
            if amend_first {
                StatusCode::OK
            } else {
                StatusCode::NO_CONTENT
            },
        );
        ok(
            &b,
            if amend_first {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::BAD_REQUEST
            },
        );
        assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_and_crossing_create_have_one_stock_movement() {
    for create_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let (a, b) = if create_first {
            f.overlap(f.create("300", "110"), f.cancel(order)).await
        } else {
            f.overlap(f.cancel(order), f.create("300", "110")).await
        };
        ok(
            &a,
            if create_first {
                StatusCode::CREATED
            } else {
                StatusCode::NO_CONTENT
            },
        );
        ok(
            &b,
            if create_first {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::CREATED
            },
        );
        assert_eq!(
            f.stock().await,
            if create_first {
                (Decimal::from(700), Decimal::ZERO)
            } else {
                (Decimal::from(1000), Decimal::ZERO)
            }
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn take_twins_recheck_reduced_remainder() {
    for quantity in ["300", "700"] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, quantity).await;
        let (a, mut b) = f.overlap(take.clone(), take).await;
        ok(&a, StatusCode::OK);
        if b.1["detail"]["code"] == "ORDER_TERMS_REVIEW_REQUIRED" {
            let fresh = f.take(order, quantity).await;
            b = f.call(fresh).await;
        }
        ok(
            &b,
            if quantity == "300" {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            },
        );
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000)));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn inventory_edit_and_publication_use_the_committed_stock() {
    for edit_first in [false, true] {
        let mut f = Fixture::new().await;
        let edit = Command::new(
            "PATCH",
            format!("/api/inventory/{}", id(ITEM)),
            SELLER,
            json!({"current_stock_mt":"800"}),
        );
        let (a, b) = if edit_first {
            f.overlap(edit, f.publish()).await
        } else {
            f.overlap(f.publish(), edit).await
        };
        ok(&a, StatusCode::OK);
        ok(
            &b,
            if edit_first {
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            },
        );
        assert_eq!(
            f.stock().await,
            (
                Decimal::ZERO,
                Decimal::from(if edit_first { 800 } else { 1000 })
            )
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn inventory_delete_and_publication_preserve_references() {
    for delete_first in [false, true] {
        let mut f = Fixture::new().await;
        let delete = Command::new(
            "DELETE",
            format!("/api/inventory/{}", id(ITEM)),
            SELLER,
            Value::Null,
        );
        let (a, b) = if delete_first {
            f.overlap(delete, f.publish()).await
        } else {
            f.overlap(f.publish(), delete).await
        };
        ok(
            &a,
            if delete_first {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::OK
            },
        );
        ok(
            &b,
            if delete_first {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            },
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn inventory_delete_and_take_keep_the_linked_item() {
    for take_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let delete = Command::new(
            "DELETE",
            format!("/api/inventory/{}", id(ITEM)),
            SELLER,
            Value::Null,
        );
        // A refused deletion writes no effect, so serialize this reachable order
        // explicitly; the successful take still uses the real route.
        if take_first {
            ok(&f.call(take.clone()).await, StatusCode::OK);
        }
        ok(&f.call(delete).await, StatusCode::CONFLICT);
        if !take_first {
            ok(&f.call(take).await, StatusCode::OK);
        }
        f.ledger().await;
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000)));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn unkeyed_publications_reuse_only_a_live_listing() {
    for fill in [0, 300, 1000] {
        let mut f = Fixture::new().await;
        if fill > 0 {
            ok(
                &f.call(f.create(&fill.to_string(), "110")).await,
                StatusCode::CREATED,
            );
        }
        let (a, b) = f.overlap(f.publish(), f.publish()).await;
        ok(&a, StatusCode::OK);
        ok(
            &b,
            if fill == 1000 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            },
        );
        if fill < 1000 {
            assert_eq!(a.1["listing_id"], b.1["listing_id"]);
        }
        let counts = f.effects().await;
        let retry = f.call(f.publish()).await;
        ok(
            &retry,
            if fill == 1000 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::OK
            },
        );
        assert_eq!(f.effects().await, counts);
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000 - fill)));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn keyed_publication_keeps_the_filled_identity() {
    let mut f = Fixture::new().await;
    ok(&f.call(f.create("1000", "110")).await, StatusCode::CREATED);
    let (a, b) = f
        .overlap(f.publish().keyed("publish"), f.publish().keyed("publish"))
        .await;
    ok(&a, StatusCode::OK);
    ok(&b, StatusCode::OK);
    assert_eq!(a.1["listing_id"], b.1["listing_id"]);
    let counts = f.effects().await;
    let replay = f.call(f.publish().keyed("publish")).await;
    assert_eq!(replay, a);
    assert_eq!(f.effects().await, counts);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn same_key_creates_replay_or_refuse_changed_terms() {
    for changed in [false, true] {
        let mut f = Fixture::new().await;
        let (a, b) = f
            .overlap(
                f.create("200", "90").keyed("create"),
                f.create(if changed { "300" } else { "200" }, "90")
                    .keyed("create"),
            )
            .await;
        ok(&a, StatusCode::CREATED);
        ok(
            &b,
            if changed {
                StatusCode::CONFLICT
            } else {
                StatusCode::CREATED
            },
        );
        if !changed {
            assert_eq!(a.1["id"], b.1["id"]);
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM orderbook_orders")
                .fetch_one(&mut f.admin)
                .await
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_receipt_is_bound_to_the_original_resource() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    ok(
        &f.call(f.cancel(order).keyed("shared")).await,
        StatusCode::NO_CONTENT,
    );
    let second = f.listing().await;
    let original = f.effects().await;
    let (a, b) = f
        .overlap(
            f.cancel(second).keyed("shared-new"),
            f.cancel(order).keyed("shared"),
        )
        .await;
    ok(&a, StatusCode::NO_CONTENT);
    ok(&b, StatusCode::NO_CONTENT);
    let wrong = f.call(f.cancel(second).keyed("shared")).await;
    ok(&wrong, StatusCode::CONFLICT);
    assert_eq!(wrong.1["detail"]["code"], "RECEIPT_RESOURCE_MISMATCH");
    let after = f.effects().await;
    assert_eq!(
        after["audit"].as_i64().unwrap(),
        original["audit"].as_i64().unwrap() + 1
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn confirm_and_decline_observe_one_pending_transition() {
    for confirm_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let result = f.call(take).await;
        ok(&result, StatusCode::OK);
        let trade = result.1["id"].as_str().unwrap().parse().unwrap();
        let (a, b) = if confirm_first {
            f.overlap(lifecycle(trade, "confirm"), lifecycle(trade, "decline"))
                .await
        } else {
            f.overlap(lifecycle(trade, "decline"), lifecycle(trade, "confirm"))
                .await
        };
        ok(&a, StatusCode::OK);
        ok(&b, StatusCode::BAD_REQUEST);
        assert_eq!(b.1["detail"]["code"], "TRADE_NOT_PENDING");
        assert_eq!(
            f.stock().await,
            (
                Decimal::ZERO,
                Decimal::from(if confirm_first { 700 } else { 1000 })
            )
        );
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn deliver_and_pay_observe_the_committed_state() {
    for deliver_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let result = f.call(take).await;
        ok(&result, StatusCode::OK);
        let trade = result.1["id"].as_str().unwrap().parse().unwrap();
        ok(&f.call(lifecycle(trade, "confirm")).await, StatusCode::OK);
        if deliver_first {
            let (a, b) = f
                .overlap(lifecycle(trade, "deliver"), lifecycle(trade, "pay"))
                .await;
            ok(&a, StatusCode::OK);
            ok(&b, StatusCode::OK);
        } else {
            let before = f.effects().await;
            ok(
                &f.call(lifecycle(trade, "pay")).await,
                StatusCode::BAD_REQUEST,
            );
            assert_eq!(f.effects().await, before);
            ok(&f.call(lifecycle(trade, "deliver")).await, StatusCode::OK);
            ok(&f.call(lifecycle(trade, "pay")).await, StatusCode::OK);
        }
        f.ledger().await;
        let state: (
            String,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<chrono::DateTime<chrono::Utc>>,
        ) = sqlx::query_as("SELECT status::text,delivered_at,paid_at FROM trades WHERE id=$1")
            .bind(trade)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(state.0, "PAID");
        assert!(state.1.is_some() && state.2.is_some());
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn take_and_crossing_publication_share_the_remainder() {
    for take_first in [false, true] {
        let mut f = Fixture::new().await;
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
                    && p["lanes"].as_array().unwrap().contains(&json!(f.point))
            })
            .unwrap();
        f.product = product["id"].as_str().unwrap().parse().unwrap();
        sqlx::query("SELECT races_publication_product($1)")
            .bind(f.product)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let created = f.call(f.create("500", "110")).await;
        ok(&created, StatusCode::CREATED);
        let order: Uuid = created.1["id"].as_str().unwrap().parse().unwrap();
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
                .bind(order)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        let digest = verdaxis_read::canonical_terms_from_row(&row)
            .and_then(|v| verdaxis_read::digest_terms(&v))
            .unwrap();
        let take = Command::new(
            "POST",
            "/api/trades/".into(),
            SELLER,
            json!({"order_id":order,"quantity_mt":"200","expected_terms_digest":digest}),
        );
        if take_first {
            let (a, b) = f.overlap(take, f.publish()).await;
            ok(&a, StatusCode::OK);
            ok(&b, StatusCode::OK);
        } else {
            let (a, b) = f.overlap(f.publish(), take).await;
            ok(&a, StatusCode::OK);
            ok(&b, StatusCode::CONFLICT);
        }
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn finite_seeded_lifecycle_sequences_preserve_the_ledger() {
    use proptest::{
        prelude::*,
        strategy::ValueTree,
        test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
    };
    for seed in [29_u64, 174, 20261030] {
        let mut runner = TestRunner::new_with_rng(
            Config {
                failure_persistence: None,
                ..Config::default()
            },
            TestRng::from_seed(RngAlgorithm::ChaCha, &seed.to_le_bytes().repeat(4)),
        );
        let choices = proptest::collection::vec(any::<bool>(), 5)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let mut f = Fixture::new().await;
        for (index, confirm) in choices.into_iter().enumerate() {
            let mut create = f.create("500", "100");
            create.actor = SELLER;
            create.body["side"] = json!("ASK");
            for (key, value) in [
                ("certification_declared", json!(true)),
                ("certification_scheme", json!("ISCC EU")),
                ("specification_standard", json!("ISO 8217:2024 RF 380")),
                ("msds_available", json!(true)),
                ("carbon_intensity_gco2_mj", json!("10")),
                ("carbon_intensity_method", json!("LIFECYCLE")),
                ("feedstock", json!("residue")),
                ("origin", json!("NL")),
            ] {
                create.body[key] = value;
            }
            create.body["availability_window"] = json!(format!("{}-Q1", 2027 + index));
            create = create.keyed(&format!("seed-{seed}-{index}"));
            let created = f.call(create.clone()).await;
            ok(&created, StatusCode::CREATED);
            f.ledger().await;
            let order: Uuid = created.1["id"].as_str().unwrap().parse().unwrap();
            ok(&f.call(create).await, StatusCode::CREATED);
            f.ledger().await;
            let amend = Command::new(
                "PUT",
                format!("/api/orderbook/{order}"),
                SELLER,
                json!({"quantity_mt":"600"}),
            );
            ok(&f.call(amend).await, StatusCode::OK);
            f.ledger().await;
            let mut take = f.take(order, "200").await;
            take.actor = BUYER;
            let taken = f.call(take).await;
            ok(&taken, StatusCode::OK);
            f.ledger().await;
            let trade = taken.1["id"].as_str().unwrap().parse().unwrap();
            let mut action = lifecycle(trade, if confirm { "confirm" } else { "decline" });
            action.actor = SELLER;
            ok(&f.call(action).await, StatusCode::OK);
            f.ledger().await;
            let cancel = Command::new(
                "DELETE",
                format!("/api/orderbook/{order}"),
                SELLER,
                Value::Null,
            )
            .keyed(&format!("cancel-{seed}-{index}"));
            ok(&f.call(cancel.clone()).await, StatusCode::NO_CONTENT);
            f.ledger().await;
            ok(&f.call(cancel).await, StatusCode::NO_CONTENT);
            f.ledger().await;
            println!(
                "FINITE_SEQUENCE seed={seed} slice={index} writers=rust-create,rust-replay,rust-amend,rust-take,rust-{},rust-cancel,rust-replay",
                if confirm { "confirm" } else { "decline" }
            );
        }
        for _ in 0..5 {
            let mut stock = Fixture::new().await;
            let listing = stock.listing().await;
            stock.ledger().await;
            let take = stock.take(listing, "300").await;
            let taken = stock.call(take).await;
            ok(&taken, StatusCode::OK);
            stock.ledger().await;
            let pending: Uuid = taken.1["id"].as_str().unwrap().parse().unwrap();
            sqlx::query("SELECT races_expire($1)")
                .bind(listing)
                .execute(&mut stock.admin)
                .await
                .unwrap();
            ok(&stock.call(stock.publish()).await, StatusCode::OK);
            stock.ledger().await;
            ok(
                &stock
                    .call(Command::new(
                        "PUT",
                        "/component/organizations/reject".into(),
                        0x3001,
                        Value::Null,
                    ))
                    .await,
                StatusCode::OK,
            );
            stock.ledger().await;
            assert_eq!(stock.stock().await, (Decimal::from(1000), Decimal::ZERO));
            ok(
                &stock.call(lifecycle(pending, "decline")).await,
                StatusCode::BAD_REQUEST,
            );
            stock.ledger().await;
            assert_eq!(stock.stock().await, (Decimal::from(1000), Decimal::ZERO));
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn amend_take_and_cancel_keep_current_pin_display() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let code = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == f.product.to_string())
        .unwrap()["code"]
        .as_str()
        .unwrap();
    sqlx::query("SELECT races_pin($1,$2)")
        .bind(order)
        .bind(code)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let history:Value=sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_object('id',id,'payload',payload) ORDER BY id) FROM market_event_outbox").fetch_one(&mut f.admin).await.unwrap();
    ok(&f.call(f.amend(order, "800")).await, StatusCode::OK);
    f.reader_check(order).await;
    let take = f.take(order, "300").await;
    ok(&f.call(take).await, StatusCode::OK);
    f.reader_check(order).await;
    ok(&f.call(f.cancel(order)).await, StatusCode::NO_CONTENT);
    f.reader_check(order).await;
    let historical_ids: Vec<Uuid> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let after:Value=sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_object('id',id,'payload',payload) ORDER BY id) FROM market_event_outbox WHERE id=ANY($1)").bind(historical_ids).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(history, after);
    f.ledger().await;
    let next = f.listing().await;
    sqlx::query("SELECT races_pin($1,$2)")
        .bind(next)
        .bind(code)
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.reader_check(next).await;
    let take = f.take(next, "700").await;
    ok(&f.call(take).await, StatusCode::OK);
    f.reader_check(next).await;
    f.ledger().await;
}
async fn assert_pin(admin: &mut PgConnection, order: Uuid) {
    let same:bool=sqlx::query_scalar("SELECT w.snapshot_price_per_mt_usd=o.price_per_mt_usd AND w.snapshot_quantity_mt=o.quantity_mt AND w.snapshot_remaining_quantity_mt=o.remaining_quantity_mt AND w.snapshot_status=o.status::text FROM watchlist_targets w JOIN orderbook_orders o ON o.id=w.order_id WHERE o.id=$1").bind(order).fetch_one(&mut *admin).await.unwrap();
    assert!(same, "PIN display follows the committed economic row");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_expiry_and_confirmation_preserve_pending_stock() {
    for confirm_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let taken = f.call(take).await;
        ok(&taken, StatusCode::OK);
        let trade = taken.1["id"].as_str().unwrap().parse().unwrap();
        sqlx::query("SELECT races_expire($1)")
            .bind(order)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let (a, b) = if confirm_first {
            f.overlap(lifecycle(trade, "confirm"), f.publish()).await
        } else {
            f.overlap(f.publish(), lifecycle(trade, "confirm")).await
        };
        ok(&a, StatusCode::OK);
        ok(&b, StatusCode::OK);
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(700)));
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM orderbook_orders WHERE id=$1")
                .bind(order)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(status, "EXPIRED");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_expiry_before_take_refuses_the_old_order() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let take = f.take(order, "300").await;
    sqlx::query("SELECT races_expire($1)")
        .bind(order)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let (a, b) = f.overlap(f.publish(), take).await;
    ok(&a, StatusCode::OK);
    ok(&b, StatusCode::CONFLICT);
    assert_eq!(b.1["detail"]["code"], "ORDER_TERMS_REVIEW_REQUIRED");
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000)));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rejection_and_cancel_leave_no_pending_commitment() {
    for cancel_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        ok(&f.call(take).await, StatusCode::OK);
        let reject = Command::new(
            "PUT",
            "/component/organizations/reject".into(),
            0x3001,
            Value::Null,
        );
        if cancel_first {
            let (a, b) = f.overlap(f.cancel(order), reject).await;
            ok(&a, StatusCode::NO_CONTENT);
            ok(&b, StatusCode::OK);
        } else {
            let (a, b) = f.overlap(reject, f.cancel(order)).await;
            ok(&a, StatusCode::OK);
            ok(&b, StatusCode::BAD_REQUEST);
        }
        // Either order ends the same way: the pending trade is cancelled and
        // its reservation is back in stock together with the order remainder.
        assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
        let trades: Vec<String> = sqlx::query_scalar("SELECT status::text FROM trades")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(trades, ["CANCELLED"]);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rejection_and_confirmation_observe_current_admission() {
    for confirm_first in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let take = f.take(order, "300").await;
        let taken = f.call(take).await;
        ok(&taken, StatusCode::OK);
        let trade = taken.1["id"].as_str().unwrap().parse().unwrap();
        let reject = Command::new(
            "PUT",
            "/component/organizations/reject".into(),
            0x3001,
            Value::Null,
        );
        let (a, b) = if confirm_first {
            f.overlap(lifecycle(trade, "confirm"), reject).await
        } else {
            f.overlap(reject, lifecycle(trade, "confirm")).await
        };
        ok(&a, StatusCode::OK);
        ok(
            &b,
            if confirm_first {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
        );
        let status: String = sqlx::query_scalar("SELECT status::text FROM trades WHERE id=$1")
            .bind(trade)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        if confirm_first {
            // The confirmed trade keeps its commitment; only the order remainder returns.
            assert_eq!(status, "CONFIRMED");
            assert_eq!(f.stock().await, (Decimal::from(700), Decimal::ZERO));
        } else {
            // The rejection cancelled the pending trade, so nothing is left to decline.
            assert_eq!(status, "CANCELLED");
            assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
            ok(
                &f.call(lifecycle(trade, "decline")).await,
                StatusCode::BAD_REQUEST,
            );
            f.ledger().await;
            assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn revoke_and_assisted_create_serialize_authority() {
    for create_first in [false, true] {
        let mut f = Fixture::new().await;
        sqlx::query("SELECT create_fixture_seed(0)")
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mut create = f.create("200", "90").keyed("assisted");
        create.actor = 0x3001;
        create.body["support_confirmation"] = json!({"external_instruction_reference":"Customer instruction","instruction_at":(chrono::Utc::now()-chrono::Duration::minutes(5)).to_rfc3339(),"evidence_excerpt":"Authorized standing order","acknowledge_exact_terms":true,"acknowledge_executable_standing_order":true});
        let revoke = Command::new(
            "POST",
            format!(
                "/api/admin/market-support/capability-assignments/{}/revoke",
                id(0x3101)
            ),
            0x3001,
            json!({"reason":"Concurrent authority review"}),
        );
        // The context header is part of the authenticated command, not its body.
        create.body["__fixture_context"] = json!(id(0x3201));
        let (a, b) = if create_first {
            f.overlap(create, revoke).await
        } else {
            f.overlap(revoke, create).await
        };
        ok(
            &a,
            if create_first {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
        );
        ok(
            &b,
            if create_first {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            },
        );
        let context: String =
            sqlx::query_scalar("SELECT status::text FROM market_support_contexts WHERE id=$1")
                .bind(id(0x3201))
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_ne!(context, "ACTIVE");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM orderbook_orders")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, i64::from(create_first));
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn server_abort_classes_retry_only_transaction_aborts() {
    for state in ["40001", "40P01", "55P03", "57014", "ZF001"] {
        let mut f = Fixture::new().await;
        sqlx::query("SELECT create_fixture_abort_once($1)")
            .bind(state)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let result = f.call(f.create("200", "90")).await;
        let retry = matches!(state, "40001" | "40P01");
        ok(
            &result,
            if retry {
                StatusCode::CREATED
            } else if state == "57014" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::CONFLICT
            },
        );
        let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM create_fixture_attempts")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(attempts, if retry { 2 } else { 1 }, "SQLSTATE {state}");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM orderbook_orders")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, i64::from(retry));
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn severed_commit_acknowledgement_never_reexecutes_create() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut f = Fixture::new().await;
    let mut target = url::Url::parse(&std::env::var("RUST_RACES_DATABASE_URL").unwrap()).unwrap();
    target.query_pairs_mut().append_pair("sslmode", "disable");
    let upstream = target.port().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    target
        .set_port(Some(listener.local_addr().unwrap().port()))
        .unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    let count = commits.clone();
    let proxy = tokio::spawn(async move {
        let (mut client, _) = listener.accept().await.unwrap();
        let mut server = tokio::net::TcpStream::connect(("127.0.0.1", upstream))
            .await
            .unwrap();
        let len = client.read_u32().await.unwrap();
        let mut startup = vec![0; len as usize - 4];
        client.read_exact(&mut startup).await.unwrap();
        server.write_u32(len).await.unwrap();
        server.write_all(&startup).await.unwrap();
        let (mut cr, mut cw) = client.into_split();
        let (mut sr, mut sw) = server.into_split();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let arm = armed.clone();
        let sender = tokio::spawn(async move {
            loop {
                let kind = cr.read_u8().await?;
                let len = cr.read_u32().await?;
                let mut bytes = vec![0; len as usize - 4];
                cr.read_exact(&mut bytes).await?;
                if kind == b'Q'
                    && bytes
                        .strip_suffix(&[0])
                        .is_some_and(|s| s.eq_ignore_ascii_case(b"COMMIT"))
                {
                    arm.store(true, Ordering::SeqCst);
                    count.fetch_add(1, Ordering::SeqCst);
                }
                sw.write_u8(kind).await?;
                sw.write_u32(len).await?;
                sw.write_all(&bytes).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), std::io::Error>(())
        });
        while let Ok(kind) = sr.read_u8().await {
            let len = sr.read_u32().await.unwrap();
            let mut bytes = vec![0; len as usize - 4];
            sr.read_exact(&mut bytes).await.unwrap();
            if armed.load(Ordering::SeqCst) {
                if kind == b'Z' {
                    cw.shutdown().await.unwrap();
                    break;
                }
            } else {
                cw.write_u8(kind).await.unwrap();
                cw.write_u32(len).await.unwrap();
                cw.write_all(&bytes).await.unwrap();
            }
        }
        sender.abort();
        let _ = sender.await;
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .min_connections(0)
        .max_connections(1)
        .connect_lazy(target.as_str())
        .unwrap();
    let router = order_create::contract_router(pool, KEY, order_create::CreateSettings::default());
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        send_create_contract(router, f.create("200", "90").keyed("commit")),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), proxy)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.0, StatusCode::SERVICE_UNAVAILABLE, "{reply:?}");
    assert_eq!(reply.1["detail"]["code"], "COMMIT_OUTCOME_UNKNOWN");
    assert_eq!(commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM orderbook_orders")
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        1
    );
    let replay = f.call(f.create("200", "90").keyed("commit")).await;
    assert_eq!(replay.0, StatusCode::CREATED, "{replay:?}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&mut f.admin)
            .await
            .unwrap(),
        1
    );
}

async fn send_create_contract(router: Router, command: Command) -> (StatusCode, Value) {
    let now = chrono::Utc::now().timestamp();
    let token=encode(&Header::default(),&json!({"sub":id(command.actor),"type":"access","iat":now,"exp":now+600,"iss":"create-tests","aud":"create-tests"}),&EncodingKey::from_secret(KEY.as_bytes())).unwrap();
    let response = router
        .oneshot(
            Request::builder()
                .method(command.method)
                .uri(command.path)
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", command.key.unwrap())
                .header("content-type", "application/json")
                .body(Body::from(command.body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn partial_take_then_cancel_preserves_the_pending_reservation() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let take = f.take(order, "300").await;
    ok(&f.call(take).await, StatusCode::OK);
    let cancel = f.call(f.cancel(order)).await;
    assert_eq!(
        f.stock().await,
        (Decimal::from(700), Decimal::from(300)),
        "cancel releases only the unfilled remainder"
    );
    ok(&cancel, StatusCode::NO_CONTENT);
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn take_first_partial_and_full_expiry_keep_pending_stock() {
    for quantity in [300, 1000] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        sqlx::query("SELECT races_near_expiry($1)")
            .bind(order)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mut take = f.take(order, &quantity.to_string()).await;
        take.body["__fixture_wait_expiry"] = json!(order);
        let (a, b) = f.overlap(take, f.publish()).await;
        ok(&a, StatusCode::OK);
        if quantity == 300 {
            ok(&b, StatusCode::OK);
        } else {
            ok(&b, StatusCode::BAD_REQUEST);
        }
        f.ledger().await;
        sqlx::query("SELECT races_expire($1)")
            .bind(order)
            .execute(&mut f.admin)
            .await
            .unwrap();
        if quantity == 300 {
            ok(&f.call(f.publish()).await, StatusCode::OK);
        } else {
            ok(&f.call(f.publish()).await, StatusCode::BAD_REQUEST);
        }
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM orderbook_orders WHERE id=$1")
                .bind(order)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(status, if quantity == 300 { "EXPIRED" } else { "FILLED" });
        assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000)));
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn delete_refusal_and_take_overlap_without_removing_inventory() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let take = f.take(order, "300").await;
    let delete = Command::new(
        "DELETE",
        format!("/api/inventory/{}", id(ITEM)),
        SELLER,
        Value::Null,
    );
    let (a, b) = f.refusal_overlap(delete, take, id(ITEM), false).await;
    ok(&a, StatusCode::CONFLICT);
    ok(&b, StatusCode::OK);
    assert_eq!(f.stock().await, (Decimal::ZERO, Decimal::from(1000)));
    let take = f.take(order, "200").await;
    let delete = Command::new(
        "DELETE",
        format!("/api/inventory/{}", id(ITEM)),
        SELLER,
        Value::Null,
    );
    let (a, b) = f.overlap(take, delete).await;
    ok(&a, StatusCode::OK);
    ok(&b, StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn pay_first_refusal_and_delivery_overlap() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let take = f.take(order, "300").await;
    let taken = f.call(take).await;
    ok(&taken, StatusCode::OK);
    let trade = taken.1["id"].as_str().unwrap().parse().unwrap();
    ok(&f.call(lifecycle(trade, "confirm")).await, StatusCode::OK);
    let before = f.effects().await;
    let (a, b) = f
        .refusal_overlap(
            lifecycle(trade, "pay"),
            lifecycle(trade, "deliver"),
            trade,
            true,
        )
        .await;
    ok(&a, StatusCode::BAD_REQUEST);
    assert_eq!(a.1["detail"]["code"], "TRADE_NOT_DELIVERED");
    ok(&b, StatusCode::OK);
    let after = f.effects().await;
    assert_eq!(
        after["audit"].as_i64().unwrap(),
        before["audit"].as_i64().unwrap() + 1
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn publication_replay_refuses_other_principal_and_changed_hash() {
    let mut f = Fixture::new().await;
    ok(
        &f.call(f.publish().keyed("principal")).await,
        StatusCode::OK,
    );
    sqlx::query("SELECT races_publication_member()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.effects().await;
    let mut other = f.publish().keyed("principal");
    other.actor = 0x2002;
    let refused = f.call(other).await;
    ok(&refused, StatusCode::FORBIDDEN);
    assert_eq!(refused.1["detail"]["code"], "REPLAY_PRINCIPAL_MISMATCH");
    assert_eq!(f.effects().await, before);
    sqlx::query("SELECT races_publication_hash()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let refused = f.call(f.publish().keyed("principal")).await;
    ok(&refused, StatusCode::CONFLICT);
    assert_eq!(refused.1["detail"]["code"], "IDEMPOTENCY_CONFLICT");
    assert_eq!(f.effects().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_cancel_keys_bind_matching_and_other_resources() {
    for reverse in [false, true] {
        let mut f = Fixture::new().await;
        let first = f.listing().await;
        let second = sqlx::query_scalar("SELECT races_other_order($1)")
            .bind(first)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        let (a, b) = if reverse {
            f.overlap(
                f.cancel(second).keyed("same"),
                f.cancel(first).keyed("same"),
            )
            .await
        } else {
            f.overlap(
                f.cancel(first).keyed("same"),
                f.cancel(second).keyed("same"),
            )
            .await
        };
        ok(&a, StatusCode::NO_CONTENT);
        ok(&b, StatusCode::CONFLICT);
        assert_eq!(b.1["detail"]["code"], "RECEIPT_RESOURCE_MISMATCH");
        let counts = f.effects().await;
        let winner = if reverse { second } else { first };
        let loser = if reverse { first } else { second };
        let (a, b) = f
            .refusal_overlap(
                f.cancel(winner).keyed("same"),
                f.cancel(loser).keyed("same"),
                id(0x2000),
                false,
            )
            .await;
        ok(&a, StatusCode::NO_CONTENT);
        ok(&b, StatusCode::CONFLICT);
        assert_eq!(f.effects().await, counts);
    }
}
