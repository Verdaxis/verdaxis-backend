#![cfg(feature = "contract-tests")]
//! Phase 10 Task 14: the order-expiry reminder job (`job reminders`).
//!
//! The database cases need the migrated schema, the committed ACL bundle and
//! the retained Python reminder functions; `rs/read/tests/run_phase10_reminders.sh`
//! provides all three and runs this target with `--include-ignored`. Order-book
//! rows are written and locked only through `fixtures/phase10_reminders.sql`.
//! Concurrency uses independent connections and lock-wait observation in
//! `pg_stat_activity`, never sleeps as barriers.
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool, Row};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;
use uuid::Uuid;
use verdaxis_read::cli::jobs::reminders::{
    self, MAX_SCAN_LIMIT, REMINDER_LOCK_ID, RunCounts, ScanCounts, ScanObserver, command_with,
    reminder_message,
};
use verdaxis_read::cli::mail::Delivery;
use verdaxis_read::cli::mail::expiry::{
    self, Counts, ExpiryMailConfig, ExpirySender, ProviderSender, epoch_seconds, render_payload,
};
use verdaxis_read::cli::{Command, Environment, Exit, parse_arguments};
use verdaxis_read::crud::referrals::invite::MailClient;

const NEEDS_DATABASE: &str =
    "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const JOB_APP: &str = "reminder-job";
const CANCEL_APP: &str = "owner-cancel";
const ORDER_FIRST_APP: &str = "order-first-scan";
/// The market write boundary's lock bound (`order_write` `MARKET_LOCK_TIMEOUT`).
const MARKET_LOCK_TIMEOUT: &str = "SET LOCAL lock_timeout = '500ms'";
const NOTIFICATION_KEYS: [&str; 10] = [
    "event",
    "order_id",
    "order_key",
    "expiry_epoch",
    "expires_at",
    "_email_only",
    "_email_status",
    "_email_payload",
    "_email_retry_at",
    "_email_deadline",
];

fn reference() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 15, 0, 0, 0).unwrap()
}

fn at(offset: Duration) -> DateTime<Utc> {
    reference() + offset
}

fn config() -> ExpiryMailConfig {
    ExpiryMailConfig::default()
}

/// A JSON number exactly as the reminder stores it.
fn number(value: f64) -> Value {
    json!(value)
}

fn disposable(variable: &str) -> String {
    let raw = std::env::var(variable).unwrap_or_else(|_| panic!("{variable}: {NEEDS_DATABASE}"));
    let parsed = url::Url::parse(&raw).expect("a disposable URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(
        parsed.path().ends_with("_test"),
        "{variable} must name a *_test database"
    );
    raw
}

async fn connect_as(variable: &str, application: &str) -> PgConnection {
    let options = PgConnectOptions::from_str(&disposable(variable))
        .unwrap()
        .application_name(application);
    PgConnection::connect_with(&options).await.unwrap()
}

fn sqlstate(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned())
}

struct Party {
    organization: Uuid,
    user: Uuid,
    email: String,
}

#[derive(Debug, Clone)]
struct Notice {
    id: Uuid,
    recipient: Uuid,
    kind: String,
    title: String,
    message: String,
    data: Value,
    is_read: bool,
    created_at: DateTime<Utc>,
}

impl Notice {
    fn status(&self) -> &str {
        self.data["_email_status"].as_str().unwrap()
    }
}

struct Database {
    owner: PgConnection,
    admin: PgConnection,
    pool: PgPool,
    product: Uuid,
}

impl Database {
    async fn open() -> Self {
        let mut owner = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", "reminder-fixture").await;
        sqlx::raw_sql(include_str!("fixtures/phase10_reminders.sql"))
            .execute(&mut owner)
            .await
            .unwrap();
        sqlx::query("SELECT reminder_fixture_quiesce()")
            .execute(&mut owner)
            .await
            .unwrap();
        let product = Uuid::new_v4();
        sqlx::query("SELECT reminder_fixture_product($1)")
            .bind(product)
            .execute(&mut owner)
            .await
            .unwrap();
        let admin = connect_as("RUST_REMINDERS_ADMIN_DATABASE_URL", "reminder-observer").await;
        let options = PgConnectOptions::from_str(&disposable("RUST_REMINDERS_DATABASE_URL"))
            .unwrap()
            .application_name(JOB_APP);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .unwrap();
        Self {
            owner,
            admin,
            pool,
            product,
        }
    }

    async fn call(&mut self, statement: &'static str, binds: &[Value]) {
        let mut query = sqlx::query(statement);
        for bind in binds {
            query = match bind {
                Value::String(text) => match Uuid::parse_str(text) {
                    Ok(id) => query.bind(id),
                    Err(_) => query.bind(text.clone()),
                },
                Value::Bool(flag) => query.bind(*flag),
                Value::Null => query.bind(Option::<Uuid>::None),
                other => panic!("unsupported bind {other}"),
            };
        }
        query.execute(&mut self.owner).await.unwrap();
    }

    async fn party(&mut self) -> Party {
        let organization = Uuid::new_v4();
        let user = Uuid::new_v4();
        let email = format!("{}@reminders.example", user.simple());
        self.call(
            "SELECT reminder_fixture_party($1, $2, $3)",
            &[json!(organization), json!(user), json!(email)],
        )
        .await;
        Party {
            organization,
            user,
            email,
        }
    }

