#![cfg(feature = "contract-tests")]

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use proptest::{
    collection::vec,
    prelude::*,
    strategy::ValueTree,
    test_runner::{Config as GeneratorConfig, RngAlgorithm, TestRng, TestRunner},
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::routes::market_write::orders::served_contract_router;
use verdaxis_read::{AppState, Config, MarketPolicy, auth::VerificationSettings};

const KEY: &str = "conservation-test-signing-key-at-least-32-characters";
const SEEDS: [u64; 8] = [1, 17, 43, 256, 1024, 65537, 20261027, 0x5eed];

#[derive(Clone, Copy, Debug)]
enum Writer {
    Create,
    Edit,
    Delete,
    Publish,
    Bid,
    Take,
    Fill,
    Amend,
    Confirm,
    Decline,
    Elapse,
    Expire,
    Cancel,
}
impl Writer {
    fn label(self) -> &'static str {
        match self {
            Self::Create => "rust:inventory.create",
            Self::Edit => "rust:inventory.edit",
            Self::Delete => "rust:inventory.delete",
            Self::Publish => "fixture:publish",
            Self::Bid => "fixture:bid",
            Self::Take => "fixture:take",
            Self::Fill => "fixture:fill",
            Self::Amend => "fixture:amend",
            Self::Confirm => "fixture:confirm",
            Self::Decline => "fixture:decline",
            Self::Elapse => "fixture:elapse",
            Self::Expire => "rust:expiry",
            Self::Cancel => "rust:order.cancel",
        }
    }
}

