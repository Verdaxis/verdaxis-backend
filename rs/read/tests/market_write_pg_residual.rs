#![cfg(feature = "contract-tests")]
//! Coexistence of the Rust market writers with the Python writers that keep
//! running after the route switch: the order-expiry reminder scan and email
//! delivery, the demo activity tick and prune, the operator quarantine and the
//! seed. Each Python writer runs unmodified in a child process
//! (`tests/postgres/phase8_residual_driver.py`) on a fresh clone of the
//! migrated database. Lock waits are observed in `pg_stat_activity`; every
//! observed wait is released at once, and the time until the waiter stops
//! waiting is measured at the database. A wait-graph sampler fails a schedule
//! on any lock cycle it does not specify.
#[path = "support/market_database.rs"]
mod market_database;

use std::collections::BTreeSet;
use std::process::Stdio;
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
use sqlx::{Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
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
const BOUNDED_CODES: [&str; 4] = [
    "MARKET_BUSY",
    "MARKET_TIMEOUT",
    "TRANSACTION_RETRY_EXHAUSTED",
    "MARKET_SLICE_CHANGED",
];
// Statement names the driver pauses after (see `STATEMENTS` in the driver).
/// The scan statement that takes the order row locks.
const SCAN_LOCK: &str = "scan_order_locks";
/// The delivery recheck that takes the recipient and order row locks.
const DELIVERY_LOCK: &str = "delivery_recheck_locks";
/// Python's transaction-scoped advisory lock statement.
const ADVISORY: &str = "advisory_lock";
/// The first order row the seed inserts.
const FIRST_ORDER_INSERT: &str = "first_order_insert";

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
fn asyncpg(url: &str) -> String {
    let mut parsed = url::Url::parse(url).unwrap();
    parsed.set_scheme("postgresql+asyncpg").unwrap();
    parsed.into()
}
fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
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
fn bounded(reply: &Reply) -> bool {
    BOUNDED_CODES.contains(&code(reply))
}
fn expect_status(reply: &Reply, status: StatusCode) {
    assert_eq!(reply.0, status, "{reply:?}");
}

/// One unchanged Python writer in a child process.
struct Driver {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}
impl Driver {
    async fn start(command: Value) -> Self {
        let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
        let database_url = command["database_url"].as_str().unwrap().to_owned();
        let mut child = tokio::process::Command::new(python)
            .arg("tests/postgres/phase8_residual_driver.py")
            .current_dir(repository_root())
            .env("ENVIRONMENT", "test")
            .env("RELEASE_SHA", "test")
            .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
            .env("DATABASE_URL", database_url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("the project Python interpreter runs");
        let mut stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .unwrap();
        stdin.flush().await.unwrap();
        let mut driver = Self {
            child,
            stdin,
            lines,
        };
        driver.line("READY").await;
        driver
    }
    /// The next protocol line that starts with `prefix`; other output is printed.
    async fn line(&mut self, prefix: &str) -> String {
        let deadline = Duration::from_secs(180);
        loop {
            let line = tokio::time::timeout(deadline, self.lines.next_line())
                .await
                .unwrap_or_else(|_| panic!("driver did not print {prefix}"))
                .unwrap()
                .unwrap_or_else(|| panic!("driver exited before {prefix}"));
            if let Some(rest) = line.strip_prefix(prefix) {
                return rest.trim().to_owned();
            }
            println!("DRIVER_OUTPUT {line}");
        }
    }
    async fn write(&mut self, word: &str) {
        self.stdin
            .write_all(format!("{word}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }
    async fn go(&mut self) {
        self.write("GO").await;
    }
    /// The backend pid of the paused writer, after its matching statement ran.
    async fn held(&mut self) -> i32 {
        self.line("HELD").await.parse().unwrap()
    }
    async fn release(&mut self) {
        self.write("RELEASE").await;
    }
    async fn finish(mut self) -> Value {
        let result: Value = serde_json::from_str(&self.line("RESULT").await).unwrap();
        let status = self.child.wait().await.unwrap();
        assert!(status.success(), "driver exit {status}");
        println!("DRIVER_RESULT {result}");
        result
    }
}
fn ok(result: &Value) -> &Value {
    assert_eq!(result["ok"], true, "Python writer failed: {result}");
    &result["result"]
}
fn sqlstate(result: &Value) -> &str {
    assert_eq!(result["ok"], false, "Python writer succeeded: {result}");
    result["error"]["sqlstate"].as_str().unwrap_or_default()
}

struct Fixture {
    _database: Arc<market_database::MarketDatabase>,
    admin: PgConnection,
    admin_url: String,
    runtime_url: String,
    owner_url: String,
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
            sqlx::query_scalar::<_, String>("SELECT version_num FROM alembic_version")
                .fetch_one(&mut admin)
                .await
                .unwrap(),
            "p89_20261022_trade_freeze"
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
            owner_url: owner.into(),
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
        let config = serde_json::from_value(json!({"environment":"test","release_sha":verdaxis_read::compiled_sha(),"migration_revision":"p89_20261022_trade_freeze","database_url":self.runtime_url,"db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":true})).unwrap();
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
    /// A contender released below its lock bound must succeed at once. A
    /// bounded refusal is accepted only as a long hold: the sampled wait graph
    /// has no cycle and the released Python writer never waited on `rust_app`.
    /// It is recorded and the command is repeated once after the writer committed.
    async fn first_attempt(
        &self,
        reply: Reply,
        contention: &Contention,
        rust_app: &str,
        command: Command,
    ) -> Reply {
        contention.assert_no_cycle(rust_app);
        if bounded(&reply) {
            assert!(
                !contention.waited(PYTHON_APP, rust_app),
                "a bounded refusal after the Python writer waited on Rust: {contention:?}"
            );
            println!("LONG_HOLD status={} body={}", reply.0, reply.1);
            return self.call(command).await;
        }
        reply
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

    async fn expire_at(&mut self, order: Uuid, at: chrono::DateTime<chrono::Utc>) {
        sqlx::query("SELECT residual_expire_at($1,$2)")
            .bind(order)
            .bind(at)
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
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
    async fn demo_ask(&mut self, order: Uuid, price: i64, window: &str) {
        sqlx::query("SELECT residual_demo_ask($1,$2,$3)")
            .bind(order)
            .bind(Decimal::from(price))
            .bind(window)
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
    async fn reminders(&mut self) -> Vec<Value> {
        let value: Value = sqlx::query_scalar("SELECT residual_reminders()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
        value.as_array().unwrap().clone()
    }
    async fn reminders_for(&mut self, order: Uuid) -> Vec<Value> {
        self.reminders()
            .await
            .into_iter()
            .filter(|notice| notice["order_id"] == json!(order.to_string()))
            .collect()
    }
    async fn snapshot(&mut self) -> Value {
        sqlx::query_scalar("SELECT residual_snapshot()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
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
    async fn fast_deadlock_detection(&mut self) {
        sqlx::query("SELECT residual_fast_deadlock_detection()")
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
    async fn deadlocks(&mut self) -> i64 {
        sqlx::query("SELECT pg_stat_clear_snapshot()")
            .execute(&mut self.admin)
            .await
            .unwrap();
        sqlx::query_scalar("SELECT residual_deadlocks()")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn probe_organization(&mut self, organization: &str) -> String {
        sqlx::query_scalar("SELECT residual_probe_organization($1::uuid)")
            .bind(organization)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
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
    /// Poll until `waiter` waits on a heavyweight lock held by any of `blockers`.
    async fn wait_lock_any(&mut self, waiter: &str, blockers: &[&str]) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT residual_waiting_on_any($1,$2)")
                .bind(waiter)
                .bind(blockers)
                .fetch_one(&mut self.admin)
                .await
                .unwrap();
            if let Some(pid) = pid {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "{waiter} did not wait for {blockers:?}"
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
    /// A session advisory barrier that Rust commands named `RUST_HELD_APP` meet
    /// at their first effect row, after they took their economic locks.
    async fn barrier(&self) -> PgConnection {
        let mut barrier = PgConnection::connect(&self.admin_url).await.unwrap();
        sqlx::query("SELECT set_config('application_name',$1,false)")
            .bind(BARRIER_APP)
            .execute(&mut barrier)
            .await
            .unwrap();
        sqlx::query("SELECT residual_hold()")
            .execute(&mut barrier)
            .await
            .unwrap();
        barrier
    }
    /// Observe `waiter` blocked by the held Rust command, then release it at once.
    async fn observe_release(barrier: &mut PgConnection, waiter: &str) -> Value {
        let observation: Value = sqlx::query_scalar("SELECT residual_observe_release($1,$2)")
            .bind(waiter)
            .bind(RUST_HELD_APP)
            .fetch_one(barrier)
            .await
            .unwrap();
        assert!(
            observation["release_ms"].as_f64().unwrap() < RELEASE_BUDGET.as_millis() as f64,
            "release must retain the lock budget: {observation}"
        );
        println!("OBSERVED_WAIT waiter={waiter} blocker={RUST_HELD_APP} {observation}");
        observation
    }

    // ---- Python side -----------------------------------------------------

    fn command(&self, op: &str) -> Value {
        json!({"op":op,"database_url":asyncpg(&self.runtime_url),"application_name":PYTHON_APP})
    }
    fn owner_command(&self, op: &str) -> Value {
        json!({"op":op,"database_url":asyncpg(&self.owner_url),"application_name":PYTHON_APP})
    }
    async fn python(&self, command: Value) -> Value {
        let mut driver = Driver::start(command).await;
        driver.go().await;
        driver.finish().await
    }
    /// Recent demo activity ticks whose seeded slice is B30 at Rotterdam.
    async fn demo_ticks(&self, count: usize) -> Vec<(String, String)> {
        let mut command = self.command("demo_find_ticks");
        command["args"] = json!({"product":"B30","port":"Rotterdam","count":count});
        let result = self.python(command).await;
        let ticks: Vec<(String, String)> = ok(&result)["ticks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["now"].as_str().unwrap().to_owned(),
                    t["window"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            ticks.len(),
            count,
            "no recent tick seeds the B30 Rotterdam slice"
        );
        ticks
    }
    async fn demo_tick(&self, now: &str) -> Value {
        let mut command = self.command("demo_tick");
        command["now"] = json!(now);
        self.python(command).await
    }
}

/// Release a paused Python writer and return how long, measured at the
/// database, `waiter` still waited on `blocker` after the wait was observed.
async fn release_and_time(
    f: &mut Fixture,
    driver: &mut Driver,
    observed: Instant,
    waiter: &str,
    blocker: &str,
) -> Duration {
    driver.release().await;
    loop {
        let pid: Option<i32> = sqlx::query_scalar("SELECT residual_waiting($1,$2)")
            .bind(waiter)
            .bind(blocker)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        if pid.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let held = observed.elapsed();
    println!(
        "RELEASED_WAIT waiter={waiter} blocker={blocker} held_ms={}",
        held.as_millis()
    );
    held
}

/// The lock waits sampled among this suite's backends during one schedule.
#[derive(Debug, Default)]
struct Contention {
    edges: BTreeSet<(String, String)>,
    cycle: Option<Vec<(String, String)>>,
}
impl Contention {
    fn waited(&self, waiter: &str, blocker: &str) -> bool {
        self.edges
            .contains(&(waiter.to_owned(), blocker.to_owned()))
    }
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

fn in_hours(hours: i64) -> chrono::DateTime<chrono::Utc> {
    use chrono::Timelike;
    (chrono::Utc::now() + chrono::Duration::hours(hours))
        .with_nanosecond(0)
        .unwrap()
}

// ---- order-expiry reminders ----------------------------------------------

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_scan_row_locks_bound_a_rust_cancel() {
    // Released early, the scan meets a lock cycle: its notice insert needs a
    // key-share lock on the recipient's user row, which the waiting Rust
    // cancellation (the order's owner) already holds FOR UPDATE, while Rust
    // waits on the order row the scan holds. Rust's 500 ms bound ends the
    // cycle before the deadlock detector (one second): Rust answers the
    // retryable busy code with no effect, the scan commits, a retry succeeds.
    // Held past the bound, the scan never waits on Rust: a plain long hold.
    for released_early in [true, false] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        f.expire_at(order, in_hours(6)).await;
        let before = f.snapshot().await;
        let mut command = f.command("reminder_scan");
        command["pause"] = json!(SCAN_LOCK);
        let mut scan = Driver::start(command).await;
        scan.go().await;
        let python = scan.held().await;
        let graph = WaitGraph::start(&f.admin_url).await;
        let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
        let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
        println!(
            "OBSERVED_WAIT waiter=rust:{rust} blocker=python-scan:{python} released_early={released_early}"
        );
        if released_early {
            scan.release().await;
            let reverse = f.wait_lock(PYTHON_APP, RUST_APP).await;
            let waits: Value = sqlx::query_scalar("SELECT residual_lock_waits($1)")
                .bind(PYTHON_APP)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
            println!("REVERSE_WAIT python-scan:{reverse} waits={waits}");
            assert!(
                waits
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|wait| wait["relation"] == "users"),
                "the scan waits on the recipient's user row: {waits}"
            );
        }
        let reply = cancel.await.unwrap();
        expect_status(&reply, StatusCode::CONFLICT);
        assert_eq!(code(&reply), "MARKET_BUSY");
        if !released_early {
            scan.release().await;
        }
        assert_eq!(ok(&scan.finish().await)["created"], 1);
        let contention = graph.stop().await;
        if released_early {
            let cycle = contention.cycle.clone().expect("the cycle was observed");
            assert!(cycle.contains(&(RUST_APP.to_owned(), PYTHON_APP.to_owned())));
            assert!(cycle.contains(&(PYTHON_APP.to_owned(), RUST_APP.to_owned())));
        } else {
            contention.assert_no_cycle("scan held past the bound");
            assert!(!contention.waited(PYTHON_APP, RUST_APP));
        }
        // The refused cancellation left no effect; the scan's notice is the
        // only new row.
        let refused = f.snapshot().await;
        for key in [
            "orders",
            "inventory",
            "audit_count",
            "outbox_count",
            "receipt_count",
        ] {
            assert_eq!(refused[key], before[key], "{key} after the refusal");
        }
        assert_eq!(f.order(order).await["status"], "OPEN");
        expect_status(
            &f.call(f.cancel(order, SELLER)).await,
            StatusCode::NO_CONTENT,
        );
        let after = f.snapshot().await;
        assert_eq!(
            after["audit_count"].as_i64().unwrap(),
            before["audit_count"].as_i64().unwrap() + 1,
            "one cancellation audit row"
        );
        assert_eq!(f.order(order).await["status"], "CANCELLED");
        assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
        assert_eq!(f.reminders_for(order).await.len(), 1);
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_cancel_row_lock_makes_the_reminder_scan_wait_then_skip_the_order() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    f.expire_at(order, in_hours(6)).await;
    let other = Uuid::new_v4();
    f.ask(other, 150, 5, "SPOT", Some(in_hours(10))).await;
    let mut scan = Driver::start(f.command("reminder_scan")).await;
    let mut barrier = f.barrier().await;
    let cancel = f.spawn(RUST_HELD_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_HELD_APP, BARRIER_APP).await;
    scan.go().await;
    Fixture::observe_release(&mut barrier, PYTHON_APP).await;
    expect_status(&cancel.await.unwrap(), StatusCode::NO_CONTENT);
    assert_eq!(ok(&scan.finish().await)["created"], 1);
    assert!(
        f.reminders_for(order).await.is_empty(),
        "no notice for a cancelled order"
    );
    assert_eq!(f.reminders_for(other).await.len(), 1);
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        0
    );
    assert_eq!(f.reminders().await.len(), 1);
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_scan_and_rust_crossing_create_have_a_bounded_outcome_under_opposite_row_order() {
    let mut f = Fixture::new().await;
    f.fast_deadlock_detection().await;
    // A expires first but has the greater UUID and the higher price: the scan
    // locks A then B, the Rust matcher locks B then A.
    let first = Uuid::from_u128(0xF000_0000_0000_0000_0000_0000_0000_00A1);
    let second = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_00B1);
    f.ask(first, 100, 1, "SPOT", Some(in_hours(5))).await;
    f.ask(second, 99, 1, "SPOT", Some(in_hours(7))).await;
    let mut holder = PgConnection::connect(&f.admin_url).await.unwrap();
    sqlx::raw_sql("SELECT set_config('application_name','residual-holder',false); BEGIN")
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::query("SELECT residual_hold_order($1)")
        .bind(first)
        .execute(&mut holder)
        .await
        .unwrap();
    let mut scan = Driver::start(f.command("reminder_scan")).await;
    let graph = WaitGraph::start(&f.admin_url).await;
    scan.go().await;
    f.wait_lock(PYTHON_APP, HOLDER_APP).await;
    let create = f.spawn(RUST_APP, f.bid("2", "110", "SPOT", None)).await;
    // Rust holds B and queues for A behind the waiting scan.
    f.wait_lock_any(RUST_APP, &[HOLDER_APP, PYTHON_APP]).await;
    let observed = Instant::now();
    sqlx::raw_sql("COMMIT").execute(&mut holder).await.unwrap();
    assert!(
        observed.elapsed() < RELEASE_BUDGET,
        "holder release took too long"
    );
    let rust = create.await.unwrap();
    let python = scan.finish().await;
    // A victim's deadlock is counted when its backend reports statistics.
    let mut deadlocks = f.deadlocks().await;
    let deadline = Instant::now() + Duration::from_secs(15);
    while deadlocks == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        deadlocks = f.deadlocks().await;
    }
    println!("OPPOSITE_ORDER_DEADLOCKS detected={deadlocks}");
    let contention = graph.stop().await;
    let cycle = contention
        .cycle
        .clone()
        .expect("the opposite row orders formed a cycle");
    assert!(cycle.contains(&(RUST_APP.to_owned(), PYTHON_APP.to_owned())));
    assert!(cycle.contains(&(PYTHON_APP.to_owned(), RUST_APP.to_owned())));
    assert!(deadlocks >= 1, "the deadlock detector broke the cycle");
    let python_victim = python["ok"] == false;
    if python_victim {
        assert_eq!(
            sqlstate(&python),
            "40P01",
            "only a deadlock may abort the scan"
        );
        assert!(
            f.reminders().await.is_empty(),
            "an aborted scan commits no notice"
        );
    } else {
        // The scan committed: both orders were eligible when it locked them.
        assert_eq!(ok(&python)["created"], 2);
        for order in [first, second] {
            assert_eq!(f.reminders_for(order).await.len(), 1, "{order}");
        }
    }
    assert!(
        rust.0.is_success() || bounded(&rust),
        "Rust create must succeed or refuse within its bound: {rust:?}"
    );
    println!(
        "OPPOSITE_ORDER_OUTCOME rust_status={} rust_code={} python_ok={} python_created={} python_error={}",
        rust.0,
        code(&rust),
        python["ok"],
        python["result"]["created"],
        python["error"]
    );
    let rerun = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
    println!("OPPOSITE_ORDER_RESCAN created={rerun}");
    assert_eq!(rerun, 0, "no second notice for an order and expiry");
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_delivery_discards_the_notice_of_a_cancelled_order() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    f.expire_at(order, in_hours(6)).await;
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        1
    );
    assert_eq!(f.reminders_for(order).await[0]["status"], "pending");
    let mut delivery = Driver::start(f.command("reminder_delivery")).await;
    let mut barrier = f.barrier().await;
    let cancel = f.spawn(RUST_HELD_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_HELD_APP, BARRIER_APP).await;
    delivery.go().await;
    Fixture::observe_release(&mut barrier, PYTHON_APP).await;
    expect_status(&cancel.await.unwrap(), StatusCode::NO_CONTENT);
    let result = delivery.finish().await;
    assert_eq!(ok(&result)["sent"], 0);
    assert_eq!(ok(&result)["sender_calls"], json!([]));
    let notice = &f.reminders_for(order).await[0];
    assert_eq!(notice["status"], "discarded");
    let mut keys: Vec<&str> = notice["data"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "_email_only",
            "_email_status",
            "event",
            "expires_at",
            "expiry_epoch",
            "order_id",
            "order_key"
        ]
    );
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_delivery_claim_bounds_a_rust_cancel() {
    for above_budget in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        f.expire_at(order, in_hours(6)).await;
        assert_eq!(
            ok(&f.python(f.command("reminder_scan")).await)["created"],
            1
        );
        let before = f.snapshot().await;
        let mut command = f.command("reminder_delivery");
        command["pause"] = json!(DELIVERY_LOCK);
        let mut delivery = Driver::start(command).await;
        delivery.go().await;
        let python = delivery.held().await;
        let graph = WaitGraph::start(&f.admin_url).await;
        let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
        let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
        let observed = Instant::now();
        println!(
            "OBSERVED_WAIT waiter=rust:{rust} blocker=python-delivery:{python} above_budget={above_budget}"
        );
        if above_budget {
            let reply = cancel.await.unwrap();
            expect_status(&reply, StatusCode::CONFLICT);
            assert_eq!(code(&reply), "MARKET_BUSY");
            assert_eq!(
                f.snapshot().await,
                before,
                "a bounded refusal has no effect"
            );
            delivery.release().await;
            let result = delivery.finish().await;
            assert_eq!(ok(&result)["sent"], 1);
            graph
                .stop()
                .await
                .assert_no_cycle("delivery held past the bound");
            expect_status(
                &f.call(f.cancel(order, SELLER)).await,
                StatusCode::NO_CONTENT,
            );
        } else {
            let held =
                release_and_time(&mut f, &mut delivery, observed, RUST_APP, PYTHON_APP).await;
            let first = cancel.await.unwrap();
            let result = delivery.finish().await;
            let contention = graph.stop().await;
            if !bounded(&first) {
                assert!(held < RELEASE_BUDGET, "the waiter was held {held:?}");
            }
            let reply = f
                .first_attempt(first, &contention, RUST_APP, f.cancel(order, SELLER))
                .await;
            expect_status(&reply, StatusCode::NO_CONTENT);
            assert_eq!(ok(&result)["sent"], 1);
        }
        assert_eq!(f.reminders_for(order).await[0]["status"], "sent");
        assert_eq!(f.order(order).await["status"], "CANCELLED");
        assert_eq!(f.stock().await, (Decimal::from(1000), Decimal::ZERO));
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_amend_of_expiry_gets_a_new_reminder_and_discards_the_old_notice() {
    let mut f = Fixture::new().await;
    let order = Uuid::new_v4();
    let first = in_hours(6);
    f.ask(order, 150, 5, "SPOT", Some(first)).await;
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        1
    );
    let later = first + chrono::Duration::hours(2);
    let reply = f
        .call(f.amend(order, json!({"expires_at": later.to_rfc3339()})))
        .await;
    expect_status(&reply, StatusCode::OK);
    assert_eq!(
        f.order(order).await["expires_epoch"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap(),
        later.timestamp() as f64
    );
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        1
    );
    let result = f.python(f.command("reminder_delivery")).await;
    assert_eq!(ok(&result)["sent"], 1);
    let notices = f.reminders_for(order).await;
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[0]["expiry_epoch"], json!(first.timestamp() as f64));
    assert_eq!(notices[0]["status"], "discarded");
    assert_eq!(notices[1]["expiry_epoch"], json!(later.timestamp() as f64));
    assert_eq!(notices[1]["status"], "sent");
    assert_eq!(
        ok(&result)["sender_calls"][0]["notice_id"],
        notices[1]["id"],
        "only the notice for the current expiry is sent"
    );
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        0
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_payload_keys_and_lease_survive_rust_writes() {
    let mut f = Fixture::new().await;
    let other = f.listing().await;
    let order = Uuid::new_v4();
    let expiry = in_hours(10);
    f.ask(order, 150, 5, "SPOT", Some(expiry)).await;
    assert_eq!(
        ok(&f.python(f.command("reminder_scan")).await)["created"],
        1
    );
    let claim = chrono::Utc::now() + chrono::Duration::seconds(1);
    let claim = chrono::DateTime::from_timestamp_micros(claim.timestamp_micros()).unwrap();
    let mut delivery = f.command("reminder_delivery");
    delivery["now"] = json!(claim.to_rfc3339());
    delivery["sender_accepts"] = json!(false);
    let result = f.python(delivery).await;
    assert_eq!(ok(&result)["sent"], 0);
    assert_eq!(ok(&result)["sender_calls"].as_array().unwrap().len(), 1);
    let notice = f.reminders_for(order).await[0].clone();
    let data = notice["data"].as_object().unwrap();
    let mut keys: Vec<&str> = data.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "_email_deadline",
            "_email_only",
            "_email_payload",
            "_email_retry_at",
            "_email_status",
            "event",
            "expires_at",
            "expiry_epoch",
            "order_id",
            "order_key"
        ]
    );
    assert_eq!(data["_email_status"], "pending");
    let claim_epoch = claim.timestamp_micros() as f64 / 1_000_000.0;
    assert!((data["_email_retry_at"].as_f64().unwrap() - (claim_epoch + 900.0)).abs() < 1e-6);
    let created = notice["created_epoch"].as_f64().unwrap();
    let deadline = (expiry.timestamp() as f64).min(created + 23.0 * 3600.0);
    assert!((data["_email_deadline"].as_f64().unwrap() - deadline).abs() < 1e-5);
    let stored = f.reminders().await;
    expect_status(
        &f.call(f.cancel(other, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    expect_status(
        &f.call(f.amend(order, json!({"price_per_mt_usd":"151"})))
            .await,
        StatusCode::OK,
    );
    let after = f.reminders().await;
    let texts =
        |notices: &[Value]| -> Vec<Value> { notices.iter().map(|n| n["text"].clone()).collect() };
    assert_eq!(
        texts(&after),
        texts(&stored),
        "Rust writes leave reminder payloads unchanged"
    );
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn reminder_dedupe_holds_for_rust_written_expiry_instants() {
    let whole = in_hours(10);
    let fraction = whole + chrono::Duration::microseconds(123_456);
    for (label, expiry) in [("whole-second", whole), ("microsecond", fraction)] {
        let mut f = Fixture::new().await;
        let text = expiry.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let reply = f.call(f.bid("5", "90", "SPOT", Some(&text))).await;
        expect_status(&reply, StatusCode::CREATED);
        let order: Uuid = reply.1["id"].as_str().unwrap().parse().unwrap();
        let first = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
        let repeat = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
        println!("REMINDER_DEDUPE create={label} first={first} repeat={repeat}");
        assert_eq!(first, 1, "{label}");
        assert_eq!(
            repeat, 0,
            "{label}: a repeated scan creates no second notice"
        );
        // The same instant written by an amendment of the Rust order.
        let moved = expiry + chrono::Duration::hours(1);
        let reply = f
            .call(Command::new(
                "PUT",
                format!("/api/orderbook/{order}"),
                BUYER,
                json!({"expires_at": moved.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)}),
            ))
            .await;
        expect_status(&reply, StatusCode::OK);
        let first = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
        let repeat = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
        println!("REMINDER_DEDUPE amend={label} first={first} repeat={repeat}");
        assert_eq!(first, 1, "{label}");
        assert_eq!(
            repeat, 0,
            "{label}: a repeated scan creates no second notice"
        );
        assert_eq!(f.reminders_for(order).await.len(), 2);
    }
}

// ---- demo activity -------------------------------------------------------

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_slice_holder_bounds_a_rust_create_without_party_deadlock() {
    for above_budget in [false, true] {
        let mut f = Fixture::new().await;
        f.demo_organizations().await;
        let (now, window) = f.demo_ticks(1).await.remove(0);
        let real = Uuid::new_v4();
        let demo = Uuid::new_v4();
        f.ask(real, 950, 5, &window, None).await;
        // Cheaper than the REAL ASK: only provenance keeps the matcher from it.
        f.demo_ask(demo, 900, &window).await;
        let mut command = f.command("demo_tick");
        command["now"] = json!(now);
        command["pause"] = json!(ADVISORY);
        let mut tick = Driver::start(command).await;
        tick.go().await;
        let python = tick.held().await;
        let graph = WaitGraph::start(&f.admin_url).await;
        let create = f.spawn(RUST_APP, f.bid("5", "960", &window, None)).await;
        let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
        let observed = Instant::now();
        println!(
            "OBSERVED_WAIT waiter=rust:{rust} blocker=python-demo:{python} above_budget={above_budget}"
        );
        if above_budget {
            // While Rust waits on the slice it already holds its organizations:
            // the REAL seller is locked, neither demo organization is.
            assert_eq!(
                f.probe_organization(&id(SELLER_ORG).to_string()).await,
                "locked"
            );
            assert_eq!(f.probe_organization(DEMO_SELLER_ORG).await, "free");
            assert_eq!(f.probe_organization(DEMO_BUYER_ORG).await, "free");
            let reply = create.await.unwrap();
            expect_status(&reply, StatusCode::CONFLICT);
            assert_eq!(code(&reply), "MARKET_BUSY");
            tick.release().await;
            let result = tick.finish().await;
            assert_eq!(ok(&result)["created_trades"], 1);
            graph
                .stop()
                .await
                .assert_no_cycle("tick held past the bound");
            expect_status(
                &f.call(f.bid("5", "960", &window, None)).await,
                StatusCode::CREATED,
            );
        } else {
            let held = release_and_time(&mut f, &mut tick, observed, RUST_APP, PYTHON_APP).await;
            let result = tick.finish().await;
            assert_eq!(ok(&result)["created_trades"], 1);
            let first = create.await.unwrap();
            let contention = graph.stop().await;
            if !bounded(&first) {
                assert!(held < RELEASE_BUDGET, "the waiter was held {held:?}");
            }
            let reply = f
                .first_attempt(
                    first,
                    &contention,
                    RUST_APP,
                    f.bid("5", "960", &window, None),
                )
                .await;
            expect_status(&reply, StatusCode::CREATED);
        }
        assert_eq!(
            f.order(real).await["status"],
            "FILLED",
            "Rust matched the REAL ASK"
        );
        assert_eq!(
            f.order(demo).await["remaining"],
            "5",
            "Rust never matched a DEMO ASK"
        );
        f.ledger().await;
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_slice_holder_bounds_the_demo_tick() {
    let mut f = Fixture::new().await;
    f.demo_organizations().await;
    let (now, window) = f.demo_ticks(1).await.remove(0);
    let real = Uuid::new_v4();
    f.ask(real, 950, 5, &window, None).await;
    let mut command = f.command("demo_tick");
    command["now"] = json!(now);
    let mut tick = Driver::start(command).await;
    let mut barrier = f.barrier().await;
    let cancel = f.spawn(RUST_HELD_APP, f.cancel(real, SELLER)).await;
    f.wait_lock(RUST_HELD_APP, BARRIER_APP).await;
    let before = f.snapshot().await;
    tick.go().await;
    let python = f.wait_lock(PYTHON_APP, RUST_HELD_APP).await;
    println!("OBSERVED_WAIT waiter=python-demo:{python} blocker={RUST_HELD_APP}");
    let result = tick.finish().await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["type"], "HTTPException");
    assert_eq!(
        result["error"]["status_code"], 409,
        "the slice lock bound refuses the tick"
    );
    assert_eq!(f.snapshot().await, before, "the refused tick wrote no row");
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT residual_release()")
            .fetch_one(&mut barrier)
            .await
            .unwrap()
    );
    expect_status(&cancel.await.unwrap(), StatusCode::NO_CONTENT);
    let created = f.demo_tick(&now).await;
    assert_eq!(ok(&created)["created_orders"], 2);
    assert_eq!(ok(&created)["created_trades"], 1);
    let repeated = f.demo_tick(&now).await;
    assert_eq!(ok(&repeated)["created_orders"], 0);
    assert_eq!(ok(&repeated)["reason"], "already generated for tick");
    f.ledger().await;
}

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

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_coverage_slice_locks_bound_rust_writers_in_both_directions() {
    // Coverage holds its sorted slice locks while it writes the demo book: a
    // Rust cancellation in one of those slices waits on that slice key and
    // its bound refuses it with no effect.
    let mut f = Fixture::new().await;
    f.demo_seed_organizations().await;
    let order = f.listing().await;
    let before = f.snapshot().await;
    let mut command = f.command("demo_coverage");
    command["pause"] = json!(FIRST_ORDER_INSERT);
    let mut coverage = Driver::start(command).await;
    coverage.go().await;
    let python = coverage.held().await;
    let graph = WaitGraph::start(&f.admin_url).await;
    let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
    let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
    println!("OBSERVED_WAIT waiter=rust:{rust} blocker=python-coverage:{python}");
    let awaited: Option<i64> = sqlx::query_scalar("SELECT residual_waiting_advisory($1)")
        .bind(RUST_APP)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let spot: i64 = sqlx::query_scalar("SELECT residual_slice_key($1::uuid,$2,'SPOT')")
        .bind(PRODUCT)
        .bind(f.point)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(awaited, Some(spot), "Rust waits on the covered slice key");
    let reply = cancel.await.unwrap();
    expect_status(&reply, StatusCode::CONFLICT);
    assert_eq!(code(&reply), "MARKET_BUSY");
    assert_eq!(
        f.snapshot().await,
        before,
        "a bounded refusal has no effect"
    );
    coverage.release().await;
    let result = coverage.finish().await;
    assert!(ok(&result)["coverage_created"].as_i64().unwrap() > 0);
    graph
        .stop()
        .await
        .assert_no_cycle("coverage held past the bound");
    expect_status(
        &f.call(f.cancel(order, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    f.ledger().await;

    // Rust holds a covered slice: coverage refuses at its slice bound, unwritten.
    let mut f = Fixture::new().await;
    f.demo_seed_organizations().await;
    let order = f.listing().await;
    let mut coverage = Driver::start(f.command("demo_coverage")).await;
    let mut barrier = f.barrier().await;
    let cancel = f.spawn(RUST_HELD_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_HELD_APP, BARRIER_APP).await;
    let before = f.snapshot().await;
    let graph = WaitGraph::start(&f.admin_url).await;
    coverage.go().await;
    let python = f.wait_lock(PYTHON_APP, RUST_HELD_APP).await;
    println!("OBSERVED_WAIT waiter=python-coverage:{python} blocker={RUST_HELD_APP}");
    let result = coverage.finish().await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["type"], "HTTPException");
    assert_eq!(result["error"]["status_code"], 409);
    assert_eq!(
        f.snapshot().await,
        before,
        "the refused coverage wrote no row"
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT residual_release()")
            .fetch_one(&mut barrier)
            .await
            .unwrap()
    );
    expect_status(&cancel.await.unwrap(), StatusCode::NO_CONTENT);
    graph
        .stop()
        .await
        .assert_no_cycle("Rust held the slice past the coverage bound");
    f.ledger().await;
}

// ---- retained deleters and freeze history --------------------------------

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_prune_quarantine_and_seed_reset_refuse_freeze_history_without_partial_effect() {
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
    assert_eq!(trades.len(), 2);
    for (trade, cleared) in trades.iter().zip([false, true]) {
        sqlx::query("SELECT residual_freeze($1::uuid,$2)")
            .bind(trade["id"].as_str().unwrap())
            .bind(cleared)
            .execute(&mut f.admin)
            .await
            .unwrap();
    }
    sqlx::query("SELECT residual_age_demo_trades(8)")
        .execute(&mut f.admin)
        .await
        .unwrap();
    // One row in every table the deleters clear before trades (watchlist
    // events and targets, suggestions, commissions, negotiation rounds and
    // negotiations), all on the ACTIVE-frozen trade and its orders.
    sqlx::query("SELECT residual_trade_references($1::uuid)")
        .bind(trades[0]["id"].as_str().unwrap())
        .execute(&mut f.admin)
        .await
        .unwrap();
    let before = f.snapshot().await;
    assert_eq!(before["reference_count"], 5, "{before}");
    let refused = |result: &Value, expected: &str, label: &str| {
        assert_eq!(sqlstate(result), expected, "{label}: {result}");
    };

    let prune = f.python(f.command("demo_prune")).await;
    refused(&prune, "23503", "demo prune");
    assert_eq!(
        f.snapshot().await,
        before,
        "demo prune left a partial effect"
    );

    // Both freeze states (ACTIVE and CLEARED) keep the trade, under the owner role.
    for trade in &trades {
        let mut command = f.owner_command("quarantine");
        command["args"] = json!({"order_ids":[trade["bid_order_id"],trade["ask_order_id"]]});
        let result = f.python(command).await;
        refused(&result, "23503", "quarantine as the owner role");
        assert_eq!(
            f.snapshot().await,
            before,
            "quarantine left a partial effect"
        );
    }
    let mut command = f.command("quarantine");
    command["args"] = json!({"order_ids":[trades[0]["bid_order_id"],trades[0]["ask_order_id"]]});
    let result = f.python(command).await;
    refused(&result, "42501", "quarantine as the runtime role");
    assert_eq!(f.snapshot().await, before);

    let reset = f.python(f.owner_command("seed_reset")).await;
    refused(&reset, "23503", "seed reset");
    assert_eq!(
        f.snapshot().await,
        before,
        "seed reset left a partial effect"
    );
    f.ledger().await;
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn quarantine_locks_follow_the_canonical_slice_order_and_bound_rust() {
    // Quarantine holds the slice; Rust cancel in that slice is bounded.
    for above_budget in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        let demo = Uuid::new_v4();
        f.demo_ask(demo, 900, "SPOT").await;
        let before = f.snapshot().await;
        let mut command = f.owner_command("quarantine");
        command["args"] = json!({"order_ids":[demo]});
        command["pause"] = json!(ADVISORY);
        // The maintenance key, then the one slice key.
        command["pause_count"] = json!(2);
        let mut quarantine = Driver::start(command).await;
        quarantine.go().await;
        let python = quarantine.held().await;
        let graph = WaitGraph::start(&f.admin_url).await;
        let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
        let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
        let observed = Instant::now();
        println!(
            "OBSERVED_WAIT waiter=rust:{rust} blocker=python-quarantine:{python} above_budget={above_budget}"
        );
        if above_budget {
            let reply = cancel.await.unwrap();
            expect_status(&reply, StatusCode::CONFLICT);
            assert_eq!(code(&reply), "MARKET_BUSY");
            assert_eq!(
                f.snapshot().await,
                before,
                "a bounded refusal has no effect"
            );
            quarantine.release().await;
            ok(&quarantine.finish().await);
            graph
                .stop()
                .await
                .assert_no_cycle("quarantine held past the bound");
            expect_status(
                &f.call(f.cancel(order, SELLER)).await,
                StatusCode::NO_CONTENT,
            );
        } else {
            let held =
                release_and_time(&mut f, &mut quarantine, observed, RUST_APP, PYTHON_APP).await;
            let first = cancel.await.unwrap();
            ok(&quarantine.finish().await);
            let contention = graph.stop().await;
            if !bounded(&first) {
                assert!(held < RELEASE_BUDGET, "the waiter was held {held:?}");
            }
            let reply = f
                .first_attempt(first, &contention, RUST_APP, f.cancel(order, SELLER))
                .await;
            expect_status(&reply, StatusCode::NO_CONTENT);
        }
        let after = f.snapshot().await;
        assert_eq!(after["quarantine_count"], 1);
        assert_eq!(
            after["order_count"].as_i64(),
            before["order_count"].as_i64().map(|n| n - 1)
        );
        assert_eq!(f.order(order).await["status"], "CANCELLED");
        f.ledger().await;
    }

    // Rust holds the slice; quarantine times out at its 500 ms bound untouched.
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let demo = Uuid::new_v4();
    f.demo_ask(demo, 900, "SPOT").await;
    let mut command = f.owner_command("quarantine");
    command["args"] = json!({"order_ids":[demo]});
    let mut quarantine = Driver::start(command).await;
    let mut barrier = f.barrier().await;
    let cancel = f.spawn(RUST_HELD_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_HELD_APP, BARRIER_APP).await;
    let before = f.snapshot().await;
    let graph = WaitGraph::start(&f.admin_url).await;
    quarantine.go().await;
    let python = f.wait_lock(PYTHON_APP, RUST_HELD_APP).await;
    println!("OBSERVED_WAIT waiter=python-quarantine:{python} blocker={RUST_HELD_APP}");
    let result = quarantine.finish().await;
    assert_eq!(sqlstate(&result), "55P03");
    assert_eq!(
        f.snapshot().await,
        before,
        "the timed-out quarantine touched no row"
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT residual_release()")
            .fetch_one(&mut barrier)
            .await
            .unwrap()
    );
    expect_status(&cancel.await.unwrap(), StatusCode::NO_CONTENT);
    graph
        .stop()
        .await
        .assert_no_cycle("Rust held the slice past the quarantine bound");

    // Two runs naming two slices in opposite orders take them in one order.
    let mut f = Fixture::new().await;
    let month = chrono::Utc::now().format("%Y-%m").to_string();
    let orders: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
    for (order, window) in orders
        .iter()
        .zip(["SPOT", month.as_str(), "SPOT", month.as_str()])
    {
        f.demo_ask(*order, 900, window).await;
    }
    let mut first = f.owner_command("quarantine");
    first["application_name"] = json!(QUARANTINE_ONE_APP);
    first["args"] = json!({"order_ids":[orders[1],orders[0]]});
    first["pause"] = json!(ADVISORY);
    // The maintenance key, then the first slice in canonical order.
    first["pause_count"] = json!(2);
    let mut second = f.owner_command("quarantine");
    second["application_name"] = json!(QUARANTINE_TWO_APP);
    second["args"] = json!({"order_ids":[orders[2],orders[3]]});
    let mut one = Driver::start(first).await;
    let mut two = Driver::start(second).await;
    let graph = WaitGraph::start(&f.admin_url).await;
    one.go().await;
    one.held().await;
    two.go().await;
    f.wait_lock(QUARANTINE_TWO_APP, QUARANTINE_ONE_APP).await;
    let observed = Instant::now();
    // Both runs sort their slices: the second waits on the lowest slice key
    // (the month window sorts before SPOT), not on the slice it named first.
    let awaited: Option<i64> = sqlx::query_scalar("SELECT residual_waiting_advisory($1)")
        .bind(QUARANTINE_TWO_APP)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    let lowest: i64 = sqlx::query_scalar("SELECT residual_slice_key($1::uuid,$2,$3)")
        .bind(PRODUCT)
        .bind(f.point)
        .bind(&month)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        awaited,
        Some(lowest),
        "the second run waits on the lowest sorted slice"
    );
    let held = release_and_time(
        &mut f,
        &mut one,
        observed,
        QUARANTINE_TWO_APP,
        QUARANTINE_ONE_APP,
    )
    .await;
    ok(&one.finish().await);
    let result = two.finish().await;
    graph
        .stop()
        .await
        .assert_no_cycle("two quarantine runs naming slices in opposite orders");
    ok(&result);
    assert!(held < RELEASE_BUDGET, "the second run was held {held:?}");
    assert_eq!(f.snapshot().await["quarantine_count"], 4);
    for order in &orders {
        assert_eq!(f.order(*order).await, Value::Null, "{order} quarantined");
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn seed_writes_exact_columns_and_holds_no_slice_lock() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    let mut command = f.owner_command("seed_market");
    command["pause"] = json!(FIRST_ORDER_INSERT);
    command["count"] = json!(ADVISORY);
    let mut seed = Driver::start(command).await;
    seed.go().await;
    let python = seed.held().await;
    let advisory: i64 = sqlx::query_scalar("SELECT residual_advisory_count($1)")
        .bind(python)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        advisory, 0,
        "the seed inserts orders without an advisory slice lock"
    );
    // The seed's open transaction has already written trading tables, so it
    // holds the one public cache generation row that every trading-table
    // write advances. A Rust cancellation therefore waits on that row, not on
    // a slice lock, and its lock bound refuses it with no effect.
    let before = f.snapshot().await;
    let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
    let mut waits = Value::Null;
    while !cancel.is_finished() {
        let observed: Value = sqlx::query_scalar("SELECT residual_lock_waits($1)")
            .bind(RUST_APP)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        if observed != json!([]) {
            waits = observed;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let cancel = cancel.await.unwrap();
    println!("SEED_CONTENDER status={} waits={waits}", cancel.0);
    expect_status(&cancel, StatusCode::CONFLICT);
    assert_eq!(code(&cancel), "MARKET_BUSY");
    let waited_on: Vec<&str> = waits
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|wait| wait["relation"].as_str())
        .collect();
    assert_eq!(
        waited_on,
        ["public_read_cache_generations"],
        "the only contended row is the cache generation: {waits}"
    );
    assert!(
        waits
            .as_array()
            .unwrap()
            .iter()
            .all(|wait| wait["blockers"][0]["pid"] == json!(python)),
        "the paused seed is the blocker: {waits}"
    );
    assert_eq!(
        f.snapshot().await,
        before,
        "a bounded refusal has no effect"
    );
    seed.release().await;
    let result = seed.finish().await;
    ok(&result);
    assert_eq!(result["counted"]["before_pause"], 0);
    // The coverage refresh at the end of the seed takes sorted slice locks.
    println!(
        "SEED_ADVISORY_STATEMENTS after_first_order_insert={}",
        result["counted"]["after_pause"]
    );
    assert!(result["counted"]["after_pause"].as_i64().unwrap() > 0);
    let shape: Value = sqlx::query_scalar("SELECT residual_seed_shape()")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    println!("SEED_SHAPE {shape}");
    assert!(shape["demo_orders"].as_i64().unwrap() > 0);
    assert_eq!(shape["demo_orders_with_owner"], 0);
    assert!(shape["demo_trades"].as_i64().unwrap() > 0);
    assert_eq!(shape["demo_trades_with_users"], 0);
    assert!(shape["demo_trades_linked"].as_i64().unwrap() > 0);
    expect_status(
        &f.call(f.cancel(order, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    assert_eq!(f.order(order).await["status"], "CANCELLED");
    f.ledger().await;
}

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
/// A Rust writer sends no wake: its row is sequenced by the next sequencer
/// poll. One poll interval plus allowance for a loaded host.
const POLL_BOUND: Duration = Duration::from_secs(5);
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

/// Rust market writers send no stream wake: their rows reach subscribers
/// through the sequencer poll.
fn assert_writers_send_no_wake() {
    let root = repository_root().join("rs/read/src/order_write");
    let mut pending = vec![root];
    let mut scanned = 0;
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap().to_lowercase();
                assert!(
                    !text.contains("pg_notify") && !text.contains("notify verdaxis"),
                    "{} sends a database notification",
                    path.display()
                );
                scanned += 1;
            }
        }
    }
    assert!(scanned > 10, "the writer sources were not found");
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
            "migration_revision":"p89_20261022_trade_freeze","database_url":self.runtime_url,
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
/// after its outbox insert, another session sees no row and the sequencer
/// statement numbers nothing, without waiting on the writer. After the commit
/// the row is numbered once, through the poll (Rust writers send no wake),
/// and reaches the organization channel once with the public marker as a
/// public invalidation. The unchanged Python dispatcher, as sequencer and hub
/// for a second cancellation and as replay for both, maps the same rows
/// without the internal evidence column.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_commit_is_invisible_to_the_dispatcher_until_commit_and_dispatched_once() {
    assert_writers_send_no_wake();
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
    let start = f.high_water().await;
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
    assert_eq!(numbered, 0, "the sequencer statement numbers no open row");
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
    assert!(
        !contention.waited(ELECTION_APP, CHAIN_HELD_APP),
        "the sequencer never waits on an open writer"
    );
    barrier.close().await.unwrap();
    observer.close().await.unwrap();

    // After the commit: one row, one sequence, one delivery, through the poll.
    let delivery = tokio::time::timeout(POLL_BOUND, seller_channel.recv())
        .await
        .expect("the poll sequences the committed row")
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
        tokio::time::timeout(Duration::from_millis(500), seller_channel.recv())
            .await
            .is_err(),
        "one delivery for one row"
    );
    drop((seller_channel, prices));
    f.stop_streams(runtime).await;
    assert_eq!(f.sequencer_holder().await, None);

    // The unchanged Python dispatcher takes the sequencer lock and is idle.
    let mut command = f.command("dispatch");
    command["args"] = json!({"orgs":[id(SELLER_ORG)],"after_seq":start,"expect_private":1});
    let mut driver = Driver::start(command).await;
    driver.go().await;
    let listener = driver.held().await;
    assert_eq!(f.sequencer_holder().await, Some(i64::from(listener)));
    expect_status(
        &f.call(f.cancel(second, SELLER)).await,
        StatusCode::NO_CONTENT,
    );
    driver.release().await;
    let result = driver.finish().await;
    let result = ok(&result);
    let second_rows = f.outbox(second).await;
    assert_eq!(second_rows.len(), 1);
    let second_row = &second_rows[0];
    assert_eq!(second_row["attempts"], 1, "numbered once");
    assert!(seq_of(second_row) > seq_of(row));
    let channel = format!("trades:{}", id(SELLER_ORG));
    let private: Vec<&Value> = result["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|frame| !frame["seq"].is_null())
        .collect();
    assert_eq!(private.len(), 1, "one Python delivery: {}", private.len());
    let frame = private[0];
    assert_eq!(frame["channel"], channel);
    assert_eq!(frame["seq"], second_row["seq"]);
    assert_eq!(frame["event_type"], second_row["event_type"]);
    assert_eq!(frame["payload"], delivered(&second_row["payload"]));
    let latency = frame["after_release_s"].as_f64().unwrap();
    println!("CHAIN_PYTHON_DELIVERY after_release_s={latency:.3}");
    assert!(
        latency < POLL_BOUND.as_secs_f64(),
        "the Python poll sequenced it"
    );
    let public: BTreeSet<&str> = result["frames"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|frame| frame["seq"].is_null())
        .map(|frame| {
            assert_eq!(frame["event_type"], "market_invalidated");
            assert_eq!(
                frame["payload"],
                json!({"schema_version":1,"resync_required":true})
            );
            frame["channel"].as_str().unwrap()
        })
        .collect();
    assert_eq!(public, BTreeSet::from(["orderbook", "prices"]));
    // Python replay of the Rust-numbered and the Python-numbered row.
    let replay = result["replay"][id(SELLER_ORG).to_string()]
        .as_array()
        .unwrap();
    assert_eq!(replay.len(), 2);
    for (replayed, stored) in replay.iter().zip([row, second_row]) {
        assert_eq!(replayed["seq"], stored["seq"]);
        assert_eq!(replayed["event_type"], stored["event_type"]);
        assert_eq!(replayed["payload"], delivered(&stored["payload"]));
        assert_eq!(replayed["public"], true);
        assert_eq!(replayed["participants"], json!([id(SELLER_ORG)]));
        assert!(
            !replayed["fields"]
                .as_array()
                .unwrap()
                .contains(&json!("producer_evidence"))
        );
    }
    assert!(
        !result["mapped_columns"]
            .as_array()
            .unwrap()
            .contains(&json!("producer_evidence")),
        "the Python outbox model maps no evidence column"
    );
    for frame in result["frames"].as_array().unwrap().iter().chain(replay) {
        assert_no_internal_identity(&frame.to_string(), Some(&frame["payload"]), &hidden);
    }
    assert_eq!(f.unsequenced().await, 0);
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
        live_frames.push(found);
        frame_texts.extend(texts);
    }
    // Every event reached the parties; the observer has heartbeats only.
    frame_texts.extend(assert_no_business(&mut live[2], Duration::from_secs(1)).await);
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
    assert!(classes.contains(&(pin, "PIN_WITHDRAWN".to_owned())));
    for event in &events {
        assert!(mutations.contains(event["source"].as_str().unwrap()));
        let target: Uuid = event["target"].as_str().unwrap().parse().unwrap();
        assert!(target == pin || target == slice);
    }
}
