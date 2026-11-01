#![cfg(feature = "contract-tests")]
//! The trusted watchlist producer against a migrated disposable database.
//!
//! Sources come from the real economic writer (cancel, new order, amendment,
//! expiry, completed match, decline). Shapes a well-behaved writer cannot
//! produce (an invalid envelope, an uncommitted row with a sequence value) and
//! bulk sizes use writer-shaped version 1 envelopes inserted through the fixture.
#[path = "support/market_database.rs"]
mod market_database;

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;
use verdaxis_read::market_contract::effects::{
    EffectProbe, MutationKind, run_effect_probe, run_expiry_effect_probe,
    run_new_order_effect_probe,
};
use verdaxis_read::market_contract::producer::{
    BatchOutcome, BootstrapOutcome, INSERT_CHUNK_ROWS, ProducerActivation, ProducerError,
    bootstrap_watermark, consume_batch, run_producer,
};

const RELEASE: &str = "0123456789abcdef0123456789abcdef01234567";
const ASSIGN_SQL: &str = include_str!("../sql/streams_assign.sql");

fn activation() -> ProducerActivation {
    ProducerActivation::from_release(
        Some("rust-market-v1"),
        Some("2026-10-01T00:00:00Z"),
        RELEASE,
    )
    .unwrap()
}

#[derive(Debug, Clone, PartialEq)]
struct Event {
    target: Uuid,
    class: String,
    key: String,
    payload: Value,
    producer: String,
    source: Uuid,
    seq: i64,
    attested: DateTime<Utc>,
    created: DateTime<Utc>,
}

struct Fixture {
    database: Arc<market_database::MarketDatabase>,
    pool: PgPool,
    admin: PgConnection,
    order: Uuid,
    opposite: Uuid,
    actor: Uuid,
    organization: Uuid,
    watchlist: Uuid,
    target: Uuid,
    product_code: String,
    point: Uuid,
    point_name: String,
}

impl Fixture {
    /// A seeded slice (REAL ASK 100 x 10, REAL BID 90 x 10) with one PIN on the
    /// ASK, and the watermark bootstrapped before any trusted source exists.
    async fn new() -> Self {
        Self::bootstrapped(true).await
    }

    async fn bootstrapped(seed: bool) -> Self {
        let mut f = Self::build(seed).await;
        let mut connection = f.pool.acquire().await.unwrap();
        assert_eq!(
            bootstrap_watermark(&mut connection).await.unwrap(),
            BootstrapOutcome::Created { watermark: 0 }
        );
        drop(connection);
        assert_eq!(f.watermark().await, Some(0));
        f
    }

    async fn unbootstrapped() -> Self {
        Self::build(true).await
    }

