#![cfg(feature = "contract-tests")]
//! Coexistence of the Rust market writers with the Python writers that keep
//! running after the route switch: the order-expiry reminder scan and email
//! delivery, the demo activity tick and prune, the operator quarantine and the
//! seed. Each Python writer runs unmodified in a child process
//! (`tests/postgres/phase8_residual_driver.py`) on a fresh clone of the
//! migrated database. Lock waits are observed in `pg_stat_activity` and every
//! observed wait is released well inside the 500 ms market lock bound.
#[path = "support/market_database.rs"]
mod market_database;

use std::process::Stdio;
use std::sync::Arc;
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
// Statement names the driver pauses after (see `PAUSE_POINTS` in the driver).
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
    /// A contender that returned a bounded refusal is retried once after the
    /// competing writer committed; the refusal is recorded, never hidden.
    async fn settle(&self, reply: Reply, command: Command) -> Reply {
        if bounded(&reply) {
            println!("BOUNDED_CONTENDER status={} body={}", reply.0, reply.1);
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

/// Release a paused Python writer and check the observation-to-release time.
async fn release_within_budget(driver: &mut Driver, observed: Instant) {
    driver.release().await;
    let elapsed = observed.elapsed();
    println!("PYTHON_RELEASE elapsed_ms={}", elapsed.as_millis());
    assert!(elapsed < RELEASE_BUDGET, "release took {elapsed:?}");
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
    for above_budget in [false, true] {
        let mut f = Fixture::new().await;
        let order = f.listing().await;
        f.expire_at(order, in_hours(6)).await;
        let before = f.snapshot().await;
        let mut command = f.command("reminder_scan");
        command["pause"] = json!(SCAN_LOCK);
        let mut scan = Driver::start(command).await;
        scan.go().await;
        let python = scan.held().await;
        let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
        let rust = f.wait_lock(RUST_APP, PYTHON_APP).await;
        let observed = Instant::now();
        println!(
            "OBSERVED_WAIT waiter=rust:{rust} blocker=python-scan:{python} above_budget={above_budget}"
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
            scan.release().await;
            assert_eq!(ok(&scan.finish().await)["created"], 1);
            expect_status(
                &f.call(f.cancel(order, SELLER)).await,
                StatusCode::NO_CONTENT,
            );
        } else {
            release_within_budget(&mut scan, observed).await;
            let reply = f
                .settle(cancel.await.unwrap(), f.cancel(order, SELLER))
                .await;
            expect_status(&reply, StatusCode::NO_CONTENT);
            assert_eq!(ok(&scan.finish().await)["created"], 1);
        }
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
    scan.go().await;
    f.wait_lock(PYTHON_APP, HOLDER_APP).await;
    let create = f.spawn(RUST_APP, f.bid("2", "110", "SPOT", None)).await;
    f.wait_lock(RUST_APP, HOLDER_APP).await;
    let observed = Instant::now();
    sqlx::raw_sql("COMMIT").execute(&mut holder).await.unwrap();
    assert!(
        observed.elapsed() < RELEASE_BUDGET,
        "holder release took too long"
    );
    let rust = create.await.unwrap();
    let python = scan.finish().await;
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
        for order in [first, second] {
            assert!(f.reminders_for(order).await.len() <= 1);
        }
    }
    assert!(
        rust.0.is_success() || bounded(&rust),
        "Rust create must succeed or refuse within its bound: {rust:?}"
    );
    println!(
        "OPPOSITE_ORDER_OUTCOME rust_status={} rust_code={} python_ok={} python_error={}",
        rust.0,
        code(&rust),
        python["ok"],
        python["error"]
    );
    let rerun = ok(&f.python(f.command("reminder_scan")).await)["created"].clone();
    println!("OPPOSITE_ORDER_RESCAN created={rerun}");
    for order in [first, second] {
        assert!(
            f.reminders_for(order).await.len() <= 1,
            "one notice per order and expiry"
        );
    }
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
            expect_status(
                &f.call(f.cancel(order, SELLER)).await,
                StatusCode::NO_CONTENT,
            );
        } else {
            release_within_budget(&mut delivery, observed).await;
            let reply = f
                .settle(cancel.await.unwrap(), f.cancel(order, SELLER))
                .await;
            expect_status(&reply, StatusCode::NO_CONTENT);
            let result = delivery.finish().await;
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
        ok(&f.python(f.command("demo_organizations")).await);
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
            expect_status(
                &f.call(f.bid("5", "960", &window, None)).await,
                StatusCode::CREATED,
            );
        } else {
            release_within_budget(&mut tick, observed).await;
            let result = tick.finish().await;
            assert_eq!(ok(&result)["created_trades"], 1);
            let reply = f
                .settle(create.await.unwrap(), f.bid("5", "960", &window, None))
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
    ok(&f.python(f.command("demo_organizations")).await);
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

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn demo_prune_quarantine_and_seed_reset_refuse_freeze_history_without_partial_effect() {
    let mut f = Fixture::new().await;
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
    let before = f.snapshot().await;
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
            expect_status(
                &f.call(f.cancel(order, SELLER)).await,
                StatusCode::NO_CONTENT,
            );
        } else {
            release_within_budget(&mut quarantine, observed).await;
            let reply = f
                .settle(cancel.await.unwrap(), f.cancel(order, SELLER))
                .await;
            expect_status(&reply, StatusCode::NO_CONTENT);
            ok(&quarantine.finish().await);
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
    let deadlocks = f.deadlocks().await;
    let mut first = f.owner_command("quarantine");
    first["application_name"] = json!("phase8_residual_py_one");
    first["args"] = json!({"order_ids":[orders[1],orders[0]]});
    first["pause"] = json!(ADVISORY);
    first["pause_count"] = json!(2);
    let mut second = f.owner_command("quarantine");
    second["application_name"] = json!("phase8_residual_py_two");
    second["args"] = json!({"order_ids":[orders[2],orders[3]]});
    let mut one = Driver::start(first).await;
    let mut two = Driver::start(second.clone()).await;
    one.go().await;
    one.held().await;
    two.go().await;
    f.wait_lock("phase8_residual_py_two", "phase8_residual_py_one")
        .await;
    let observed = Instant::now();
    release_within_budget(&mut one, observed).await;
    ok(&one.finish().await);
    let result = two.finish().await;
    if result["ok"] == false {
        assert_eq!(
            sqlstate(&result),
            "55P03",
            "only the lock bound may refuse: {result}"
        );
        println!("BOUNDED_CONTENDER quarantine-two 55P03; rerun after the first committed");
        ok(&f.python(second).await);
    }
    assert_eq!(f.snapshot().await["quarantine_count"], 4);
    for order in &orders {
        assert_eq!(f.order(*order).await, Value::Null, "{order} quarantined");
    }
    assert_eq!(
        f.deadlocks().await,
        deadlocks,
        "sorted slice locks form no cycle"
    );
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
    let cancel = tokio::time::timeout(Duration::from_secs(30), f.call(f.cancel(order, SELLER)))
        .await
        .expect("the seed blocks no Rust cancellation");
    expect_status(&cancel, StatusCode::NO_CONTENT);
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
    ];
    for (label, command) in steps {
        let reply = f.call(command).await;
        println!(
            "PHASE7_REFERENCE_STEP {label} status={} code={}",
            reply.0,
            code(&reply)
        );
        assert!(
            reply.0.is_success() || reply.0.is_client_error(),
            "{label}: {reply:?}"
        );
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
/// forms a cycle with a Rust cancellation that the server breaks with 40P01.
/// It shows that these schedules would detect such a lock order.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn forbidden_slice_then_user_order_is_detected_as_a_deadlock() {
    let mut f = Fixture::new().await;
    let order = f.listing().await;
    f.fast_deadlock_detection().await;
    let before = f.deadlocks().await;
    let slice: i64 = sqlx::query_scalar("SELECT residual_slice_key($1::uuid,$2,'SPOT')")
        .bind(PRODUCT)
        .bind(f.point)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
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
    let cancel = f.spawn(RUST_APP, f.cancel(order, SELLER)).await;
    f.wait_lock(RUST_APP, FORBIDDEN_APP).await;
    // Let the Rust waiter's own deadlock check pass first, so the cycle closes
    // on the forbidden writer's request.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let request = sqlx::query("SELECT residual_forbidden_user($1)")
        .bind(id(SELLER))
        .execute(&mut forbidden)
        .await;
    let forbidden_code = match &request {
        Err(sqlx::Error::Database(error)) => error.code().map(|c| c.to_string()),
        Err(error) => panic!("unexpected error {error}"),
        Ok(_) => None,
    };
    sqlx::raw_sql("ROLLBACK")
        .execute(&mut forbidden)
        .await
        .unwrap();
    forbidden.close().await.unwrap();
    let reply = f
        .settle(cancel.await.unwrap(), f.cancel(order, SELLER))
        .await;
    expect_status(&reply, StatusCode::NO_CONTENT);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut after = f.deadlocks().await;
    while after == before && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        after = f.deadlocks().await;
    }
    println!(
        "NEGATIVE_CONTROL forbidden_code={forbidden_code:?} deadlocks_before={before} after={after}"
    );
    assert!(after > before, "the server detected the cycle");
    assert!(
        forbidden_code.as_deref() == Some("40P01") || forbidden_code.is_none(),
        "the cycle ends in a deadlock refusal: {forbidden_code:?}"
    );
    f.ledger().await;
}
