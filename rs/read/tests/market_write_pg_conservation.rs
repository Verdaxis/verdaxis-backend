#![cfg(feature = "contract-tests")]

use std::{collections::BTreeMap, sync::Arc};

use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}, Router};
use proptest::{collection::vec, prelude::*, strategy::ValueTree, test_runner::{Config as GeneratorConfig, RngAlgorithm, TestRng, TestRunner}};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{AppState, Config, MarketPolicy, auth::VerificationSettings};
use verdaxis_read::routes::market_write::orders::served_contract_router;

const KEY: &str = "conservation-test-signing-key-at-least-32-characters";
const SEEDS: [u64; 8] = [1, 17, 43, 256, 1024, 65537, 20261027, 0x5eed];

#[derive(Clone, Copy, Debug)]
enum Writer {
    Create, Edit, Delete, Publish, Bid, Take, Fill, Amend, Confirm, Decline, Elapse, Expire, Cancel,
}
impl Writer {
    fn label(self) -> &'static str {
        match self {
            Self::Create => "rust:inventory.create", Self::Edit => "rust:inventory.edit",
            Self::Delete => "rust:inventory.delete", Self::Publish => "fixture:publish",
            Self::Bid => "fixture:bid", Self::Take => "fixture:take", Self::Fill => "fixture:fill",
            Self::Amend => "fixture:amend", Self::Confirm => "fixture:confirm",
            Self::Decline => "fixture:decline", Self::Elapse => "fixture:elapse",
            Self::Expire => "rust:expiry", Self::Cancel => "rust:order.cancel",
        }
    }
}

