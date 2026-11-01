//! Phase 10 Task 28: the ordered alert handoff journal and its dispatcher
//! (`monitor alert`).
//!
//! Every case uses scratch directories, fake clocks and fake destinations, or a
//! loopback fake HTTP server; nothing reaches a real provider. Crash and refusal
//! cases run a real disposable process (this test binary, re-run as a child)
//! and stop it with a real signal. The handover cases run the unchanged Python
//! dispatcher (`deploy/monitor/alert_dispatch.py`) against the same files.
//!
//! The three `alert_handoff_*` identity cases check here what one identity can
//! check. The real three-identity suite (sysusers, tmpfiles, ACLs, unit
//! sandboxes) is `deploy/tests/alert-handoff-contract.sh --disposable-identities`,
//! whose cells carry the same names.
#![cfg(feature = "contract-tests")]
#[path = "support/monitor_fixture.rs"]
mod support;

use chrono::{DateTime, Duration as Span, TimeZone, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use support::{TempDir, contract_fixture, repository_root};
use tokio::sync::Notify;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::monitors::alert::{
    AlertSettings, BlockReason, Clock, Delivery, Destination, DispatchHook, DispatchStage,
    Dispatcher, DispatcherConfig, HttpTransport, Kind, Outcome, Resolution, SendOutcome, Transport,
    exit_for,
};
use verdaxis_read::cli::monitors::alert_journal::{
    AdmissionHook, AdmissionStage, Admitted, AlertObservation, AlertState, Check, EntryAccess,
    JournalAdmission, JournalLimits, JournalPaths, Producer, Refusal, Sequence, enqueue,
};
use verdaxis_read::cli::monitors::backup::{BackupConfig, run_at};

const SHORT_LOCK: Duration = Duration::from_millis(300);
const TOKEN: &str = "fake-bot-token-73a1c9";
const CHAT: &str = "fake-chat-5521";
const CHILD_ROLE: &str = "PHASE10_ALERT_CHILD";
const CHILD_DIR: &str = "PHASE10_ALERT_DIR";
const CHILD_STAGE: &str = "PHASE10_ALERT_STAGE";
const CHILD_STATE: &str = "PHASE10_ALERT_STATE";
const CHILD_MAX_ENTRIES: &str = "PHASE10_ALERT_MAX_ENTRIES";
const BACKUP_ID: &str = "20260720-030000";

// ---- fixtures -------------------------------------------------------------------------

/// One scratch layout: the alert state directory and both producers' state
/// directories, with short lock bounds.
struct Layout {
    /// Removes the scratch directory with the layout; a child process
    /// borrows its parent's directory and owns nothing.
    _owner: Option<TempDir>,
    base: PathBuf,
    config: DispatcherConfig,
}

impl Layout {
    fn new() -> Self {
        let owner = TempDir::new();
        let base = owner.path().to_path_buf();
        Self::at(Some(owner), base)
    }

    fn borrowed(base: PathBuf) -> Self {
        Self::at(None, base)
    }

    fn at(owner: Option<TempDir>, base: PathBuf) -> Self {
        let mut config =
            DispatcherConfig::new(base.join("alert"), base.join("health"), base.join("backup"));
        config.lock_timeout = SHORT_LOCK;
        Self {
            _owner: owner,
            base,
            config,
        }
    }

    fn path(&self) -> &Path {
        &self.base
    }

    fn journal(&self, check: Check) -> JournalPaths {
        self.config.journal(check).clone()
    }

    fn producer(&self, check: Check) -> Producer {
        Producer::new(self.journal(check)).with_limits(JournalLimits {
            lock_timeout: SHORT_LOCK,
            ..JournalLimits::default()
        })
    }

    fn enqueue(&self, check: Check, state: AlertState) -> Admitted {
        self.producer(check)
            .enqueue(&observation(check, state))
            .expect("admit the observation")
    }

    fn pending(&self, check: Check) -> Vec<u64> {
        let mut sequences: Vec<u64> =
            match std::fs::read_dir(self.journal(check).handoff_directory()) {
                Ok(reader) => reader
                    .filter_map(|item| item.ok())
                    .filter_map(|item| {
                        let name = item.file_name().to_string_lossy().into_owned();
                        name.strip_suffix(".json")?.parse().ok()
                    })
                    .collect(),
                Err(_) => Vec::new(),
            };
        sequences.sort_unstable();
        sequences
    }

    fn handoff_names(&self, check: Check) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.journal(check).handoff_directory())
            .map(|reader| {
                reader
                    .filter_map(|item| item.ok())
                    .map(|item| item.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn legacy(&self, check: Check) -> Option<Value> {
        read_json(&self.config.legacy_state(check))
    }

    fn delivery(&self, check: Check) -> Option<Value> {
        read_json(&self.config.delivery_state(check))
    }

    fn cursor(&self, check: Check) -> Value {
        read_json(&self.journal(check).cursor()).expect("cursor exists")
    }

    fn lock_mode(&self, check: Check) -> u32 {
        std::fs::metadata(self.config.delivery_lock(check))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    fn backup_config(&self) -> BackupConfig {
        let backups = self.base.join("backups");
        BackupConfig {
            backup_status: backups.join("status.json"),
            backup_directory: backups.clone(),
            attempt_directory: backups.join("attempts"),
            output: self.base.join("backup/status.json"),
            max_age_seconds: 30 * 60 * 60,
            future_tolerance_seconds: 300,
            max_timestamp_skew_seconds: 5,
            max_metadata_bytes: 16_384,
            max_attempt_records: 100,
            max_attempt_bytes: 4_096,
            max_artifacts: 100,
            max_artifact_bytes: 10_000_000,
            max_uncompressed_bytes: 10_000_000,
            total_timeout: Duration::from_secs(5),
        }
    }

    fn status_bytes(&self) -> Option<Vec<u8>> {
        std::fs::read(self.base.join("backup/status.json")).ok()
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    Some(serde_json::from_slice(&bytes).expect("a JSON document"))
}

fn observation(check: Check, state: AlertState) -> AlertObservation {
    AlertObservation {
        check,
        state,
        observed_at: Utc::now(),
    }
}

fn settings() -> AlertSettings {
    variables_settings(&[
        ("TELEGRAM_BOT_TOKEN", TOKEN),
        ("TELEGRAM_CHAT_ID", CHAT),
        ("TELEGRAM_API_BASE", "http://127.0.0.1:9"),
        ("HEALTHCHECKS_HEALTH_URL", "http://127.0.0.1:9/hc-health"),
        ("HEALTHCHECKS_BACKUP_URL", "http://127.0.0.1:9/hc-backup"),
    ])
}

fn variables_settings(pairs: &[(&str, &str)]) -> AlertSettings {
    let variables: BTreeMap<String, String> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    AlertSettings::from_variables(&variables)
}

/// A clock that only moves when told to, or when a delivery waits.
struct FakeClock {
    now: Mutex<DateTime<Utc>>,
    slept: Mutex<Vec<Duration>>,
}

impl FakeClock {
    fn at(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
            slept: Mutex::new(Vec::new()),
        }
    }

    /// The current time in whole seconds, so waits come out exact.
    fn real() -> Self {
        let now = Utc::now();
        Self::at(now - Span::nanoseconds(i64::from(now.timestamp_subsec_nanos())))
    }

    fn advance(&self, by: Span) {
        *self.now.lock().unwrap() += by;
    }

    fn slept(&self) -> Vec<Duration> {
        self.slept.lock().unwrap().clone()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, duration: Duration) {
        self.slept.lock().unwrap().push(duration);
        *self.now.lock().unwrap() += Span::from_std(duration).unwrap();
    }
}

/// One recorded send.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    check: Check,
    state: AlertState,
    kind: Kind,
    destination: Destination,
    sequence: Option<u64>,
    outcome: SendOutcome,
}

/// Fake destinations: scripted outcomes per destination (delivered by
/// default), every send recorded, and an optional gate that holds the first
/// send until the test releases it. With a log file, sends from several
/// processes are counted together.
#[derive(Default)]
struct FakeTransport {
    scripted: Mutex<HashMap<Destination, VecDeque<SendOutcome>>>,
    sends: Mutex<Vec<Sent>>,
    gate: Option<Arc<Gate>>,
    log: Option<PathBuf>,
}

#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl FakeTransport {
    fn logging(log: PathBuf) -> Self {
        Self {
            log: Some(log),
            ..Self::default()
        }
    }

    fn script(&self, destination: Destination, outcomes: &[SendOutcome]) {
        self.scripted
            .lock()
            .unwrap()
            .entry(destination)
            .or_default()
            .extend(outcomes.iter().copied());
    }

    fn sends(&self) -> Vec<Sent> {
        self.sends.lock().unwrap().clone()
    }

    fn delivered(&self) -> Vec<(AlertState, Kind, Destination)> {
        self.sends()
            .into_iter()
            .filter(|sent| sent.outcome == SendOutcome::Delivered)
            .map(|sent| (sent.state, sent.kind, sent.destination))
            .collect()
    }
}

impl Transport for FakeTransport {
    async fn send(&self, _settings: &AlertSettings, delivery: &Delivery) -> SendOutcome {
        if let Some(gate) = &self.gate
            && gate.armed.swap(false, Ordering::SeqCst)
        {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        let outcome = self
            .scripted
            .lock()
            .unwrap()
            .get_mut(&delivery.destination)
            .and_then(VecDeque::pop_front)
            .unwrap_or(SendOutcome::Delivered);
        let sent = Sent {
            check: delivery.check,
            state: delivery.state,
            kind: delivery.kind,
            destination: delivery.destination,
            sequence: delivery.sequence.map(|sequence| sequence.0),
            outcome,
        };
        if let Some(log) = &self.log {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .unwrap();
            writeln!(file, "{}", log_line(&sent)).unwrap();
        }
        self.sends.lock().unwrap().push(sent);
        outcome
    }
}

fn log_line(sent: &Sent) -> String {
    format!(
        "{} {} {} {} {}",
        sent.check.as_str(),
        sent.state.as_str(),
        sent.kind.as_str(),
        sent.destination.as_str(),
        sent.sequence
            .map_or("-".to_owned(), |sequence| sequence.to_string())
    )
}

fn logged(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn dispatcher<'a, T: Transport>(
    layout: &'a Layout,
    settings: &'a AlertSettings,
    transport: &'a T,
    clock: &'a FakeClock,
) -> Dispatcher<'a, T, FakeClock> {
    Dispatcher::new(&layout.config, settings, transport, clock)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Hold an exclusive `flock` on `path` until the guard is dropped.
fn hold_lock(path: &Path) -> std::fs::File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    file.try_lock().expect("take the lock for the test");
    file
}

// ---- backup fixtures for real producer processes ------------------------------------

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).unwrap()
}