    /// `seed` false leaves the market empty for a test that seeds its own slice.
    async fn build(seed: bool) -> Self {
        let database = Arc::new(market_database::MarketDatabase::new("RUST_PRODUCER").await);
        let admin_url = url::Url::parse(&database.admin_url).unwrap();
        let runtime_url = url::Url::parse(&database.runtime_url).unwrap();
        assert_eq!(admin_url.host_str(), Some("127.0.0.1"));
        assert!(admin_url.path().ends_with("_test"));
        assert_eq!(
            (
                runtime_url.host_str(),
                runtime_url.port(),
                runtime_url.path()
            ),
            (admin_url.host_str(), admin_url.port(), admin_url.path())
        );
        assert_ne!(runtime_url.username(), admin_url.username());
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        assert_eq!(revision, "p89_20261022_trade_freeze");
        for fixture in [
            include_str!("fixtures/phase8_effects.sql"),
            include_str!("fixtures/phase8_producer.sql"),
        ] {
            sqlx::raw_sql(fixture).execute(&mut admin).await.unwrap();
        }
        let policy: Value =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let product = policy["products"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["qualification_profile"] == "scalar" && p["execution_mode"] == "ORDERBOOK")
            .unwrap();
        let product_id: Uuid = product["id"].as_str().unwrap().parse().unwrap();
        let point: Uuid = product["lanes"][0].as_str().unwrap().parse().unwrap();
        let product_code = product["code"].as_str().unwrap().to_owned();
        let (organization, actor, order, opposite, watchlist, target) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        if seed {
            sqlx::query("SELECT effect_fixture_seed($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
                .bind(organization)
                .bind(actor)
                .bind(order)
                .bind(opposite)
                .bind(watchlist)
                .bind(target)
                .bind(product_id)
                .bind(point)
                .bind(&product_code)
                .bind(
                    product["specification_standard"]
                        .as_str()
                        .unwrap_or("ISO 8217"),
                )
                .execute(&mut admin)
                .await
                .unwrap();
        }
        let point_name: String = sqlx::query_scalar("SELECT name FROM delivery_points WHERE id=$1")
            .bind(point)
            .fetch_one(&mut admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .connect(&database.runtime_url)
            .await
            .unwrap();
        Self {
            database,
            pool,
            admin,
            order,
            opposite,
            actor,
            organization,
            watchlist,
            target,
            product_code,
            point,
            point_name,
        }
    }

    fn probe(&self, kind: MutationKind, order: Uuid) -> EffectProbe {
        EffectProbe {
            order_id: order,
            actor_id: self.actor,
            organization_id: self.organization,
            kind,
            replay: false,
            commit: true,
        }
    }

    /// The real writer cancels the seeded ASK and commits its evidence.
    async fn cancel(&self) {
        run_effect_probe(
            &self.pool,
            self.probe(MutationKind::OrderCancelled, self.order),
        )
        .await
        .unwrap();
    }

    /// The real writer inserts an ASK at 95 on the seeded slice.
    async fn insert_order(&self) -> Uuid {
        let id = Uuid::new_v4();
        run_new_order_effect_probe(
            &self.pool,
            self.probe(MutationKind::InventoryPublished, self.order),
            id,
        )
        .await
        .unwrap();
        id
    }

    /// The sequencer's own statement assigns committed rows in order.
    async fn sequence(&mut self) {
        let mut tx = self.admin.begin().await.unwrap();
        sqlx::query(ASSIGN_SQL)
            .bind(1000_i64)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn slice_target(&mut self) -> Uuid {
        let target = Uuid::new_v4();
        sqlx::query("SELECT effect_fixture_slice_target($1,$2)")
            .bind(self.order)
            .bind(target)
            .execute(&mut self.admin)
            .await
            .unwrap();
        target
    }

    async fn pin_target(&mut self, order: Uuid) -> Uuid {
        let target = Uuid::new_v4();
        sqlx::query("SELECT producer_fixture_target($1,$2,'PIN',$3,$4,$5,'SPOT')")
            .bind(target)
            .bind(self.actor)
            .bind(order)
            .bind(&self.product_code)
            .bind(self.point)
            .execute(&mut self.admin)
            .await
            .unwrap();
        target
    }

    async fn row(&mut self, evidence: Option<&Value>) -> Uuid {
        sqlx::query_scalar("SELECT producer_fixture_row($1,$2)")
            .bind(evidence)
            .bind(self.organization)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn fixture(&mut self, statement: &str, value: i64) {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
            .bind(value)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    async fn consume_once(&self) -> Result<BatchOutcome, ProducerError> {
        let mut connection = self.pool.acquire().await.unwrap();
        consume_batch(&mut connection, &activation()).await
    }

    /// Consume until drained; returns the outcomes in order.
    async fn consume(&self) -> Vec<BatchOutcome> {
        let mut outcomes = Vec::new();
        for _ in 0..50 {
            let outcome = self.consume_once().await.unwrap();
            let drained = outcome.drained;
            outcomes.push(outcome);
            if drained {
                return outcomes;
            }
        }
        panic!("the backlog did not drain");
    }

    async fn watermark(&mut self) -> Option<i64> {
        sqlx::query_scalar("SELECT consumed_through_seq FROM watchlist_producer_state")
            .fetch_optional(&mut self.admin)
            .await
            .unwrap()
    }

    async fn high_water(&mut self) -> i64 {
        sqlx::query_scalar("SELECT COALESCE(max(stream_seq),0) FROM market_event_outbox")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn events(&mut self) -> Vec<Event> {
        sqlx::query("SELECT watchlist_target_id,event_type,derived_event_key,event_payload::jsonb AS payload,producer,source_event_id,source_stream_seq,scope_attested_at,created_at FROM watchlist_events ORDER BY watchlist_target_id,derived_event_key")
            .fetch_all(&mut self.admin)
            .await
            .unwrap()
            .iter()
            .map(|row| Event {
                target: row.get("watchlist_target_id"),
                class: row.get("event_type"),
                key: row.get("derived_event_key"),
                payload: row.get("payload"),
                producer: row.get("producer"),
                source: row.get("source_event_id"),
                seq: row.get("source_stream_seq"),
                attested: row.get("scope_attested_at"),
                created: row.get("created_at"),
            })
            .collect()
    }

    /// The evidence of the first outbox row of the latest mutation.
    async fn evidence(&mut self) -> Value {
        sqlx::query_scalar("SELECT producer_evidence FROM market_event_outbox WHERE producer_evidence IS NOT NULL ORDER BY created_at DESC, stream_seq NULLS LAST LIMIT 1")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    fn slice_text(&self) -> String {
        format!("{}:{}:SPOT", self.product_code, self.point)
    }

    /// A writer-shaped envelope for one change of the seeded ASK and its PIN.
    fn amend_envelope(&self, before: (&str, &str, &str), after: (&str, &str, &str)) -> Value {
        let state = |(status, price, remaining): (&str, &str, &str)| {
            json!({"order_id":self.order,"version":2,"status":status,"side":"ASK",
                "market_product":self.product_code,"delivery_point_id":self.point,
                "availability_window":"SPOT","price_per_mt_usd":price,"quantity_mt":"10",
                "remaining_quantity_mt":remaining,"provenance":"REAL","public_admitted":true,
                "benchmark_qualified":true,"public_projection":{"delivery_point_name":self.point_name},
                "observed_at":"2026-10-30T10:00:00Z"})
        };
        json!({"version":1,"mutation_id":Uuid::new_v4(),"captured_at":"2026-10-30T10:00:00.250Z",
            "orders":[{"order_id":self.order,"transition_index":0,"before":state(before),"after":state(after)}],
            "aggregates_before":[],"aggregates_after":[],
            "targets":[{"target_id":self.target,"watchlist_id":self.watchlist,"observer_id":self.actor,
                "target_type":"PIN","order_id":self.order,"market_product":self.product_code,
                "delivery_point_id":self.point,"availability_window":"SPOT"}]})
    }
}

fn keys(events: &[Event]) -> BTreeSet<(Uuid, String, String)> {
    events
        .iter()
        .map(|e| (e.target, e.class.clone(), e.key.clone()))
        .collect()
}

fn instant(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

/// The exact set a committed cancel of the only ASK derives for a PIN on it
/// and a SLICE on its slice.
fn cancel_keys(f: &Fixture, pin: Uuid, slice: Uuid) -> BTreeSet<(Uuid, String, String)> {
    let slice_text = f.slice_text();
    [
        (
            pin,
            "PIN_WITHDRAWN",
            format!("PIN_WITHDRAWN|{}|ASK|0", f.order),
        ),
        (
            slice,
            "SLICE_BEST_PRICE_MOVED",
            format!("SLICE_BEST_PRICE_MOVED|{slice_text}|ASK|1"),
        ),
        (
            slice,
            "SLICE_WENT_QUIET",
            format!("SLICE_WENT_QUIET|{slice_text}|ASK|1"),
        ),
    ]
    .into_iter()
    .map(|(target, class, key)| (target, class.to_owned(), key))
    .collect()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_restart_replays_without_duplicates() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    let evidence = f.evidence().await;
    let source_seq = f.high_water().await;
    let first = f.consume().await;
    assert_eq!(first.iter().map(|o| o.inserted).sum::<u64>(), 3);
    let events = f.events().await;
    assert_eq!(keys(&events), cancel_keys(&f, f.target, slice));
    // PostgreSQL keeps the writer instant at microsecond precision.
    let captured =
        chrono::SubsecRound::trunc_subsecs(instant(evidence["captured_at"].as_str().unwrap()), 6);
    let mutation: Uuid = evidence["mutation_id"].as_str().unwrap().parse().unwrap();
    for event in &events {
        assert_eq!(event.producer, "rust-market-v1");
        assert_eq!(event.source, mutation);
        assert_eq!(event.seq, source_seq);
        assert_eq!(event.attested, captured);
        assert_eq!(event.created, captured);
    }
    assert_eq!(f.watermark().await, Some(source_seq));
    // A restart from an older checkpoint re-reads the source on new connections.
    f.fixture("SELECT producer_fixture_watermark($1)", 0).await;
    f.pool.close().await;
    f.pool = PgPool::connect(&f.database.runtime_url).await.unwrap();
    let replay = f.consume().await;
    assert_eq!(replay.iter().map(|o| o.inserted).sum::<u64>(), 0);
    assert_eq!(f.events().await, events);
    assert_eq!(f.watermark().await, Some(source_seq));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_writes_one_row_per_target_and_derived_event() {
    let mut f = Fixture::new().await;
    let second_pin = f.pin_target(f.order).await;
    let slice = f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    f.consume().await;
    let events = f.events().await;
    let mut expected = cancel_keys(&f, f.target, slice);
    expected.insert((
        second_pin,
        "PIN_WITHDRAWN".into(),
        format!("PIN_WITHDRAWN|{}|ASK|0", f.order),
    ));
    assert_eq!(keys(&events), expected);
    assert_eq!(events.len(), 4);
    let pin = events.iter().find(|e| e.target == f.target).unwrap();
    assert_eq!(pin.payload["status"], "CANCELLED");
    assert_eq!(pin.payload["order_id"], f.order.to_string());
    assert_eq!(pin.payload["source_kind"], "LIVE_ORDER");
    assert_eq!(pin.payload["demo_status"], "REAL_ONLY");
    assert_eq!(pin.payload["scope"], "DELIVERY_POINT");
    assert_eq!(pin.payload["delivery_point_name"], f.point_name.as_str());
    assert_eq!(pin.payload["market_product_code"], f.product_code.as_str());
    assert!(pin.payload["observed_at"].is_string());
    // The opposite BID did not change, so its side derives nothing.
    assert!(events.iter().all(|e| !e.key.contains("|BID|")));
    let _ = f.opposite;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn last_quote_removal_keeps_best_price_and_quiet_events() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    f.consume().await;
    let events = f.events().await;
    let moved = events
        .iter()
        .find(|e| e.target == slice && e.class == "SLICE_BEST_PRICE_MOVED")
        .unwrap();
    assert_eq!(moved.payload["old_price_per_mt_usd"].to_string(), "100");
    assert_eq!(moved.payload["new_price_per_mt_usd"], Value::Null);
    assert_eq!(moved.payload["side"], "ASK");
    // No quote is left: the aggregate provenance reports no data.
    assert_eq!(moved.payload["source_kind"], "NO_DATA");
    assert_eq!(moved.payload["demo_status"], "NOT_APPLICABLE");
    assert_eq!(moved.payload["real_count"], 0);
    assert_eq!(moved.payload["observed_at"], Value::Null);
    let quiet = events
        .iter()
        .find(|e| e.target == slice && e.class == "SLICE_WENT_QUIET")
        .unwrap();
    assert_eq!(quiet.payload["order_id"], f.order.to_string());
    assert_eq!(quiet.payload["source_kind"], "LIVE_ORDER");
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn new_best_quote_keeps_new_order_and_best_price_events() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    let inserted = f.insert_order().await;
    f.sequence().await;
    f.consume().await;
    let events = f.events().await;
    let slice_text = f.slice_text();
    assert_eq!(
        keys(&events),
        BTreeSet::from([
            (
                slice,
                "SLICE_NEW_ORDER".to_owned(),
                format!("SLICE_NEW_ORDER|{inserted}|ASK|0")
            ),
            (
                slice,
                "SLICE_BEST_PRICE_MOVED".to_owned(),
                format!("SLICE_BEST_PRICE_MOVED|{slice_text}|ASK|1")
            ),
        ])
    );
    let new_order = events
        .iter()
        .find(|e| e.class == "SLICE_NEW_ORDER")
        .unwrap();
    assert_eq!(new_order.payload["order_id"], inserted.to_string());
    assert_eq!(new_order.payload["price_per_mt_usd"].to_string(), "95");
    assert_eq!(new_order.payload["remaining_quantity_mt"].to_string(), "10");
    let moved = events
        .iter()
        .find(|e| e.class == "SLICE_BEST_PRICE_MOVED")
        .unwrap();
    // The new quote attributes the move to itself.
    assert_eq!(moved.payload["order_id"], inserted.to_string());
    assert_eq!(moved.payload["old_price_per_mt_usd"].to_string(), "100");
    assert_eq!(moved.payload["new_price_per_mt_usd"].to_string(), "95");
    // A later quote above the new best moves nothing.
    f.insert_order().await;
    f.sequence().await;
    f.consume().await;
    let later = f.events().await;
    assert_eq!(later.len(), 3);
    assert_eq!(
        later
            .iter()
            .filter(|e| e.class == "SLICE_BEST_PRICE_MOVED")
            .count(),
        1
    );
}

/// A real amendment: the BID of the matching fixture (110 x 200, no ASK)
/// with a PIN on it and a SLICE on its slice, amended through the amendment
/// writer's own router.
struct AmendSource {
    f: Fixture,
    router: axum::Router,
    bid: Uuid,
    pin: Uuid,
    slice: Uuid,
}

const AMEND_KEY: &str = "synthetic-amend-signing-key-at-least-32-characters";

impl AmendSource {
    async fn new() -> Self {
        use verdaxis_read::routes::market_write::amend::{AmendSettings, contract_router};
        let mut f = Fixture::bootstrapped(false).await;
        for fixture in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_amend.sql"),
        ] {
            sqlx::raw_sql(fixture).execute(&mut f.admin).await.unwrap();
        }
        sqlx::query("SELECT amend_fixture_minimum(1)")
            .execute(&mut f.admin)
            .await
            .unwrap();
        let bid = Uuid::from_u128(1);
        sqlx::query("SELECT matching_fixture_seed($1,0,200)")
            .bind(bid)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let (code, point) = matching_slice(&mut f.admin, bid).await;
        let buyer = Uuid::from_u128(0x1001);
        let pin = Uuid::new_v4();
        sqlx::query("SELECT producer_fixture_target($1,$2,'PIN',$3,$4,$5,'SPOT')")
            .bind(pin)
            .bind(buyer)
            .bind(bid)
            .bind(&code)
            .bind(point)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let slice = Uuid::new_v4();
        sqlx::query("SELECT producer_fixture_target($1,$2,'SLICE',NULL,$3,$4,'SPOT')")
            .bind(slice)
            .bind(buyer)
            .bind(&code)
            .bind(point)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let router = contract_router(f.pool.clone(), AMEND_KEY, AmendSettings::default());
        Self {
            f,
            router,
            bid,
            pin,
            slice,
        }
    }

    /// Amend the BID to 120 x 150 as its owner and commit.
    async fn amend(&self) {
        use axum::{body::Body, http::Request};
        use jsonwebtoken::{EncodingKey, Header, encode};
        use tower::ServiceExt;
        let now = Utc::now().timestamp();
        let token = encode(
            &Header::default(),
            &json!({"sub":Uuid::from_u128(0x1001),"type":"access","iat":now,"exp":now+600,"iss":"amend-tests","aud":"amend-tests"}),
            &EncodingKey::from_secret(AMEND_KEY.as_bytes()),
        )
        .unwrap();
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/orderbook/{}", self.bid))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(
                        json!({"price_per_mt_usd":"120","quantity_mt":"150"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}

/// The product code and point of a matching-fixture order.
async fn matching_slice(admin: &mut PgConnection, order: Uuid) -> (String, Uuid) {
    let row = sqlx::query("SELECT product_id,delivery_point_id FROM orderbook_orders WHERE id=$1")
        .bind(order)
        .fetch_one(&mut *admin)
        .await
        .unwrap();
    let product_id: Uuid = row.get("product_id");
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let code = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == product_id.to_string())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    (code, row.get("delivery_point_id"))
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn one_source_target_keeps_price_and_quantity_events() {
    let mut a = AmendSource::new().await;
    a.amend().await;
    a.f.sequence().await;
    a.f.consume().await;
    let events = a.f.events().await;
    let (code, point) = matching_slice(&mut a.f.admin, a.bid).await;
    assert_eq!(
        keys(&events),
        BTreeSet::from([
            (
                a.pin,
                "PIN_PRICE_CHANGED".to_owned(),
                format!("PIN_PRICE_CHANGED|{}|BID|0", a.bid)
            ),
            (
                a.pin,
                "PIN_QUANTITY_CHANGED".to_owned(),
                format!("PIN_QUANTITY_CHANGED|{}|BID|0", a.bid)
            ),
            (
                a.slice,
                "SLICE_BEST_PRICE_MOVED".to_owned(),
                format!("SLICE_BEST_PRICE_MOVED|{code}:{point}:SPOT|BID|0")
            ),
        ])
    );
    let class = |name: &str| events.iter().find(|e| e.class == name).unwrap();
    let price = class("PIN_PRICE_CHANGED");
    assert_eq!(price.payload["old_price_per_mt_usd"].to_string(), "110");
    assert_eq!(price.payload["new_price_per_mt_usd"].to_string(), "120");
    let quantity = class("PIN_QUANTITY_CHANGED");
    assert_eq!(
        quantity.payload["old_remaining_quantity_mt"].to_string(),
        "200"
    );
    assert_eq!(
        quantity.payload["new_remaining_quantity_mt"].to_string(),
        "150"
    );
    assert_eq!(quantity.payload["status"], "OPEN");
    let moved = class("SLICE_BEST_PRICE_MOVED");
    assert_eq!(moved.payload["old_price_per_mt_usd"].to_string(), "110");
    assert_eq!(moved.payload["new_price_per_mt_usd"].to_string(), "120");
    assert_eq!(moved.payload["source_kind"], "LIVE_ORDER");
    assert_eq!(moved.payload["real_count"], 1);
    // The amended row's change instant is the event's observed instant.
    let changed: DateTime<Utc> = sqlx::query_scalar(
        "SELECT COALESCE(updated_at,created_at) FROM orderbook_orders WHERE id=$1",
    )
    .bind(a.bid)
    .fetch_one(&mut a.f.admin)
    .await
    .unwrap();
    assert_eq!(
        instant(price.payload["observed_at"].as_str().unwrap()),
        changed
    );
    // A replay of the same source adds nothing.
    a.f.fixture("SELECT producer_fixture_watermark($1)", 0)
        .await;
    a.f.consume().await;
    assert_eq!(a.f.events().await, events);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn delayed_source_ignores_later_python_target_snapshot() {
    let mut a = AmendSource::new().await;
    a.amend().await;
    a.f.sequence().await;
    // The writer moved the display in its own transaction.
    assert_eq!(
        pin_display(&mut a.f.admin, a.pin).await,
        ("120".to_owned(), "150".to_owned())
    );
    // Another writer moves the display again before the delayed consumption.
    sqlx::query("SELECT producer_fixture_display($1,130,7)")
        .bind(a.pin)
        .execute(&mut a.f.admin)
        .await
        .unwrap();
    a.f.consume().await;
    let events = a.f.events().await;
    let class = |name: &str| events.iter().find(|e| e.class == name).unwrap();
    let price = class("PIN_PRICE_CHANGED");
    assert_eq!(price.payload["old_price_per_mt_usd"].to_string(), "110");
    assert_eq!(price.payload["new_price_per_mt_usd"].to_string(), "120");
    let quantity = class("PIN_QUANTITY_CHANGED");
    assert_eq!(
        quantity.payload["old_remaining_quantity_mt"].to_string(),
        "200"
    );
    assert_eq!(
        quantity.payload["new_remaining_quantity_mt"].to_string(),
        "150"
    );
    assert_eq!(
        pin_display(&mut a.f.admin, a.pin).await,
        ("130".to_owned(), "7".to_owned())
    );
}

async fn pin_display(admin: &mut PgConnection, pin: Uuid) -> (String, String) {
    sqlx::query_as("SELECT snapshot_price_per_mt_usd::numeric::text,snapshot_remaining_quantity_mt::numeric::text FROM watchlist_targets WHERE id=$1")
        .bind(pin)
        .fetch_one(admin)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn target_created_after_source_capture_gets_no_historical_event() {
    let mut f = Fixture::new().await;
    let before_capture = f.pin_target(f.order).await;
    f.cancel().await;
    let after_capture = f.pin_target(f.order).await;
    f.sequence().await;
    f.consume().await;
    let targets: BTreeSet<Uuid> = f.events().await.iter().map(|e| e.target).collect();
    assert_eq!(targets, BTreeSet::from([f.target, before_capture]));
    assert!(!targets.contains(&after_capture));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn deleted_or_recreated_target_gets_no_history() {
    let mut f = Fixture::new().await;
    f.cancel().await;
    sqlx::query("SELECT producer_fixture_drop_target($1)")
        .bind(f.target)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let recreated = f.pin_target(f.order).await;
    f.sequence().await;
    let outcomes = f.consume().await;
    assert_eq!(outcomes.iter().map(|o| o.inserted).sum::<u64>(), 0);
    assert!(f.events().await.is_empty());
    let high_water = f.high_water().await;
    assert_eq!(f.watermark().await, Some(high_water));
    let _ = recreated;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn restart_preserves_exact_historical_payloads_and_target_set() {
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.pin_target(f.order).await;
    f.cancel().await;
    f.sequence().await;
    f.consume().await;
    let first = f.events().await;
    assert_eq!(first.len(), 4);
    for _ in 0..2 {
        f.fixture("SELECT producer_fixture_watermark($1)", 0).await;
        f.consume().await;
        assert_eq!(f.events().await, first);
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_skips_a_sequence_hole_and_advances_the_watermark() {
    let mut f = Fixture::new().await;
    f.row(None).await;
    f.sequence().await;
    let hole: i64 = sqlx::query_scalar("SELECT producer_fixture_burn()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    f.cancel().await;
    f.sequence().await;
    let high_water = f.high_water().await;
    assert!(high_water > hole);
    let present: i64 =
        sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE stream_seq=$1")
            .bind(hole)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(present, 0);
    f.consume().await;
    assert_eq!(f.watermark().await, Some(high_water));
    let events = f.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, high_water);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_ignores_legacy_rows_without_a_mutation_identity() {
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.row(None).await;
    f.row(None).await;
    f.sequence().await;
    let outcomes = f.consume().await;
    assert_eq!(outcomes.iter().map(|o| o.sources).sum::<usize>(), 2);
    assert!(f.events().await.is_empty());
    let high_water = f.high_water().await;
    assert_eq!(f.watermark().await, Some(high_water));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_never_touches_another_writers_outbox_row() {
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.row(None).await;
    f.cancel().await;
    f.sequence().await;
    let before: Value = sqlx::query_scalar("SELECT producer_fixture_untouched()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    f.consume().await;
    assert_eq!(f.events().await.len(), 3);
    let after: Value = sqlx::query_scalar("SELECT producer_fixture_untouched()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(after, before);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn uncommitted_source_is_invisible_and_rollback_leaves_no_event() {
    let mut f = Fixture::new().await;
    let mut probe = f.probe(MutationKind::OrderCancelled, f.order);
    probe.commit = false;
    run_effect_probe(&f.pool, probe).await.unwrap();
    // An open writer transaction holds a sequenced trusted row.
    let mut writer = PgConnection::connect(&f.database.admin_url).await.unwrap();
    let mut open = writer.begin().await.unwrap();
    let envelope = f.amend_envelope(("OPEN", "100", "10"), ("OPEN", "110", "8"));
    let id: Uuid = sqlx::query_scalar("SELECT producer_fixture_row($1,$2)")
        .bind(&envelope)
        .bind(f.organization)
        .fetch_one(&mut *open)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE market_event_outbox SET stream_seq=nextval('market_event_stream_seq') WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *open)
    .await
    .unwrap();
    f.sequence().await;
    let outcome = f.consume_once().await.unwrap();
    assert_eq!((outcome.sources, outcome.inserted), (0, 0));
    open.rollback().await.unwrap();
    f.consume().await;
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_consumers_serialize_on_the_watermark() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    // An independent session holds the watermark row while both consumers start.
    let mut holder = PgConnection::connect(&f.database.admin_url).await.unwrap();
    let mut held = holder.begin().await.unwrap();
    sqlx::query(
        "SELECT consumed_through_seq FROM watchlist_producer_state WHERE singleton FOR UPDATE",
    )
    .execute(&mut *held)
    .await
    .unwrap();
    let start = |url: String| {
        tokio::spawn(async move {
            let mut connection = PgConnection::connect(&url).await.unwrap();
            consume_batch(&mut connection, &activation()).await
        })
    };
    let first = start(f.database.runtime_url.clone());
    let second = start(f.database.runtime_url.clone());
    for _ in 0..30 {
        let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE '%watchlist_producer_state%' AND pid<>pg_backend_pid()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        if waiting == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    held.commit().await.unwrap();
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    let mut productive = 0;
    for outcome in &outcomes {
        match outcome {
            Ok(outcome) if outcome.sources > 0 => productive += 1,
            Ok(outcome) => assert_eq!(outcome.inserted, 0),
            Err(error) => assert_eq!(*error, ProducerError::Busy),
        }
    }
    assert!(productive <= 1);
    f.consume().await;
    assert_eq!(keys(&f.events().await), cancel_keys(&f, f.target, slice));
    let high_water = f.high_water().await;
    assert_eq!(f.watermark().await, Some(high_water));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn failure_before_watermark_commit_rolls_back_events() {
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    sqlx::query("SELECT producer_fixture_fail_watermark(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.consume_once().await, Err(ProducerError::Database));
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
    sqlx::query("SELECT producer_fixture_fail_watermark(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.consume().await;
    assert_eq!(f.events().await.len(), 3);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn invalid_trusted_evidence_stops_at_its_source() {
    let mut f = Fixture::new().await;
    f.row(None).await;
    f.sequence().await;
    let covered = f.high_water().await;
    let malformed = json!({"version":1,"mutation_id":Uuid::new_v4(),"orders":"not a list"});
    f.row(Some(&malformed)).await;
    f.sequence().await;
    let invalid = f.high_water().await;
    let valid = f.amend_envelope(("OPEN", "100", "10"), ("OPEN", "110", "8"));
    f.row(Some(&valid)).await;
    f.sequence().await;
    let first = f.consume_once().await.unwrap();
    assert_eq!(first.watermark, covered);
    assert_eq!(
        f.consume_once().await,
        Err(ProducerError::InvalidEvidence {
            stream_seq: invalid,
            reason: "evidence_malformed"
        })
    );
    assert_eq!(f.watermark().await, Some(covered));
    assert!(f.events().await.is_empty());
    let unsupported = json!({"version":2});
    let mut g = Fixture::new().await;
    g.row(Some(&unsupported)).await;
    g.sequence().await;
    assert!(matches!(
        g.consume_once().await,
        Err(ProducerError::InvalidEvidence {
            reason: "evidence_version_unsupported",
            ..
        })
    ));
    assert_eq!(g.watermark().await, Some(0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn coverage_loss_stops_without_state_change() {
    let mut f = Fixture::new().await;
    f.cancel().await;
    f.sequence().await;
    let high_water = f.high_water().await;
    f.fixture("SELECT producer_fixture_floor($1)", high_water)
        .await;
    assert_eq!(f.consume_once().await, Err(ProducerError::CoverageLost));
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn retention_fence_not_blocked_stops_consumption() {
    let mut f = Fixture::new().await;
    f.cancel().await;
    f.sequence().await;
    sqlx::query("SELECT producer_fixture_fence('rust_active')")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.consume_once().await,
        Err(ProducerError::RetentionNotBlocked)
    );
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
    sqlx::query("SELECT producer_fixture_fence('blocked')")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.consume().await;
    assert_eq!(f.events().await.len(), 1);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn pause_retains_backlog_and_reactivation_drains_it_in_order() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    // Paused: the release names the legacy producer, so no activation exists
    // and nothing can run the consumer transaction.
    assert!(
        ProducerActivation::from_release(
            Some("legacy-python"),
            Some("2026-10-01T00:00:00Z"),
            RELEASE
        )
        .is_err()
    );
    let mut inserted = Vec::new();
    for _ in 0..3 {
        inserted.push(f.insert_order().await);
        f.sequence().await;
    }
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM market_event_outbox WHERE producer_evidence IS NOT NULL",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(retained, 3);
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
    // Reactivation keeps the original instant and drains in sequence order.
    f.consume().await;
    let events = f.events().await;
    let new_orders: Vec<&Event> = events
        .iter()
        .filter(|e| e.target == slice && e.class == "SLICE_NEW_ORDER")
        .collect();
    assert_eq!(new_orders.len(), 3);
    let mut by_seq: Vec<(i64, String)> = new_orders
        .iter()
        .map(|e| (e.seq, e.payload["order_id"].as_str().unwrap().to_owned()))
        .collect();
    by_seq.sort();
    assert_eq!(
        by_seq.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
        inserted.iter().map(Uuid::to_string).collect::<Vec<_>>()
    );
    // Only the first quote moved the best price.
    assert_eq!(
        events
            .iter()
            .filter(|e| e.class == "SLICE_BEST_PRICE_MOVED")
            .count(),
        1
    );
    let high_water = f.high_water().await;
    assert_eq!(f.watermark().await, Some(high_water));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn bootstrap_sets_the_barrier_only_without_trusted_evidence() {
    let mut f = Fixture::unbootstrapped().await;
    f.row(None).await;
    f.row(None).await;
    f.sequence().await;
    let high_water = f.high_water().await;
    assert!(high_water >= 2);
    assert_eq!(f.consume_once().await, Err(ProducerError::WatermarkMissing));
    let mut connection = f.pool.acquire().await.unwrap();
    assert_eq!(
        bootstrap_watermark(&mut connection).await.unwrap(),
        BootstrapOutcome::Created {
            watermark: high_water
        }
    );
    assert_eq!(
        bootstrap_watermark(&mut connection).await.unwrap(),
        BootstrapOutcome::AlreadyPresent {
            watermark: high_water
        }
    );
    drop(connection);
    assert_eq!(f.watermark().await, Some(high_water));
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn bootstrap_refuses_to_pass_trusted_evidence() {
    let mut f = Fixture::unbootstrapped().await;
    f.cancel().await;
    f.sequence().await;
    let mut connection = f.pool.acquire().await.unwrap();
    assert_eq!(
        bootstrap_watermark(&mut connection).await,
        Err(ProducerError::TrustedEvidenceExists)
    );
    drop(connection);
    assert_eq!(f.watermark().await, None);
    let mut g = Fixture::unbootstrapped().await;
    sqlx::query("SELECT producer_fixture_fence('rust_active')")
        .execute(&mut g.admin)
        .await
        .unwrap();
    let mut connection = g.pool.acquire().await.unwrap();
    assert_eq!(
        bootstrap_watermark(&mut connection).await,
        Err(ProducerError::RetentionNotBlocked)
    );
    drop(connection);
    assert_eq!(g.watermark().await, None);
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn transport_projection_never_reads_evidence_after_consumption() {
    use verdaxis_read::streams::store::{PageLimits, dispatch_page, replay_page};
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    f.consume().await;
    assert_eq!(f.events().await.len(), 3);
    let high_water = f.high_water().await;
    let dispatch = dispatch_page(&f.pool, 0).await.unwrap();
    let replay = replay_page(
        &f.pool,
        f.organization,
        0,
        high_water,
        PageLimits::default(),
    )
    .await
    .unwrap();
    assert!(!dispatch.events.is_empty());
    assert!(!replay.events.is_empty());
    let target = f.target.to_string();
    let watchlist = f.watchlist.to_string();
    for event in dispatch.events.iter().chain(&replay.events) {
        let text = event.payload().to_string();
        for internal in [
            "mutation_id",
            "aggregates_before",
            "aggregates_after",
            "targets",
            "producer_evidence",
            "transition_index",
            target.as_str(),
            watchlist.as_str(),
        ] {
            assert!(!text.contains(internal), "{internal} reached the transport");
        }
    }
}

/// A real completed match against three resting ASKs with a PIN each and one
/// SLICE: the mutation appends several outbox rows with one envelope.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn multi_fill_source_replay_keeps_every_distinct_derived_event_once() {
    use verdaxis_read::market_contract::matching::{CompletionOptions, run_completed_match};
    let mut f = Fixture::bootstrapped(false).await;
    for fixture in [
        include_str!("fixtures/market_write_matching.sql"),
        include_str!("fixtures/market_write_match_completion.sql"),
    ] {
        sqlx::raw_sql(fixture).execute(&mut f.admin).await.unwrap();
    }
    let aggressor = Uuid::from_u128(1);
    sqlx::query("SELECT matching_fixture_seed($1,3,3)")
        .bind(aggressor)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let resting: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM orderbook_orders WHERE organization_id='00000000-0000-0000-0000-000000002000' ORDER BY id",
    )
    .fetch_all(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(resting.len(), 3);
    let row = sqlx::query("SELECT product_id,delivery_point_id FROM orderbook_orders WHERE id=$1")
        .bind(aggressor)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let product_id: Uuid = row.get("product_id");
    let point: Uuid = row.get("delivery_point_id");
    let policy: Value =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let code = policy["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == product_id.to_string())
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let observer = Uuid::from_u128(0x1001);
    let mut pins = Vec::new();
    for order in &resting {
        let target = Uuid::new_v4();
        sqlx::query("SELECT producer_fixture_target($1,$2,'PIN',$3,$4,$5,'SPOT')")
            .bind(target)
            .bind(observer)
            .bind(order)
            .bind(&code)
            .bind(point)
            .execute(&mut f.admin)
            .await
            .unwrap();
        pins.push(target);
    }
    let slice = Uuid::new_v4();
    sqlx::query("SELECT producer_fixture_target($1,$2,'SLICE',NULL,$3,$4,'SPOT')")
        .bind(slice)
        .bind(observer)
        .bind(&code)
        .bind(point)
        .execute(&mut f.admin)
        .await
        .unwrap();
    run_completed_match(
        &f.pool,
        aggressor,
        "2026-10-25T12:00:00Z".parse().unwrap(),
        CompletionOptions::default(),
    )
    .await
    .unwrap();
    f.sequence().await;
    let rows: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT stream_seq,producer_evidence FROM market_event_outbox WHERE producer_evidence IS NOT NULL ORDER BY stream_seq",
    )
    .fetch_all(&mut f.admin)
    .await
    .unwrap();
    assert!(rows.len() >= 2, "one match appends several rows");
    let mutation = rows[0].1["mutation_id"].clone();
    assert!(
        rows.iter()
            .all(|(_, evidence)| evidence["mutation_id"] == mutation)
    );
    f.consume().await;
    let events = f.events().await;
    for (pin, order) in pins.iter().zip(&resting) {
        let mine: Vec<&Event> = events.iter().filter(|e| e.target == *pin).collect();
        assert_eq!(mine.len(), 1, "one PIN class per filled order");
        assert_eq!(mine[0].class, "PIN_FILLED");
        assert_eq!(mine[0].payload["order_id"], order.to_string());
        assert_eq!(
            mine[0].payload["new_remaining_quantity_mt"].to_string(),
            "0"
        );
    }
    // Each slice transition the envelope records appears once, on the first row.
    let evidence = &rows[0].1;
    let selected = |aggregate: &Value| -> Value {
        if aggregate["real"]["order_count"].as_u64() > Some(0) {
            aggregate["real"]["best_price_per_mt_usd"].clone()
        } else if aggregate["demo"]["order_count"].as_u64() > Some(0) {
            aggregate["demo"]["best_price_per_mt_usd"].clone()
        } else {
            Value::Null
        }
    };
    let moved_sides: BTreeSet<String> = evidence["aggregates_before"]
        .as_array()
        .unwrap()
        .iter()
        .zip(evidence["aggregates_after"].as_array().unwrap())
        .filter(|(before, after)| selected(before) != selected(after))
        .map(|(_, after)| after["side"].as_str().unwrap().to_owned())
        .collect();
    assert!(moved_sides.contains("ASK"));
    let slice_moves: BTreeSet<String> = events
        .iter()
        .filter(|e| e.target == slice && e.class == "SLICE_BEST_PRICE_MOVED")
        .map(|e| e.payload["side"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(slice_moves, moved_sides);
    let distinct: BTreeSet<_> = keys(&events);
    assert_eq!(distinct.len(), events.len());
    assert!(events.iter().all(|e| e.seq == rows[0].0));
    assert!(
        events
            .iter()
            .all(|e| Value::String(e.source.to_string()) == mutation)
    );
    // A replay of every row of the mutation adds nothing.
    f.fixture("SELECT producer_fixture_watermark($1)", 0).await;
    f.consume().await;
    assert_eq!(f.events().await, events);
}

/// A real decline restores the remainder of a partially taken ASK; the
/// consumer derives the restored quantity for its PIN exactly once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn decline_restoration_source_emits_the_restored_quantity_once() {
    use axum::{body::Body, http::Request};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use tower::ServiceExt;
    use verdaxis_read::routes::market_write::trade_lifecycle::contract_router;
    const KEY: &str = "synthetic-lifecycle-signing-key-at-least-32";
    let database = Arc::new(market_database::MarketDatabase::new("RUST_PRODUCER").await);
    let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
    for fixture in [
        include_str!("fixtures/phase9_trades.sql"),
        include_str!("fixtures/phase9_confirm.sql"),
    ] {
        sqlx::raw_sql(fixture).execute(&mut admin).await.unwrap();
    }
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
        })
        .unwrap();
    sqlx::query("SELECT trades_fixture_seed($1,$2)")
        .bind(product["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .bind(
            product["lanes"][0]
                .as_str()
                .unwrap()
                .parse::<Uuid>()
                .unwrap(),
        )
        .execute(&mut admin)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('confirm.policy',$1,false)")
        .bind(policy["products"].to_string())
        .execute(&mut admin)
        .await
        .unwrap();
    let trade: Uuid = sqlx::query_scalar("SELECT confirm_fixture_pending('BUYER',false)")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    sqlx::query("SELECT confirm_fixture_mirrors()")
        .execute(&mut admin)
        .await
        .unwrap();
    let pin: Uuid = sqlx::query_scalar("SELECT id FROM watchlist_targets WHERE target_type='PIN'")
        .fetch_one(&mut admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database.runtime_url)
        .await
        .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    bootstrap_watermark(&mut connection).await.unwrap();
    drop(connection);
    let before_decline: DateTime<Utc> = sqlx::query_scalar(
        "SELECT COALESCE(updated_at,created_at) FROM orderbook_orders WHERE id=$1",
    )
    .bind(Uuid::from_u128(1))
    .fetch_one(&mut admin)
    .await
    .unwrap();
    let seller = Uuid::from_u128(0x2001);
    let now = Utc::now().timestamp();
    let token = encode(
        &Header::default(),
        &json!({"sub":seller,"type":"access","iat":now,"exp":now+600,"iss":"lifecycle-tests","aud":"lifecycle-tests"}),
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    let response = contract_router(pool.clone(), KEY, true)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/trades/{trade}/decline"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let mut tx = admin.begin().await.unwrap();
    sqlx::query(ASSIGN_SQL)
        .bind(1000_i64)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for _ in 0..2 {
        let mut connection = pool.acquire().await.unwrap();
        while !consume_batch(&mut connection, &activation())
            .await
            .unwrap()
            .drained
        {}
        drop(connection);
        let events: Vec<(Uuid, String, Value)> = sqlx::query_as(
            "SELECT watchlist_target_id,event_type,event_payload::jsonb FROM watchlist_events",
        )
        .fetch_all(&mut admin)
        .await
        .unwrap();
        assert_eq!(events.len(), 1);
        let (target, class, payload) = &events[0];
        assert_eq!(*target, pin);
        assert_eq!(class, "PIN_QUANTITY_CHANGED");
        assert_eq!(payload["old_remaining_quantity_mt"].to_string(), "7");
        assert_eq!(payload["new_remaining_quantity_mt"].to_string(), "10");
        assert_eq!(payload["status"], "OPEN");
        // The restoration is the order's change instant, not its earlier fill.
        let changed: DateTime<Utc> = sqlx::query_scalar(
            "SELECT COALESCE(updated_at,created_at) FROM orderbook_orders WHERE id=$1",
        )
        .bind(Uuid::from_u128(1))
        .fetch_one(&mut admin)
        .await
        .unwrap();
        assert_eq!(instant(payload["observed_at"].as_str().unwrap()), changed);
        assert!(changed > before_decline);
        // The second pass replays from the start and must add nothing.
        sqlx::query("UPDATE watchlist_producer_state SET consumed_through_seq=0")
            .execute(&mut admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn expiry_event_carries_the_expiry_instant() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT effect_fixture_expiry($1)")
        .bind(f.order)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let changed_before: DateTime<Utc> = sqlx::query_scalar(
        "SELECT COALESCE(updated_at,created_at) FROM orderbook_orders WHERE id=$1",
    )
    .bind(f.order)
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    run_expiry_effect_probe(&f.pool, f.probe(MutationKind::OrderExpired, f.order))
        .await
        .unwrap();
    // The expiry component port captures no last-live projection, so this
    // source derives no PIN class; its evidence still carries the instant the
    // consumer copies into every order payload.
    let evidence = f.evidence().await;
    let after = &evidence["orders"][0]["after"];
    assert_eq!(after["status"], "EXPIRED");
    let changed: DateTime<Utc> = sqlx::query_scalar(
        "SELECT COALESCE(updated_at,created_at) FROM orderbook_orders WHERE id=$1",
    )
    .bind(f.order)
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert!(
        changed > changed_before,
        "the expiry records its change instant"
    );
    assert_eq!(instant(after["observed_at"].as_str().unwrap()), changed);
}

/// A source whose insert outlasts the statement deadline is retried: the
/// batch rolls back as busy with the watermark unchanged, and the running
/// consumer completes it once the inserts can finish.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn a_timed_out_source_is_retried_without_moving_the_watermark() {
    let mut f = Fixture::new().await;
    let slice = f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    sqlx::query("SELECT producer_fixture_slow_inserts(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.consume_once().await, Err(ProducerError::Busy));
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
    // The running consumer keeps retrying with backoff; it never stops for good.
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let producer = tokio::spawn(run_producer(
        f.pool.clone(),
        activation(),
        Arc::new(tokio::sync::Notify::new()),
        receiver,
    ));
    tokio::time::sleep(std::time::Duration::from_secs(7)).await;
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
    sqlx::query("SELECT producer_fixture_slow_inserts(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let high_water = f.high_water().await;
    let mut done = false;
    for _ in 0..120 {
        if f.watermark().await == Some(high_water) {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    stop.send_replace(true);
    producer.await.unwrap();
    assert!(done, "the consumer resumed after the deadline cleared");
    assert_eq!(keys(&f.events().await), cancel_keys(&f, f.target, slice));
}

/// One source larger than one insert statement is inserted whole, in one
/// transaction with its watermark.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn a_source_larger_than_one_insert_statement_is_inserted_whole() {
    let mut f = Fixture::new().await;
    let count = i32::try_from(INSERT_CHUNK_ROWS + 25).unwrap();
    let targets: Value = sqlx::query_scalar("SELECT producer_fixture_many_slices($1,$2,$3,$4)")
        .bind(f.actor)
        .bind(&f.product_code)
        .bind(f.point)
        .bind(count)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let aggregate = |best: Option<&str>| {
        json!({"market_product":f.product_code,"delivery_point_id":f.point,"availability_window":"SPOT",
            "side":"ASK","real_count":u8::from(best.is_some()),"demo_count":0,
            "real":{"order_count":u8::from(best.is_some()),"best_price_per_mt_usd":best,"observed_at":null},
            "demo":{"order_count":0,"best_price_per_mt_usd":null,"observed_at":null}})
    };
    let envelope = json!({"version":1,"mutation_id":Uuid::new_v4(),"captured_at":"2026-10-30T10:00:00Z",
        "orders":[],"aggregates_before":[aggregate(Some("100"))],"aggregates_after":[aggregate(Some("99"))],
        "targets":targets});
    f.row(Some(&envelope)).await;
    f.sequence().await;
    let outcome = f.consume_once().await.unwrap();
    assert_eq!(outcome.inserted, u64::try_from(count).unwrap());
    assert_eq!(outcome.sources, 1);
    assert_eq!(f.events().await.len(), usize::try_from(count).unwrap());
    let high_water = f.high_water().await;
    assert_eq!(f.watermark().await, Some(high_water));
}

/// The fence and floor are read again after the outbox read: a retention
/// delete that commits while the read runs stops the batch.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn coverage_is_rechecked_after_the_outbox_read() {
    let mut f = Fixture::new().await;
    f.slice_target().await;
    f.cancel().await;
    f.sequence().await;
    let high_water = f.high_water().await;
    sqlx::query("SELECT producer_fixture_hold_reads(true)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let url = f.database.runtime_url.clone();
    let consumer = tokio::spawn(async move {
        let mut connection = PgConnection::connect(&url).await.unwrap();
        consume_batch(&mut connection, &activation()).await
    });
    let mut held = false;
    for _ in 0..300 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE usename='rust_app' AND wait_event='PgSleep'",
        )
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
        if waiting > 0 {
            held = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(held, "the outbox read is held");
    f.fixture("SELECT producer_fixture_floor($1)", high_water)
        .await;
    sqlx::query("SELECT producer_fixture_hold_reads(false)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(consumer.await.unwrap(), Err(ProducerError::CoverageLost));
    assert!(f.events().await.is_empty());
    assert_eq!(f.watermark().await, Some(0));
}
