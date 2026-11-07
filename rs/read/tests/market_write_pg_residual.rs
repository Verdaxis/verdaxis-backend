#![cfg(feature = "contract-tests")]
//! Coexistence of the Rust market writers with the Python writers that keep
//! running after the route switch: the order-expiry reminder scan and email
//! delivery, the demo activity tick and prune, the operator quarantine and the
//! seed. Each Python writer runs unmodified in a child process
//! (`tests/postgres/phase8_residual_driver.py`) on a fresh clone of the
//! migrated database. Lock waits are observed in `pg_stat_activity`; every
//! observed wait is released at once, and the time until the waiter stops
//! waiting is measured at the database. A wait-graph sampler fails a schedule
//! on any lock cycle it does not specify. The last section covers the trusted
//! watchlist producer and the stream transport beside these writers (see
//! "producer chain and stream transport").
#[path = "support/market_database.rs"]
mod market_database;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    AppState,
    auth::VerificationSettings,
    routes::market_write::{amend, inventory, order_create, orders, trades},
    streams::{
        bus::{Delivery, PublicChannel},
        http::{StreamsState, routes as stream_routes},
        runtime::{CapacityConfig, StreamRuntime, start_streams},
    },
};

const KEY: &str = "synthetic-residual-signing-key-at-least-32";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const SELLER_ORG: u128 = 0x2000;
const ITEM: u128 = 0x4000;
const PRODUCT: &str = "c4cecebc-3d2d-5840-8021-57a9a11bc673";
const DEMO_BUYER_ORG: &str = "7cc77115-0a9f-4ec4-8c74-05aa10050111";
const DEMO_SELLER_ORG: &str = "d1e43e55-3fb0-4b5e-9f0b-93aa10050222";
const PYTHON_APP: &str = "phase8_residual_py";
const RUST_APP: &str = "residual-rust";
const RUST_HELD_APP: &str = "residual-rust-held";
const BARRIER_APP: &str = "residual-barrier";
const HOLDER_APP: &str = "residual-holder";
const FORBIDDEN_APP: &str = "residual-forbidden";
const QUARANTINE_ONE_APP: &str = "phase8_residual_py_one";
const QUARANTINE_TWO_APP: &str = "phase8_residual_py_two";
/// A Rust command held after its outbox insert, before it commits.
const CHAIN_HELD_APP: &str = "chain-rust-held";
const CHAIN_BARRIER_APP: &str = "chain-barrier";
/// The Rust stream runtime's sequencer session.
const ELECTION_APP: &str = "verdaxis-stream-election";
/// Longest time from an observed lock wait to its release. It keeps the
/// waiter inside the 500 ms market lock bound on a loaded host.
const RELEASE_BUDGET: Duration = Duration::from_millis(350);
/// Bounded refusals a contender may return instead of waiting longer.
fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}
fn verification() -> VerificationSettings {
    VerificationSettings::new(
        KEY.into(),
        None,
        "residual-tests".into(),
        "residual-tests".into(),
    )
}
#[derive(Clone, Debug)]
struct Command {
    method: &'static str,
    path: String,
    actor: u128,
    body: Value,
}
impl Command {
    fn new(method: &'static str, path: String, actor: u128, body: Value) -> Self {
        Self {
            method,
            path,
            actor,
            body,
        }
    }
}
type Reply = (StatusCode, Value);