fn gzip(content: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(content).unwrap();
    encoder.finish().unwrap()
}

/// A complete healthy backup generation, as the Task 27 tests build it.
fn write_healthy_backup(layout: &Layout) {
    let directory = layout.base.join("backups");
    std::fs::create_dir_all(directory.join("attempts")).unwrap();
    let at = fixed_now() - Span::hours(1);
    let metadata = json!({
        "ok": true,
        "completed_at": at.to_rfc3339(),
        "backup_id": BACKUP_ID,
        "databases": ["verdaxis_prod", "verdaxis_staging", "umami"],
    });
    std::fs::write(directory.join("status.json"), metadata.to_string()).unwrap();
    let archive = gzip(&std::fs::read(contract_fixture("backup/sample-dump.sql")).unwrap());
    for prefix in ["verdaxis-prod", "verdaxis-staging", "umami"] {
        let path = directory.join(format!("{prefix}-{BACKUP_ID}.sql.gz"));
        std::fs::write(&path, &archive).unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(at.timestamp() as u64);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }
    let attempt = json!({
        "schema_version": 1,
        "attempt_id": format!("attempt-{BACKUP_ID}"),
        "state": "succeeded",
        "started_at": (at - Span::minutes(5)).to_rfc3339(),
        "finished_at": at.to_rfc3339(),
        "backup_id": BACKUP_ID,
    });
    std::fs::write(
        directory.join(format!("attempts/attempt-{BACKUP_ID}.json")),
        attempt.to_string(),
    )
    .unwrap();
}

/// Remove the backup generation, so the next observation is a failure.
fn break_backup(layout: &Layout) {
    let _ = std::fs::remove_dir_all(layout.base.join("backups"));
}

// ---- child processes ------------------------------------------------------------------

/// Stop the process at one admission stage with a real signal.
struct AbortAt(AdmissionStage);

impl AdmissionHook for AbortAt {
    fn reached(&self, stage: AdmissionStage) {
        if stage == self.0 {
            std::process::abort();
        }
    }
}

/// Stop the process at one delivery stage with a real signal.
struct AbortAtDispatch(DispatchStage);

impl DispatchHook for AbortAtDispatch {
    fn reached(&self, stage: DispatchStage) {
        if stage == self.0 {
            std::process::abort();
        }
    }
}

fn admission_stage(name: &str) -> AdmissionStage {
    match name {
        "lock_held" => AdmissionStage::LockHeld,
        "reserved" => AdmissionStage::Reserved,
        "before_rename" => AdmissionStage::BeforeRename,
        "after_rename" => AdmissionStage::AfterRename,
        "after_directory_sync" => AdmissionStage::AfterDirectorySync,
        "recorded" => AdmissionStage::Recorded,
        other => panic!("unknown stage {other}"),
    }
}

/// Entry point of the disposable child processes. A normal test run (no role
/// in the environment) does nothing here.
#[test]
fn child_process_entry() {
    let Ok(role) = std::env::var(CHILD_ROLE) else {
        return;
    };
    let layout = Layout::borrowed(PathBuf::from(std::env::var(CHILD_DIR).unwrap()));
    let stage = std::env::var(CHILD_STAGE).ok();
    match role.as_str() {
        // A real backup monitor run whose status passes through the journal.
        "produce-backup" => {
            let mut limits = JournalLimits {
                lock_timeout: SHORT_LOCK,
                ..JournalLimits::default()
            };
            if let Ok(maximum) = std::env::var(CHILD_MAX_ENTRIES) {
                limits.max_entries = maximum.parse().unwrap();
            }
            let mut producer = Producer::new(layout.journal(Check::Backup)).with_limits(limits);
            if let Some(stage) = &stage {
                producer = producer.with_hook(Arc::new(AbortAt(admission_stage(stage))));
            }
            let exit = run_at(
                &layout.backup_config(),
                &JournalAdmission::new(producer),
                fixed_now(),
            );
            std::process::exit(exit as i32);
        }
        // A bare admission of one observation.
        "produce-observation" => {
            let state = match std::env::var(CHILD_STATE).unwrap().as_str() {
                "failure" => AlertState::Failure,
                _ => AlertState::Healthy,
            };
            let mut producer = layout.producer(Check::Health);
            if let Some(stage) = &stage {
                producer = producer.with_hook(Arc::new(AbortAt(admission_stage(stage))));
            }
            let code = match producer.enqueue(&observation(Check::Health, state)) {
                Ok(_) => 0,
                Err(refusal) => refusal.exit() as i32,
            };
            std::process::exit(code);
        }
        // A dispatcher wake that logs its sends and stops at one stage.
        "dispatch-health" => {
            let transport = FakeTransport::logging(layout.base.join("sends.log"));
            let clock = FakeClock::real();
            let settings = settings();
            let hook = AbortAtDispatch(match stage.as_deref() {
                Some("in_flight_telegram") => DispatchStage::InFlight(Destination::Telegram),
                Some("legacy_published") => DispatchStage::LegacyPublished,
                Some("acknowledged") => DispatchStage::Acknowledged,
                other => panic!("unknown dispatch stage {other:?}"),
            });
            let result = runtime().block_on(
                dispatcher(&layout, &settings, &transport, &clock)
                    .with_hook(&hook)
                    .wake(Check::Health),
            );
            std::process::exit(exit_for(&result) as i32);
        }
        other => panic!("unknown child role {other}"),
    }
}

/// Run this test binary as a disposable child with `role` on `layout`.
fn child(layout: &Layout, role: &str, extra: &[(&str, &str)]) -> Output {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "child_process_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ROLE, role)
        .env(CHILD_DIR, layout.path())
        .env_remove(CHILD_STAGE)
        .env_remove(CHILD_MAX_ENTRIES)
        .stdin(Stdio::null());
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().expect("start the child process")
}

fn killed_by_abort(output: &Output) -> bool {
    output.status.signal() == Some(libc::SIGABRT)
}

// ---- Python dispatcher ---------------------------------------------------------------