    async fn member(&mut self, organization: Uuid, verified: bool) -> (Uuid, String) {
        let user = Uuid::new_v4();
        let email = format!("{}@reminders.example", user.simple());
        self.call(
            "SELECT reminder_fixture_member($1, $2, $3, $4)",
            &[
                json!(organization),
                json!(user),
                json!(email),
                json!(verified),
            ],
        )
        .await;
        (user, email)
    }

    async fn order(
        &mut self,
        organization: Uuid,
        owner: Option<Uuid>,
        side: &str,
        expiry: DateTime<Utc>,
    ) -> Uuid {
        let order = Uuid::new_v4();
        sqlx::query("SELECT reminder_fixture_order($1, $2, $3, $4, $5, $6)")
            .bind(order)
            .bind(organization)
            .bind(owner)
            .bind(self.product)
            .bind(side)
            .bind(expiry)
            .execute(&mut self.owner)
            .await
            .unwrap();
        order
    }

    async fn owned_order(&mut self, party: &Party, expiry: DateTime<Utc>) -> Uuid {
        self.order(party.organization, Some(party.user), "BID", expiry)
            .await
    }

    async fn set_expiry(&mut self, order: Uuid, expiry: DateTime<Utc>) {
        sqlx::query("SELECT reminder_fixture_set_expiry($1, $2)")
            .bind(order)
            .bind(expiry)
            .execute(&mut self.owner)
            .await
            .unwrap();
    }

    async fn order_status(&mut self, order: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM orderbook_orders WHERE id = $1")
            .bind(order)
            .fetch_one(&mut self.owner)
            .await
            .unwrap()
    }

    async fn notices(&mut self) -> Vec<Notice> {
        let rows = sqlx::query(
            "SELECT id, recipient_id, type, title, message, data::text AS data, is_read, created_at \
             FROM notifications ORDER BY created_at, (data ->> 'expiry_epoch'), recipient_id",
        )
        .fetch_all(&mut self.owner)
        .await
        .unwrap();
        rows.into_iter()
            .map(|row| Notice {
                id: row.get("id"),
                recipient: row.get("recipient_id"),
                kind: row.get("type"),
                title: row.get("title"),
                message: row.get("message"),
                data: serde_json::from_str(row.get::<String, _>("data").as_str()).unwrap(),
                is_read: row.get("is_read"),
                created_at: row.get("created_at"),
            })
            .collect()
    }

    async fn notices_for(&mut self, order: Uuid) -> Vec<Notice> {
        self.notices()
            .await
            .into_iter()
            .filter(|notice| notice.data["order_id"] == json!(order.to_string()))
            .collect()
    }