fn token(actor: Uuid) -> String {
    let now = chrono::Utc::now().timestamp();
    jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({"sub":actor,"type":"access","iss":"cancel-tests","aud":"cancel-tests","iat":now,"exp":now+600}), &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap()
}
async fn request(
    router: Router,
    actor: Uuid,
    method: &str,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(actor)));
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    let data = body.map(|value| value.to_string()).unwrap_or_default();
    if !data.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let response = router
        .oneshot(builder.body(Body::from(data)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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
        let admin_url = std::env::var("RUST_CONSERVATION_ADMIN_DATABASE_URL")
            .expect("disposable administrator URL");
        let runtime_url =
            std::env::var("RUST_CONSERVATION_DATABASE_URL").expect("disposable runtime URL");
        let admin_target = url::Url::parse(&admin_url).unwrap();
        let runtime_target = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
        assert!(admin_target.path().ends_with("_test"));
        assert_eq!(
            (
                admin_target.host_str(),
                admin_target.port(),
                admin_target.path()
            ),
            (
                runtime_target.host_str(),
                runtime_target.port(),
                runtime_target.path()
            )
        );
        assert_ne!(admin_target.username(), runtime_target.username());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_effects.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_conservation.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("SELECT conservation_reset() ")
            .execute(&mut admin)
            .await
            .unwrap();
        let raw: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = raw["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["qualification_profile"] == "scalar"
                    && p["specification_standard"].is_null()
                    && p["execution_mode"] == "ORDERBOOK"
            })
            .unwrap()
            .clone();
        let product_id = product["id"].as_str().unwrap().parse().unwrap();
        let point = product["lanes"][0].as_str().unwrap().parse().unwrap();
        let supplier = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let organization = Uuid::new_v4();
        let buyer_org = Uuid::new_v4();
        let port = format!("c-{}", &supplier.simple().to_string()[..12]);
        sqlx::query("SELECT conservation_seed($1,$2,$3,$4,$5,$6,$7)")
            .bind(organization)
            .bind(supplier)
            .bind(buyer_org)
            .bind(buyer)
            .bind(&port)
            .bind(point)
            .bind(product_id)
            .execute(&mut admin)
            .await
            .unwrap();
        let policy: MarketPolicy = serde_json::from_value(raw).unwrap();
        let config = Config {
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "p89_20261022_watchlist_producer".into(),
            database_url: runtime_url.clone(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 8,
            pool_max: 9,
            python_budget: 52,
            configured_max_connections: 100,
            kyc_execution_gate_enabled: true,
        };
        let pools = verdaxis_read::connect(&config).await.unwrap();
        let state = AppState::new(config, pools, policy).unwrap();
        let verification = VerificationSettings::new(
            KEY.into(),
            None,
            "cancel-tests".into(),
            "cancel-tests".into(),
        );
        let inventory = verdaxis_read::routes::market_write::inventory::router(state, verification);
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&runtime_url)
            .await
            .unwrap();
        let orders = served_contract_router(pool, KEY, true);
        Self {
            admin,
            inventory,
            orders,
            supplier,
            buyer,
            organization,
            buyer_org,
            product,
            product_id,
            point,
            port,
            supplied: BTreeMap::new(),
            admin_url,
        }
    }
    async fn create(&mut self, quantity: &str) -> (StatusCode, Value) {
        let before = self.snapshot().await;
        let body = json!({"port_id":self.port,"product_name":self.product["name"],"fuel_type":self.product["fuel_type"],"current_stock_mt":quantity,"incoming_stock_mt":"0","price_per_mt_usd":"100"});
        let result = request(
            self.inventory.clone(),
            self.supplier,
            "POST",
            "/api/inventory",
            Some(body),
            None,
        )
        .await;
        if result.0 == StatusCode::OK {
            self.supplied.insert(
                result.1["id"].as_str().unwrap().parse().unwrap(),
                Decimal::from_str_exact(quantity).unwrap(),
            );
        }
        if result.0.is_success() {
            let item: Uuid = result.1["id"].as_str().unwrap().parse().unwrap();
            self.inventory_effect(&before, item, "inventory_created")
                .await;
        } else {
            assert_eq!(self.snapshot().await, before);
        }
        result
    }
    async fn inventory_effect(&mut self, before: &Value, item: Uuid, event_type: &str) {
        let after = self.snapshot().await;
        assert_eq!(after["audit"], before["audit"]);
        assert_eq!(after["notifications"], before["notifications"]);
        let old_events = before["events"].as_array().unwrap();
        let events = after["events"].as_array().unwrap();
        assert_eq!(events.len(), old_events.len() + 1);
        for old in old_events {
            assert!(events.contains(old));
        }
        let added: Vec<_> = events
            .iter()
            .filter(|event| !old_events.contains(event))
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0]["event_type"], event_type);
        assert_eq!(added[0]["aggregate_id"], item.to_string());
        assert_eq!(added[0]["participant_org_ids"], json!([self.organization]));
    }
    async fn stock_step(
        &mut self,
        operation: &str,
        item: Uuid,
        order: Option<Uuid>,
        trade: Option<Uuid>,
        amount: Decimal,
    ) -> Value {
        self.validate_domain().await;
        sqlx::query_scalar("SELECT conservation_step($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(operation)
            .bind(item)
            .bind(order)
            .bind(trade)
            .bind(amount)
            .bind(self.product_id)
            .bind(self.point)
            .bind(self.buyer_org)
            .bind(self.buyer)
            .bind(self.product["code"].as_str().unwrap())
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn validate_domain(&mut self) {
        let row=sqlx::query("SELECT p.name,p.fuel_type,p.fuel_grade,p.is_active,p.min_lot_size,d.is_active AS point_active,d.name AS point_name,pt.name AS port_name FROM products p CROSS JOIN delivery_points d JOIN ports pt ON pt.id=$3 WHERE p.id=$1 AND d.id=$2")
            .bind(self.product_id).bind(self.point).bind(&self.port).fetch_one(&mut self.admin).await.unwrap();
        assert!(row.get::<bool, _>("is_active") && row.get::<bool, _>("point_active"));
        assert_eq!(
            row.get::<String, _>("name"),
            self.product["name"].as_str().unwrap()
        );
        assert_eq!(
            row.get::<String, _>("fuel_type"),
            self.product["fuel_type"].as_str().unwrap()
        );
        assert_eq!(
            row.get::<String, _>("fuel_grade"),
            self.product["fuel_grade"].as_str().unwrap()
        );
        assert_eq!(
            row.get::<String, _>("point_name"),
            row.get::<String, _>("port_name")
        );
        assert!(row.get::<Decimal, _>("min_lot_size") <= Decimal::from(10));
        let policy: MarketPolicy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        assert_eq!(self.product["qualification_profile"], "scalar");
        assert_eq!(self.product["execution_mode"], "ORDERBOOK");
        assert!(
            self.product["lanes"]
                .as_array()
                .unwrap()
                .contains(&json!(self.point))
        );
        for side in ["ASK", "BID"] {
            let point = self.point.to_string();
            let input = verdaxis_read::market::terms::QualificationInput {
                product_id: Some(self.product_id),
                market_product: self.product["code"].as_str(),
                side,
                delivery_point_id: Some(&point),
                availability_window: "SPOT",
                off_spec: false,
                msds_available: true,
                certification_scheme: Some("ISCC EU"),
                certification_declared: true,
                fame_terms: None,
                lng_terms: None,
                specification_standard: None,
                carbon_intensity_method: Some("LIFECYCLE"),
                carbon_intensity_gco2_mj: Some("10"),
                feedstock: Some("residue"),
                origin: Some("NL"),
            };
            assert!(verdaxis_read::market::terms::order_is_execution_qualified(
                &policy, &input
            ));
        }
    }
    async fn cancel_effects(&mut self, order: Uuid, receipt_count: i64) {
        let audit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_logs WHERE action='order.cancelled' AND resource_id=$1",
        )
        .bind(order.to_string())
        .fetch_one(&mut self.admin)
        .await
        .unwrap();
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE event_type='order_cancelled' AND aggregate_id=$1").bind(order.to_string()).fetch_one(&mut self.admin).await.unwrap();
        let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM market_command_results WHERE actor_user_id=$1 AND operation='order.cancel'").bind(self.supplier).fetch_one(&mut self.admin).await.unwrap();
        let notifications: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notifications WHERE recipient_id IN ($1,$2)")
                .bind(self.supplier)
                .bind(self.buyer)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        assert_eq!(
            (audit, events, receipts, notifications),
            (1, 1, receipt_count, 0)
        );
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT jsonb_build_object('inventory',coalesce((SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM inventory_items i WHERE supplier_id=$1),'[]'::jsonb),'orders',coalesce((SELECT jsonb_agg(to_jsonb(o) ORDER BY o.id) FROM orderbook_orders o WHERE organization_id IN ($1,$2)),'[]'::jsonb),'trades',coalesce((SELECT jsonb_agg(to_jsonb(t) ORDER BY t.id) FROM trades t WHERE buyer_id IN ($1,$2) OR seller_id IN ($1,$2)),'[]'::jsonb),'audit',coalesce((SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM audit_logs a WHERE user_id IN ($3,$4)),'[]'::jsonb),'events',coalesce((SELECT jsonb_agg(to_jsonb(e) ORDER BY e.id) FROM market_event_outbox e WHERE participant_org_ids::jsonb @> jsonb_build_array($1::text) OR participant_org_ids::jsonb @> jsonb_build_array($2::text)),'[]'::jsonb),'receipts',coalesce((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM market_command_results r WHERE actor_user_id IN ($3,$4)),'[]'::jsonb),'notifications',coalesce((SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n WHERE recipient_id IN ($3,$4)),'[]'::jsonb))").bind(self.organization).bind(self.buyer_org).bind(self.supplier).bind(self.buyer).fetch_one(&mut self.admin).await.unwrap()
    }
    async fn check(&mut self, seed: u64, index: usize, writer: Writer, input: &str) {
        for (&item, &supply) in &self.supplied {
            let row = sqlx::query("SELECT i.current_stock_mt, i.reserved_stock_mt, i.incoming_stock_mt, coalesce((SELECT sum(o.remaining_quantity_mt) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status IN ('OPEN','PARTIALLY_FILLED')),0) AS live, coalesce((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=coalesce(t.ask_order_id,t.bid_order_id) WHERE o.inventory_item_id=i.id AND t.status='PENDING_CONFIRMATION'),0) AS pending, coalesce((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=t.ask_order_id WHERE o.inventory_item_id=i.id AND t.status IN ('CONFIRMED','DELIVERED','PAID')),0) AS consumed, (SELECT count(*) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.side<>'ASK') AS invalid_side FROM inventory_items i WHERE i.id=$1").bind(item).fetch_one(&mut self.admin).await.unwrap();
            let current: Decimal = row.get("current_stock_mt");
            let reserved: Decimal = row.get("reserved_stock_mt");
            let incoming: Decimal = row.get("incoming_stock_mt");
            let live: Decimal = row.get("live");
            let pending: Decimal = row.get("pending");
            let consumed: Decimal = row.get("consumed");
            let context = format!(
                "seed={seed} step={index} writer={} input={input} item={item}",
                writer.label()
            );
            assert_eq!(row.get::<i64, _>("invalid_side"), 0, "{context}");
            assert_eq!(reserved, live + pending, "{context} reserved");
            assert_eq!(
                current + reserved + consumed,
                supply,
                "{context} conservation"
            );
            for value in [current, reserved, incoming] {
                assert!(
                    value >= Decimal::ZERO && value <= Decimal::from(100_000) && value.scale() <= 2,
                    "{context} quantity={value}"
                );
            }
            let rows = sqlx::query("SELECT o.id,o.status,o.quantity_mt,o.remaining_quantity_mt,coalesce((SELECT sum(t.quantity_mt) FROM trades t WHERE (t.ask_order_id=o.id OR t.bid_order_id=o.id) AND t.status IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')),0) AS traded FROM orderbook_orders o WHERE o.inventory_item_id=$1 OR (o.organization_id=$2 AND o.side='BID')").bind(item).bind(self.buyer_org).fetch_all(&mut self.admin).await.unwrap();
            let bid_links: i64 = sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE organization_id=$1 AND side='BID' AND inventory_item_id IS NOT NULL").bind(self.buyer_org).fetch_one(&mut self.admin).await.unwrap();
            assert_eq!(bid_links, 0, "{context} bid inventory link");
            for order in rows {
                let quantity: Decimal = order.get("quantity_mt");
                let remaining: Decimal = order.get("remaining_quantity_mt");
                let traded: Decimal = order.get("traded");
                let status: String = order.get("status");
                assert!(
                    remaining >= Decimal::ZERO && remaining <= quantity,
                    "{context} order={:?}",
                    order.get::<Uuid, _>("id")
                );
                match status.as_str() {
                    "OPEN" => {
                        assert_eq!(remaining, quantity, "{context}");
                        assert_eq!(remaining, quantity - traded, "{context}");
                    }
                    "PARTIALLY_FILLED" => {
                        assert!(
                            remaining > Decimal::ZERO && remaining < quantity,
                            "{context}"
                        );
                        assert_eq!(remaining, quantity - traded, "{context}");
                    }
                    "FILLED" => {
                        assert_eq!(remaining, Decimal::ZERO, "{context}");
                        assert_eq!(remaining, quantity - traded, "{context}");
                    }
                    "CANCELLED" | "EXPIRED" => {}
                    _ => panic!("{context} status={status}"),
                }
            }
        }
    }
}