fn python() -> String {
    std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

fn python_dispatcher() -> PathBuf {
    repository_root().join("deploy/monitor/alert_dispatch.py")
}

/// Run the unchanged Python dispatcher with Telegram pointed at `server`.
fn run_python(arguments: &[&str], alert_directory: &Path, server: &FakeHttp) -> Output {
    Command::new(python())
        .arg(python_dispatcher())
        .args(arguments)
        .arg("--state-directory")
        .arg(alert_directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TELEGRAM_BOT_TOKEN", TOKEN)
        .env("TELEGRAM_CHAT_ID", CHAT)
        .env("TELEGRAM_API_BASE", server.base())
        .output()
        .expect("run the Python dispatcher")
}

/// Python's own validity check of a state document.
fn python_accepts_state(path: &Path) -> bool {
    let script = "import sys, json, datetime; sys.path.insert(0, sys.argv[1]); \
                  import alert_dispatch; \
                  payload = json.load(open(sys.argv[2])); \
                  now = datetime.datetime.now(datetime.timezone.utc); \
                  print(alert_dispatch.valid_alert_state(payload, now))";
    let output = Command::new(python())
        .args(["-c", script])
        .arg(repository_root().join("deploy/monitor"))
        .arg(path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run Python");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).trim() == "True"
}

/// A loopback HTTP server standing in for Telegram and Healthchecks: it records
/// every request and answers with the configured status.
struct FakeHttp {
    port: u16,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    status: Arc<AtomicU16>,
}

impl FakeHttp {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(AtomicU16::new(200));
        let (recorded, answer) = (requests.clone(), status.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                recorded
                    .lock()
                    .unwrap()
                    .push((path, String::from_utf8_lossy(&body).into_owned()));
                let code = answer.load(Ordering::SeqCst);
                let reply = if code == 200 {
                    "{\"ok\":true}"
                } else {
                    "{\"ok\":false}"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self {
            port,
            requests,
            status,
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn settings(&self) -> AlertSettings {
        let base = self.base();
        variables_settings(&[
            ("TELEGRAM_BOT_TOKEN", TOKEN),
            ("TELEGRAM_CHAT_ID", CHAT),
            ("TELEGRAM_API_BASE", &base),
        ])
    }

    /// The Telegram texts received, in order.
    fn texts(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.ends_with("/sendMessage"))
            .map(|(_, body)| {
                let value: Value = serde_json::from_str(body).unwrap();
                value["text"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    fn set_status(&self, status: u16) {
        self.status.store(status, Ordering::SeqCst);
    }
}

// ---- ordering ---------------------------------------------------------------------------

#[test]
fn alert_dispatch_serializes_events() {
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let healthy = layout
        .enqueue(Check::Health, AlertState::Healthy)
        .sequence();
    assert!(healthy > failure);
    let gate = Arc::new(Gate::default());
    gate.armed.store(true, Ordering::SeqCst);
    let transport = FakeTransport {
        gate: Some(gate.clone()),
        ..FakeTransport::default()
    };
    // The first attempt of the failure times out without taking effect.
    transport.script(Destination::Healthchecks, &[SendOutcome::NotDelivered]);
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);

    let (first, second) = runtime().block_on(async {
        let first = dispatcher.wake(Check::Health);
        let second = async {
            // The failure wakeup holds the delivery lock inside its send; the
            // healthy wakeup waits for the lock, times out and exits 4.
            gate.entered.notified().await;
            let result = dispatcher.wake(Check::Health).await;
            gate.release.notify_one();
            result
        };
        tokio::join!(first, second)
    });
    assert_eq!(first, Ok(Outcome::RetryLater(failure)));
    assert_eq!(second, Err(Exit::Dependency));
    assert_eq!(exit_for(&second), Exit::Dependency);
    assert_eq!(layout.pending(Check::Health), vec![failure.0, healthy.0]);

    // During the restart delay the healthy wakeup runs first: it must attempt
    // the failure (sequence N), never the later healthy entry.
    let third = runtime().block_on(dispatcher.wake(Check::Health));
    assert_eq!(third, Ok(Outcome::Delivered(failure)));
    assert!(
        transport
            .sends()
            .iter()
            .all(|sent| sent.sequence == Some(failure.0)),
        "only sequence N was attempted so far: {:?}",
        transport.sends()
    );
    // The retried failure wakeup now delivers N+1.
    let fourth = runtime().block_on(dispatcher.wake(Check::Health));
    assert_eq!(fourth, Ok(Outcome::Delivered(healthy)));
    assert_eq!(
        transport.delivered(),
        vec![
            (AlertState::Failure, Kind::Failure, Destination::Telegram),
            (
                AlertState::Failure,
                Kind::Failure,
                Destination::Healthchecks
            ),
            (
                AlertState::Healthy,
                Kind::Recovery,
                Destination::Healthchecks
            ),
            (AlertState::Healthy, Kind::Recovery, Destination::Telegram),
        ]
    );
    let legacy = layout.legacy(Check::Health).unwrap();
    assert_eq!(legacy["state"], "healthy");
    for destination in ["healthchecks", "telegram"] {
        assert!(legacy["receipts"][destination]["failure"].is_object());
        assert!(legacy["receipts"][destination]["healthy"].is_object());
    }
    assert!(layout.pending(Check::Health).is_empty());
    assert_eq!(
        layout.delivery(Check::Health).unwrap()["acknowledged"]["sequence"],
        json!(healthy.0)
    );
}

#[test]
fn alert_healthy_wakeup_before_failure_retry_cannot_overtake() {
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let healthy = layout
        .enqueue(Check::Backup, AlertState::Healthy)
        .sequence();
    let transport = FakeTransport::default();
    transport.script(Destination::Telegram, &[SendOutcome::NotDelivered]);
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();

    // The `backup-failure` wakeup fails its first attempt.
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::RetryLater(failure))
    );
    // The `backup-healthy` wakeup runs before the failure's restart.
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(failure))
    );
    // The confirmed Healthchecks send is not repeated; the retry waited 5 s.
    let attempted: Vec<(Option<u64>, Destination)> = transport
        .sends()
        .iter()
        .map(|sent| (sent.sequence, sent.destination))
        .collect();
    assert_eq!(
        attempted,
        vec![
            (Some(failure.0), Destination::Healthchecks),
            (Some(failure.0), Destination::Telegram),
            (Some(failure.0), Destination::Telegram),
        ]
    );
    assert_eq!(clock.slept(), vec![Duration::from_secs(5)]);
    // The failure's own retry then reaches the healthy transition.
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(healthy))
    );
    let states: Vec<AlertState> = transport
        .delivered()
        .iter()
        .map(|(state, _, _)| *state)
        .collect();
    assert_eq!(
        states,
        vec![
            AlertState::Failure,
            AlertState::Failure,
            AlertState::Healthy,
            AlertState::Healthy
        ]
    );
    assert_eq!(layout.legacy(Check::Backup).unwrap()["state"], "healthy");
}

#[test]
fn alert_different_checks_do_not_share_a_global_lock() {
    let layout = Layout::new();
    layout.enqueue(Check::Health, AlertState::Failure);
    let backup = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();

    let held = hold_lock(&layout.config.delivery_lock(Check::Health));
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(backup))
    );
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Err(Exit::Dependency)
    );
    drop(held);

    // A held health journal lock does not stop the backup producer either.
    let held = hold_lock(&layout.journal(Check::Health).journal_lock());
    assert!(matches!(
        layout
            .producer(Check::Backup)
            .enqueue(&observation(Check::Backup, AlertState::Healthy)),
        Ok(Admitted::Committed(_))
    ));
    assert_eq!(
        layout
            .producer(Check::Health)
            .enqueue(&observation(Check::Health, AlertState::Healthy)),
        Err(Refusal::LockTimeout)
    );
    drop(held);
    assert!(
        transport
            .sends()
            .iter()
            .all(|sent| sent.check == Check::Backup)
    );
}

#[test]
fn alert_delivery_lock_timeout_retains_admitted_head() {
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();

    let held = hold_lock(&layout.config.delivery_lock(Check::Health));
    let started = std::time::Instant::now();
    let result = rt.block_on(dispatcher.wake(Check::Health));
    assert_eq!(result, Err(Exit::Dependency));
    assert_eq!(
        exit_for(&result) as i32,
        4,
        "a lock timeout is never a no-op success"
    );
    assert!(started.elapsed() >= SHORT_LOCK);
    assert_eq!(layout.pending(Check::Health), vec![failure.0]);
    assert!(layout.delivery(Check::Health).is_none());
    assert!(transport.sends().is_empty());
    drop(held);

    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    assert!(layout.pending(Check::Health).is_empty());
}

// ---- crash, exhaustion and uncertainty --------------------------------------------------