async fn send(router: Router, command: Command) -> Reply {
    let now = chrono::Utc::now().timestamp();
    let token = encode(
        &Header::default(),
        &json!({"sub":id(command.actor),"type":"access","iat":now,"exp":now+600,"iss":"residual-tests","aud":"residual-tests"}),
        &EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    let mut request = Request::builder()
        .method(command.method)
        .uri(command.path)
        .header("authorization", format!("Bearer {token}"));
    let body = if command.body.is_null() {
        String::new()
    } else {
        request = request.header("content-type", "application/json");
        command.body.to_string()
    };
    let response = router
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn code(reply: &Reply) -> &str {
    reply.1["detail"]["code"].as_str().unwrap_or_default()
}
fn expect_status(reply: &Reply, status: StatusCode) {
    assert_eq!(reply.0, status, "{reply:?}");
}

/// One unchanged Python writer in a child process.
fn ok(result: &Value) -> &Value {
    assert_eq!(result["ok"], true);
    &result["result"]
}

struct Fixture {
    _database: Arc<market_database::MarketDatabase>,
    admin: PgConnection,
    admin_url: String,
    runtime_url: String,
    point: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let database = Arc::new(market_database::MarketDatabase::new("RUST_RESIDUAL").await);
        let admin_url = database.admin_url.clone();
        let runtime_url = database.runtime_url.clone();
        let a = url::Url::parse(&admin_url).unwrap();
        let r = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(a.host_str(), Some("127.0.0.1"));
        assert!(a.path().ends_with("_test"));
        assert_ne!(a.username(), r.username());
        let mut owner = url::Url::parse(
            &std::env::var("RUST_RESIDUAL_OWNER_DATABASE_URL")
                .expect("explicit disposable owner-role URL"),
        )
        .unwrap();
        assert_eq!(
            (owner.host_str(), owner.port()),
            (a.host_str(), a.port()),
            "owner URL names the same disposable server"
        );
        owner.set_path(a.path());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        sqlx::query("SELECT set_config('application_name','residual-observer',false)")
            .execute(&mut admin)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT 'sqlx_1'::text FROM _sqlx_migrations WHERE version=1 AND success"
            )
            .fetch_one(&mut admin)
            .await
            .unwrap(),
            "sqlx_1"
        );
        for source in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_publish.sql"),
            include_str!("fixtures/phase8_residual.sql"),
        ] {
            sqlx::raw_sql(source).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT residual_seed()")
            .execute(&mut admin)
            .await
            .unwrap();
        let point = sqlx::query_scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
            .fetch_one(&mut admin)
            .await
            .unwrap();
        Self {
            _database: database,
            admin,
            admin_url,
            runtime_url,
            point,
        }
    }

    // ---- Rust side -------------------------------------------------------

    async fn app(&self, name: &'static str) -> (AppState, PgPool) {
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
        let config = serde_json::from_value(json!({"environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":ALLOWED_REVISIONS[0],"database_url":self.runtime_url,"db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":true})).unwrap();
        let mut pools = verdaxis_read::connect(&config).await.unwrap();
        pools.read = pool.clone();
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        (AppState::new(config, pools, policy).unwrap(), pool)
    }
    async fn writer(&self, name: &'static str) -> (Router, PgPool) {
        let (app, pool) = self.app(name).await;
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
            app,
            verification(),
            true,
            Default::default(),
            None,
        ));
        (router, pool)
    }
    /// The public read routes, which decode every stored row they select.
    async fn reader(&self) -> (Router, PgPool) {
        let (app, pool) = self.app(RUST_APP).await;
        let router = Router::new()
            .merge(verdaxis_read::routes::orderbook_lists::routes())
            .merge(verdaxis_read::routes::trade_tape::routes())
            .with_state(app);
        (router, pool)
    }
    async fn call(&self, command: Command) -> Reply {
        let (router, pool) = self.writer(RUST_APP).await;
        let reply = send(router, command).await;
        pool.close().await;
        reply
    }
    async fn read(&self, path: &str) -> Reply {
        let (router, pool) = self.reader().await;
        let reply = send(router, Command::new("GET", path.into(), BUYER, Value::Null)).await;
        pool.close().await;
        reply
    }
    /// A Rust command on its own pool, running concurrently with the test.
    async fn spawn(&self, name: &'static str, command: Command) -> tokio::task::JoinHandle<Reply> {
        let (router, pool) = self.writer(name).await;
        tokio::spawn(async move {
            let reply = send(router, command).await;
            pool.close().await;
            reply
        })
    }
    fn cancel(&self, order: Uuid, actor: u128) -> Command {
        Command::new(
            "DELETE",
            format!("/api/orderbook/{order}"),
            actor,
            Value::Null,
        )
    }
    fn amend(&self, order: Uuid, body: Value) -> Command {
        Command::new("PUT", format!("/api/orderbook/{order}"), SELLER, body)
    }
    fn bid(&self, quantity: &str, price: &str, window: &str, expires: Option<&str>) -> Command {
        let mut body = json!({"side":"BID","product_id":PRODUCT,"delivery_point_id":self.point,"quantity_mt":quantity,"price_per_mt_usd":price,"availability_window":window});
        if let Some(expires) = expires {
            body["expires_at"] = json!(expires);
        }
        Command::new("POST", "/api/orderbook".into(), BUYER, body)
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
    /// Publish the seeded item through the Rust route: a 1000 MT REAL ASK.
    async fn listing(&self) -> Uuid {
        let reply = self
            .call(Command::new(
                "POST",
                format!("/api/inventory/{}/publish", id(ITEM)),
                SELLER,
                Value::Null,
            ))
            .await;
        expect_status(&reply, StatusCode::OK);
        reply.1["listing_id"].as_str().unwrap().parse().unwrap()
    }

    // ---- fixture SQL -----------------------------------------------------

    async fn ask(
        &mut self,
        order: Uuid,
        price: i64,
        quantity: i64,
        window: &str,
        expires: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        sqlx::query("SELECT residual_ask($1,$2,$3,$4,$5)")
            .bind(order)
            .bind(Decimal::from(price))
            .bind(Decimal::from(quantity))
            .bind(window)
            .bind(expires)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }

    /// The order's status, remainder and expiry, or null once the row is gone.
    async fn order(&mut self, order: Uuid) -> Value {
        let row: Option<Value> = sqlx::query_scalar("SELECT residual_order($1)")
            .bind(order)
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        row.unwrap_or(Value::Null)
    }

    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT residual_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn ledger(&mut self) {
        let invariant: Value = sqlx::query_scalar("SELECT residual_invariants()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        assert_eq!(
            invariant,
            json!({"stock":true,"orders":true,"freeze":true}),
            "conservation after the schedule: {invariant}"
        );
    }

    // ---- lock observation ------------------------------------------------

    /// Poll until `waiter` waits on a heavyweight lock held by `blocker`.
    async fn wait_lock(&mut self, waiter: &str, blocker: &str) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT residual_waiting($1,$2)")
                .bind(waiter)
                .bind(blocker)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
            if let Some(pid) = pid {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "{waiter} did not wait for {blocker}"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    async fn demo_seed_organizations(&mut self) {
        sqlx::query("SELECT residual_demo_seed_organizations()")
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn demo_organizations(&mut self) {
        sqlx::query("SELECT residual_demo_organizations()")
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn demo_ticks(&self, count: usize) -> Vec<(String, String)> {
        use verdaxis_read::cli::jobs::demo::model::{activity_tick, tick_choice};
        rust_b30_ticks(count)
            .into_iter()
            .map(|tick| {
                let choice = tick_choice(activity_tick(tick));
                (tick.to_rfc3339(), choice.window)
            })
            .collect()
    }
    async fn demo_tick(&self, now: &str) -> Value {
        let (report, result) = self.rust_demo(now.parse().unwrap(), Vec::new()).await;
        result.unwrap();
        let created = usize::from(matches!(
            report.market.tick,
            Some(TickOutcome::Created { .. })
        ));
        json!({"ok":true,"result":{"created_trades":created}})
    }
}

/// The lock waits sampled among this suite's backends during one schedule.
#[derive(Debug, Default)]
struct Contention {
    edges: BTreeSet<(String, String)>,
    cycle: Option<Vec<(String, String)>>,
}
impl Contention {
    fn assert_no_cycle(&self, label: &str) {
        assert!(
            self.cycle.is_none(),
            "{label}: unexpected lock cycle {:?}",
            self.cycle
        );
    }
}
/// A cycle among the waits observed at one instant, if any.
fn find_cycle(edges: &[(String, String)]) -> Option<Vec<(String, String)>> {
    fn walk(
        edges: &[(String, String)],
        start: &str,
        node: &str,
        path: &mut Vec<(String, String)>,
    ) -> bool {
        for edge in edges.iter().filter(|(waiter, _)| waiter == node) {
            if path.contains(edge) {
                continue;
            }
            path.push(edge.clone());
            if edge.1 == start || walk(edges, start, &edge.1, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    edges.iter().find_map(|(start, _)| {
        let mut path = Vec::new();
        walk(edges, start, start, &mut path).then_some(path)
    })
}
/// Samples `pg_locks` wait edges among the named backends until stopped.
struct WaitGraph {
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<Contention>,
}
impl WaitGraph {
    async fn start(admin_url: &str) -> Self {
        let mut connection = PgConnection::connect(admin_url).await.unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let task = tokio::spawn(async move {
            let apps = [
                PYTHON_APP,
                RUST_APP,
                RUST_HELD_APP,
                BARRIER_APP,
                HOLDER_APP,
                FORBIDDEN_APP,
                QUARANTINE_ONE_APP,
                QUARANTINE_TWO_APP,
                CHAIN_HELD_APP,
                CHAIN_BARRIER_APP,
                ELECTION_APP,
            ];
            let mut contention = Contention::default();
            while !flag.load(Ordering::Relaxed) {
                let snapshot: Value = sqlx::query_scalar("SELECT residual_wait_edges($1)")
                    .bind(&apps[..])
                    .fetch_one(&mut connection)
                    .await
                    .unwrap();
                let edges: Vec<(String, String)> = snapshot
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|edge| {
                        (
                            edge[0].as_str().unwrap().to_owned(),
                            edge[1].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect();
                if contention.cycle.is_none() {
                    contention.cycle = find_cycle(&edges);
                }
                contention.edges.extend(edges);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            connection.close().await.ok();
            contention
        });
        Self { stop, task }
    }
    async fn stop(self) -> Contention {
        self.stop.store(true, Ordering::Relaxed);
        let contention = self.task.await.unwrap();
        println!(
            "WAIT_GRAPH edges={:?} cycle={:?}",
            contention.edges, contention.cycle
        );
        contention
    }
}

// ---- order-expiry reminders ----------------------------------------------

// ---- demo activity -------------------------------------------------------

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_tick_rows_are_exactly_the_documented_columns_and_read_by_rust() {
    let mut f = Fixture::new().await;
    f.demo_organizations().await;
    let (now, _window) = f.demo_ticks(1).await.remove(0);
    assert_eq!(ok(&f.demo_tick(&now).await)["created_trades"], 1);
    let shape: Value = sqlx::query_scalar("SELECT residual_demo_shape()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let orders = shape["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 2, "{shape}");
    // The tick allocates the ASK ordinal first.
    assert_eq!(orders[0]["side"], "ASK");
    assert_eq!(orders[0]["organization_id"], DEMO_SELLER_ORG);
    assert_eq!(orders[1]["side"], "BID");
    assert_eq!(orders[1]["organization_id"], DEMO_BUYER_ORG);
    assert!(orders[0]["acceptance_ordinal"].as_i64() < orders[1]["acceptance_ordinal"].as_i64());
    for order in orders {
        assert_eq!(order["creation_method"], "SYSTEM");
        assert_eq!(order["provenance"], "DEMO");
        assert_eq!(order["owner_user_id"], Value::Null);
        assert_eq!(order["created_by_actor_user_id"], Value::Null);
        assert_eq!(order["status"], "FILLED");
    }
    let trades = shape["trades"].as_array().unwrap();
    assert_eq!(trades.len(), 1, "{shape}");
    let trade = &trades[0];
    assert_eq!(trade["buyer_user_id"], Value::Null);
    assert_eq!(trade["seller_user_id"], Value::Null);
    assert_eq!(trade["buyer_provenance"], "DEMO");
    assert_eq!(trade["seller_provenance"], "DEMO");
    assert_eq!(trade["confirmed"], true);
    assert_eq!(trade["ask_order_id"], orders[0]["id"]);
    assert_eq!(trade["bid_order_id"], orders[1]["id"]);
    let outbox = shape["outbox"].as_array().unwrap();
    assert_eq!(outbox.len(), 1, "{shape}");
    assert_eq!(outbox[0]["event_type"], "demo_market_changed");
    assert_eq!(outbox[0]["public"], true);
    assert_eq!(outbox[0]["evidence_absent"], true);
    // The participant set is stored from a frozenset: its order is not fixed.
    let mut participants: Vec<&str> = outbox[0]["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    participants.sort_unstable();
    let mut expected = [DEMO_BUYER_ORG, DEMO_SELLER_ORG];
    expected.sort_unstable();
    assert_eq!(participants, expected);
    let tape = f.read("/api/trade-tape?limit=200").await;
    expect_status(&tape, StatusCode::OK);
    let prefix = &trade["id"].as_str().unwrap().replace('-', "")[..8];
    let item = tape.1["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == prefix)
        .unwrap_or_else(|| panic!("demo trade absent from the Rust tape: {}", tape.1))
        .clone();
    assert_eq!(item["is_demo_trade"], true);
    expect_status(&f.read("/api/orderbook?limit=100").await, StatusCode::OK);
    expect_status(&f.read("/api/orderbook/asks").await, StatusCode::OK);
}

// ---- retained deleters and freeze history --------------------------------

// ---- the Phase 7 seam ----------------------------------------------------

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_writers_leave_negotiation_and_offer_references_intact() {
    let mut f = Fixture::new().await;
    let listing = f.listing().await;
    let reply = f.call(f.bid("5", "90", "SPOT", None)).await;
    expect_status(&reply, StatusCode::CREATED);
    let bid: Uuid = reply.1["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("SELECT residual_phase7_references($1,$2)")
        .bind(bid)
        .bind(listing)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let references: Value = sqlx::query_scalar("SELECT residual_phase7_state()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(references["referenced_orders"], 2);
    // The supplier offer belongs to the listing's own supplier and user.
    assert_eq!(
        references["offers"][0]["supplier_org_id"],
        json!(id(SELLER_ORG))
    );
    assert_eq!(
        references["offers"][0]["supplier_user_id"],
        json!(id(SELLER))
    );
    let take = f.take(listing, "10").await;
    let steps = [
        ("take", take),
        (
            "amend bid",
            Command::new(
                "PUT",
                format!("/api/orderbook/{bid}"),
                BUYER,
                json!({"price_per_mt_usd":"95"}),
            ),
        ),
        (
            "amend listing",
            f.amend(listing, json!({"price_per_mt_usd":"101"})),
        ),
        ("cancel bid", f.cancel(bid, BUYER)),
        ("cancel listing", f.cancel(listing, SELLER)),
        // A new publication of the same inventory after the references exist.
        (
            "publish again",
            Command::new(
                "POST",
                format!("/api/inventory/{}/publish", id(ITEM)),
                SELLER,
                Value::Null,
            ),
        ),
    ];
    let expected = [
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::NO_CONTENT,
        StatusCode::NO_CONTENT,
        StatusCode::OK,
    ];
    for ((label, command), status) in steps.into_iter().zip(expected) {
        let reply = f.call(command).await;
        println!(
            "PHASE7_REFERENCE_STEP {label} status={} code={}",
            reply.0,
            code(&reply)
        );
        expect_status(&reply, status);
        let state: Value = sqlx::query_scalar("SELECT residual_phase7_state()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            state, references,
            "{label} changed a Phase 7 row or removed a referenced order"
        );
    }
    assert_eq!(f.order(bid).await["status"], "CANCELLED");
    assert_eq!(f.order(listing).await["status"], "CANCELLED");
    f.ledger().await;
}

// ---- negative control ----------------------------------------------------

/// A writer that takes the slice before a user row, the order Rust forbids,
/// forms a cycle with a Rust cancellation. Under the same server settings and
/// the same wait-graph sampler as the bounded cells, the sampler records the
/// cycle and the cells' assertion refuses it; Rust's 500 ms bound ends it.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn forbidden_slice_then_user_order_is_detected_as_a_lock_cycle() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let slice: i64 = sqlx::query_scalar("SELECT residual_slice_key($1::uuid,$2,'SPOT')")
        .bind(PRODUCT)
        .bind(f.point)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let before = f.snapshot().await;
    let mut forbidden = PgConnection::connect(&f.admin_url).await.unwrap();
    sqlx::raw_sql("SELECT set_config('application_name','residual-forbidden',false); BEGIN")
        .execute(&mut forbidden)
        .await
        .unwrap();
    sqlx::query("SELECT residual_forbidden_slice($1)")
        .bind(slice)
        .execute(&mut forbidden)
        .await
        .unwrap();
    let graph = WaitGraph::start(&f.admin_url).await;
    let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_APP, FORBIDDEN_APP).await;
    // Rust holds the owner's user row and waits on the slice; now the writer
    // asks for that user row.
    let request = tokio::spawn(async move {
        let result = sqlx::query("SELECT residual_forbidden_user($1)")
            .bind(id(SELLER))
            .execute(&mut forbidden)
            .await
            .map(|_| ());
        (forbidden, result)
    });
    f.wait_lock(FORBIDDEN_APP, RUST_APP).await;
    let reply = cancel.await.unwrap();
    expect_status(&reply, StatusCode::CONFLICT);
    assert_eq!(code(&reply), "MARKET_BUSY", "the lock bound ends the cycle");
    let (mut forbidden, result) = request.await.unwrap();
    assert!(
        result.is_ok(),
        "the writer gets the row once Rust gives up: {result:?}"
    );
    sqlx::raw_sql("ROLLBACK")
        .execute(&mut forbidden)
        .await
        .unwrap();
    forbidden.close().await.unwrap();
    let contention = graph.stop().await;
    let cycle = contention
        .cycle
        .clone()
        .expect("the sampler records the cycle");
    assert!(cycle.contains(&(RUST_APP.to_owned(), FORBIDDEN_APP.to_owned())));
    assert!(cycle.contains(&(FORBIDDEN_APP.to_owned(), RUST_APP.to_owned())));
    let refused = std::panic::catch_unwind(|| contention.assert_no_cycle("negative control"));
    assert!(
        refused.is_err(),
        "the bounded cells' check refuses this schedule"
    );
    println!("NEGATIVE_CONTROL cycle={cycle:?} rust_status={}", reply.0);
    assert_eq!(
        f.snapshot().await,
        before,
        "the refused cancellation has no effect"
    );
    expect_status(
        &f.call(f.cancel(order, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    f.ledger().await;
}

// ---- producer chain and stream transport -----------------------------------
//
// The trusted watchlist producer and the stream transport against the same
// database as the retained Python writers: a Python outbox row without
// producer evidence among Rust sources, the commit boundary of the sequencer
// (Rust and Python dispatchers), and the private and public stream frames of
// rows that carry captured watchlist targets in their internal column.

const CHAIN_RELEASE: &str = "0123456789abcdef0123456789abcdef01234567";
const ASSIGN_SQL: &str = include_str!("../sql/streams_assign.sql");
const BUYER_ORG: u128 = 0x1000;
const OBSERVER_ORG: u128 = 0x5000;
const OBSERVER: u128 = 0x5001;
const STREAM_KEY: &str = "synthetic-chain-stream-key-at-least-32-chars";
const STREAM_PARTY: &str = "chain-tests";
/// How long a frame or delivery that is already on its way may take.
const FRAME_BOUND: Duration = Duration::from_secs(10);
/// The longest time from a commit to its delivery: the next sequencer poll
/// at the latest, plus allowance for a loaded host.
const POLL_BOUND: Duration = Duration::from_secs(5);
/// How long a "delivered once" check keeps listening after the delivery: past
/// the next 2 s poll of either dispatcher, so a repeat on that poll is seen.
const PAST_NEXT_POLL: Duration = Duration::from_secs(3);
/// Keys that would carry producer evidence or watchlist subscriptions.
const INTERNAL_KEYS: [&str; 7] = [
    "producer_evidence",
    "targets",
    "target_id",
    "watchlist_id",
    "observer_id",
    "watchlist_target_id",
    "mutation_id",
];

fn producer_activation() -> verdaxis_read::market_contract::producer::ProducerActivation {
    verdaxis_read::market_contract::producer::ProducerActivation::from_release(
        Some("rust-market-v1"),
        Some("2026-10-01T00:00:00Z"),
        CHAIN_RELEASE,
    )
    .unwrap()
}

/// The payload a stream subscriber receives for a stored outbox payload: the
/// public marker removed and the schema version added when absent.
fn delivered(stored: &Value) -> Value {
    let mut payload = stored.clone();
    let object = payload.as_object_mut().expect("an object payload");
    object.remove("_public_market_invalidation");
    object.entry("schema_version").or_insert(json!(1));
    payload
}

/// Every object key at any depth of `value`.
fn all_keys(value: &Value, keys: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                keys.insert(key.clone());
                all_keys(inner, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|inner| all_keys(inner, keys)),
        _ => {}
    }
}

/// A delivered payload carries no internal key and names none of `hidden`.
fn assert_no_internal_identity(text: &str, payload: Option<&Value>, hidden: &[Uuid]) {
    if let Some(payload) = payload {
        let mut keys = BTreeSet::new();
        all_keys(payload, &mut keys);
        for key in INTERNAL_KEYS {
            assert!(!keys.contains(key), "internal key {key} delivered");
        }
    }
    for identity in hidden {
        assert!(
            !text.contains(&identity.to_string()),
            "a watcher identity reached a frame"
        );
    }
}

/// The two month windows after the current one, as canonical codes.
fn month_windows() -> [String; 2] {
    let now = chrono::Utc::now();
    [
        (now + chrono::Duration::days(32))
            .format("%Y-%m")
            .to_string(),
        (now + chrono::Duration::days(63))
            .format("%Y-%m")
            .to_string(),
    ]
}

/// One row of `chain_outbox` or `chain_sequenced_after`.
fn seq_of(row: &Value) -> i64 {
    row["seq"].as_i64().expect("a sequenced row")
}

/// Whole SSE frames from a live body, each read within a bound.
struct Frames {
    body: axum::body::BodyDataStream,
    buffer: Vec<u8>,
}
impl Frames {
    async fn open(router: &Router, path: &str, cursor: Option<i64>) -> Self {
        let mut request = Request::builder().uri(path);
        if let Some(cursor) = cursor {
            request = request.header("last-event-id", cursor.to_string());
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        Self {
            body: response.into_body().into_data_stream(),
            buffer: Vec::new(),
        }
    }
    /// The next frame, or `None` when none arrives within `bound`.
    async fn next_within(&mut self, bound: Duration) -> Option<String> {
        use futures_util::StreamExt;
        let read = async {
            loop {
                if let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\n\n") {
                    let frame: Vec<u8> = self.buffer.drain(..end + 2).take(end).collect();
                    return String::from_utf8(frame).unwrap();
                }
                let chunk = self.body.next().await.expect("the stream stays open");
                self.buffer.extend_from_slice(&chunk.unwrap());
            }
        };
        tokio::time::timeout(bound, read).await.ok()
    }
}

/// The `(field, value)` lines of one frame.
fn frame_fields(frame: &str) -> Vec<(String, String)> {
    frame
        .split('\n')
        .map(|line| match line.split_once(':') {
            Some((field, value)) => (
                field.to_owned(),
                value.strip_prefix(' ').unwrap_or(value).to_owned(),
            ),
            None => (line.to_owned(), String::new()),
        })
        .collect()
}
fn frame_value(frame: &str, name: &str) -> Option<String> {
    frame_fields(frame)
        .into_iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value)
}
fn is_heartbeat(frame: &str) -> bool {
    frame_value(frame, "event").as_deref() == Some("heartbeat")
}

/// One private business frame: id, event name and data, nothing else.
type Business = (i64, String, Value);
fn business(frame: &str) -> Business {
    assert_eq!(frame_fields(frame).len(), 3, "one id, event and data line");
    (
        frame_value(frame, "id").unwrap().parse().unwrap(),
        frame_value(frame, "event").unwrap(),
        serde_json::from_str(&frame_value(frame, "data").unwrap()).unwrap(),
    )
}

/// Read `count` business frames, skipping heartbeats; every frame text is
/// returned too for the identity checks.
async fn business_frames(frames: &mut Frames, count: usize) -> (Vec<Business>, Vec<String>) {
    let (mut found, mut texts) = (Vec::new(), Vec::new());
    while found.len() < count {
        let frame = frames
            .next_within(FRAME_BOUND)
            .await
            .expect("a business frame within the bound");
        if !is_heartbeat(&frame) {
            found.push(business(&frame));
        }
        texts.push(frame);
    }
    (found, texts)
}

/// No business frame arrives within `bound`; heartbeats only.
async fn assert_no_business(frames: &mut Frames, bound: Duration) -> Vec<String> {
    let mut texts = Vec::new();
    let deadline = Instant::now() + bound;
    while let Some(frame) = frames
        .next_within(deadline.saturating_duration_since(Instant::now()))
        .await
    {
        assert!(is_heartbeat(&frame), "an unexpected frame: {frame:?}");
        texts.push(frame);
    }
    texts
}

/// A signed stream token for `user` in `organization`, valid for one minute.
fn stream_token(user: u128, organization: u128) -> String {
    let now = chrono::Utc::now();
    encode(
        &Header::default(),
        &json!({"sub":id(user),"org_id":id(organization),"environment":"test","type":"stream",
            "jti":Uuid::new_v4(),"iss":STREAM_PARTY,"aud":STREAM_PARTY,
            "iat":now.timestamp(),"exp":(now+chrono::Duration::seconds(60)).timestamp()}),
        &EncodingKey::from_secret(STREAM_KEY.as_bytes()),
    )
    .unwrap()
}
fn trades_path(user: u128, organization: u128) -> String {
    format!(
        "/api/stream/trades?stream_token={}",
        stream_token(user, organization)
    )
}

impl Fixture {
    /// The producer and chain fixture functions and one unrelated observer.
    async fn chain(&mut self) {
        for source in [
            include_str!("fixtures/phase8_producer.sql"),
            include_str!("fixtures/phase8_chain.sql"),
        ] {
            sqlx::raw_sql(source)
                .execute(&mut self.admin)
                .await
                .unwrap();
        }
        sqlx::query("SELECT chain_observer($1,$2)")
            .bind(id(OBSERVER_ORG))
            .bind(id(OBSERVER))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    /// A target of the observer in its own watchlist: `(target, watchlist)`.
    async fn watch(&mut self, kind: &str, order: Option<Uuid>, window: &str) -> (Uuid, Uuid) {
        let target = Uuid::new_v4();
        let watchlist =
            sqlx::query_scalar("SELECT producer_fixture_target($1,$2,$3,$4,'B30',$5,$6)")
                .bind(target)
                .bind(id(OBSERVER))
                .bind(kind)
                .bind(order)
                .bind(self.point)
                .bind(window)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
        (target, watchlist)
    }
    async fn outbox(&mut self, order: Uuid) -> Vec<Value> {
        let rows: Value = sqlx::query_scalar("SELECT chain_outbox($1)")
            .bind(order.to_string())
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        rows.as_array().unwrap().clone()
    }
    async fn sequenced_after(&mut self, seq: i64) -> Vec<Value> {
        let rows: Value = sqlx::query_scalar("SELECT chain_sequenced_after($1)")
            .bind(seq)
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        rows.as_array().unwrap().clone()
    }
    async fn chain_scalar(&mut self, function: &str) -> Option<i64> {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {function}()::bigint")))
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn high_water(&mut self) -> i64 {
        self.chain_scalar("chain_high_water").await.unwrap()
    }
    async fn unsequenced(&mut self) -> i64 {
        self.chain_scalar("chain_unsequenced").await.unwrap()
    }
    async fn producer_watermark(&mut self) -> Option<i64> {
        self.chain_scalar("chain_watermark").await
    }
    async fn sequencer_holder(&mut self) -> Option<i64> {
        self.chain_scalar("chain_sequencer_holder").await
    }
    async fn backends(&mut self, application: &str) -> i64 {
        sqlx::query_scalar("SELECT chain_backends($1)")
            .bind(application)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn trusted_events(&mut self) -> Vec<Value> {
        let rows: Value = sqlx::query_scalar("SELECT chain_events()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        rows.as_array().unwrap().clone()
    }
    /// The sequencer's own assignment statement, committed: the rows it numbered.
    async fn sequence(&mut self) -> u64 {
        let mut tx = self.admin.begin().await.unwrap();
        let assigned = sqlx::query(ASSIGN_SQL)
            .bind(1000_i64)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        tx.commit().await.unwrap();
        assigned
    }
    /// Wait until every committed outbox row has a stream sequence.
    async fn wait_sequenced(&mut self) {
        let deadline = Instant::now() + POLL_BOUND;
        while self.unsequenced().await > 0 {
            assert!(Instant::now() < deadline, "rows stayed unsequenced");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    /// The explicit operator step that creates the producer watermark.
    async fn bootstrap(&self) -> i64 {
        use verdaxis_read::market_contract::producer::{BootstrapOutcome, bootstrap_watermark};
        let mut connection = PgConnection::connect(&self.runtime_url).await.unwrap();
        let outcome = bootstrap_watermark(&mut connection).await.unwrap();
        connection.close().await.unwrap();
        match outcome {
            BootstrapOutcome::Created { watermark } => watermark,
            other => panic!("the watermark already existed: {other:?}"),
        }
    }
    /// Consume until drained on a fresh runtime connection, as a restarted
    /// producer does; returns the event rows inserted.
    async fn consume(&self) -> u64 {
        use verdaxis_read::market_contract::producer::consume_batch;
        let mut connection = PgConnection::connect(&self.runtime_url).await.unwrap();
        let mut inserted = 0;
        for _ in 0..50 {
            let outcome = consume_batch(&mut connection, &producer_activation())
                .await
                .unwrap();
            inserted += outcome.inserted;
            if outcome.drained {
                connection.close().await.unwrap();
                return inserted;
            }
        }
        panic!("the producer backlog did not drain");
    }
    /// The Rust stream runtime (sequencer election, dispatch and hub) on the
    /// runtime role, ready to serve.
    async fn stream_app(&self) -> (AppState, StreamRuntime) {
        let pool_max = 9;
        let mut config: verdaxis_read::Config = serde_json::from_value(json!({
            "environment":"test","release_sha":verdaxis_read::compiled_sha(),
            "migration_revision":ALLOWED_REVISIONS[0],"database_url":self.runtime_url,
            "db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,
            "pool_max":pool_max,"python_budget":52,"configured_max_connections":100,
            "kyc_execution_gate_enabled":true,
            "phase2":{"jwt_secret":STREAM_KEY,"jwt_issuer":STREAM_PARTY,"jwt_audience":STREAM_PARTY,
                "frontend_url":"https://example.test","email_from":"fixture@example.test",
                "resend_api_key":"fixture-mail-key"}}))
        .unwrap();
        // Two services of four workers, each with pool 2, overflow 1 and one
        // listener, plus the reserve: the Python budget of 52 connections.
        config.prepared_budget = Some(
            json!({"version":1,"release_sha":config.release_sha,"environment_digest":"a".repeat(64),
                "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":pool_max,
                "python_budget":52,"cache_listener":0,"stream_listener":1,"total":52+pool_max+1,
                "configured_max_connections":100})
            .to_string(),
        );
        let pools = verdaxis_read::connect(&config).await.unwrap();
        let policy =
            serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
        let mut state = AppState::new(config, pools, policy).unwrap();
        let runtime = start_streams(&mut state, CapacityConfig::default())
            .await
            .expect("the stream runtime starts");
        let deadline = Instant::now() + FRAME_BOUND;
        while !verdaxis_read::streams::runtime::ready(&state) {
            assert!(Instant::now() < deadline, "the stream runtime is not ready");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (state, runtime)
    }
    async fn stop_streams(&mut self, runtime: StreamRuntime) {
        runtime.shutdown().await;
        let deadline = Instant::now() + FRAME_BOUND;
        while self.backends(ELECTION_APP).await > 0 {
            assert!(
                Instant::now() < deadline,
                "the sequencer session stayed open"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// A Python outbox row without producer evidence between two Rust sources:
/// the producer derives events from the two Rust sources only, its watermark
/// covers all three rows, and neither a restart nor a rewound watermark adds
/// or changes an event.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn legacy_outbox_row_without_evidence_is_covered_by_the_producer_without_events() {
    let mut f = Fixture::new().await;
    f.chain().await;
    f.demo_organizations().await;
    let (now, demo_window) = f.demo_ticks(1).await.remove(0);
    // Each Rust source is the only order of its own slice, apart from the
    // slice of the demo tick.
    let windows: Vec<String> = std::iter::once("SPOT".to_owned())
        .chain(month_windows())
        .filter(|window| *window != demo_window)
        .take(2)
        .collect();
    let orders = [Uuid::new_v4(), Uuid::new_v4()];
    let mut expected = BTreeSet::new();
    let mut hidden = vec![id(OBSERVER), id(OBSERVER_ORG)];
    for (order, window) in orders.iter().zip(&windows) {
        f.ask(*order, 150, 5, window, None).await;
        let (pin, pin_list) = f.watch("PIN", Some(*order), window).await;
        let (slice, slice_list) = f.watch("SLICE", None, window).await;
        expected.insert((pin, "PIN_WITHDRAWN".to_owned()));
        expected.insert((slice, "SLICE_BEST_PRICE_MOVED".to_owned()));
        expected.insert((slice, "SLICE_WENT_QUIET".to_owned()));
        hidden.extend([pin, pin_list, slice, slice_list]);
    }
    let bootstrap = f.bootstrap().await;

    // Rust source, Python demo source, Rust source; each sequenced in turn.
    expect_status(
        &f.call(f.cancel(orders[0], SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    assert!(f.sequence().await >= 1);
    let first = f.outbox(orders[0]).await;
    let after_first = first.iter().map(seq_of).max().unwrap();
    assert_eq!(ok(&f.demo_tick(&now).await)["created_trades"], 1);
    assert_eq!(f.sequence().await, 1, "the tick appends one outbox row");
    let demo = f.sequenced_after(after_first).await;
    assert_eq!(demo.len(), 1);
    assert_eq!(demo[0]["event_type"], "demo_market_changed");
    assert_eq!(demo[0]["mutation"], Value::Null, "no producer evidence");
    let demo_seq = seq_of(&demo[0]);
    expect_status(
        &f.call(f.cancel(orders[1], SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    assert!(f.sequence().await >= 1);
    let second = f.outbox(orders[1]).await;
    for (rows, label) in [(&first, "first"), (&second, "second")] {
        assert!(!rows.is_empty(), "{label} source has outbox rows");
        let mutations: BTreeSet<&str> = rows
            .iter()
            .map(|row| row["mutation"].as_str().expect("Rust rows carry evidence"))
            .collect();
        assert_eq!(mutations.len(), 1, "{label}: one mutation");
    }
    assert!(first.iter().all(|row| seq_of(row) < demo_seq));
    assert!(second.iter().all(|row| seq_of(row) > demo_seq));
    let mutation = |rows: &[Value]| -> (Uuid, i64) {
        (
            rows[0]["mutation"].as_str().unwrap().parse().unwrap(),
            rows.iter().map(seq_of).min().unwrap(),
        )
    };
    let sources = [mutation(&first), mutation(&second)];
    let high_water = f.high_water().await;
    assert_eq!(high_water, second.iter().map(seq_of).max().unwrap());

    let inserted = f.consume().await;
    let events = f.trusted_events().await;
    assert_eq!(inserted as usize, events.len());
    let derived: BTreeSet<(Uuid, String)> = events
        .iter()
        .map(|event| {
            (
                event["target"].as_str().unwrap().parse().unwrap(),
                event["class"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    println!("CHAIN_DERIVED {derived:?}");
    assert_eq!(derived, expected, "events of exactly the two Rust sources");
    for event in &events {
        assert_eq!(event["producer"], "rust-market-v1");
        let source: Uuid = event["source"].as_str().unwrap().parse().unwrap();
        let seq = event["seq"].as_i64().unwrap();
        assert!(sources.contains(&(source, seq)), "source {source} at {seq}");
        assert_ne!(seq, demo_seq, "no event from the evidence-free row");
    }
    assert_eq!(
        f.producer_watermark().await,
        Some(high_water),
        "the watermark covers the evidence-free row and the last source"
    );

    // A restart on new connections adds nothing.
    assert_eq!(f.consume().await, 0);
    assert_eq!(f.trusted_events().await, events);
    assert_eq!(f.producer_watermark().await, Some(high_water));
    // A restart from the bootstrap checkpoint re-reads all three rows and
    // changes nothing.
    sqlx::query("SELECT producer_fixture_watermark($1)")
        .bind(bootstrap)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(f.consume().await, 0);
    assert_eq!(f.trusted_events().await, events);
    assert_eq!(f.producer_watermark().await, Some(high_water));
    // The demo row keeps its shape: two demo participants, public marker, no evidence.
    let demo = f.sequenced_after(after_first).await;
    assert_eq!(demo[0]["payload"]["_public_market_invalidation"], true);
    assert_eq!(demo[0]["participants"].as_array().unwrap().len(), 2);
    assert_no_internal_identity(&demo[0].to_string(), None, &hidden);
}

/// The commit boundary of the sequencer. While a Rust cancellation is open
/// after its outbox insert, its order change is still uncommitted, another
/// session sees no outbox row, and the sequencer statement, with no other row
/// pending, numbers nothing and returns at once. After the commit the order
/// change and the row become visible together; the row is numbered once and
/// reaches the organization channel once, also past the next poll, with the
/// public marker as a public invalidation. The unchanged Python dispatcher, as
/// sequencer and hub for a second cancellation and as replay for both, maps
/// the same rows without the internal evidence column, once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_commit_is_invisible_to_the_dispatcher_until_commit_and_dispatched_once() {
    let mut f = Fixture::new().await;
    f.chain().await;
    let [later, _] = month_windows();
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    f.ask(first, 150, 5, "SPOT", None).await;
    f.ask(second, 160, 5, &later, None).await;
    let mut hidden = vec![id(OBSERVER), id(OBSERVER_ORG)];
    for (order, window) in [(first, "SPOT"), (second, later.as_str())] {
        let (pin, pin_list) = f.watch("PIN", Some(order), window).await;
        hidden.extend([pin, pin_list]);
    }
    let (state, runtime) = f.stream_app().await;
    let hub = state.streams.as_ref().unwrap().hub.clone();
    let mut seller_channel = hub.subscribe_org(id(SELLER_ORG)).unwrap();
    let mut prices = hub.subscribe_public(PublicChannel::Prices).unwrap();

    // Hold the cancellation after its outbox insert, before its commit.
    let mut barrier = PgConnection::connect(&f.admin_url).await.unwrap();
    sqlx::query("SELECT set_config('application_name',$1,false)")
        .bind(CHAIN_BARRIER_APP)
        .execute(&mut barrier)
        .await
        .unwrap();
    sqlx::query("SELECT chain_hold()")
        .execute(&mut barrier)
        .await
        .unwrap();
    let graph = WaitGraph::start(&f.admin_url).await;
    let held = f.spawn(CHAIN_HELD_APP, f.cancel(first, SELLER)).await;
    f.wait_lock(CHAIN_HELD_APP, CHAIN_BARRIER_APP).await;
    let observed = Instant::now();
    // The held transaction is the cancellation itself: it has changed the
    // order row (its transaction id is the row's xmax) and not committed it.
    let open: Value = sqlx::query_scalar("SELECT chain_open_change($1,$2)")
        .bind(first)
        .bind(CHAIN_HELD_APP)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        open,
        json!({"status":"OPEN","remaining_untouched":true,"changed_by_held":true}),
        "the order change is open in the held transaction"
    );
    assert!(
        f.outbox(first).await.is_empty(),
        "another session sees no row"
    );
    let mut observer = PgConnection::connect(&f.runtime_url).await.unwrap();
    let visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE aggregate_id=$1")
            .bind(first.to_string())
            .fetch_one(&mut observer)
            .await
            .unwrap();
    assert_eq!(visible, 0, "the runtime role sees no row");
    let mut tx = observer.begin().await.unwrap();
    let numbered = sqlx::query(ASSIGN_SQL)
        .bind(1000_i64)
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    tx.rollback().await.unwrap();
    assert_eq!(
        numbered, 0,
        "with no other row pending, the sequencer statement numbers nothing and returns"
    );
    assert_eq!(f.unsequenced().await, 0);
    let release_ms = observed.elapsed();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT chain_release()")
            .fetch_one(&mut barrier)
            .await
            .unwrap()
    );
    assert!(release_ms < RELEASE_BUDGET, "held {release_ms:?}");
    println!("CHAIN_HELD_OBSERVED held_ms={}", release_ms.as_millis());
    expect_status(&held.await.unwrap(), StatusCode::NO_CONTENT);
    let committed = Instant::now();
    let contention = graph.stop().await;
    contention.assert_no_cycle("held cancellation");
    // One snapshot: the order change and its outbox row commit together.
    let visible: Value = sqlx::query_scalar("SELECT chain_committed_change($1)")
        .bind(first)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(visible, json!({"status":"CANCELLED","outbox_rows":1}));
    barrier.close().await.unwrap();
    observer.close().await.unwrap();

    // After the commit: one row, one sequence, one delivery.
    let delivery = tokio::time::timeout(POLL_BOUND, seller_channel.recv())
        .await
        .expect("the committed row is delivered within one poll")
        .expect("an open channel");
    println!(
        "CHAIN_RUST_DELIVERY after_commit_ms={}",
        committed.elapsed().as_millis()
    );
    let rows = f.outbox(first).await;
    assert_eq!(
        rows.len(),
        1,
        "one row for the cancellation: {}",
        rows.len()
    );
    let row = &rows[0];
    assert_eq!(row["participants"], json!([id(SELLER_ORG)]));
    assert_eq!(row["payload"]["_public_market_invalidation"], true);
    assert_eq!(row["attempts"], 1, "numbered once");
    assert_eq!(row["dispatched"], true);
    assert!(row["mutation"].is_string());
    assert_eq!(
        row["targets"].as_array().map(Vec::len),
        Some(1),
        "the internal column keeps the captured PIN"
    );
    let Delivery::Private(event) = delivery else {
        panic!("a private delivery first: {delivery:?}");
    };
    assert_eq!(event.seq(), seq_of(row));
    assert_eq!(event.event_type(), row["event_type"].as_str().unwrap());
    assert_eq!(event.payload(), &delivered(&row["payload"]));
    assert_eq!(event.participants().to_vec(), vec![id(SELLER_ORG)]);
    assert_no_internal_identity(&event.payload().to_string(), Some(event.payload()), &hidden);
    assert!(matches!(
        tokio::time::timeout(FRAME_BOUND, prices.recv()).await,
        Ok(Some(Delivery::PublicInvalidation))
    ));
    assert!(
        tokio::time::timeout(PAST_NEXT_POLL, seller_channel.recv())
            .await
            .is_err(),
        "one delivery for one row, also after the next poll"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), prices.recv())
            .await
            .is_err(),
        "one public invalidation for one row"
    );
    drop((seller_channel, prices));
    f.stop_streams(runtime).await;
    assert_eq!(f.sequencer_holder().await, None);
}

/// Private stream frames of rows that carry captured watchlist targets: both
/// economic parties and an unrelated observer holding a PIN and a SLICE target
/// on the traded order, live and with an explicit replay cursor. Frames carry
/// exactly the stored participant payload, never the evidence column or a
/// watcher identity; the observer receives none; public frames keep the exact
/// invalidation shape. The internal column keeps the captured target set, and
/// the producer derives the observer's events from it.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn private_stream_frames_never_carry_producer_evidence_live_or_replayed() {
    let mut f = Fixture::new().await;
    f.chain().await;
    // The watermark exists before the first trusted source; the publication
    // is numbered before the streams start and before any target exists.
    f.bootstrap().await;
    let listing = f.listing().await;
    assert!(f.sequence().await >= 1);
    let (pin, pin_list) = f.watch("PIN", Some(listing), "SPOT").await;
    let (slice, slice_list) = f.watch("SLICE", None, "SPOT").await;
    let hidden = [
        id(OBSERVER),
        id(OBSERVER_ORG),
        pin,
        pin_list,
        slice,
        slice_list,
    ];
    let start = f.high_water().await;
    let (state, runtime) = f.stream_app().await;
    let router = stream_routes().with_state(StreamsState {
        state: Arc::new(state),
    });
    let parties = [
        (BUYER, BUYER_ORG),
        (SELLER, SELLER_ORG),
        (OBSERVER, OBSERVER_ORG),
    ];
    let mut live = Vec::new();
    for (user, organization) in parties {
        live.push(Frames::open(&router, &trades_path(user, organization), None).await);
    }
    let mut public = Vec::new();
    for path in ["/api/stream/prices", "/api/stream/orderbook"] {
        public.push(Frames::open(&router, path, None).await);
    }

    let take = f.take(listing, "10").await;
    expect_status(&f.call(take).await, StatusCode::OK);
    expect_status(
        &f.call(f.cancel(listing, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    f.wait_sequenced().await;
    let rows = f.sequenced_after(start).await;
    let mutations: BTreeSet<&str> = rows
        .iter()
        .map(|row| {
            row["mutation"]
                .as_str()
                .expect("every row carries evidence")
        })
        .collect();
    assert_eq!(mutations.len(), 2, "the take and the cancellation");
    // The internal column keeps the exact captured set: the observer's PIN
    // and SLICE target on the traded slice.
    let captured = json!([
        {"target_id":pin,"watchlist_id":pin_list,"observer_id":id(OBSERVER),"target_type":"PIN"},
        {"target_id":slice,"watchlist_id":slice_list,"observer_id":id(OBSERVER),"target_type":"SLICE"},
    ]);
    for row in &rows {
        let mut targets: Vec<Value> = row["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| {
                json!({"target_id":target["target_id"],"watchlist_id":target["watchlist_id"],
                    "observer_id":target["observer_id"],"target_type":target["target_type"]})
            })
            .collect();
        targets.sort_by_key(|target| target["target_type"].as_str().unwrap().to_owned());
        assert_eq!(Value::Array(targets), captured, "captured set of a row");
    }
    let expected = |organization: u128| -> Vec<Business> {
        rows.iter()
            .filter(|row| {
                row["participants"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(id(organization)))
            })
            .map(|row| {
                (
                    seq_of(row),
                    row["event_type"].as_str().unwrap().to_owned(),
                    delivered(&row["payload"]),
                )
            })
            .collect()
    };
    assert!(expected(OBSERVER_ORG).is_empty());
    let mut frame_texts = Vec::new();
    let mut live_frames = Vec::new();
    for (frames, (_, organization)) in live.iter_mut().zip(parties).take(2) {
        let wanted = expected(organization);
        assert!(!wanted.is_empty());
        let (found, texts) = business_frames(frames, wanted.len()).await;
        assert_eq!(
            found, wanted,
            "live frames are the stored participant payloads"
        );
        println!(
            "CHAIN_PARTY organization={} events={:?}",
            id(organization),
            found
                .iter()
                .map(|(seq, event, _)| (*seq, event.clone()))
                .collect::<Vec<_>>()
        );
        live_frames.push(found);
        frame_texts.extend(texts);
    }
    // Past the next poll no party receives a repeat, and the observer
    // receives heartbeats only. Later deliveries queue in each body, so the
    // first wait covers the poll for all three streams.
    frame_texts.extend(assert_no_business(&mut live[0], PAST_NEXT_POLL).await);
    for frames in &mut live[1..] {
        frame_texts.extend(assert_no_business(frames, Duration::from_millis(200)).await);
    }
    for frames in &mut public {
        let mut invalidations = 0;
        while let Some(frame) = frames.next_within(Duration::from_secs(1)).await {
            if !is_heartbeat(&frame) {
                assert_eq!(frame_fields(&frame).len(), 2, "no id on a public frame");
                assert_eq!(frame_value(&frame, "event").unwrap(), "market_invalidated");
                assert_eq!(
                    serde_json::from_str::<Value>(&frame_value(&frame, "data").unwrap()).unwrap(),
                    json!({"schema_version":1,"resync_required":true})
                );
                invalidations += 1;
            }
            frame_texts.push(frame);
        }
        assert!(invalidations >= 1, "the public marker invalidates");
    }
    drop((live, public));

    // Explicit-cursor replay gives the same frames, then the heartbeat.
    for ((user, organization), live) in parties.iter().zip(&live_frames) {
        let mut replay =
            Frames::open(&router, &trades_path(*user, *organization), Some(start)).await;
        let mut found = Vec::new();
        loop {
            let frame = replay.next_within(FRAME_BOUND).await.unwrap();
            if is_heartbeat(&frame) {
                frame_texts.push(frame);
                break;
            }
            found.push(business(&frame));
            frame_texts.push(frame);
        }
        assert_eq!(&found, live, "replay equals live delivery");
    }
    let mut observer_replay =
        Frames::open(&router, &trades_path(OBSERVER, OBSERVER_ORG), Some(start)).await;
    let first = observer_replay.next_within(FRAME_BOUND).await.unwrap();
    assert!(is_heartbeat(&first), "the observer replays no event");
    frame_texts.push(first);
    drop(observer_replay);
    for text in &frame_texts {
        let payload = frame_value(text, "data").and_then(|data| serde_json::from_str(&data).ok());
        assert_no_internal_identity(text, payload.as_ref(), &hidden);
    }
    println!("CHAIN_FRAMES checked={}", frame_texts.len());
    f.stop_streams(runtime).await;

    // The producer derives the observer's events from the captured set.
    f.consume().await;
    let events = f.trusted_events().await;
    let classes: BTreeSet<(Uuid, String)> = events
        .iter()
        .map(|event| {
            (
                event["target"].as_str().unwrap().parse().unwrap(),
                event["class"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    println!("CHAIN_OBSERVER_EVENTS {classes:?}");
    let cancellation = rows
        .iter()
        .find(|row| row["event_type"] == "order_cancelled")
        .expect("the cancellation row")["mutation"]
        .as_str()
        .unwrap();
    let take = mutations
        .iter()
        .find(|mutation| **mutation != cancellation)
        .unwrap();
    let derived: BTreeSet<(Uuid, String, String)> = events
        .iter()
        .map(|event| {
            (
                event["target"].as_str().unwrap().parse().unwrap(),
                event["class"].as_str().unwrap().to_owned(),
                event["source"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: BTreeSet<(Uuid, String, String)> = [
        (pin, "PIN_PARTIALLY_FILLED", *take),
        (pin, "PIN_WITHDRAWN", cancellation),
        (slice, "SLICE_BEST_PRICE_MOVED", cancellation),
        (slice, "SLICE_WENT_QUIET", cancellation),
    ]
    .into_iter()
    .map(|(target, class, source)| (target, class.to_owned(), source.to_owned()))
    .collect();
    assert_eq!(
        derived, expected,
        "the observer's events from the captured set"
    );
}

/// The frame check itself refuses an evidence key, a subscription key and a
/// watcher identity, and accepts the plain participant payload.
#[test]
fn internal_identity_check_refuses_evidence_and_watcher_identities() {
    let watcher = Uuid::new_v4();
    let plain = json!({"id":Uuid::new_v4(),"side":"ASK","schema_version":1});
    assert_no_internal_identity(&plain.to_string(), Some(&plain), &[watcher]);
    for leaked in [
        json!({"id":1,"producer_evidence":{"version":1}}),
        json!({"id":1,"nested":[{"watchlist_id":watcher}]}),
        json!({"id":1,"label":watcher.to_string()}),
    ] {
        let refused = std::panic::catch_unwind(|| {
            assert_no_internal_identity(&leaked.to_string(), Some(&leaked), &[watcher])
        });
        assert!(refused.is_err(), "not refused: {leaked}");
    }
    assert_eq!(
        delivered(&json!({"id":1,"_public_market_invalidation":true})),
        json!({"id":1,"schema_version":1})
    );
}

// ---- the Rust demo job against the Python demo writer ---------------------------

use verdaxis_read::cli::jobs::demo::model::{
    CoverageOrder, DEMO_SEED_BUYERS, DEMO_SEED_SUPPLIERS, build_coverage,
};
use verdaxis_read::cli::jobs::demo::{DemoFailure, DemoReport, PolicyCatalog, run_with_coverage};
use verdaxis_read::order_write::{DemoMarketFailure, TickOutcome};
use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;

/// The B30 Rotterdam book at `now`: the slice the Python driver limits its
/// coverage to. Organization and creation time are not managed fields, so
/// the ordinal offset of the full book changes nothing a refresh compares.
fn fixture_coverage(now: chrono::DateTime<chrono::Utc>) -> Vec<CoverageOrder> {
    let policy = verdaxis_read::runtime::policy::committed_policy().unwrap();
    let catalog = PolicyCatalog::covering(&policy).expect("the policy covers the demo book");
    let rows: Vec<CoverageOrder> =
        build_coverage(now, &catalog, &DEMO_SEED_BUYERS, &DEMO_SEED_SUPPLIERS)
            .into_iter()
            .filter(|order| order.product == "B30" && order.port == "Rotterdam")
            .collect();
    assert!(!rows.is_empty());
    assert_eq!(rows[0].product_id.to_string(), PRODUCT);
    rows
}

/// A whole-second instant, so Python's `fromisoformat` and Rust agree.
fn demo_instant() -> chrono::DateTime<chrono::Utc> {
    use chrono::SubsecRound;
    chrono::Utc::now().trunc_subsecs(0)
}

impl Fixture {
    async fn runtime_pool(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&self.runtime_url)
            .await
            .unwrap()
    }
    async fn rust_demo(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        wanted: Vec<CoverageOrder>,
    ) -> (DemoReport, Result<(), DemoFailure>) {
        let pool = self.runtime_pool().await;
        let mut report = DemoReport::default();
        let result = run_with_coverage(&pool, now, wanted, &mut report).await;
        pool.close().await;
        (report, result)
    }
    /// Every coverage row by key, with the fields a refresh owns.
    async fn coverage_book(&mut self) -> Value {
        sqlx::query_scalar(
            "SELECT coalesce(jsonb_object_agg(idempotency_key, jsonb_build_object( \
               'id', id, 'organization_id', organization_id, 'ordinal', acceptance_ordinal, \
               'updated_at', updated_at, 'status', status, 'price', trim_scale(price_per_mt_usd), \
               'quantity', trim_scale(quantity_mt), 'remaining', trim_scale(remaining_quantity_mt), \
               'certifications', certifications::jsonb, 'scheme', certification_scheme, \
               'expires_at', expires_at, 'hash', idempotency_request_hash, \
               'creation_method', creation_method, 'verified', is_verdaxis_verified)), '{}') \
             FROM orderbook_orders WHERE idempotency_operation = 'DEMO_COVERAGE'",
        )
        .fetch_one(&mut self.admin)
        .await
        .unwrap()
    }
    /// The demo_market_changed outbox rows, oldest first, without identity
    /// or time.
    async fn demo_events(&mut self) -> Vec<Value> {
        sqlx::query_scalar(
            "SELECT jsonb_build_object('event_type', event_type, \
               'aggregate_type', aggregate_type, 'aggregate_id', aggregate_id, \
               'participants', to_jsonb(participant_org_ids), 'payload', to_jsonb(payload), \
               'evidence', to_jsonb(producer_evidence)) \
             FROM market_event_outbox WHERE event_type = 'demo_market_changed' \
             ORDER BY created_at, id",
        )
        .fetch_all(&mut self.admin)
        .await
        .unwrap()
    }
    /// The demo_market_changed rows of one phase (`coverage`, `prune` or
    /// `tick`); a Rust run also ticks, so phase tests read their own rows.
    async fn phase_events(&mut self, phase: &str) -> Vec<Value> {
        self.demo_events()
            .await
            .into_iter()
            .filter(|event| event["aggregate_id"] == phase)
            .collect()
    }
    async fn admin_sql(&mut self, sql: &str) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
}

fn coverage_event() -> Value {
    json!({
        "event_type": "demo_market_changed",
        "aggregate_type": "demo_market",
        "aggregate_id": "coverage",
        "participants": [DEMO_BUYER_ORG, DEMO_SELLER_ORG],
        "payload": {"operation": "coverage", "schema_version": 1, "_public_market_invalidation": true},
        "evidence": null,
    })
}

/// The same drift on a fresh Python-built book: one row repriced, one
/// filled, one renamed to a key no run wants.
async fn drifted_book(now: chrono::DateTime<chrono::Utc>) -> (Fixture, Value) {
    let mut f = Fixture::new().await;
    f.demo_seed_organizations().await;
    let (report, result) = f.rust_demo(now, fixture_coverage(now)).await;
    result.unwrap();
    assert!(report.market.coverage.created > 0);
    sqlx::query("SELECT residual_demo_coverage_drift()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let book = f.coverage_book().await;
    (f, book)
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_coverage_preserves_order_identity_and_updates_only_managed_fields() {
    // Twin databases with the same drift: Python's refresh on one, Rust's on
    // the other. Counts, row identity, re-queueing and every managed field
    // agree; the repriced and the reopened rows re-queue, the renamed row
    // expires and its key is created again.
    let now = demo_instant();
    let later = now + chrono::Duration::minutes(5);

    let (mut rust_db, rust_before) = drifted_book(now).await;
    let (report, result) = rust_db.rust_demo(later, fixture_coverage(later)).await;
    result.unwrap();
    let rust_after = rust_db.coverage_book().await;

    assert_eq!(report.market.coverage.expired, 1, "{report:?}");
    assert!(report.market.coverage.refreshed >= 2, "{report:?}");
    let retired = &rust_after["B30|Rotterdam|2001-Q1|BID|0"];
    assert_eq!(retired["status"], "EXPIRED");
    assert_eq!(
        retired["id"],
        rust_before["B30|Rotterdam|2001-Q1|BID|0"]["id"]
    );
    rust_db.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_system_admission_refuses_a_seed_organization_that_is_not_approved() {
    // One seed organization loses its approval: the product's coverage
    // transaction refuses before any write, and no event is staged.
    let mut f = Fixture::new().await;
    f.demo_seed_organizations().await;
    f.admin_sql(
        "UPDATE organizations SET verification_status = 'PENDING' \
         WHERE id = '79609f48-0a3e-560e-a1e1-63d90601d84a'",
    )
    .await;
    let before = f.snapshot().await;
    let (report, result) = f
        .rust_demo(demo_instant(), fixture_coverage(demo_instant()))
        .await;
    assert!(
        matches!(
            result,
            Err(DemoFailure::Market(DemoMarketFailure::Coverage(_)))
        ),
        "{result:?}"
    );
    assert_eq!(report.market.coverage_products, 0);
    assert_eq!(f.snapshot().await, before, "a refused phase writes nothing");
    assert!(f.demo_events().await.is_empty());
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_provisioning_refuses_a_real_row_with_an_activity_id() {
    let mut f = Fixture::new().await;
    // A REAL row can only arrive by an operator migration; the fixture
    // writes it with triggers off.
    f.admin_sql(&format!(
        "BEGIN; SET LOCAL session_replication_role = replica; \
         INSERT INTO organizations(id,name,type,verification_status,provenance) \
         VALUES('{DEMO_BUYER_ORG}','Real Company','FUEL_BUYER','APPROVED','REAL'); COMMIT;"
    ))
    .await;
    let before = f.snapshot().await;
    let (_, result) = f.rust_demo(demo_instant(), Vec::new()).await;
    assert!(
        matches!(
            result,
            Err(DemoFailure::Provision(
                verdaxis_read::cli::jobs::demo::ProvisionError::NotDemo
            ))
        ),
        "{result:?}"
    );
    let provenance: String =
        sqlx::query_scalar("SELECT provenance::text FROM organizations WHERE id = $1::uuid")
            .bind(DEMO_BUYER_ORG)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(provenance, "REAL", "a REAL row is never converted");
    assert_eq!(f.snapshot().await, before);

    // On an empty database provisioning creates both, then writes nothing.
    let f = Fixture::new().await;
    let (report, result) = f.rust_demo(demo_instant(), Vec::new()).await;
    result.unwrap();
    assert!(report.organizations_written);
    let (report, result) = f.rust_demo(demo_instant(), Vec::new()).await;
    result.unwrap();
    assert!(
        !report.organizations_written,
        "an exact row is not rewritten"
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_prune_respects_customer_pins_and_history() {
    // Two aged activity trades: Python's and Rust's prune agree on what goes.
    // A pinned, suggested and commissioned trade keeps itself and its orders;
    // the other trade and its orders go, with one prune event.
    let mut counts = Vec::new();
    for _ in [true] {
        let mut f = Fixture::new().await;
        f.demo_organizations().await;
        let ticks = f.demo_ticks(2).await;
        for (now, _) in &ticks {
            assert_eq!(ok(&f.demo_tick(now).await)["created_trades"], 1);
        }
        let shape: Value = sqlx::query_scalar("SELECT residual_demo_shape()")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        let trades = shape["trades"].as_array().unwrap().clone();
        sqlx::query("SELECT residual_age_demo_trades(8)")
            .execute(&mut f.admin)
            .await
            .unwrap();
        sqlx::query("SELECT residual_trade_references($1::uuid)")
            .bind(trades[0]["id"].as_str().unwrap())
            .execute(&mut f.admin)
            .await
            .unwrap();
        let events_before = f.phase_events("prune").await.len();
        let (report, result) = f.rust_demo(demo_instant(), Vec::new()).await;
        result.unwrap();
        let pruned = json!({"trades_pruned":report.market.prune.trades,"orders_pruned":report.market.prune.orders});
        // The aged trades only: a Rust run ticks after the prune.
        let left: Vec<String> = sqlx::query_scalar(
            "SELECT id::text FROM trades WHERE buyer_id = $1::uuid \
             AND created_at < now() - interval '1 day' ORDER BY id",
        )
        .bind(DEMO_BUYER_ORG)
        .fetch_all(&mut f.admin)
        .await
        .unwrap();
        assert_eq!(left, vec![trades[0]["id"].as_str().unwrap().to_owned()]);
        assert_eq!(f.phase_events("prune").await.len(), events_before + 1);
        counts.push((
            pruned["trades_pruned"].clone(),
            pruned["orders_pruned"].clone(),
        ));
        f.ledger().await;
    }
    assert_eq!(counts[0].0, 1);
}

// ---- the Rust demo tick ----------------------------------------------------------

/// Recent ticks whose Rust draw is B30 at Rotterdam (the slice the fixture
/// catalog holds) with a window open for at least six more hours.
fn rust_b30_ticks(count: usize) -> Vec<chrono::DateTime<chrono::Utc>> {
    use verdaxis_read::cli::jobs::demo::model::{activity_tick, tick_choice};
    let now = chrono::Utc::now();
    let mut found = Vec::new();
    let mut tick = activity_tick(now);
    while found.len() < count && now - tick < chrono::Duration::days(6) {
        tick -= chrono::Duration::minutes(5);
        let choice = tick_choice(tick);
        if choice.product == "B30"
            && choice.port == "Rotterdam"
            && choice.expires_at > now + chrono::Duration::hours(6)
        {
            found.push(tick);
        }
    }
    assert_eq!(
        found.len(),
        count,
        "no recent Rust tick draws the B30 Rotterdam slice"
    );
    found
}

async fn tick_times(f: &mut Fixture) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
           'orders', (SELECT jsonb_agg(jsonb_build_object('side', side, 'created_at', created_at, \
                        'updated_at', updated_at) ORDER BY side) \
                      FROM orderbook_orders WHERE organization_id IN ($1::uuid, $2::uuid)), \
           'trades', (SELECT jsonb_agg(jsonb_build_object('created_at', created_at, \
                        'confirmed_at', confirmed_at)) \
                      FROM trades WHERE buyer_id = $1::uuid AND seller_id = $2::uuid))",
    )
    .bind(DEMO_BUYER_ORG)
    .bind(DEMO_SELLER_ORG)
    .fetch_one(&mut f.admin)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_tick_writes_the_documented_pair_once_and_python_sees_it() {
    let mut f = Fixture::new().await;
    f.demo_organizations().await;
    let tick = rust_b30_ticks(1)[0];
    let (report, result) = f
        .rust_demo(tick + chrono::Duration::seconds(30), Vec::new())
        .await;
    result.unwrap();
    assert!(
        matches!(
            report.market.tick,
            Some(TickOutcome::Created { product: "B30", .. })
        ),
        "{report:?}"
    );

    // The documented rows, as the Python tick writes them.
    let shape: Value = sqlx::query_scalar("SELECT residual_demo_shape()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let orders = shape["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 2, "{shape}");
    assert_eq!(orders[0]["side"], "ASK");
    assert_eq!(orders[0]["organization_id"], DEMO_SELLER_ORG);
    assert_eq!(orders[1]["side"], "BID");
    assert_eq!(orders[1]["organization_id"], DEMO_BUYER_ORG);
    assert!(orders[0]["acceptance_ordinal"].as_i64() < orders[1]["acceptance_ordinal"].as_i64());
    for order in orders {
        assert_eq!(order["creation_method"], "SYSTEM");
        assert_eq!(order["provenance"], "DEMO");
        assert_eq!(order["owner_user_id"], Value::Null);
        assert_eq!(order["created_by_actor_user_id"], Value::Null);
        assert_eq!(order["status"], "FILLED");
    }
    let trades = shape["trades"].as_array().unwrap();
    assert_eq!(trades.len(), 1, "{shape}");
    let trade = &trades[0];
    assert_eq!(trade["buyer_user_id"], Value::Null);
    assert_eq!(trade["seller_user_id"], Value::Null);
    assert_eq!(trade["buyer_provenance"], "DEMO");
    assert_eq!(trade["seller_provenance"], "DEMO");
    assert_eq!(trade["confirmed"], true);
    assert_eq!(trade["ask_order_id"], orders[0]["id"]);
    assert_eq!(trade["bid_order_id"], orders[1]["id"]);

    // Python's times: ask tick-8 min, bid tick-7 min, both updated at the
    // tick; the trade created tick-3 min and confirmed tick-2 min.
    let at = |minutes: i64| json!(tick - chrono::Duration::minutes(minutes));
    let times = tick_times(&mut f).await;
    let parse = |value: &Value| {
        chrono::DateTime::parse_from_rfc3339(value.as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let stored = |path: [&str; 2], index: usize| parse(&times[path[0]][index][path[1]]);
    assert_eq!(json!(stored(["orders", "created_at"], 0)), at(8), "{times}");
    assert_eq!(json!(stored(["orders", "created_at"], 1)), at(7), "{times}");
    assert_eq!(json!(stored(["orders", "updated_at"], 0)), at(0), "{times}");
    assert_eq!(json!(stored(["orders", "updated_at"], 1)), at(0), "{times}");
    assert_eq!(json!(stored(["trades", "created_at"], 0)), at(3), "{times}");
    assert_eq!(
        json!(stored(["trades", "confirmed_at"], 0)),
        at(2),
        "{times}"
    );

    // One tick event, Python's shape.
    let mut expected = coverage_event();
    expected["aggregate_id"] = json!("tick");
    expected["payload"]["operation"] = json!("tick");
    assert_eq!(f.demo_events().await, vec![expected]);

    // A rerun of the same tick, by Rust and then by Python, writes nothing.
    let before = f.snapshot().await;
    let (report, result) = f
        .rust_demo(tick + chrono::Duration::seconds(90), Vec::new())
        .await;
    result.unwrap();
    assert_eq!(report.market.tick, Some(TickOutcome::AlreadyGenerated));
    let after = f.snapshot().await;
    assert_eq!(after["trades"], before["trades"]);
    assert_eq!(after["orders"], before["orders"]);
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn concurrent_demo_ticks_give_one_trade() {
    let mut f = Fixture::new().await;
    f.demo_organizations().await;
    let tick = rust_b30_ticks(1)[0] + chrono::Duration::seconds(30);
    let (first, second) =
        tokio::join!(f.rust_demo(tick, Vec::new()), f.rust_demo(tick, Vec::new()));
    let outcomes = [first, second];
    let created = outcomes
        .iter()
        .filter(|(report, _)| matches!(report.market.tick, Some(TickOutcome::Created { .. })))
        .count();
    assert_eq!(created, 1, "{outcomes:?}");
    for (report, result) in &outcomes {
        // The other tick either found the trade under the locks or was bounded
        // by them with no effect.
        let found = report.market.tick == Some(TickOutcome::AlreadyGenerated);
        let bounded = matches!(result, Err(DemoFailure::Market(DemoMarketFailure::Tick(_))));
        let won = matches!(report.market.tick, Some(TickOutcome::Created { .. }));
        assert!(won || found || bounded, "{report:?} {result:?}");
    }
    let trades: i64 = sqlx::query_scalar("SELECT count(*) FROM trades WHERE buyer_id = $1::uuid")
        .bind(DEMO_BUYER_ORG)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(trades, 1);
    f.ledger().await;
}