fn sequence(seed: u64) -> Vec<(u8, u16)> {
    let mut bytes = [0_u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let config = GeneratorConfig {
        failure_persistence: None,
        ..GeneratorConfig::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
    vec((0_u8..12, 1_u16..100), 40)
        .new_tree(&mut runner)
        .unwrap()
        .current()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn inventory_successes_append_one_participant_event() {
    let mut f = Fixture::new().await;
    let (status, value) = f.create("10.00").await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
    for current in ["10.00", "12.34"] {
        let before = f.snapshot().await;
        let (status, body) = request(
            f.inventory.clone(),
            f.supplier,
            "PATCH",
            &format!("/api/inventory/{item}"),
            Some(json!({"current_stock_mt":current})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        f.inventory_effect(&before, item, "inventory_updated").await;
    }
    let before = f.snapshot().await;
    let (status, body) = request(
        f.inventory.clone(),
        f.supplier,
        "DELETE",
        &format!("/api/inventory/{item}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    f.inventory_effect(&before, item, "inventory_deleted").await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn stock_invariant_after_random_sequence() {
    let seeds = match std::env::var("CONSERVATION_SEED") {
        Ok(value) => vec![value.parse().unwrap()],
        Err(_) => SEEDS.to_vec(),
    };
    println!("conservation seeds={seeds:?}");
    for seed in seeds {
        let steps = sequence(seed);
        let mut f = Fixture::new().await;
        let (status, value) = f.create("100.00").await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
        f.check(seed, 0, Writer::Create, "100.00").await;
        let before = f.snapshot().await;
        let (status, body) = f.create("1.001").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(f.snapshot().await, before);
        let order: Uuid = f
            .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
            .await["order"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        f.check(seed, 1, Writer::Publish, "initial listing").await;
        let trade: Uuid = f
            .stock_step(
                Writer::Take.label(),
                item,
                Some(order),
                None,
                Decimal::from(2),
            )
            .await["trade"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        f.check(seed, 2, Writer::Take, "2").await;
        f.stock_step(
            Writer::Confirm.label(),
            item,
            Some(order),
            Some(trade),
            Decimal::ZERO,
        )
        .await;
        f.check(seed, 3, Writer::Confirm, "2").await;
        let up = f
            .stock_step(
                Writer::Amend.label(),
                item,
                Some(order),
                None,
                Decimal::from(101),
            )
            .await;
        assert_eq!(
            up["refused"], true,
            "amend increase without available stock"
        );
        let down = f
            .stock_step(
                Writer::Amend.label(),
                item,
                Some(order),
                None,
                Decimal::from(90),
            )
            .await;
        assert_ne!(down["refused"], true);
        f.check(seed, 4, Writer::Amend, "90").await;
        let up = f
            .stock_step(
                Writer::Amend.label(),
                item,
                Some(order),
                None,
                Decimal::from(100),
            )
            .await;
        assert_ne!(up["refused"], true);
        f.check(seed, 5, Writer::Amend, "100").await;
        let before = f.snapshot().await;
        let (status, body) = request(
            f.inventory.clone(),
            f.supplier,
            "PATCH",
            &format!("/api/inventory/{item}"),
            Some(json!({"current_stock_mt":"5"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(f.snapshot().await, before);
        let too_large = f
            .stock_step(
                Writer::Take.label(),
                item,
                Some(order),
                None,
                Decimal::from(100),
            )
            .await;
        assert_eq!(too_large["refused"], true);
        assert_eq!(f.snapshot().await, before);
        let again = f
            .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
            .await;
        assert_eq!(again["order"], order.to_string());
        assert_eq!(f.snapshot().await, before);
        for (index, (kind, amount)) in steps.into_iter().enumerate() {
            let writer = match kind {
                0 => Writer::Publish,
                1 => Writer::Take,
                2 => Writer::Confirm,
                3 => Writer::Decline,
                4 => Writer::Amend,
                5 => Writer::Edit,
                6 => Writer::Delete,
                7 => Writer::Cancel,
                8 => Writer::Bid,
                9 => Writer::Fill,
                10 => Writer::Elapse,
                _ => Writer::Expire,
            };
            let context = format!("kind={kind} amount={amount}");
            println!(
                "seed={seed} step={} writer={} input={context}",
                index + 6,
                writer.label()
            );
            let live_order: Option<Uuid> = sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE inventory_item_id=$1 AND status IN ('OPEN','PARTIALLY_FILLED') ORDER BY created_at DESC LIMIT 1").bind(item).fetch_optional(&mut f.admin).await.unwrap();
            let pending: Option<(Uuid, Uuid)> = sqlx::query_as("SELECT t.id,o.id FROM trades t JOIN orderbook_orders o ON o.id=t.ask_order_id WHERE o.inventory_item_id=$1 AND t.status='PENDING_CONFIRMATION' ORDER BY t.created_at LIMIT 1").bind(item).fetch_optional(&mut f.admin).await.unwrap();
            let trade = pending.map(|pair| pair.0);
            let order = if matches!(writer, Writer::Confirm | Writer::Decline) {
                pending.map(|pair| pair.1)
            } else {
                live_order
            };
            match writer {
                Writer::Edit | Writer::Delete => {
                    let before = f.snapshot().await;
                    let old_current: Decimal = sqlx::query_scalar(
                        "SELECT current_stock_mt FROM inventory_items WHERE id=$1",
                    )
                    .bind(item)
                    .fetch_one(&mut f.admin)
                    .await
                    .unwrap();
                    let path = format!("/api/inventory/{item}");
                    let (status, body) = if matches!(writer, Writer::Edit) {
                        request(
                            f.inventory.clone(),
                            f.supplier,
                            "PATCH",
                            &path,
                            Some(json!({"current_stock_mt":"100.00"})),
                            None,
                        )
                        .await
                    } else {
                        request(f.inventory.clone(), f.supplier, "DELETE", &path, None, None).await
                    };
                    if status.is_client_error() {
                        assert_eq!(f.snapshot().await, before, "seed={seed} {context} {body}");
                    } else if matches!(writer, Writer::Delete) {
                        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
                        f.inventory_effect(&before, item, "inventory_deleted").await;
                        f.supplied.remove(&item);
                        break;
                    } else {
                        assert_eq!(status, StatusCode::OK, "{body}");
                        f.inventory_effect(&before, item, "inventory_updated").await;
                        *f.supplied.get_mut(&item).unwrap() += Decimal::from(100) - old_current;
                    }
                }
                Writer::Cancel => {
                    if let Some(order) = order {
                        let before = f.snapshot().await;
                        let (status, body) = request(
                            f.orders.clone(),
                            f.supplier,
                            "DELETE",
                            &format!("/api/orderbook/{order}"),
                            None,
                            None,
                        )
                        .await;
                        if status.is_client_error() {
                            assert_eq!(f.snapshot().await, before, "{body}");
                        }
                    }
                }
                Writer::Expire => {
                    verdaxis_read::market_contract::stock_contract::expire(
                        &mut f.admin,
                        f.product_id,
                        Some(f.point),
                        "SPOT",
                        "ASK",
                        chrono::Utc::now(),
                    )
                    .await
                    .unwrap();
                }
                Writer::Fill => {
                    let bid: Option<Uuid>=sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE organization_id=$1 AND side='BID' AND status IN ('OPEN','PARTIALLY_FILLED') ORDER BY created_at LIMIT 1").bind(f.buyer_org).fetch_optional(&mut f.admin).await.unwrap();
                    let before = f.snapshot().await;
                    let outcome = f
                        .stock_step(writer.label(), item, order, bid, Decimal::ZERO)
                        .await;
                    if outcome["refused"] == true {
                        assert_eq!(f.snapshot().await, before, "{context}");
                    }
                }
                _ => {
                    let before = f.snapshot().await;
                    let outcome = f
                        .stock_step(
                            writer.label(),
                            item,
                            order,
                            trade,
                            Decimal::new(i64::from(amount), 2),
                        )
                        .await;
                    if outcome["refused"] == true {
                        assert_eq!(
                            f.snapshot().await,
                            before,
                            "seed={seed} {context}: {outcome}"
                        );
                    }
                }
            }
            f.check(seed, index + 6, writer, &context).await;
        }
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn cancel_release_is_exactly_once() {
    let mut f = Fixture::new().await;
    let (_, item) = f.create("10.00").await;
    let item: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let order: Uuid = f
        .stock_step("fixture:publish", item, None, None, Decimal::ZERO)
        .await["order"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let trade: Uuid = f
        .stock_step("fixture:take", item, Some(order), None, Decimal::from(4))
        .await["trade"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let declined: Uuid = f
        .stock_step("fixture:take", item, Some(order), None, Decimal::from(2))
        .await["trade"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let path = format!("/api/orderbook/{order}");
    assert_eq!(
        request(
            f.orders.clone(),
            f.supplier,
            "DELETE",
            &path,
            None,
            Some("same-key")
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let before = f.snapshot().await;
    assert_eq!(
        request(
            f.orders.clone(),
            f.supplier,
            "DELETE",
            &path,
            None,
            Some("same-key")
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.snapshot().await, before);
    assert_eq!(
        request(
            f.orders.clone(),
            f.supplier,
            "DELETE",
            &path,
            None,
            Some("new-key")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.snapshot().await, before);
    f.check(0, 1, Writer::Cancel, "same-key").await;
    f.cancel_effects(order, 1).await;
    f.stock_step(
        "fixture:confirm",
        item,
        Some(order),
        Some(trade),
        Decimal::ZERO,
    )
    .await;
    f.check(0, 2, Writer::Confirm, "pending").await;
    f.stock_step(
        Writer::Decline.label(),
        item,
        Some(order),
        Some(declined),
        Decimal::ZERO,
    )
    .await;
    f.check(0, 3, Writer::Decline, "pending").await;
    let before = f.snapshot().await;
    let (status, body) = request(
        f.inventory.clone(),
        f.supplier,
        "DELETE",
        &format!("/api/inventory/{item}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn completed_fill_leaves_no_stock_to_publish() {
    let mut f = Fixture::new().await;
    let (_, value) = f.create("10.00").await;
    let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
    let ask: Uuid = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await["order"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let bid: Uuid = f
        .stock_step(Writer::Bid.label(), item, None, None, Decimal::from(10))
        .await["order"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    f.check(0, 1, Writer::Bid, "10").await;
    let fill = f
        .stock_step(
            Writer::Fill.label(),
            item,
            Some(ask),
            Some(bid),
            Decimal::ZERO,
        )
        .await;
    assert!(fill["trade"].is_string());
    f.check(0, 2, Writer::Fill, "10").await;
    let before = f.snapshot().await;
    let refused = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await;
    assert_eq!(refused["refused"], true);
    assert_eq!(f.snapshot().await, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn elapsed_decline_expires_the_full_remainder() {
    let mut f = Fixture::new().await;
    let (_, value) = f.create("10.00").await;
    let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
    let order: Uuid = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await["order"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let trade: Uuid = f
        .stock_step(
            Writer::Take.label(),
            item,
            Some(order),
            None,
            Decimal::from(4),
        )
        .await["trade"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    f.stock_step(
        Writer::Elapse.label(),
        item,
        Some(order),
        None,
        Decimal::ZERO,
    )
    .await;
    f.check(0, 1, Writer::Elapse, "elapsed").await;
    f.stock_step(
        Writer::Decline.label(),
        item,
        Some(order),
        Some(trade),
        Decimal::ZERO,
    )
    .await;
    f.check(0, 2, Writer::Decline, "elapsed").await;
    let status: String = sqlx::query_scalar("SELECT status FROM orderbook_orders WHERE id=$1")
        .bind(order)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(status, "EXPIRED");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_preserves_pending_and_allows_republication() {
    let mut f = Fixture::new().await;
    let (status, value) = f.create("10.00").await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
    let order: Uuid = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await["order"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let trade: Uuid = f
        .stock_step(
            Writer::Take.label(),
            item,
            Some(order),
            None,
            Decimal::from(4),
        )
        .await["trade"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    f.stock_step(
        Writer::Elapse.label(),
        item,
        Some(order),
        None,
        Decimal::ZERO,
    )
    .await;
    f.check(0, 1, Writer::Elapse, "elapsed").await;
    let before = f.snapshot().await;
    let refused = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await;
    assert_eq!(refused["refused"], true);
    assert_eq!(f.snapshot().await, before);
    let expired = verdaxis_read::market_contract::stock_contract::expire(
        &mut f.admin,
        f.product_id,
        Some(f.point),
        "SPOT",
        "ASK",
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!(expired.len(), 1);
    f.check(0, 2, Writer::Expire, "pending").await;
    let next = f
        .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
        .await;
    assert!(next["order"].is_string());
    f.check(0, 3, Writer::Publish, "republish").await;
    f.stock_step(
        Writer::Decline.label(),
        item,
        Some(order),
        Some(trade),
        Decimal::ZERO,
    )
    .await;
    f.check(0, 4, Writer::Decline, "expired pending").await;
}

async fn wait_for_lock(
    observer: &mut PgConnection,
    blocker: i32,
    waiter: Option<i32>,
    kind: &str,
) -> i32 {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(450);
    loop {
        let pid: Option<i32> = sqlx::query_scalar("SELECT l.pid FROM pg_locks l WHERE NOT l.granted AND l.locktype=$1 AND $2=ANY(pg_blocking_pids(l.pid)) AND ($3::int IS NULL OR l.pid=$3) LIMIT 1")
            .bind(kind).bind(blocker).bind(waiter).fetch_optional(&mut *observer).await.unwrap();
        if let Some(pid) = pid {
            println!("LOCK_MILESTONE blocker={blocker} waiter={pid} kind={kind}");
            return pid;
        }
        if tokio::time::Instant::now() >= deadline {
            let locks: Vec<(i32,String,bool)> = sqlx::query_as("SELECT pid,locktype,granted FROM pg_locks WHERE pid<>pg_backend_pid() AND database=(SELECT oid FROM pg_database WHERE datname=current_database())").fetch_all(&mut *observer).await.unwrap();
            panic!("lock milestone not reached: blocker={blocker} kind={kind} locks={locks:?}");
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn take_and_cancel_on_one_order_conserve_stock() {
    for take_first in [true, false] {
        let mut f = Fixture::new().await;
        let (_, value) = f.create("10.00").await;
        let item: Uuid = value["id"].as_str().unwrap().parse().unwrap();
        let order: Uuid = f
            .stock_step(Writer::Publish.label(), item, None, None, Decimal::ZERO)
            .await["order"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let key = verdaxis_read::market_contract::locks::market_slice_lock_key(
            f.product_id,
            Some(f.point),
            "SPOT",
        );
        let mut holder = PgConnection::connect(&f.admin_url).await.unwrap();
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut holder)
            .await
            .unwrap();
        let mut hold = holder.begin().await.unwrap();
        if take_first {
            sqlx::query("SELECT conservation_hold($1,$2,$3)")
                .bind("slice")
                .bind(item)
                .bind(key)
                .execute(&mut *hold)
                .await
                .unwrap();
            let outcome: Value =
                sqlx::query_scalar("SELECT conservation_step($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
                    .bind("fixture:take")
                    .bind(item)
                    .bind(order)
                    .bind(Option::<Uuid>::None)
                    .bind(Decimal::from(4))
                    .bind(f.product_id)
                    .bind(f.point)
                    .bind(f.buyer_org)
                    .bind(f.buyer)
                    .bind(f.product["code"].as_str().unwrap())
                    .fetch_one(&mut *hold)
                    .await
                    .unwrap();
            assert!(outcome["trade"].is_string());
        } else {
            sqlx::query("SELECT conservation_hold($1,$2,$3)")
                .bind("inventory")
                .bind(item)
                .bind(key)
                .execute(&mut *hold)
                .await
                .unwrap();
        }
        let barrier = Arc::new(Barrier::new(2));
        let waiter = barrier.clone();
        let router = f.orders.clone();
        let actor = f.supplier;
        let cancel = tokio::spawn(async move {
            waiter.wait().await;
            request(
                router,
                actor,
                "DELETE",
                &format!("/api/orderbook/{order}"),
                None,
                None,
            )
            .await
        });
        barrier.wait().await;
        let cancel_pid = wait_for_lock(
            &mut f.admin,
            holder_pid,
            None,
            if take_first {
                "advisory"
            } else {
                "transactionid"
            },
        )
        .await;
        if take_first {
            hold.commit().await.unwrap();
            println!("COMMIT_MILESTONE fixture:take before rust:order.cancel");
            let (status, body) = cancel.await.unwrap();
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        } else {
            let mut taker = PgConnection::connect(&f.admin_url).await.unwrap();
            let take_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut taker)
                .await
                .unwrap();
            let product = f.product_id;
            let point = f.point;
            let buyer_org = f.buyer_org;
            let buyer = f.buyer;
            let code = f.product["code"].as_str().unwrap().to_owned();
            let barrier = Arc::new(Barrier::new(2));
            let waiter = barrier.clone();
            let take = tokio::spawn(async move {
                let mut tx = taker.begin().await.unwrap();
                waiter.wait().await;
                let result: Value =
                    sqlx::query_scalar("SELECT conservation_race_take($1,$2,$3,$4,$5,$6,$7)")
                        .bind(item)
                        .bind(order)
                        .bind(product)
                        .bind(point)
                        .bind(buyer_org)
                        .bind(buyer)
                        .bind(code)
                        .fetch_one(&mut *tx)
                        .await
                        .unwrap();
                tx.commit().await.unwrap();
                result
            });
            barrier.wait().await;
            wait_for_lock(&mut f.admin, cancel_pid, Some(take_pid), "advisory").await;
            hold.commit().await.unwrap();
            let (status, body) = cancel.await.unwrap();
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
            let result = take.await.unwrap();
            assert_eq!(result["refused"], true);
            println!("COMMIT_MILESTONE rust:order.cancel before fixture:take refusal");
        }
        f.check(0, 0, Writer::Cancel, "take/cancel milestone schedule")
            .await;
        f.cancel_effects(order, 0).await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn parallel_cancel_has_one_winner() {
    for keyed in [false, true] {
        let mut f = Fixture::new().await;
        let (_, item) = f.create("10.00").await;
        let item = item["id"].as_str().unwrap().parse().unwrap();
        let order: Uuid = f
            .stock_step("fixture:publish", item, None, None, Decimal::ZERO)
            .await["order"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        f.stock_step("fixture:take", item, Some(order), None, Decimal::from(4))
            .await;
        let barrier = Arc::new(Barrier::new(6));
        let mut tasks = Vec::new();
        for _ in 0..6 {
            let barrier = barrier.clone();
            let router = f.orders.clone();
            let actor = f.supplier;
            let path = format!("/api/orderbook/{order}");
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                request(
                    router,
                    actor,
                    "DELETE",
                    &path,
                    None,
                    keyed.then_some("parallel-key"),
                )
                .await
            }));
        }
        let mut success = 0;
        for task in tasks {
            let (status, body) = task.await.unwrap();
            if status == StatusCode::NO_CONTENT {
                success += 1;
            } else {
                let code = body["detail"]["code"].as_str().unwrap_or("");
                assert!(
                    (status == StatusCode::BAD_REQUEST && code == "ORDER_TERMINAL")
                        || (status == StatusCode::CONFLICT && code == "MARKET_BUSY")
                        || (keyed
                            && status == StatusCode::SERVICE_UNAVAILABLE
                            && code == "IDEMPOTENCY_BUSY"),
                    "{status} {body}"
                );
            }
        }
        if keyed {
            assert!(success >= 1);
        } else {
            assert_eq!(success, 1);
        }
        f.check(0, 0, Writer::Cancel, "parallel").await;
        f.cancel_effects(order, i64::from(keyed)).await;
        if keyed {
            let before = f.snapshot().await;
            assert_eq!(
                request(
                    f.orders.clone(),
                    f.supplier,
                    "DELETE",
                    &format!("/api/orderbook/{order}"),
                    None,
                    Some("parallel-key")
                )
                .await
                .0,
                StatusCode::NO_CONTENT
            );
            assert_eq!(f.snapshot().await, before);
        }
    }
}