#[test]
fn alert_crash_resume_retains_head_and_confirmed_destination_progress() {
    // A crash while the Telegram send is in flight.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let output = child(
        &layout,
        "dispatch-health",
        &[(CHILD_STAGE, "in_flight_telegram")],
    );
    assert!(killed_by_abort(&output), "{output:?}");
    let log = layout.base.join("sends.log");
    assert_eq!(
        logged(&log),
        vec![format!("health failure FAILURE healthchecks {}", failure.0)]
    );
    let progress = layout.delivery(Check::Health).unwrap();
    assert_eq!(progress["head"]["sequence"], json!(failure.0));
    assert_eq!(progress["head"]["attempts"], json!(1));
    assert_eq!(
        progress["head"]["destinations"]["healthchecks"]["status"],
        "delivered"
    );
    assert_eq!(
        progress["head"]["destinations"]["telegram"]["status"],
        "in_flight"
    );
    assert_eq!(layout.pending(Check::Health), vec![failure.0]);

    // A restart cannot know whether Telegram got it: the check blocks.
    let transport = FakeTransport::logging(log.clone());
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    let blocked = rt.block_on(dispatcher.wake(Check::Health));
    assert_eq!(
        blocked,
        Ok(Outcome::Blocked(failure, BlockReason::Uncertain))
    );
    assert_eq!(exit_for(&blocked), Exit::Ambiguous);
    assert!(transport.sends().is_empty());

    // The operator finds it was not delivered; only Telegram is sent again.
    dispatcher
        .reconcile(
            Check::Health,
            failure,
            Destination::Telegram,
            Resolution::NotDelivered,
        )
        .unwrap();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(
        logged(&log),
        vec![
            format!("health failure FAILURE healthchecks {}", failure.0),
            format!("health failure FAILURE telegram {}", failure.0),
        ]
    );
    assert!(layout.pending(Check::Health).is_empty());

    // A crash after the state document is published but before the
    // acknowledgement resumes without any send.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let output = child(
        &layout,
        "dispatch-health",
        &[(CHILD_STAGE, "legacy_published")],
    );
    assert!(killed_by_abort(&output), "{output:?}");
    let log = layout.base.join("sends.log");
    assert_eq!(logged(&log).len(), 2);
    assert_eq!(layout.legacy(Check::Health).unwrap()["state"], "failure");
    assert_eq!(layout.pending(Check::Health), vec![failure.0]);
    let transport = FakeTransport::logging(log.clone());
    let dispatcher = self::dispatcher(&layout, &settings, &transport, &clock);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(logged(&log).len(), 2, "no send repeated after the crash");
    assert!(layout.pending(Check::Health).is_empty());
}

#[test]
fn alert_exhaustion_or_uncertain_send_blocks_later_sequence_until_reconciled() {
    // Exhaustion: three failed attempts, 5 s apart, then a block.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let healthy = layout
        .enqueue(Check::Health, AlertState::Healthy)
        .sequence();
    let transport = FakeTransport::default();
    transport.script(Destination::Telegram, &[SendOutcome::NotDelivered; 3]);
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::RetryLater(failure))
    );
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::RetryLater(failure))
    );
    let exhausted = rt.block_on(dispatcher.wake(Check::Health));
    assert_eq!(
        exhausted,
        Ok(Outcome::Blocked(failure, BlockReason::Exhausted))
    );
    assert_eq!(exit_for(&exhausted), Exit::Precondition);
    assert_eq!(clock.slept(), vec![Duration::from_secs(5); 2]);
    // Blocked: no send at all, and the healthy sequence never moves ahead.
    let before = transport.sends().len();
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Blocked(failure, BlockReason::Exhausted))
    );
    assert_eq!(transport.sends().len(), before);
    assert!(
        transport
            .sends()
            .iter()
            .all(|sent| sent.sequence == Some(failure.0))
    );
    assert_eq!(layout.pending(Check::Health), vec![failure.0, healthy.0]);
    // Re-arm only the exhausted sequence; a wrong one is refused.
    assert_eq!(
        dispatcher.rearm(Check::Health, healthy),
        Err(Exit::Precondition)
    );
    dispatcher.rearm(Check::Health, failure).unwrap();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(healthy))
    );
    assert_eq!(layout.legacy(Check::Health).unwrap()["state"], "healthy");

    // Uncertainty: an unknown result blocks until reconciled, with no resend.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let healthy = layout
        .enqueue(Check::Backup, AlertState::Healthy)
        .sequence();
    let transport = FakeTransport::default();
    transport.script(Destination::Telegram, &[SendOutcome::Uncertain]);
    let dispatcher = self::dispatcher(&layout, &settings, &transport, &clock);
    let uncertain = rt.block_on(dispatcher.wake(Check::Backup));
    assert_eq!(
        uncertain,
        Ok(Outcome::Blocked(failure, BlockReason::Uncertain))
    );
    assert_eq!(exit_for(&uncertain), Exit::Ambiguous);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Blocked(failure, BlockReason::Uncertain))
    );
    assert_eq!(transport.sends().len(), 2);
    // Re-arming does not settle an uncertain send.
    assert_eq!(
        dispatcher.rearm(Check::Backup, failure),
        Err(Exit::Precondition)
    );
    dispatcher
        .reconcile(
            Check::Backup,
            failure,
            Destination::Telegram,
            Resolution::Delivered,
        )
        .unwrap();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(
        transport.sends().len(),
        2,
        "a reconciled send is not repeated"
    );
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(healthy))
    );
    let legacy = layout.legacy(Check::Backup).unwrap();
    assert_eq!(legacy["state"], "healthy");
    assert!(legacy["receipts"]["telegram"]["failure"].is_object());
}

// ---- admission -------------------------------------------------------------------------

#[test]
fn alert_enqueue_is_durable_idempotent_and_bounded_before_status_publication() {
    let layout = Layout::new();
    let producer = layout.producer(Check::Health);
    let first = enqueue(&producer, observation(Check::Health, AlertState::Failure)).unwrap();
    let entry_path = layout.journal(Check::Health).entry(first);
    let entry: Value = read_json(&entry_path).unwrap();
    assert_eq!(entry["check"], "health");
    assert_eq!(entry["state"], "failure");
    assert_eq!(entry["sequence"], json!(first.0));
    assert_eq!(entry["schema_version"], json!(1));
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&entry_path), 0o600);
    assert_eq!(mode(&layout.journal(Check::Health).cursor()), 0o600);
    assert_eq!(
        mode(&layout.journal(Check::Health).handoff_directory()),
        0o700
    );
    assert!(std::fs::metadata(&entry_path).unwrap().len() <= 4096);
    let cursor = layout.cursor(Check::Health);
    assert_eq!(cursor["admitted"]["sequence"], json!(first.0));
    assert_eq!(cursor["reserved"], Value::Null);

    // The same state again is already admitted: no second entry.
    assert_eq!(
        producer.enqueue(&observation(Check::Health, AlertState::Failure)),
        Ok(Admitted::AlreadyAdmitted(first))
    );
    assert_eq!(layout.pending(Check::Health), vec![first.0]);
    let second = layout.enqueue(Check::Health, AlertState::Healthy);
    assert!(matches!(second, Admitted::Committed(sequence) if sequence > first));

    // A monitor publishes its status only after the admission is committed.
    let layout = Layout::new();
    struct StatusAbsentAtCommit(PathBuf);
    impl AdmissionHook for StatusAbsentAtCommit {
        fn reached(&self, stage: AdmissionStage) {
            if stage == AdmissionStage::Recorded {
                assert!(!self.0.exists(), "status published before admission");
            }
        }
    }
    let watched = layout
        .producer(Check::Backup)
        .with_hook(Arc::new(StatusAbsentAtCommit(
            layout.base.join("backup/status.json"),
        )));
    let exit = run_at(
        &layout.backup_config(),
        &JournalAdmission::new(watched),
        fixed_now(),
    );
    assert_eq!(exit, Exit::Usage, "a failed backup exits 2 after admission");
    assert!(layout.status_bytes().is_some());
    assert_eq!(layout.pending(Check::Backup).len(), 1);

    // Bounded: a full journal refuses and nothing is published.
    let layout = Layout::new();
    let small = layout.producer(Check::Backup).with_limits(JournalLimits {
        max_entries: 2,
        lock_timeout: SHORT_LOCK,
        ..JournalLimits::default()
    });
    small
        .enqueue(&observation(Check::Backup, AlertState::Failure))
        .unwrap();
    small
        .enqueue(&observation(Check::Backup, AlertState::Healthy))
        .unwrap();
    assert_eq!(
        small.enqueue(&observation(Check::Backup, AlertState::Failure)),
        Err(Refusal::Full)
    );
    let exit = run_at(
        &layout.backup_config(),
        &JournalAdmission::new(small.clone()),
        fixed_now(),
    );
    assert_eq!(exit, Exit::Dependency);
    assert!(layout.status_bytes().is_none());
    assert_eq!(layout.pending(Check::Backup).len(), 2);
    let byte_bound = layout.producer(Check::Health).with_limits(JournalLimits {
        max_bytes: 64,
        lock_timeout: SHORT_LOCK,
        ..JournalLimits::default()
    });
    assert_eq!(
        byte_bound.enqueue(&observation(Check::Health, AlertState::Failure)),
        Err(Refusal::Full)
    );
}