    async fn probe(&mut self, statement: &'static str, id: Uuid) -> bool {
        sqlx::query_scalar(statement)
            .bind(id)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    /// Wait until a session of `waiter` waits on a lock held by `blocker`.
    async fn wait_blocked(&mut self, waiter: &str, blocker: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity w JOIN pg_stat_activity b \
                 ON b.pid = ANY(pg_blocking_pids(w.pid)) \
                 WHERE w.application_name = $1 AND b.application_name = $2)",
            )
            .bind(waiter)
            .bind(blocker)
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
            if blocked {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{waiter} never waited on {blocker}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    async fn scan(&self, now: DateTime<Utc>, limit: u32) -> ScanCounts {
        reminders::scan(&self.pool, &config(), now, limit, &())
            .await
            .unwrap()
    }

    async fn deliver(&self, sender: &FakeSender, now: DateTime<Utc>) -> Counts {
        expiry::deliver(&self.pool, sender, &move || now, expiry::MAX_BATCH)
            .await
            .unwrap()
    }

    async fn run(&self, sender: &FakeSender, now: DateTime<Utc>) -> RunCounts {
        reminders::run_once(
            &self.pool,
            &config(),
            sender,
            &move || now,
            reminders::DEFAULT_SCAN_LIMIT,
            expiry::MAX_BATCH,
        )
        .await
        .unwrap()
    }
}

/// Records each provider call. With a gate, each call waits for a release.
#[derive(Default)]
struct FakeSender {
    refuse: AtomicBool,
    calls: Mutex<Vec<(Value, String)>>,
    gate: Option<Arc<Gate>>,
}

#[derive(Default)]
struct Gate {
    entered: Notify,
    release: Notify,
}

impl FakeSender {
    fn refusing() -> Self {
        let sender = Self::default();
        sender.refuse.store(true, Ordering::SeqCst);
        sender
    }
    fn gated(gate: Arc<Gate>) -> Self {
        Self {
            gate: Some(gate),
            ..Self::default()
        }
    }
    fn calls(&self) -> Vec<(Value, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl ExpirySender for FakeSender {
    async fn send(&self, payload: &Value, idempotency_key: &str) -> Delivery {
        self.calls
            .lock()
            .unwrap()
            .push((payload.clone(), idempotency_key.to_owned()));
        if let Some(gate) = &self.gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if self.refuse.load(Ordering::SeqCst) {
            Delivery::Refused
        } else {
            Delivery::Sent
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    UsersLocked,
    OrderLocked,
}

/// Pauses one scan transaction at each listed stage until released.
struct Pause {
    stages: Vec<Stage>,
    reached: Notify,
    release: Notify,
}

impl Pause {
    fn at(stages: &[Stage]) -> Arc<Self> {
        Arc::new(Self {
            stages: stages.to_vec(),
            reached: Notify::new(),
            release: Notify::new(),
        })
    }
    async fn hold(&self, stage: Stage) {
        if self.stages.contains(&stage) {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
}

impl ScanObserver for Pause {
    async fn users_locked(&self, _order_id: Uuid) {
        self.hold(Stage::UsersLocked).await;
    }
    async fn order_locked(&self, _order_id: Uuid) {
        self.hold(Stage::OrderLocked).await;
    }
}

fn spawn_scan(
    pool: &PgPool,
    now: DateTime<Utc>,
    pause: Arc<Pause>,
) -> tokio::task::JoinHandle<Result<ScanCounts, Exit>> {
    let pool = pool.clone();
    tokio::spawn(async move {
        reminders::scan(
            &pool,
            &config(),
            now,
            reminders::DEFAULT_SCAN_LIMIT,
            &*pause,
        )
        .await
    })
}

fn python(command: Value) -> Value {
    let interpreter = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new(interpreter)
        .current_dir(&root)
        .arg("tests/postgres/phase10_reminders_driver.py")
        .arg(command.to_string())
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
        .env(
            "DATABASE_URL",
            disposable("RUST_REMINDERS_PYTHON_DATABASE_URL"),
        )
        .output()
        .expect("start the Python reminder driver");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Python driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("RESULT "))
        .expect("a RESULT line");
    serde_json::from_str(line).unwrap()
}

fn iso(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339()
}

// ---------------------------------------------------------------------------
// Command refusals (no database).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reminders_refuse_nonproduction_before_pool() {
    let cases = [
        ("staging", true, Exit::Usage, 0),
        ("staging", false, Exit::Usage, 0),
        ("production", false, Exit::Usage, 0),
        // Production with --apply reaches the context opener, which refuses here.
        ("production", true, Exit::Precondition, 1),
    ];
    for (environment, apply, expected, opens) in cases {
        let mut arguments = vec![
            "job",
            "reminders",
            "--environment",
            environment,
            "--release-sha",
            SHA,
        ];
        if apply {
            arguments.push("--apply");
        }
        let parsed = parse_arguments(arguments.iter().map(Into::into).collect()).unwrap();
        let Command::Job(args) = parsed else {
            panic!("job arguments")
        };
        let opened = AtomicUsize::new(0);
        let exit = command_with(&args, |opened_environment, release_sha| {
            opened.fetch_add(1, Ordering::SeqCst);
            assert_eq!(opened_environment, Environment::Production);
            assert_eq!(release_sha, SHA);
            async { Err(Exit::Precondition) }
        })
        .await;
        assert_eq!(exit, expected, "{environment} apply={apply}");
        assert_eq!(
            opened.load(Ordering::SeqCst),
            opens,
            "{environment} apply={apply}"
        );
    }
    // The binary's dispatch reaches the same refusal for staging.
    let exit = verdaxis_read::cli::dispatch_os(
        [
            "job",
            "reminders",
            "--environment",
            "staging",
            "--release-sha",
            SHA,
            "--apply",
        ]
        .iter()
        .map(Into::into)
        .collect(),
    )
    .await;
    assert_eq!(exit, Exit::Usage);
}

// ---------------------------------------------------------------------------
// Provider transport (loopback stub, no database).
// ---------------------------------------------------------------------------

async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, Value) {
    use tokio::io::AsyncReadExt;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "connection closed before the request ended");
        buffer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&buffer).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if body.len() >= length {
                return (head.to_lowercase(), serde_json::from_str(body).unwrap());
            }
        }
    }
}

#[tokio::test]
async fn provider_sender_sends_the_canonical_payload_under_the_notification_key() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://127.0.0.1:{}/emails",
        listener.local_addr().unwrap().port()
    );
    let sender = ProviderSender::new(
        MailClient::loopback(endpoint, "synthetic-provider-key".into()).unwrap(),
    );
    let notification = Uuid::new_v4();
    let key = expiry::idempotency_key(notification);
    assert_eq!(key, format!("order-expiry/{notification}"));
    let mut payload = render_payload(&config(), "buyer@reminders.example", "Synthetic message");
    payload["unexpected"] = json!("dropped before sending");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .unwrap();
        (request, listener)
    });
    assert_eq!(sender.send(&payload, &key).await, Delivery::Sent);
    let ((head, body), listener) = server.await.unwrap();
    assert!(
        head.contains(&format!("\r\nidempotency-key: {key}")),
        "the notification key is the provider key: {head}"
    );
    assert_eq!(body, expiry::canonical_payload(&payload).unwrap());
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["from", "to", "subject", "html", "reply_to"]);

    // A payload that is not one string recipient is refused without a request.
    let mut invalid = render_payload(&config(), "buyer@reminders.example", "Synthetic");
    invalid["to"] = json!(["one@reminders.example", "two@reminders.example"]);
    assert_eq!(sender.send(&invalid, &key).await, Delivery::Refused);
    let accepted = tokio::time::timeout(std::time::Duration::from_millis(300), listener.accept());
    assert!(accepted.await.is_err(), "no request for an invalid payload");
}