fn token(actor: Uuid) -> String {
    let now = chrono::Utc::now().timestamp();
    jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({"sub":actor,"type":"access","iss":"cancel-tests","aud":"cancel-tests","iat":now,"exp":now+600}), &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap()
}
async fn request(router: Router, actor: Uuid, method: &str, path: &str, body: Option<Value>, key: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {}", token(actor)));
    if let Some(key) = key { builder = builder.header("Idempotency-Key", key); }
    let data = body.map(|value| value.to_string()).unwrap_or_default();
    if !data.is_empty() { builder = builder.header("content-type", "application/json"); }
    let response = router.oneshot(builder.body(Body::from(data)).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct Fixture {
    admin: PgConnection,
    inventory: Router,
    orders: Router,
    supplier: Uuid,
    buyer: Uuid,
    organization: Uuid,
    buyer_org: Uuid,
    product: Value,
    product_id: Uuid,
    point: Uuid,
    port: String,
    supplied: BTreeMap<Uuid, Decimal>,
    admin_url: String,
}
impl Fixture {
    async fn new() -> Self {
        let admin_url = std::env::var("RUST_CONSERVATION_ADMIN_DATABASE_URL").expect("disposable administrator URL");
        let runtime_url = std::env::var("RUST_CONSERVATION_DATABASE_URL").expect("disposable runtime URL");
        let admin_target = url::Url::parse(&admin_url).unwrap();
        let runtime_target = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!((admin_target.host_str(), admin_target.port(), admin_target.path()), (runtime_target.host_str(), runtime_target.port(), runtime_target.path()));
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_effects.sql")).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_conservation.sql")).execute(&mut admin).await.unwrap();
        sqlx::query("SELECT conservation_reset() ").execute(&mut admin).await.unwrap();
        let raw: Value = serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = raw["products"].as_array().unwrap().iter().find(|p| p["qualification_profile"] == "scalar" && p["specification_standard"].is_null() && p["execution_mode"] == "ORDERBOOK").unwrap().clone();
        let product_id = product["id"].as_str().unwrap().parse().unwrap();
        let point = product["lanes"][0].as_str().unwrap().parse().unwrap();
        let supplier = Uuid::new_v4(); let buyer = Uuid::new_v4();
        let organization = Uuid::new_v4(); let buyer_org = Uuid::new_v4();
        let port = format!("c-{}", &supplier.simple().to_string()[..12]);
        sqlx::query("SELECT conservation_seed($1,$2,$3,$4,$5,$6)").bind(organization).bind(supplier).bind(buyer_org).bind(buyer).bind(&port).bind(point).execute(&mut admin).await.unwrap();
        let policy: MarketPolicy = serde_json::from_value(raw).unwrap();
        let config = Config { environment: "test".into(), release_sha: verdaxis_read::compiled_sha().into(), migration_revision: "p89_20261022_watchlist_producer".into(), database_url: runtime_url.clone(), db_session_time_zone: "UTC".into(), prepared_budget: None, bind: "127.0.0.1:0".parse().unwrap(), read_admission_slots: 8, pool_max: 9, python_budget: 52, configured_max_connections: 100, kyc_execution_gate_enabled: true };
        let pools = verdaxis_read::connect(&config).await.unwrap();
        let state = AppState::new(config, pools, policy).unwrap();
        let verification = VerificationSettings::new(KEY.into(), None, "cancel-tests".into(), "cancel-tests".into());
        let inventory = verdaxis_read::routes::market_write::inventory::router(state, verification);
        let pool = PgPoolOptions::new().max_connections(10).connect(&runtime_url).await.unwrap();
        let orders = served_contract_router(pool, KEY, true);
        Self { admin, inventory, orders, supplier, buyer, organization, buyer_org, product, product_id, point, port, supplied: BTreeMap::new(), admin_url }
    }
    async fn create(&mut self, quantity: &str) -> (StatusCode, Value) {
        let body = json!({"port_id":self.port,"product_name":self.product["name"],"fuel_type":self.product["fuel_type"],"current_stock_mt":quantity,"incoming_stock_mt":"0","price_per_mt_usd":"100"});
        let result = request(self.inventory.clone(), self.supplier, "POST", "/api/inventory", Some(body), None).await;
        if result.0 == StatusCode::OK {
            self.supplied.insert(result.1["id"].as_str().unwrap().parse().unwrap(), Decimal::from_str_exact(quantity).unwrap());
        }
        result
    }
    async fn stock_step(&mut self, operation: &str, item: Uuid, order: Option<Uuid>, trade: Option<Uuid>, amount: Decimal) -> Value {
        sqlx::query_scalar("SELECT conservation_step($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(operation).bind(item).bind(order).bind(trade).bind(amount).bind(self.product_id).bind(self.point)
            .bind(self.buyer_org).bind(self.buyer).bind(self.product["code"].as_str().unwrap())
            .fetch_one(&mut self.admin).await.unwrap()
    }
    async fn cancel_effects(&mut self, order: Uuid, receipt_count: i64) {
        let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action='order.cancelled' AND resource_id=$1").bind(order.to_string()).fetch_one(&mut self.admin).await.unwrap();
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE event_type='order_cancelled' AND aggregate_id=$1").bind(order.to_string()).fetch_one(&mut self.admin).await.unwrap();
        let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM market_command_results WHERE actor_user_id=$1 AND operation='order.cancel'").bind(self.supplier).fetch_one(&mut self.admin).await.unwrap();
        let notifications: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE recipient_id IN ($1,$2)").bind(self.supplier).bind(self.buyer).fetch_one(&mut self.admin).await.unwrap();
        assert_eq!((audit,events,receipts,notifications),(1,1,receipt_count,0));
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('inventory',coalesce((SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM inventory_items i WHERE supplier_id=$1),'[]'::jsonb),'orders',coalesce((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM orderbook_orders o WHERE organization_id IN ($1,$2)),'[]'::jsonb),'trades',coalesce((SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM trades t WHERE buyer_id IN ($1,$2) OR seller_id IN ($1,$2)),'[]'::jsonb),'audit',coalesce((SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM audit_logs a WHERE user_id IN ($3,$4)),'[]'::jsonb),'events',coalesce((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM market_event_outbox e WHERE participant_org_ids::jsonb @> jsonb_build_array($1::text) OR participant_org_ids::jsonb @> jsonb_build_array($2::text)),'[]'::jsonb),'receipts',coalesce((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM market_command_results r WHERE actor_user_id IN ($3,$4)),'[]'::jsonb),'notifications',coalesce((SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n WHERE recipient_id IN ($3,$4)),'[]'::jsonb))").bind(self.organization).bind(self.buyer_org).bind(self.supplier).bind(self.buyer).fetch_one(&mut self.admin).await.unwrap()
    }
    async fn check(&mut self, seed: u64, index: usize, writer: Writer, input: &str) {
        for (&item, &supply) in &self.supplied {
            let row = sqlx::query("SELECT i.current_stock_mt, i.reserved_stock_mt, i.incoming_stock_mt, coalesce((SELECT sum(o.remaining_quantity_mt) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status IN ('OPEN','PARTIALLY_FILLED')),0) AS live, coalesce((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=coalesce(t.ask_order_id,t.bid_order_id) WHERE o.inventory_item_id=i.id AND t.status='PENDING_CONFIRMATION'),0) AS pending, coalesce((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=t.ask_order_id WHERE o.inventory_item_id=i.id AND t.status IN ('CONFIRMED','DELIVERED','PAID')),0) AS consumed, (SELECT count(*) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.side<>'ASK') AS invalid_side FROM inventory_items i WHERE i.id=$1").bind(item).fetch_one(&mut self.admin).await.unwrap();
            let current: Decimal = row.get("current_stock_mt"); let reserved: Decimal = row.get("reserved_stock_mt");
            let incoming: Decimal = row.get("incoming_stock_mt"); let live: Decimal = row.get("live");
            let pending: Decimal = row.get("pending"); let consumed: Decimal = row.get("consumed");
            let context = format!("seed={seed} step={index} writer={} input={input} item={item}", writer.label());
            assert_eq!(row.get::<i64,_>("invalid_side"), 0, "{context}");
            assert_eq!(reserved, live + pending, "{context} reserved");
            assert_eq!(current + reserved + consumed, supply, "{context} conservation");
            for value in [current, reserved, incoming] { assert!(value >= Decimal::ZERO && value <= Decimal::from(100_000) && value.scale() <= 2, "{context} quantity={value}"); }
            let rows = sqlx::query("SELECT o.id,o.status,o.quantity_mt,o.remaining_quantity_mt,coalesce((SELECT sum(t.quantity_mt) FROM trades t WHERE (t.ask_order_id=o.id OR t.bid_order_id=o.id) AND t.status IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')),0) AS traded FROM orderbook_orders o WHERE o.inventory_item_id=$1").bind(item).fetch_all(&mut self.admin).await.unwrap();
            for order in rows {
                let quantity: Decimal = order.get("quantity_mt"); let remaining: Decimal = order.get("remaining_quantity_mt");
                let traded: Decimal = order.get("traded"); let status: String = order.get("status");
                assert!(remaining >= Decimal::ZERO && remaining <= quantity, "{context} order={:?}", order.get::<Uuid,_>("id"));
                match status.as_str() {
                    "OPEN" => { assert_eq!(remaining, quantity, "{context}"); assert_eq!(remaining, quantity-traded, "{context}"); },
                    "PARTIALLY_FILLED" => { assert!(remaining > Decimal::ZERO && remaining < quantity, "{context}"); assert_eq!(remaining, quantity-traded, "{context}"); },
                    "FILLED" => assert_eq!(remaining, Decimal::ZERO, "{context}"),
                    "CANCELLED" | "EXPIRED" => {}, _ => panic!("{context} status={status}"),
                }
            }
        }
    }
}

fn sequence(seed: u64) -> Vec<(u8,u16)> {
    let mut bytes = [0_u8;32]; bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let config = GeneratorConfig { failure_persistence: None, ..GeneratorConfig::default() };
    let mut runner = TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    vec((0_u8..12,1_u16..100), 40).new_tree(&mut runner).unwrap().current()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn stock_invariant_after_random_sequence() {
    let seeds = match std::env::var("CONSERVATION_SEED") { Ok(value) => vec![value.parse().unwrap()], Err(_) => SEEDS.to_vec() };
    println!("conservation seeds={seeds:?}");
    for seed in seeds {
        let steps = sequence(seed);
        let mut f = Fixture::new().await;
        let (status, value) = f.create("100.00").await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
        f.check(seed, 0, Writer::Create, "100.00").await;
        let order: Uuid = f.stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO).await["order"].as_str().unwrap().parse().unwrap();
        f.check(seed, 1, Writer::Publish, "initial listing").await;
        let trade: Uuid = f.stock_step(Writer::Take.label(), item, Some(order), None, Decimal::from(2)).await["trade"].as_str().unwrap().parse().unwrap();
        f.check(seed, 2, Writer::Take, "2").await;
        f.stock_step(Writer::Confirm.label(), item, Some(order), Some(trade), Decimal::ZERO).await;
        f.check(seed, 3, Writer::Confirm, "2").await;
        for (index, (kind, amount)) in steps.into_iter().enumerate() {
            let writer = match kind { 0 => Writer::Publish, 1 => Writer::Take, 2 => Writer::Confirm, 3 => Writer::Decline, 4 => Writer::Amend, 5 => Writer::Edit, 6 => Writer::Delete, 7 => Writer::Cancel, 8 => Writer::Bid, 9 => Writer::Fill, 10 => Writer::Elapse, _ => Writer::Expire };
            let context = format!("kind={kind} amount={amount}");
            let live_order: Option<Uuid> = sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE inventory_item_id=$1 AND status IN ('OPEN','PARTIALLY_FILLED') ORDER BY created_at DESC LIMIT 1").bind(item).fetch_optional(&mut f.admin).await.unwrap();
            let pending: Option<(Uuid, Uuid)> = sqlx::query_as("SELECT t.id,o.id FROM trades t JOIN orderbook_orders o ON o.id=t.ask_order_id WHERE o.inventory_item_id=$1 AND t.status='PENDING_CONFIRMATION' ORDER BY t.created_at LIMIT 1").bind(item).fetch_optional(&mut f.admin).await.unwrap();
            let trade = pending.map(|pair| pair.0);
            let order = if matches!(writer, Writer::Confirm | Writer::Decline) { pending.map(|pair| pair.1) } else { live_order };
            match writer {
                Writer::Edit | Writer::Delete => {
                    let before = f.snapshot().await;
                    let old_current: Decimal = sqlx::query_scalar("SELECT current_stock_mt FROM inventory_items WHERE id=$1").bind(item).fetch_one(&mut f.admin).await.unwrap();
                    let path = format!("/api/inventory/{item}");
                    let (status, body) = if matches!(writer,Writer::Edit) { request(f.inventory.clone(),f.supplier,"PATCH",&path,Some(json!({"current_stock_mt":"100.00"})),None).await } else { request(f.inventory.clone(),f.supplier,"DELETE",&path,None,None).await };
                    if status.is_client_error() { assert_eq!(f.snapshot().await,before,"seed={seed} {context} {body}"); }
                    else if matches!(writer,Writer::Delete) { f.supplied.remove(&item); break; }
                    else { *f.supplied.get_mut(&item).unwrap() += Decimal::from(100) - old_current; }
                },
                Writer::Cancel => if let Some(order) = order { let before=f.snapshot().await; let (status,body)=request(f.orders.clone(),f.supplier,"DELETE",&format!("/api/orderbook/{order}"),None,None).await; if status.is_client_error() { assert_eq!(f.snapshot().await,before,"{body}"); } },
                Writer::Expire => { verdaxis_read::market_contract::stock_contract::expire(&mut f.admin,f.product_id,Some(f.point),"SPOT","ASK",chrono::Utc::now()).await.unwrap(); },
                Writer::Fill => { let bid: Option<Uuid>=sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE organization_id=$1 AND side='BID' AND status IN ('OPEN','PARTIALLY_FILLED') ORDER BY created_at LIMIT 1").bind(f.buyer_org).fetch_optional(&mut f.admin).await.unwrap(); let before=f.snapshot().await; let outcome=f.stock_step(writer.label(),item,order,bid,Decimal::ZERO).await; if outcome["refused"]==true {assert_eq!(f.snapshot().await,before,"{context}");} },
                _ => { let before=f.snapshot().await; let outcome = f.stock_step(writer.label(),item,order,trade,Decimal::new(i64::from(amount),2)).await; if outcome["refused"] == true { assert_eq!(f.snapshot().await,before,"seed={seed} {context}: {outcome}"); } }
            }
            f.check(seed,index+4,writer,&context).await;
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_release_is_exactly_once() {
    let mut f=Fixture::new().await;
    let (_, item)=f.create("10.00").await; let item: Uuid=item["id"].as_str().unwrap().parse().unwrap();
    let order: Uuid=f.stock_step("fixture:publish",item,None,None,Decimal::ZERO).await["order"].as_str().unwrap().parse().unwrap();
    let trade: Uuid=f.stock_step("fixture:take",item,Some(order),None,Decimal::from(4)).await["trade"].as_str().unwrap().parse().unwrap();
    let path=format!("/api/orderbook/{order}");
    assert_eq!(request(f.orders.clone(),f.supplier,"DELETE",&path,None,Some("same-key")).await.0,StatusCode::NO_CONTENT);
    let before=f.snapshot().await;
    assert_eq!(request(f.orders.clone(),f.supplier,"DELETE",&path,None,Some("same-key")).await.0,StatusCode::NO_CONTENT);
    assert_eq!(f.snapshot().await,before);
    assert_eq!(request(f.orders.clone(),f.supplier,"DELETE",&path,None,Some("new-key")).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(f.snapshot().await,before);
    f.check(0,1,Writer::Cancel,"same-key").await;
    f.cancel_effects(order,1).await;
    f.stock_step("fixture:confirm",item,Some(order),Some(trade),Decimal::ZERO).await;
    f.check(0,2,Writer::Confirm,"pending").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn parallel_cancel_has_one_winner() {
    for keyed in [false,true] {
        let mut f=Fixture::new().await;
        let (_,item)=f.create("10.00").await; let item=item["id"].as_str().unwrap().parse().unwrap();
        let order: Uuid=f.stock_step("fixture:publish",item,None,None,Decimal::ZERO).await["order"].as_str().unwrap().parse().unwrap();
        f.stock_step("fixture:take",item,Some(order),None,Decimal::from(4)).await;
        let barrier=Arc::new(Barrier::new(6)); let mut tasks=Vec::new();
        for _ in 0..6 { let barrier=barrier.clone(); let router=f.orders.clone(); let actor=f.supplier; let path=format!("/api/orderbook/{order}"); tasks.push(tokio::spawn(async move { barrier.wait().await; request(router,actor,"DELETE",&path,None,keyed.then_some("parallel-key")).await })); }
        let mut success=0;
        for task in tasks { let (status,body)=task.await.unwrap(); if status==StatusCode::NO_CONTENT {success+=1;} else { let code=body["detail"]["code"].as_str().unwrap_or(""); assert!((status==StatusCode::BAD_REQUEST && code=="ORDER_TERMINAL") || (status==StatusCode::CONFLICT && code=="MARKET_BUSY") || (keyed && status==StatusCode::SERVICE_UNAVAILABLE && code=="IDEMPOTENCY_BUSY"),"{status} {body}"); } }
        if keyed { assert!(success>=1); } else { assert_eq!(success,1); }
        f.check(0,0,Writer::Cancel,"parallel").await;
        f.cancel_effects(order, i64::from(keyed)).await;
        if keyed { let before=f.snapshot().await; assert_eq!(request(f.orders.clone(),f.supplier,"DELETE",&format!("/api/orderbook/{order}"),None,Some("parallel-key")).await.0,StatusCode::NO_CONTENT); assert_eq!(f.snapshot().await,before); }
    }
}