#[test]
fn alert_full_journal_refuses_before_admission_then_converges() {
    let layout = Layout::new();
    write_healthy_backup(&layout);
    // The prior status is healthy and its observation was delivered.
    let first = child(&layout, "produce-backup", &[]);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    assert!(matches!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(_))
    ));
    // Fill the journal (limit 4) with admitted transitions.
    let small = layout.producer(Check::Backup).with_limits(JournalLimits {
        max_entries: 4,
        lock_timeout: SHORT_LOCK,
        ..JournalLimits::default()
    });
    for state in [
        AlertState::Failure,
        AlertState::Healthy,
        AlertState::Failure,
        AlertState::Healthy,
    ] {
        small.enqueue(&observation(Check::Backup, state)).unwrap();
    }
    let journal_before = layout.handoff_names(Check::Backup);
    let status_before = layout.status_bytes().unwrap();

    // A real producer observes a failure and is refused before admission.
    break_backup(&layout);
    let refused = child(&layout, "produce-backup", &[(CHILD_MAX_ENTRIES, "4")]);
    assert_eq!(refused.status.code(), Some(4), "{refused:?}");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("publish_refused"));
    assert_eq!(layout.handoff_names(Check::Backup), journal_before);
    assert_eq!(layout.status_bytes().unwrap(), status_before);

    // Capacity returns as the dispatcher drains the admitted entries.
    for _ in 0..4 {
        assert!(matches!(
            rt.block_on(dispatcher.wake(Check::Backup)),
            Ok(Outcome::Delivered(_))
        ));
    }
    assert!(layout.pending(Check::Backup).is_empty());
    // The fixture is healthy again; a fresh producer observes it.
    write_healthy_backup(&layout);
    let fresh = child(&layout, "produce-backup", &[(CHILD_MAX_ENTRIES, "4")]);
    assert_eq!(fresh.status.code(), Some(0), "{fresh:?}");
    while let Ok(Outcome::Delivered(_)) = rt.block_on(dispatcher.wake(Check::Backup)) {}
    assert!(layout.pending(Check::Backup).is_empty());
    assert_eq!(layout.legacy(Check::Backup).unwrap()["state"], "healthy");
    // The refused failure was never promised: four admitted transitions only.
    let failures = transport
        .delivered()
        .iter()
        .filter(|(state, _, _)| *state == AlertState::Failure)
        .count();
    assert_eq!(failures, 4, "two admitted failures, two destinations each");
}

#[test]
fn alert_journal_lock_timeout_refuses_before_admission_then_converges() {
    for prior in [AlertState::Healthy, AlertState::Failure] {
        let layout = Layout::new();
        if prior == AlertState::Healthy {
            write_healthy_backup(&layout);
        }
        let first = child(&layout, "produce-backup", &[]);
        let expected = if prior == AlertState::Healthy { 0 } else { 2 };
        assert_eq!(first.status.code(), Some(expected), "{first:?}");
        let transport = FakeTransport::default();
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        let rt = runtime();
        assert!(matches!(
            rt.block_on(dispatcher.wake(Check::Backup)),
            Ok(Outcome::Delivered(_))
        ));
        let status_before = layout.status_bytes().unwrap();

        // The journal lock is held: a healthy and a failing observation are
        // both refused with exit 4, no entry and an unchanged status.
        let held = hold_lock(&layout.journal(Check::Backup).journal_lock());
        for healthy in [true, false] {
            if healthy {
                write_healthy_backup(&layout);
            } else {
                break_backup(&layout);
            }
            let refused = child(&layout, "produce-backup", &[]);
            assert_eq!(refused.status.code(), Some(4), "{refused:?}");
            assert!(layout.pending(Check::Backup).is_empty());
            assert_eq!(layout.status_bytes().unwrap(), status_before);
        }
        drop(held);

        // Released and healthy: the fresh observation converges.
        write_healthy_backup(&layout);
        let fresh = child(&layout, "produce-backup", &[]);
        assert_eq!(fresh.status.code(), Some(0), "{fresh:?}");
        while let Ok(Outcome::Delivered(_)) = rt.block_on(dispatcher.wake(Check::Backup)) {}
        assert!(layout.pending(Check::Backup).is_empty());
        assert_eq!(layout.legacy(Check::Backup).unwrap()["state"], "healthy");
        let kinds: Vec<Kind> = transport
            .delivered()
            .iter()
            .map(|(_, kind, _)| *kind)
            .collect();
        if prior == AlertState::Failure {
            assert_eq!(
                kinds,
                vec![Kind::Failure, Kind::Failure, Kind::Recovery, Kind::Recovery]
            );
        } else {
            assert!(kinds.is_empty(), "a healthy start sends nothing: {kinds:?}");
        }
    }
}

#[test]
fn alert_producer_death_before_commit_then_changed_observation_converges() {
    for stage in ["reserved", "before_rename"] {
        let layout = Layout::new();
        write_healthy_backup(&layout);
        assert_eq!(child(&layout, "produce-backup", &[]).status.code(), Some(0));
        let transport = FakeTransport::default();
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        let rt = runtime();
        while let Ok(Outcome::Delivered(_)) = rt.block_on(dispatcher.wake(Check::Backup)) {}
        let status_before = layout.status_bytes().unwrap();

        // The producer observes a failure and dies before the rename.
        break_backup(&layout);
        let died = child(&layout, "produce-backup", &[(CHILD_STAGE, stage)]);
        assert!(killed_by_abort(&died), "{stage}: {died:?}");
        assert!(layout.pending(Check::Backup).is_empty(), "{stage}");
        assert_eq!(layout.status_bytes().unwrap(), status_before);

        // A fresh producer observes healthy; the incomplete temp is quarantined.
        write_healthy_backup(&layout);
        assert_eq!(child(&layout, "produce-backup", &[]).status.code(), Some(0));
        assert!(
            layout
                .handoff_names(Check::Backup)
                .iter()
                .all(|name| !name.ends_with(".tmp")),
            "{stage}: {:?}",
            layout.handoff_names(Check::Backup)
        );
        while let Ok(Outcome::Delivered(_)) = rt.block_on(dispatcher.wake(Check::Backup)) {}
        assert_eq!(layout.legacy(Check::Backup).unwrap()["state"], "healthy");
        assert!(
            transport
                .sends()
                .iter()
                .all(|sent| sent.state == AlertState::Healthy),
            "{stage}: the lost failure is not replayed"
        );
    }

    // A failure committed before the producer died (and before a reboot that
    // lost its wakeup) is delivered by the reminder before the later healthy.
    for stage in ["after_directory_sync", "recorded"] {
        let layout = Layout::new();
        let died = child(&layout, "produce-backup", &[(CHILD_STAGE, stage)]);
        assert!(killed_by_abort(&died), "{stage}: {died:?}");
        assert!(layout.status_bytes().is_none(), "status follows admission");
        let committed = layout.pending(Check::Backup);
        assert_eq!(committed.len(), 1);
        write_healthy_backup(&layout);
        assert_eq!(child(&layout, "produce-backup", &[]).status.code(), Some(0));
        let pending = layout.pending(Check::Backup);
        assert_eq!(pending.len(), 2, "{stage}: one failure and one healthy");
        assert_eq!(pending[0], committed[0]);

        let transport = FakeTransport::default();
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        let rt = runtime();
        assert_eq!(
            rt.block_on(dispatcher.remind(Check::Backup)),
            Ok(Outcome::Delivered(Sequence(pending[0])))
        );
        assert_eq!(
            rt.block_on(dispatcher.remind(Check::Backup)),
            Ok(Outcome::Delivered(Sequence(pending[1])))
        );
        let kinds: Vec<Kind> = transport
            .delivered()
            .iter()
            .map(|(_, kind, _)| *kind)
            .collect();
        assert_eq!(
            kinds,
            vec![Kind::Failure, Kind::Failure, Kind::Recovery, Kind::Recovery]
        );
    }
}

#[test]
fn alert_recovery_after_final_name_rename_completes_fsync_and_bookkeeping() {
    for stage in ["before_rename", "after_rename", "after_directory_sync"] {
        let layout = Layout::new();
        let died = child(
            &layout,
            "produce-observation",
            &[(CHILD_STAGE, stage), (CHILD_STATE, "failure")],
        );
        assert!(killed_by_abort(&died), "{stage}: {died:?}");
        let cursor = layout.cursor(Check::Health);
        let reserved = cursor["reserved"].as_u64().expect("reservation recorded");
        let renamed = stage != "before_rename";
        assert_eq!(layout.pending(Check::Health).len(), usize::from(renamed));

        // The same observation retried: an in-doubt admission is recovered
        // under the lock, never admitted twice.
        let retried = layout.enqueue(Check::Health, AlertState::Failure);
        if renamed {
            assert_eq!(
                retried,
                Admitted::AlreadyAdmitted(Sequence(reserved)),
                "{stage}"
            );
        } else {
            assert!(
                matches!(retried, Admitted::Committed(sequence) if sequence.0 > reserved),
                "{stage}: {retried:?}"
            );
        }
        let cursor = layout.cursor(Check::Health);
        assert_eq!(cursor["reserved"], Value::Null, "{stage}");
        assert_eq!(cursor["admitted"]["state"], "failure", "{stage}");
        assert_eq!(layout.pending(Check::Health).len(), 1, "{stage}");
        assert!(
            layout
                .handoff_names(Check::Health)
                .iter()
                .all(|name| !name.starts_with('.')),
            "{stage}: temp quarantined"
        );

        // The later healthy observation follows it, in order.
        layout.enqueue(Check::Health, AlertState::Healthy);
        let transport = FakeTransport::default();
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        let rt = runtime();
        while let Ok(Outcome::Delivered(_)) = rt.block_on(dispatcher.wake(Check::Health)) {}
        let states: Vec<AlertState> = transport
            .delivered()
            .iter()
            .filter(|(_, _, destination)| *destination == Destination::Telegram)
            .map(|(state, _, _)| *state)
            .collect();
        assert_eq!(
            states,
            vec![AlertState::Failure, AlertState::Healthy],
            "{stage}"
        );
    }
}