// ---------------------------------------------------------------------------
// Database cases.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn reminders_scan_is_bounded_and_locks_the_order() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let mut orders = Vec::new();
    for hours in [1, 2, 3] {
        orders.push(db.owned_order(&party, at(Duration::hours(hours))).await);
    }
    // Out of the 48-hour window on both sides: never reminded.
    db.owned_order(&party, at(Duration::hours(48) + Duration::microseconds(1)))
        .await;
    db.owned_order(&party, reference()).await;
    for limit in [0, MAX_SCAN_LIMIT + 1] {
        assert_eq!(
            reminders::scan(&db.pool, &config(), reference(), limit, &()).await,
            Err(Exit::Usage)
        );
    }
    assert!(
        db.notices().await.is_empty(),
        "a refused bound writes nothing"
    );

    assert_eq!(db.scan(reference(), 2).await.created, 2);
    let first: Vec<String> = db
        .notices()
        .await
        .iter()
        .map(|notice| notice.data["order_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        first,
        [orders[0].to_string(), orders[1].to_string()],
        "earliest expiry first"
    );
    assert_eq!(db.scan(reference(), 2).await.created, 1);
    assert_eq!(db.scan(reference(), 2).await.created, 0);
    assert_eq!(db.notices().await.len(), 3);

    // Lock order inside one order's transaction: user row, then order row.
    let other = db.party().await;
    let order = db.owned_order(&other, at(Duration::hours(4))).await;
    let pause = Pause::at(&[Stage::UsersLocked, Stage::OrderLocked]);
    let scan = spawn_scan(&db.pool, reference(), pause.clone());
    pause.reached.notified().await;
    assert!(
        !db.probe(
            "SELECT reminder_fixture_user_update_lock_free($1)",
            other.user
        )
        .await,
        "the recipient's user row is locked first"
    );
    assert!(
        db.probe(
            "SELECT reminder_fixture_user_no_key_lock_free($1)",
            other.user
        )
        .await,
        "the user lock is KEY SHARE: ordinary user updates proceed"
    );
    assert!(
        db.probe("SELECT reminder_fixture_order_lock_free($1)", order)
            .await,
        "the order row is not locked before the user row"
    );
    pause.release.notify_one();
    pause.reached.notified().await;
    assert!(
        !db.probe("SELECT reminder_fixture_order_lock_free($1)", order)
            .await,
        "the scan holds the order row while it writes the reminder"
    );
    pause.release.notify_one();
    assert_eq!(scan.await.unwrap().unwrap().created, 1);
    assert!(
        db.probe("SELECT reminder_fixture_order_lock_free($1)", order)
            .await
    );
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn expiry_scan_and_claim_yield_one_send() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let order = db.owned_order(&party, at(Duration::hours(6))).await;

    // Process A holds the scan key inside its transaction.
    let pause = Pause::at(&[Stage::OrderLocked]);
    let first = spawn_scan(&db.pool, reference(), pause.clone());
    pause.reached.notified().await;
    // Process B runs completely meanwhile: the key is busy, so it creates
    // nothing and has nothing to send.
    let second = FakeSender::default();
    let busy = db.run(&second, reference()).await;
    assert_eq!(
        busy.scan,
        ScanCounts {
            created: 0,
            lock_busy: true
        }
    );
    assert_eq!(busy.mail, Counts::default());
    pause.release.notify_one();
    assert_eq!(first.await.unwrap().unwrap().created, 1);

    // Both processes now deliver at once: one claim, one send.
    let sender = FakeSender::default();
    let (left, right) = tokio::join!(
        db.deliver(&sender, reference()),
        db.deliver(&sender, reference())
    );
    assert_eq!(left.sent + right.sent, 1);
    assert_eq!(sender.calls().len(), 1);
    // B's next run sees the duplicate identity and the sent state.
    let again = db.run(&second, reference()).await;
    assert_eq!(again.scan.created, 0);
    assert_eq!(again.mail, Counts::default());
    let notices = db.notices_for(order).await;
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].status(), "sent");

    // Two whole runs racing on a fresh order also yield one reminder and one send.
    let order = db.owned_order(&party, at(Duration::hours(7))).await;
    let racing = FakeSender::default();
    let (left, right) = tokio::join!(db.run(&racing, reference()), db.run(&racing, reference()));
    assert_eq!(left.scan.created + right.scan.created, 1);
    assert_eq!(racing.calls().len(), 1);
    assert_eq!(db.notices_for(order).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn expiry_mail_lease_is_exclusive() {
    let mut db = Database::open().await;
    let party = db.party().await;
    db.owned_order(&party, at(Duration::hours(6))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);

    let gate = Arc::new(Gate::default());
    let held = Arc::new(FakeSender::gated(gate.clone()));
    let pool = db.pool.clone();
    let holder = {
        let held = held.clone();
        tokio::spawn(async move {
            let now = reference();
            expiry::deliver(&pool, &*held, &move || now, expiry::MAX_BATCH).await
        })
    };
    // The claim has committed and the provider call is in flight.
    gate.entered.notified().await;
    let other = FakeSender::default();
    for now in [
        reference(),
        at(Duration::minutes(15) - Duration::microseconds(1)),
    ] {
        assert_eq!(db.deliver(&other, now).await, Counts::default());
    }
    assert!(other.calls().is_empty(), "no second claim inside the lease");
    gate.release.notify_one();
    assert_eq!(holder.await.unwrap().unwrap().sent, 1);
    assert_eq!(held.calls().len(), 1);

    // A refused send keeps the lease, then retries the same payload and key.
    let order = db.owned_order(&party, at(Duration::hours(8))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    let refusing = FakeSender::refusing();
    assert_eq!(
        db.deliver(&refusing, reference()).await,
        Counts {
            deferred: 1,
            ..Counts::default()
        }
    );
    let leased = db.notices_for(order).await.remove(0);
    assert_eq!(leased.status(), "pending");
    // PostgreSQL wrote the lease; compare the number, not its spelling.
    assert_eq!(
        leased.data["_email_retry_at"].as_f64(),
        Some(epoch_seconds(at(Duration::minutes(15))))
    );
    let accepting = FakeSender::default();
    assert_eq!(
        db.deliver(&accepting, at(Duration::minutes(10))).await,
        Counts::default()
    );
    assert_eq!(
        db.deliver(&accepting, at(Duration::minutes(15))).await.sent,
        1
    );
    assert_eq!(accepting.calls(), refusing.calls(), "same payload and key");
    assert_eq!(accepting.calls()[0].1, expiry::idempotency_key(leased.id));
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn expiry_mail_discards_changed_owner_and_past_deadline() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let (successor, successor_email) = db.member(party.organization, true).await;
    let lease_ends = at(Duration::minutes(15));

    // Ownership moves after a claim commits: the next claim discards it, and
    // the new owner gets its own reminder from a later scan.
    let moved = db.owned_order(&party, at(Duration::hours(6))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    assert_eq!(
        db.deliver(&FakeSender::refusing(), reference())
            .await
            .deferred,
        1
    );
    sqlx::query("SELECT reminder_fixture_set_owner($1, $2)")
        .bind(moved)
        .bind(successor)
        .execute(&mut db.owner)
        .await
        .unwrap();
    let sender = FakeSender::default();
    assert_eq!(
        db.deliver(&sender, lease_ends).await,
        Counts {
            discarded: 1,
            ..Counts::default()
        }
    );
    assert!(sender.calls().is_empty());
    let discarded = db.notices_for(moved).await.remove(0);
    assert_eq!(discarded.recipient, party.user);
    assert_eq!(discarded.status(), "discarded");
    for key in ["_email_payload", "_email_retry_at", "_email_deadline"] {
        assert!(discarded.data.get(key).is_none(), "{key} is removed");
    }
    assert_eq!(db.scan(lease_ends, 1000).await.created, 1);
    let renewed = db.notices_for(moved).await;
    assert_eq!(renewed.len(), 2);
    assert_eq!(renewed[1].recipient, successor);
    assert_eq!(
        renewed[1].data["_email_payload"]["to"],
        json!([successor_email])
    );
    db.call("SELECT reminder_fixture_quiesce()", &[]).await;

    // The address changes after a claim: discarded, nothing sent.
    let readdressed = db.owned_order(&party, at(Duration::hours(6))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    assert_eq!(
        db.deliver(&FakeSender::refusing(), reference())
            .await
            .deferred,
        1
    );
    db.call(
        "SELECT reminder_fixture_set_email($1, $2)",
        &[json!(party.user), json!(format!("moved-{}", party.email))],
    )
    .await;
    assert_eq!(db.deliver(&sender, lease_ends).await.discarded, 1);
    assert_eq!(db.notices_for(readdressed).await[0].status(), "discarded");
    db.call(
        "SELECT reminder_fixture_set_email($1, $2)",
        &[json!(party.user), json!(party.email)],
    )
    .await;
    db.call("SELECT reminder_fixture_quiesce()", &[]).await;

    // Email updates turned off after creation: discarded.
    let muted = db.owned_order(&party, at(Duration::hours(6))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    db.call(
        "SELECT reminder_fixture_preferences($1, $2, $3)",
        &[json!(party.user), json!(false), json!(true)],
    )
    .await;
    assert_eq!(db.deliver(&sender, reference()).await.discarded, 1);
    assert_eq!(db.notices_for(muted).await[0].status(), "discarded");
    db.call(
        "SELECT reminder_fixture_preferences($1, $2, $3)",
        &[json!(party.user), json!(true), json!(true)],
    )
    .await;
    db.call("SELECT reminder_fixture_quiesce()", &[]).await;

    // The deadline is the expiry for a near expiry and creation + 23 h for a
    // far one; at the deadline the reminder is discarded unsent.
    let near = db.owned_order(&party, at(Duration::hours(2))).await;
    let far = db.owned_order(&party, at(Duration::hours(40))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 2);
    assert_eq!(
        db.notices_for(near).await[0].data["_email_deadline"],
        number(epoch_seconds(at(Duration::hours(2))))
    );
    assert_eq!(
        db.notices_for(far).await[0].data["_email_deadline"],
        number(epoch_seconds(at(Duration::hours(23))))
    );
    let late = FakeSender::default();
    assert_eq!(
        db.deliver(&late, at(Duration::hours(23))).await.discarded,
        2
    );
    assert!(late.calls().is_empty());
    assert!(
        db.notices()
            .await
            .iter()
            .all(|notice| notice.status() == "discarded")
    );
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn expiry_payload_preserves_all_order_and_provider_keys() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let expiry = at(Duration::hours(6) + Duration::microseconds(123_456));
    let order = db
        .order(party.organization, Some(party.user), "ASK", expiry)
        .await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    let notice = db.notices_for(order).await.remove(0);
    let first8 = &order.to_string()[..8];
    let message = format!(
        "Your ASK order {first8} expires on 15 Jan 2030 at 06:00 UTC. Review it before expiry if you want to keep trading."
    );
    assert_eq!(reminder_message("ASK", order, expiry), message);
    assert_eq!(notice.recipient, party.user);
    assert_eq!(notice.kind, "ORDER_UPDATE");
    assert_eq!(notice.title, "Order expires within 48 hours");
    assert_eq!(notice.message, message);
    assert!(!notice.is_read);
    assert_eq!(notice.created_at, reference());
    let keys: Vec<&str> = notice
        .data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, NOTIFICATION_KEYS, "Python's keys in Python's order");
    let payload = render_payload(&config(), &party.email, &message);
    assert_eq!(
        notice.data,
        json!({
            "event": "order_expiring",
            "order_id": order.to_string(),
            "order_key": order.simple().to_string(),
            "expiry_epoch": number(epoch_seconds(expiry)),
            "expires_at": "2030-01-15T06:00:00.123456+00:00",
            "_email_only": false,
            "_email_status": "pending",
            "_email_payload": payload,
            "_email_retry_at": number(epoch_seconds(reference())),
            "_email_deadline": number(epoch_seconds(expiry)),
        })
    );
    assert_eq!(notice.data["expiry_epoch"].to_string(), "1894687200.123456");
    assert_eq!(payload["to"], json!([party.email]));
    assert_eq!(payload["reply_to"], "admin@verdaxis.exchange");
    assert_eq!(payload["from"], "Verdaxis <noreply@verdaxis.exchange>");
    assert_eq!(
        payload["subject"],
        "Your Verdaxis order expires within 48 hours"
    );
    let html = payload["html"].as_str().unwrap();
    assert!(html.contains(&format!("<p>{message}</p>")));
    assert!(html.contains("href=\"https://app.verdaxis.exchange/app/home\""));

    let sender = FakeSender::default();
    assert_eq!(db.deliver(&sender, reference()).await.sent, 1);
    assert_eq!(
        sender.calls(),
        [(payload.clone(), format!("order-expiry/{}", notice.id))],
        "the provider key is the notification UUID"
    );
    let sent = db.notices_for(order).await.remove(0);
    let mut expected = notice.data.clone();
    let object = expected.as_object_mut().unwrap();
    object["_email_status"] = json!("sent");
    for key in ["_email_payload", "_email_retry_at", "_email_deadline"] {
        object.shift_remove(key);
    }
    assert_eq!(sent.data, expected, "order keys are kept exactly");
    assert_eq!(sent.data["expiry_epoch"].to_string(), "1894687200.123456");

    // Preferences: no email, no in-app; and an ownerless order reaches every
    // approved member (verified or not), as in Python.
    db.call("SELECT reminder_fixture_quiesce()", &[]).await;
    let quiet = db.party().await;
    db.call(
        "SELECT reminder_fixture_preferences($1, $2, $3)",
        &[json!(quiet.user), json!(false), json!(false)],
    )
    .await;
    let (unverified, _) = db.member(quiet.organization, false).await;
    let ownerless = db
        .order(quiet.organization, None, "BID", at(Duration::hours(5)))
        .await;
    assert_eq!(db.scan(reference(), 1000).await.created, 2);
    let notices = db.notices_for(ownerless).await;
    for notice in &notices {
        assert_eq!(notice.status(), "discarded");
        assert_eq!(notice.data["_email_payload"], Value::Null);
    }
    let muted = notices.iter().find(|n| n.recipient == quiet.user).unwrap();
    assert_eq!(muted.data["_email_only"], json!(true));
    let unverified = notices.iter().find(|n| n.recipient == unverified).unwrap();
    assert_eq!(unverified.data["_email_only"], json!(false));
    assert_eq!(db.deliver(&sender, reference()).await, Counts::default());
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn expiry_batch_is_at_most_twenty() {
    let mut db = Database::open().await;
    let party = db.party().await;
    for minute in 1..=25 {
        db.owned_order(&party, at(Duration::hours(1) + Duration::minutes(minute)))
            .await;
    }
    assert_eq!(db.scan(reference(), 1000).await.created, 25);
    let sender = FakeSender::default();
    for batch in [0, expiry::MAX_BATCH + 1] {
        assert_eq!(
            expiry::deliver(&db.pool, &sender, &reference, batch).await,
            Err(Exit::Usage)
        );
    }
    assert!(sender.calls().is_empty());
    assert_eq!(db.deliver(&sender, reference()).await.sent, 20);
    assert_eq!(sender.calls().len(), 20);
    assert_eq!(db.deliver(&sender, reference()).await.sent, 5);
    let keys: std::collections::BTreeSet<String> =
        sender.calls().into_iter().map(|(_, key)| key).collect();
    assert_eq!(keys.len(), 25, "one send per reminder");
    assert!(db.notices().await.iter().all(|n| n.status() == "sent"));
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn reminder_scan_and_owner_cancel_do_not_deadlock() {
    let mut db = Database::open().await;
    let party = db.party().await;

    // 1. The scan holds the owner's user row and the order row; the owner's
    //    cancel waits on the user row, not on the order, so when the scan
    //    commits the cancel proceeds within its 500 ms bound.
    let order = db.owned_order(&party, at(Duration::hours(6))).await;
    let pause = Pause::at(&[Stage::OrderLocked]);
    let scan = spawn_scan(&db.pool, reference(), pause.clone());
    pause.reached.notified().await;
    let mut cancel = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", CANCEL_APP).await;
    let user = party.user;
    let cancelling = tokio::spawn(async move {
        let mut transaction = cancel.begin().await?;
        sqlx::query(MARKET_LOCK_TIMEOUT)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT reminder_fixture_lock_user($1)")
            .bind(user)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT reminder_fixture_cancel_order($1)")
            .bind(order)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    });
    db.wait_blocked(CANCEL_APP, JOB_APP).await;
    pause.release.notify_one();
    let scanned = scan.await.unwrap();
    let cancelled = cancelling.await.unwrap();
    assert_eq!(
        scanned,
        Ok(ScanCounts {
            created: 1,
            lock_busy: false
        })
    );
    if let Err(error) = &cancelled {
        panic!(
            "the cancel must not time out or deadlock: {:?}",
            sqlstate(error)
        );
    }
    assert_eq!(db.order_status(order).await, "CANCELLED");
    assert_eq!(db.notices_for(order).await.len(), 1);

    // 2. The owner's cancel holds the user row first; the scan waits on it
    //    without holding the order, the cancel completes, and the scan then
    //    finds the order cancelled and writes nothing.
    let order = db.owned_order(&party, at(Duration::hours(7))).await;
    let mut cancel = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", CANCEL_APP).await;
    let mut transaction = cancel.begin().await.unwrap();
    sqlx::query(MARKET_LOCK_TIMEOUT)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("SELECT reminder_fixture_lock_user($1)")
        .bind(party.user)
        .execute(&mut *transaction)
        .await
        .unwrap();
    let scan = spawn_scan(&db.pool, reference(), Pause::at(&[]));
    db.wait_blocked(JOB_APP, CANCEL_APP).await;
    sqlx::query("SELECT reminder_fixture_cancel_order($1)")
        .bind(order)
        .execute(&mut *transaction)
        .await
        .expect("the cancel never waits on the scan for the order row");
    transaction.commit().await.unwrap();
    assert_eq!(
        scan.await.unwrap(),
        Ok(ScanCounts {
            created: 0,
            lock_busy: false
        })
    );
    assert_eq!(db.order_status(order).await, "CANCELLED");
    assert!(db.notices_for(order).await.is_empty());
}

/// Control: the order-first lock shape (Python's scan) against the same
/// cancel forms the cycle, and the cancel's 500 ms bound ends it. This proves
/// the case above can see the cycle when the order is wrong.
#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn order_first_scan_and_owner_cancel_form_a_lock_cycle() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let order = db.owned_order(&party, at(Duration::hours(6))).await;
    let mut scanner = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", ORDER_FIRST_APP).await;
    sqlx::query("BEGIN").execute(&mut scanner).await.unwrap();
    sqlx::query("SELECT reminder_fixture_lock_order($1)")
        .bind(order)
        .execute(&mut scanner)
        .await
        .unwrap();
    let mut cancel = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", CANCEL_APP).await;
    let user = party.user;
    let cancelling = tokio::spawn(async move {
        let mut transaction = cancel.begin().await?;
        sqlx::query(MARKET_LOCK_TIMEOUT)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT reminder_fixture_lock_user($1)")
            .bind(user)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("SELECT reminder_fixture_cancel_order($1)")
            .bind(order)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    });
    db.wait_blocked(CANCEL_APP, ORDER_FIRST_APP).await;
    let inserting = tokio::spawn(async move {
        let result = sqlx::query("SELECT reminder_fixture_insert_notice($1)")
            .bind(user)
            .execute(&mut scanner)
            .await;
        let _ = sqlx::query("ROLLBACK").execute(&mut scanner).await;
        result
    });
    let cancelled = cancelling.await.unwrap();
    let inserted = inserting.await.unwrap();
    let codes: Vec<String> = [cancelled.err(), inserted.err()]
        .iter()
        .flatten()
        .filter_map(sqlstate)
        .collect();
    assert!(
        codes.iter().any(|code| code == "55P03" || code == "40P01"),
        "the order-first shape must meet the cycle: {codes:?}"
    );
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn python_and_rust_share_the_scan_key_and_the_duplicate_identity() {
    let mut db = Database::open().await;
    let party = db.party().await;
    // Whole-second and fractional expiry instants, including both ends of the
    // microsecond range.
    let offsets = [
        Duration::hours(5),
        Duration::hours(5) + Duration::microseconds(1),
        Duration::hours(6) + Duration::microseconds(123_456),
        Duration::hours(7) + Duration::microseconds(999_999),
        Duration::hours(8) + Duration::milliseconds(500),
    ];
    let mut orders = Vec::new();
    for offset in offsets {
        orders.push(db.owned_order(&party, at(offset)).await);
    }
    let now = json!(iso(reference()));

    // Rust first: Python and Rust both see each reminder as existing.
    assert_eq!(db.scan(reference(), 1000).await.created, 5);
    assert_eq!(python(json!({"command": "scan", "now": now}))["created"], 0);
    assert_eq!(db.scan(reference(), 1000).await.created, 0);

    // An amended expiry is a new reminder identity for both, created once.
    let amended = at(Duration::hours(9) + Duration::microseconds(654_321));
    db.set_expiry(orders[2], amended).await;
    assert_eq!(python(json!({"command": "scan", "now": now}))["created"], 1);
    assert_eq!(db.scan(reference(), 1000).await.created, 0);
    let python_notice = db
        .notices_for(orders[2])
        .await
        .into_iter()
        .find(|notice| notice.data["expiry_epoch"] == number(epoch_seconds(amended)))
        .expect("Python stored the same epoch number");
    // Python's JSON for its reminder equals Rust's for the same facts.
    let message = reminder_message("BID", orders[2], amended);
    assert_eq!(python_notice.message, message);
    assert_eq!(
        python_notice.data,
        reminders::reminder_data(
            &config(),
            orders[2],
            amended,
            Some(&party.email),
            true,
            &message,
            reference(),
        )
    );

    // The scan key: while it is held, neither side scans.
    db.call("SELECT reminder_fixture_quiesce()", &[]).await;
    let order = db.owned_order(&party, at(Duration::hours(10))).await;
    let mut holder = connect_as("RUST_REMINDERS_OWNER_DATABASE_URL", "scan-key-holder").await;
    let mut transaction = holder.begin().await.unwrap();
    sqlx::query("SELECT reminder_fixture_hold_scan_key()")
        .execute(&mut *transaction)
        .await
        .unwrap();
    assert_eq!(REMINDER_LOCK_ID, 8_246_192_048);
    assert_eq!(
        db.scan(reference(), 1000).await,
        ScanCounts {
            created: 0,
            lock_busy: true
        }
    );
    assert_eq!(python(json!({"command": "scan", "now": now}))["created"], 0);
    transaction.rollback().await.unwrap();

    // Python first: Rust sees Python's reminders.
    assert_eq!(python(json!({"command": "scan", "now": now}))["created"], 1);
    assert_eq!(db.scan(reference(), 1000).await.created, 0);
    assert_eq!(db.notices_for(order).await.len(), 1);
}

#[tokio::test]
#[ignore = "needs the migrated reminder database: rs/read/tests/run_phase10_reminders.sh"]
async fn python_and_rust_deliver_each_others_reminders() {
    let mut db = Database::open().await;
    let party = db.party().await;
    let now = json!(iso(reference()));

    // A Python reminder with a fractional expiry: Rust's claim recheck
    // matches its stored epoch and sends Python's frozen payload once.
    let python_order = db
        .owned_order(
            &party,
            at(Duration::hours(6) + Duration::microseconds(777_777)),
        )
        .await;
    assert_eq!(python(json!({"command": "scan", "now": now}))["created"], 1);
    let python_notice = db.notices_for(python_order).await.remove(0);
    let sender = FakeSender::default();
    assert_eq!(db.deliver(&sender, reference()).await.sent, 1);
    assert_eq!(
        sender.calls(),
        [(
            python_notice.data["_email_payload"].clone(),
            expiry::idempotency_key(python_notice.id)
        )]
    );
    // Python's frozen payload is the payload Rust renders for the same text.
    assert_eq!(
        python_notice.data["_email_payload"],
        render_payload(&config(), &party.email, &python_notice.message)
    );
    assert_eq!(db.notices_for(python_order).await[0].status(), "sent");

    // A Rust reminder: Python's recheck accepts it, sends it and marks it sent.
    let rust_order = db
        .owned_order(&party, at(Duration::hours(7) + Duration::microseconds(1)))
        .await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    let rust_notice = db.notices_for(rust_order).await.remove(0);
    let delivered = python(json!({"command": "deliver", "now": now, "accept": true}));
    assert_eq!(delivered["sent"], 1);
    assert_eq!(
        delivered["calls"],
        json!([{
            "payload": rust_notice.data["_email_payload"],
            "key": expiry::idempotency_key(rust_notice.id),
        }])
    );
    assert_eq!(db.notices_for(rust_order).await[0].status(), "sent");

    // A lease written by Rust holds Python off, and Python's lease holds Rust off.
    let leased = db.owned_order(&party, at(Duration::hours(8))).await;
    assert_eq!(db.scan(reference(), 1000).await.created, 1);
    assert_eq!(
        db.deliver(&FakeSender::refusing(), reference())
            .await
            .deferred,
        1
    );
    let held_off = python(json!({"command": "deliver", "now": now, "accept": true}));
    assert_eq!(held_off["sent"], 0);
    assert_eq!(held_off["calls"], json!([]));
    let later = json!(iso(at(Duration::minutes(15))));
    let refused = python(json!({"command": "deliver", "now": later, "accept": false}));
    assert_eq!(refused["calls"].as_array().unwrap().len(), 1);
    let waiting = FakeSender::default();
    assert_eq!(
        db.deliver(&waiting, at(Duration::minutes(29))).await,
        Counts::default()
    );
    assert_eq!(
        db.deliver(&waiting, at(Duration::minutes(30))).await.sent,
        1
    );
    assert_eq!(db.notices_for(leased).await[0].status(), "sent");
}