#[test]
fn alert_failed_enqueue_has_failed_unit_and_no_status_publication() {
    let layout = Layout::new();
    write_healthy_backup(&layout);
    assert_eq!(child(&layout, "produce-backup", &[]).status.code(), Some(0));
    let status_before = layout.status_bytes().unwrap();
    // A permission refusal: the handoff directory is not writable.
    let handoff = layout.journal(Check::Backup).handoff_directory();
    std::fs::set_permissions(&handoff, std::fs::Permissions::from_mode(0o500)).unwrap();
    break_backup(&layout);
    let refused = child(&layout, "produce-backup", &[]);
    std::fs::set_permissions(&handoff, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        refused.status.code(),
        Some(4),
        "the unit fails: {refused:?}"
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("\"publish_refused\""));
    assert_eq!(layout.status_bytes().unwrap(), status_before);
    assert_eq!(
        layout.pending(Check::Backup).len(),
        1,
        "only the first admission"
    );

    // The reviewed units keep a refusal visible: no success override, no
    // ignored command failure, and check-only wakeups.
    for (unit, check) in [
        ("verdaxis-health.service", "health"),
        ("verdaxis-backup-verify.service", "backup"),
    ] {
        let text =
            std::fs::read_to_string(contract_fixture(&format!("alert-handoff/{unit}"))).unwrap();
        assert!(!text.contains("SuccessExitStatus"), "{unit}");
        assert!(!text.contains("ExecStart=-"), "{unit}");
        assert!(!text.contains("Restart="), "{unit}");
        assert!(text.contains(&format!(
            "OnFailure=verdaxis-monitor-alert@{check}.service\n"
        )));
        assert!(text.contains(&format!(
            "OnSuccess=verdaxis-monitor-alert@{check}.service\n"
        )));
        assert!(text.contains("ExecStart=/usr/local/libexec/verdaxis/verdaxis-read monitor "));
        assert!(!text.contains("python"), "{unit}");
    }
}

// ---- identities and permissions (one-identity part) ----------------------------------------

#[test]
fn alert_handoff_real_identities_enqueue_and_dispatch() {
    let layout = Layout::new();
    let paths = layout.journal(Check::Health);
    let producer = layout
        .producer(Check::Health)
        .with_access(EntryAccess::DispatcherAcl);
    std::fs::create_dir_all(paths.handoff_directory()).unwrap();
    if cfg!(target_os = "linux") {
        // Without a default ACL naming the dispatcher, admission is refused.
        assert_eq!(
            producer.enqueue(&observation(Check::Health, AlertState::Failure)),
            Err(Refusal::Unavailable)
        );
        assert!(layout.pending(Check::Health).is_empty());
        // With it, the entry gets owner rw-, the named reader r--, mask r--.
        let status = Command::new("setfacl")
            .args(["-d", "-m", "u::rwx,u:65534:r--,g::---,m::r--,o::---"])
            .arg(paths.handoff_directory())
            .status()
            .expect("setfacl is required for the ACL cell");
        assert!(status.success());
        let sequence = producer
            .enqueue(&observation(Check::Health, AlertState::Failure))
            .unwrap()
            .sequence();
        let entry = paths.entry(sequence);
        assert_eq!(
            std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let acl = Command::new("getfacl")
            .args(["--omit-header", "--numeric"])
            .arg(&entry)
            .output()
            .expect("getfacl");
        let text = String::from_utf8_lossy(&acl.stdout);
        for line in [
            "user::rw-",
            "user:65534:r--",
            "group::---",
            "mask::r--",
            "other::---",
        ] {
            assert!(text.lines().any(|found| found == line), "{line} in {text}");
        }
        // The dispatcher reads and removes it.
        let transport = FakeTransport::default();
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        assert_eq!(
            runtime().block_on(dispatcher.wake(Check::Health)),
            Ok(Outcome::Delivered(sequence))
        );
    } else {
        // No POSIX ACLs on this platform: the installed access fails closed.
        assert_eq!(
            producer.enqueue(&observation(Check::Health, AlertState::Failure)),
            Err(Refusal::Unavailable)
        );
        assert!(layout.pending(Check::Health).is_empty());
    }
    assert_eq!(EntryAccess::installed(), EntryAccess::DispatcherAcl);
}

#[test]
fn alert_handoff_producers_cannot_read_other_check_receipts_or_credentials() {
    let alert = DispatcherConfig::installed();
    for check in Check::ALL {
        let own = JournalPaths::installed(check);
        let other = JournalPaths::installed(match check {
            Check::Health => Check::Backup,
            Check::Backup => Check::Health,
        });
        for path in [
            own.handoff_directory(),
            own.cursor(),
            own.journal_lock(),
            own.quarantine_directory(),
        ] {
            assert!(path.starts_with(&own.state_directory), "{path:?}");
            assert!(!path.starts_with(&other.state_directory));
            assert!(!path.starts_with(&alert.alert_directory));
        }
    }
    // A producer refuses an observation of the other check.
    let layout = Layout::new();
    assert_eq!(
        layout
            .producer(Check::Health)
            .enqueue(&observation(Check::Backup, AlertState::Failure)),
        Err(Refusal::Unavailable)
    );
    assert!(!layout.config.alert_directory.exists());
    // Producer units load no environment file and reach no alert state.
    for unit in ["verdaxis-health.service", "verdaxis-backup-verify.service"] {
        let text =
            std::fs::read_to_string(contract_fixture(&format!("alert-handoff/{unit}"))).unwrap();
        assert!(!text.contains("EnvironmentFile"), "{unit}");
        assert!(!text.contains("verdaxis-monitor-alert/"), "{unit}");
        assert!(!text.contains("alert.env"), "{unit}");
        assert!(!text.contains("ReadWritePaths"), "{unit}");
    }
    // The dispatcher units write only the two handoff directories.
    for unit in [
        "verdaxis-monitor-alert@.service",
        "verdaxis-monitor-alert-reminder.service",
    ] {
        let text =
            std::fs::read_to_string(contract_fixture(&format!("alert-handoff/{unit}"))).unwrap();
        assert!(text.contains(
            "ReadWritePaths=-/var/lib/verdaxis-health/alert-handoff -/var/lib/verdaxis-backup-verify/alert-handoff\n"
        ));
        assert!(text.contains("User=verdaxis-monitor-alert"));
    }
}

#[test]
fn alert_handoff_acl_modes_and_dispatcher_read_remove_only() {
    // The reviewed ACL policy, line by line.
    let policy = std::fs::read_to_string(contract_fixture(
        "alert-handoff/verdaxis-monitor-handoff.acl.tmpfiles",
    ))
    .unwrap();
    let rules: Vec<&str> = policy
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .collect();
    let mut expected = Vec::new();
    for (directory, check) in [
        ("/var/lib/verdaxis-health", "health"),
        ("/var/lib/verdaxis-backup-verify", "backup"),
    ] {
        expected.push(format!(
            "a {directory} - - - - u::rwx,u:verdaxis-monitor-alert:--x,g::---,m::--x,o::---"
        ));
        expected.push(format!(
            "a {directory}/.{check}.journal.lock - - - - u::rw-,u:verdaxis-monitor-alert:r--,g::---,m::r--,o::---"
        ));
        expected.push(format!(
            "a {directory}/alert-handoff - - - - u::rwx,u:verdaxis-monitor-alert:rwx,g::---,m::rwx,o::---,d:u::rwx,d:u:verdaxis-monitor-alert:r--,d:g::---,d:m::r--,d:o::---"
        ));
    }
    assert_eq!(rules, expected);
    let tmpfiles =
        std::fs::read_to_string(contract_fixture("alert-handoff/verdaxis-monitor.tmpfiles"))
            .unwrap();
    for line in [
        "d /var/lib/verdaxis-health/alert-handoff 0700 verdaxis-health verdaxis-health -",
        "d /var/lib/verdaxis-backup-verify/alert-handoff 0700 verdaxis-backup-reader verdaxis-backup-readers -",
        "f /var/lib/verdaxis-health/.health.journal.lock 0600 verdaxis-health verdaxis-health -",
        "f /var/lib/verdaxis-backup-verify/.backup.journal.lock 0600 verdaxis-backup-reader verdaxis-backup-readers -",
        "d /var/lib/verdaxis-monitor-alert 0700 verdaxis-monitor-alert verdaxis-monitor-alert -",
    ] {
        assert!(tmpfiles.lines().any(|found| found == line), "{line}");
    }

    // The dispatcher only reads an entry and removes it after acknowledgement:
    // an entry its owner made read-only is still delivered and removed, and
    // the producer's cursor is never touched.
    let layout = Layout::new();
    let sequence = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let entry = layout.journal(Check::Health).entry(sequence);
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o400)).unwrap();
    let entry_bytes = std::fs::read(&entry).unwrap();
    let cursor_before = std::fs::read(layout.journal(Check::Health).cursor()).unwrap();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    // While its delivery is blocked, the entry's bytes stay exactly as written.
    transport.script(Destination::Telegram, &[SendOutcome::NotDelivered]);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::RetryLater(sequence))
    );
    assert_eq!(std::fs::read(&entry).unwrap(), entry_bytes);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(sequence))
    );
    assert!(!entry.exists());
    assert_eq!(
        std::fs::read(layout.journal(Check::Health).cursor()).unwrap(),
        cursor_before
    );
    // A link in place of the head is never followed or skipped.
    let layout = Layout::new();
    let sequence = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let entry = layout.journal(Check::Health).entry(sequence);
    let target = layout.base.join("elsewhere.json");
    std::fs::rename(&entry, &target).unwrap();
    std::os::unix::fs::symlink(&target, &entry).unwrap();
    let dispatcher = self::dispatcher(&layout, &settings, &transport, &clock);
    let blocked = rt.block_on(dispatcher.wake(Check::Health));
    assert_eq!(
        blocked,
        Ok(Outcome::Blocked(sequence, BlockReason::InvalidEntry))
    );
    assert_eq!(exit_for(&blocked), Exit::Precondition);
}

// ---- Python handover --------------------------------------------------------------------

#[test]
fn alert_python_rust_same_check_handover_preserves_failure_then_healthy() {
    let layout = Layout::new();
    let server = FakeHttp::start();
    let alert_directory = layout.config.alert_directory.clone();
    // Python owns the check and delivers a failure.
    let python = run_python(&["--event", "health-failure"], &alert_directory, &server);
    assert!(python.status.success(), "{python:?}");
    assert_eq!(server.texts(), vec!["Verdaxis local health: FAILURE"]);
    let python_state = layout.legacy(Check::Health).unwrap();
    assert_eq!(python_state["state"], "failure");

    // Fenced transfer: Python is stopped; the native producer admits the
    // recovery and the native dispatcher delivers it from Python's receipts.
    let sequence = layout
        .enqueue(Check::Health, AlertState::Healthy)
        .sequence();
    let transport = HttpTransport::new();
    let clock = FakeClock::real();
    let settings = server.settings();
    let dispatcher = Dispatcher::new(&layout.config, &settings, &transport, &clock);
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(sequence))
    );
    assert_eq!(
        server.texts(),
        vec![
            "Verdaxis local health: FAILURE",
            "Verdaxis local health: RECOVERY"
        ]
    );
    let native_state = layout.legacy(Check::Health).unwrap();
    assert_eq!(native_state["state"], "healthy");
    assert_eq!(
        native_state["receipts"]["telegram"]["failure"],
        python_state["receipts"]["telegram"]["failure"],
        "Python's failure receipt is kept"
    );
    assert!(python_accepts_state(
        &layout.config.legacy_state(Check::Health)
    ));
    assert_eq!(
        layout.lock_mode(Check::Health),
        0o400,
        "native ownership is fenced"
    );
}

#[test]
fn alert_python_direct_invocation_is_fenced_until_native_journal_drained() {
    let layout = Layout::new();
    let server = FakeHttp::start();
    let alert_directory = layout.config.alert_directory.clone();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let transport = HttpTransport::new();
    let clock = FakeClock::real();
    let settings = server.settings();
    let dispatcher = Dispatcher::new(&layout.config, &settings, &transport, &clock);
    let rt = runtime();
    // The destination refuses: the native head stays pending.
    server.set_status(503);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::RetryLater(failure))
    );
    assert_eq!(layout.lock_mode(Check::Health), 0o400);
    let requests_before = server.requests.lock().unwrap().len();
    server.set_status(200);

    // A direct Python invocation cannot overtake the native journal.
    let state_before = std::fs::read(layout.config.legacy_state(Check::Health)).ok();
    let python = run_python(&["--event", "health-healthy"], &alert_directory, &server);
    assert!(
        !python.status.success(),
        "fenced Python must fail: {python:?}"
    );
    let python = run_python(&["--remind", "all"], &alert_directory, &server);
    assert!(!python.status.success(), "fenced Python reminder must fail");
    assert_eq!(server.requests.lock().unwrap().len(), requests_before);
    assert_eq!(
        std::fs::read(layout.config.legacy_state(Check::Health)).ok(),
        state_before
    );
    // The handback is refused while the native journal is not drained.
    assert_eq!(
        dispatcher.release_to_python(Check::Health),
        Err(Exit::Precondition)
    );
    assert_eq!(layout.lock_mode(Check::Health), 0o400);

    // Drained, the check is released and Python follows the native receipts.
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    dispatcher.release_to_python(Check::Health).unwrap();
    assert_eq!(layout.lock_mode(Check::Health), 0o600);
    let python = run_python(&["--event", "health-healthy"], &alert_directory, &server);
    assert!(python.status.success(), "{python:?}");
    // The refused attempt, the native delivery, then Python's recovery.
    assert_eq!(
        server.texts(),
        vec![
            "Verdaxis local health: FAILURE",
            "Verdaxis local health: FAILURE",
            "Verdaxis local health: RECOVERY"
        ]
    );
}

#[test]
fn alert_acknowledgement_before_remove_survives_restart_and_revert() {
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let died = child(&layout, "dispatch-health", &[(CHILD_STAGE, "acknowledged")]);
    assert!(killed_by_abort(&died), "{died:?}");
    let log = layout.base.join("sends.log");
    assert_eq!(logged(&log).len(), 2);
    assert_eq!(layout.pending(Check::Health), vec![failure.0]);
    let progress = layout.delivery(Check::Health).unwrap();
    assert_eq!(progress["acknowledged"]["sequence"], json!(failure.0));
    assert_eq!(progress["head"], Value::Null);

    // Restart: the acknowledged entry is removed without a resend.
    let transport = FakeTransport::logging(log.clone());
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Idle)
    );
    assert!(layout.pending(Check::Health).is_empty());
    assert_eq!(logged(&log).len(), 2);

    // A blocked revert: a new head is pending, so the handback is refused.
    let healthy = layout
        .enqueue(Check::Health, AlertState::Healthy)
        .sequence();
    transport.script(Destination::Telegram, &[SendOutcome::NotDelivered]);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::RetryLater(healthy))
    );
    assert_eq!(
        dispatcher.release_to_python(Check::Health),
        Err(Exit::Precondition)
    );
    assert_eq!(layout.lock_mode(Check::Health), 0o400);
    // Drained, the revert succeeds and Python accepts the projected state.
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(healthy))
    );
    dispatcher.release_to_python(Check::Health).unwrap();
    assert_eq!(layout.lock_mode(Check::Health), 0o600);
    let legacy_path = layout.config.legacy_state(Check::Health);
    assert!(python_accepts_state(&legacy_path));
    assert_eq!(layout.legacy(Check::Health).unwrap()["state"], "healthy");
}

// ---- receipts and reminders --------------------------------------------------------------

#[test]
fn alert_receipts_prevent_duplicate_delivery_per_destination() {
    let layout = Layout::new();
    let first = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(first))
    );
    assert_eq!(transport.sends().len(), 2);

    // The producer's cursor is lost: the same failure is admitted again, but
    // both destinations hold a receipt younger than an hour, so nothing is sent.
    std::fs::remove_file(layout.journal(Check::Health).cursor()).unwrap();
    let again = layout.enqueue(Check::Health, AlertState::Failure);
    let Admitted::Committed(second) = again else {
        panic!("an unknown admitted state admits the observation: {again:?}");
    };
    assert!(second > first, "a lost cursor never reuses a sequence");
    clock.advance(Span::minutes(10));
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(second))
    );
    assert_eq!(
        transport.sends().len(),
        2,
        "receipts suppress the duplicate"
    );
    let progress = layout.delivery(Check::Health).unwrap();
    assert_eq!(progress["acknowledged"]["sequence"], json!(second.0));

    // A healthy observation without a confirmed failure is not sent either.
    let layout = Layout::new();
    let healthy = layout
        .enqueue(Check::Backup, AlertState::Healthy)
        .sequence();
    let transport = FakeTransport::default();
    let dispatcher = self::dispatcher(&layout, &settings, &transport, &clock);
    assert_eq!(
        rt.block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(healthy))
    );
    assert!(transport.sends().is_empty());
    assert_eq!(layout.legacy(Check::Backup).unwrap()["state"], "healthy");
}

#[test]
fn alert_reminder_respects_the_hourly_repeat() {
    let layout = Layout::new();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    let rt = runtime();
    // No state yet: nothing to remind.
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Idle)
    );
    // A pending failure is drained by the reminder as the head, not repeated.
    let failure = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(transport.sends().len(), 2);
    clock.advance(Span::minutes(30));
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Idle)
    );
    assert_eq!(transport.sends().len(), 2);
    clock.advance(Span::minutes(30));
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Reminded)
    );
    let kinds: Vec<Kind> = transport.sends()[2..]
        .iter()
        .map(|sent| sent.kind)
        .collect();
    assert_eq!(kinds, vec![Kind::Reminder, Kind::Reminder]);
    assert!(
        transport.sends()[2..]
            .iter()
            .all(|sent| sent.sequence.is_none())
    );
    clock.advance(Span::minutes(59));
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Idle)
    );
    // After the recovery is acknowledged, no older failure is repeated.
    let healthy = layout
        .enqueue(Check::Health, AlertState::Healthy)
        .sequence();
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Delivered(healthy))
    );
    let sent = transport.sends().len();
    clock.advance(Span::hours(3));
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Health)),
        Ok(Outcome::Idle)
    );
    assert_eq!(transport.sends().len(), sent);
    // The other check has no state of its own.
    assert_eq!(
        rt.block_on(dispatcher.remind(Check::Backup)),
        Ok(Outcome::Idle)
    );
}

#[test]
fn alert_telegram_and_healthchecks_failures_are_independent() {
    for failing in [Destination::Telegram, Destination::Healthchecks] {
        let layout = Layout::new();
        let failure = layout
            .enqueue(Check::Health, AlertState::Failure)
            .sequence();
        let transport = FakeTransport::default();
        transport.script(failing, &[SendOutcome::NotDelivered]);
        let clock = FakeClock::real();
        let settings = settings();
        let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
        let rt = runtime();
        assert_eq!(
            rt.block_on(dispatcher.wake(Check::Health)),
            Ok(Outcome::RetryLater(failure))
        );
        let progress = layout.delivery(Check::Health).unwrap();
        let working = match failing {
            Destination::Telegram => Destination::Healthchecks,
            Destination::Healthchecks => Destination::Telegram,
        };
        assert_eq!(
            progress["head"]["destinations"][working.as_str()]["status"],
            "delivered"
        );
        assert_eq!(
            progress["head"]["destinations"][failing.as_str()]["status"],
            "pending"
        );
        assert_eq!(
            rt.block_on(dispatcher.wake(Check::Health)),
            Ok(Outcome::Delivered(failure))
        );
        let count = |destination| {
            transport
                .sends()
                .iter()
                .filter(|sent| sent.destination == destination)
                .count()
        };
        assert_eq!(count(working), 1, "the confirmed destination is sent once");
        assert_eq!(count(failing), 2);
        let legacy = layout.legacy(Check::Health).unwrap();
        assert!(legacy["receipts"]["telegram"]["failure"].is_object());
        assert!(legacy["receipts"]["healthchecks"]["failure"].is_object());
    }
    // Only Healthchecks configured: Telegram is not required.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let only = variables_settings(&[("HEALTHCHECKS_BACKUP_URL", "http://127.0.0.1:9/hc")]);
    let dispatcher = dispatcher(&layout, &only, &transport, &clock);
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(failure))
    );
    assert_eq!(transport.sends().len(), 1);
    // Nothing configured: acknowledged, and exit 2 as in Python.
    let layout = Layout::new();
    let failure = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let none = AlertSettings::default();
    let dispatcher = self::dispatcher(&layout, &none, &transport, &clock);
    let result = runtime().block_on(dispatcher.wake(Check::Backup));
    assert_eq!(result, Ok(Outcome::Unconfigured(Some(failure))));
    assert_eq!(exit_for(&result), Exit::Usage);
}

// ---- documents and secrets ------------------------------------------------------------------

#[test]
fn alert_state_is_atomic_and_schema_compatible() {
    let schema: Value = read_json(&contract_fixture("alert-journal.schema.json")).unwrap();
    let layout = Layout::new();
    let sequence = layout
        .enqueue(Check::Backup, AlertState::Failure)
        .sequence();
    let bytes = std::fs::read(layout.journal(Check::Backup).entry(sequence)).unwrap();
    let entry: Value = serde_json::from_slice(&bytes).unwrap();
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(keys, required, "sorted keys, exactly the schema's");
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(
        schema["properties"]["schema_version"]["const"],
        entry["schema_version"]
    );
    assert_eq!(
        schema["properties"]["state"]["enum"],
        json!(["failure", "healthy"])
    );
    assert_eq!(
        schema["properties"]["check"]["enum"],
        json!(["backup", "health"])
    );
    assert_eq!(schema["x-bounds"]["max_pending_entries"], json!(256));
    assert_eq!(schema["x-bounds"]["max_pending_bytes"], json!(1_048_576));
    assert_eq!(schema["x-bounds"]["max_entry_bytes"], json!(4096));
    let observed = entry["observed_at"].as_str().unwrap();
    assert!(
        observed.len() == 20 && observed.ends_with('Z'),
        "{observed}"
    );
    assert_eq!(bytes, serde_json::to_vec(&entry).unwrap(), "compact JSON");

    // The dispatcher's documents are private, complete and leave no temp files.
    let transport = FakeTransport::default();
    let clock = FakeClock::real();
    let settings = settings();
    let dispatcher = dispatcher(&layout, &settings, &transport, &clock);
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(sequence))
    );
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&layout.config.legacy_state(Check::Backup)), 0o600);
    assert_eq!(mode(&layout.config.delivery_state(Check::Backup)), 0o600);
    assert_eq!(mode(&layout.config.alert_directory), 0o700);
    let names: Vec<String> = std::fs::read_dir(&layout.config.alert_directory)
        .unwrap()
        .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|name| !name.contains(".tmp")), "{names:?}");
    let legacy = layout.legacy(Check::Backup).unwrap();
    let legacy_keys: Vec<&str> = legacy
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        legacy_keys,
        ["observed_at", "receipts", "schema_version", "state"]
    );
    assert_eq!(legacy["schema_version"], json!(3));
    assert!(python_accepts_state(
        &layout.config.legacy_state(Check::Backup)
    ));

    // A damaged progress document is quarantined; the head is retried, never skipped.
    let recovery = layout
        .enqueue(Check::Backup, AlertState::Healthy)
        .sequence();
    transport.script(Destination::Telegram, &[SendOutcome::NotDelivered]);
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::RetryLater(recovery))
    );
    std::fs::write(layout.config.delivery_state(Check::Backup), b"{\"head\": [").unwrap();
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Backup)),
        Ok(Outcome::Delivered(recovery))
    );
    assert!(
        std::fs::read_dir(&layout.config.alert_directory)
            .unwrap()
            .any(|item| item
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("backup.delivery.json.invalid-"))
    );
}

#[test]
fn alert_never_prints_a_token_or_chat_id() {
    let settings = settings();
    let debug = format!("{settings:?}");
    assert!(!debug.contains(TOKEN) && !debug.contains(CHAT), "{debug}");
    assert!(!debug.contains("hc-health"), "{debug}");

    // The command, its usage refusal and its failure print no secret.
    for arguments in [
        vec!["monitor", "alert", "--event", "health"],
        vec!["monitor", "alert", "--remind", "all"],
        vec!["monitor", "alert", "--event", TOKEN],
        vec![
            "monitor",
            "alert",
            "--reconcile",
            "health:1:telegram:delivered",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
            .args(&arguments)
            .env("TELEGRAM_BOT_TOKEN", TOKEN)
            .env("TELEGRAM_CHAT_ID", CHAT)
            .env(
                "HEALTHCHECKS_HEALTH_URL",
                "http://127.0.0.1:9/hc-health-secret",
            )
            .output()
            .unwrap();
        assert_ne!(output.status.code(), Some(0), "{arguments:?}");
        for stream in [&output.stdout, &output.stderr] {
            let text = String::from_utf8_lossy(stream);
            assert!(!text.contains(TOKEN), "{arguments:?}: {text}");
            assert!(!text.contains(CHAT), "{arguments:?}: {text}");
            assert!(!text.contains("hc-health-secret"), "{arguments:?}: {text}");
        }
    }

    // A real delivery puts the token only in the request path.
    let layout = Layout::new();
    let server = FakeHttp::start();
    let sequence = layout
        .enqueue(Check::Health, AlertState::Failure)
        .sequence();
    let transport = HttpTransport::new();
    let clock = FakeClock::real();
    let settings = server.settings();
    let dispatcher = Dispatcher::new(&layout.config, &settings, &transport, &clock);
    assert_eq!(
        runtime().block_on(dispatcher.wake(Check::Health)),
        Ok(Outcome::Delivered(sequence))
    );
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, format!("/bot{TOKEN}/sendMessage"));
    for path in [
        layout.config.legacy_state(Check::Health),
        layout.config.delivery_state(Check::Health),
        layout.journal(Check::Health).cursor(),
    ] {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains(TOKEN) && !text.contains(CHAT));
    }
}
