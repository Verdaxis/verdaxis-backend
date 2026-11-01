//! Phase 10 Task 16: the onboarding attention monitor.
//!
//! Everything here runs without a database: candidates come from a fixed list
//! and the alert channel is a recording fake or a loopback server. The
//! PostgreSQL candidate projection is in `phase10_onboarding_postgres`.
use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use verdaxis_read::business::attention::{Candidate, classify, format_message};
use verdaxis_read::cli::jobs::onboarding::{
    AlertSender, CandidateSource, Failure, Fault, Invocation, Mode, Options, Outcome, SendError,
    State, TelegramConfig, TelegramSender, bootstrap_state, mask_email, preflight,
    python_timestamp, reconcile, recovery_message, run, run_command,
};
use verdaxis_read::cli::state::StateLock;
use verdaxis_read::cli::{Environment, Exit};

fn at(seconds: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 4, 8, 0, 0).unwrap() + Duration::seconds(seconds)
}

/// The shared fixture: an approved account that has not logged in for a day,
/// so it is in the `first_login_overdue` stage.
fn candidate() -> Candidate {
    Candidate {
        key: "user:1".into(),
        email: "person@example.com".into(),
        first_name: Some("Pat".into()),
        last_name: Some("Lee".into()),
        role: Some("BUYER".into()),
        created_at: at(-86_400),
        email_verified: true,
        verification_expires_at: None,
        user_status: Some("APPROVED".into()),
        organization_name: Some("Example Energy".into()),
        organization_status: Some("APPROVED".into()),
        organization_provenance: Some("REAL".into()),
        membership_status: Some("APPROVED".into()),
        membership_reviewed_at: Some(at(-3 * 3600)),
        user_approved_at: Some(at(-3 * 3600)),
        organization_approved_at: Some(at(-3 * 3600)),
        last_login: None,
        pending_registration_expires_at: None,
    }
}

fn logged_in() -> Candidate {
    Candidate {
        last_login: Some(at(60)),
        ..candidate()
    }
}

fn awaiting_approval() -> Candidate {
    Candidate {
        membership_status: Some("PENDING".into()),
        ..candidate()
    }
}

fn second_candidate() -> Candidate {
    Candidate {
        key: "user:2".into(),
        email: "second@example.com".into(),
        ..candidate()
    }
}

/// One private scratch directory (mode 0700), removed with its contents.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "verdaxis-onboarding-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).expect("create a scratch directory");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("make the scratch directory private");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn state(&self) -> PathBuf {
        self.0.join("state.json")
    }

    fn lock(&self) -> PathBuf {
        self.0.join("onboarding.lock")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixed {
    candidates: Vec<Candidate>,
    loads: AtomicUsize,
}

impl Fixed {
    fn new(candidates: Vec<Candidate>) -> Self {
        Self {
            candidates,
            loads: AtomicUsize::new(0),
        }
    }
}

impl CandidateSource for Fixed {
    async fn load(&self) -> Result<Vec<Candidate>, Exit> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.candidates.clone())
    }
}

type Hook = Box<dyn Fn() + Send + Sync>;

/// A recording alert channel. It can fail after a number of deliveries, wait
/// during a delivery, and run a hook once a delivery has been recorded.
#[derive(Default)]
struct Recorder {
    messages: Mutex<Vec<String>>,
    fail_after: Option<usize>,
    delay: Option<StdDuration>,
    after_send: Option<Hook>,
}

impl Recorder {
    fn failing_after(deliveries: usize) -> Self {
        Self {
            fail_after: Some(deliveries),
            ..Self::default()
        }
    }

    fn messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().clone()
    }
}

impl AlertSender for Recorder {
    async fn send(&self, message: &str) -> Result<(), SendError> {
        if self
            .fail_after
            .is_some_and(|limit| self.messages.lock().unwrap().len() >= limit)
        {
            return Err(SendError::Failed);
        }
        if let Some(delay) = self.delay {
            tokio::time::sleep(delay).await;
        }
        self.messages.lock().unwrap().push(message.to_owned());
        if let Some(hook) = &self.after_send {
            hook();
        }
        Ok(())
    }
}

fn options(directory: &TempDir, mode: Mode) -> Options {
    Options {
        state_file: directory.state(),
        mode,
        show_emails: false,
    }
}

async fn monitor_detailed(
    directory: &TempDir,
    candidates: Vec<Candidate>,
    sender: &Recorder,
    now: DateTime<Utc>,
) -> Result<Outcome, Failure> {
    run(
        &Fixed::new(candidates),
        sender,
        &options(directory, Mode::Monitor),
        now,
    )
    .await
}

/// The same run, reduced to its fault.
async fn monitor(
    directory: &TempDir,
    candidates: Vec<Candidate>,
    sender: &Recorder,
    now: DateTime<Utc>,
) -> Result<Outcome, Fault> {
    monitor_detailed(directory, candidates, sender, now)
        .await
        .map_err(|failure| failure.fault)
}

/// Python writes this exact document: sorted keys, compact separators, one
/// silent bootstrap entry and one alerted entry.
const PYTHON_STATE: &str = concat!(
    r#"{"candidates":{"pending:2":{"silent":"true","stage":"organization_setup_expiring","#,
    r#""updated_at":"2026-08-04T08:00:00.123456+00:00"},"#,
    r#""user:1":{"stage":"first_login_overdue","updated_at":"2026-08-04T08:00:00+00:00"}},"#,
    r#""schema_version":1}"#
);

fn write_private(path: &Path, content: &str) {
    std::fs::write(path, content).expect("write a state file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// What an unfenced Python run does: a temp file in the same directory, then
/// a rename over the state file. It takes no lock.
fn python_style_write(path: &Path, content: &str) {
    let temporary = path.with_extension("python-tmp");
    write_private(&temporary, content);
    std::fs::rename(&temporary, path).expect("replace the state file");
}

fn read_state(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("read the state file"))
        .expect("the state file is JSON")
}

#[tokio::test]
async fn onboarding_reads_python_v1_state_without_alerting() {
    let directory = TempDir::new();
    write_private(&directory.state(), PYTHON_STATE);
    let sender = Recorder::default();

    // user:1 is still in the stage Python recorded, so nothing is sent. The
    // pending entry is not in the candidate list and is kept, as Python keeps
    // an entry it does not see.
    let outcome = monitor(&directory, vec![candidate()], &sender, at(300))
        .await
        .expect("the run succeeds");
    assert!(
        sender.messages().is_empty(),
        "no alert for a recorded stage"
    );
    assert_eq!(
        outcome,
        Outcome::Checked {
            candidates: 1,
            alerts: 0,
            recoveries: 0
        }
    );
    // Rust reads v1 and writes v1: the document round-trips byte for byte.
    assert_eq!(
        std::fs::read_to_string(directory.state()).unwrap(),
        PYTHON_STATE
    );
}

#[tokio::test]
async fn onboarding_suppresses_same_stage_and_alerts_on_change() {
    let directory = TempDir::new();
    let sender = Recorder::default();

    monitor(&directory, vec![candidate()], &sender, at(0))
        .await
        .unwrap();
    assert_eq!(sender.messages().len(), 1);
    assert!(sender.messages()[0].contains("First login overdue"));

    // The same stage five minutes later is not repeated and keeps its time.
    monitor(&directory, vec![candidate()], &sender, at(300))
        .await
        .unwrap();
    assert_eq!(sender.messages().len(), 1);
    assert_eq!(
        read_state(&directory.state())["candidates"]["user:1"]["updated_at"],
        "2026-08-04T08:00:00+00:00"
    );

    // A different stage alerts with the new action and sends no recovery.
    let outcome = monitor(&directory, vec![awaiting_approval()], &sender, at(600))
        .await
        .unwrap();
    assert_eq!(sender.messages().len(), 2);
    assert!(sender.messages()[1].contains("Approval required"));
    assert!(
        !sender.messages()[1].to_lowercase().contains("recovered"),
        "a stage change is not a recovery"
    );
    assert_eq!(
        outcome,
        Outcome::Checked {
            candidates: 1,
            alerts: 1,
            recoveries: 0
        }
    );
    let state = read_state(&directory.state());
    assert_eq!(state["candidates"]["user:1"]["stage"], "approval_required");
    assert_eq!(
        state["candidates"]["user:1"]["updated_at"],
        "2026-08-04T08:10:00+00:00"
    );

    // A successful login ends the issue: one recovery, and the entry is gone.
    let outcome = monitor(&directory, vec![logged_in()], &sender, at(900))
        .await
        .unwrap();
    assert_eq!(sender.messages().len(), 3);
    assert!(sender.messages()[2].contains("Verdaxis onboarding recovered"));
    assert_eq!(
        outcome,
        Outcome::Checked {
            candidates: 1,
            alerts: 0,
            recoveries: 1
        }
    );
    assert!(
        read_state(&directory.state())["candidates"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn onboarding_bootstrap_is_silent() {
    let directory = TempDir::new();
    // An older state is replaced, not merged: bootstrap records the present.
    write_private(&directory.state(), PYTHON_STATE);
    let sender = Recorder::default();
    let source = Fixed::new(vec![candidate(), logged_in_other()]);

    let outcome = run(
        &source,
        &sender,
        &options(&directory, Mode::Bootstrap),
        at(0),
    )
    .await
    .unwrap();
    assert_eq!(outcome, Outcome::Bootstrapped { recorded: 1 });
    assert!(sender.messages().is_empty());
    let state = read_state(&directory.state());
    let entries = state["candidates"].as_object().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "only a candidate with an issue is recorded"
    );
    assert_eq!(entries["user:1"]["silent"], "true");
    assert_eq!(entries["user:1"]["stage"], "first_login_overdue");

    // The recorded stage stays quiet, and so does its recovery.
    monitor(
        &directory,
        vec![candidate(), logged_in_other()],
        &sender,
        at(300),
    )
    .await
    .unwrap();
    assert!(sender.messages().is_empty());
    monitor(
        &directory,
        vec![logged_in(), logged_in_other()],
        &sender,
        at(360),
    )
    .await
    .unwrap();
    assert!(
        sender.messages().is_empty(),
        "a silent entry recovers silently"
    );
    assert!(
        read_state(&directory.state())["candidates"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

fn logged_in_other() -> Candidate {
    Candidate {
        last_login: Some(at(-60)),
        ..second_candidate()
    }
}

#[test]
fn bootstrap_state_marks_each_entry_silent_with_a_python_timestamp() {
    let state: State = bootstrap_state(&[candidate(), logged_in()], at(0));
    assert_eq!(state.len(), 1);
    let entry = &state["user:1"];
    assert_eq!(entry["silent"], "true");
    assert_eq!(entry["stage"], "first_login_overdue");
    assert_eq!(entry["updated_at"], "2026-08-04T08:00:00+00:00");
}

#[test]
fn python_timestamp_matches_isoformat() {
    assert_eq!(python_timestamp(at(0)), "2026-08-04T08:00:00+00:00");
    let with_microseconds = at(0) + Duration::microseconds(123_456);
    assert_eq!(
        python_timestamp(with_microseconds),
        "2026-08-04T08:00:00.123456+00:00"
    );
}

#[tokio::test]
async fn reconcile_keeps_entries_it_cannot_see_and_resends_nothing_for_them() {
    let sender = Recorder::default();
    let mut previous = State::new();
    previous.insert(
        "user:gone".into(),
        BTreeMap::from([("stage".into(), "approval_required".into())]),
    );
    let (current, alerts, recoveries) = reconcile(&[candidate()], &previous, at(0), &sender)
        .await
        .unwrap();
    assert_eq!((alerts, recoveries), (1, 0));
    assert!(current.contains_key("user:gone"));
    assert!(current.contains_key("user:1"));
}

#[test]
fn recovery_message_names_the_candidate_without_an_email_when_a_name_exists() {
    assert_eq!(
        recovery_message(&logged_in()),
        "Verdaxis onboarding recovered\n\
         Pat Lee · person@example.com\n\
         Organization: Example Energy\n\
         The previously reported onboarding issue is now resolved."
    );
    let bare = Candidate {
        first_name: Some("  ".into()),
        last_name: None,
        organization_name: None,
        ..logged_in()
    };
    assert_eq!(
        recovery_message(&bare),
        "Verdaxis onboarding recovered\n\
         person@example.com · person@example.com\n\
         Organization: Not created\n\
         The previously reported onboarding issue is now resolved."
    );
}

#[tokio::test]
async fn delivery_failure_leaves_the_state_untouched_and_the_next_run_retries() {
    let directory = TempDir::new();
    let failing = Recorder::failing_after(0);
    let result = monitor(&directory, vec![candidate()], &failing, at(0)).await;
    assert_eq!(result, Err(Fault::DeliveryFailed));
    assert_eq!(Fault::DeliveryFailed.exit(), Exit::Dependency);
    assert!(
        !directory.state().exists(),
        "a failed delivery must not record the stage"
    );

    let sender = Recorder::default();
    monitor(&directory, vec![candidate()], &sender, at(300))
        .await
        .unwrap();
    assert_eq!(sender.messages().len(), 1);
    assert_eq!(
        read_state(&directory.state())["candidates"]["user:1"]["stage"],
        "first_login_overdue"
    );

    // A failure with an older state in place leaves it byte for byte.
    let before = std::fs::read(directory.state()).unwrap();
    let failing = Recorder::failing_after(0);
    let result = monitor(&directory, vec![awaiting_approval()], &failing, at(600)).await;
    assert_eq!(result, Err(Fault::DeliveryFailed));
    assert_eq!(std::fs::read(directory.state()).unwrap(), before);
}

#[tokio::test]
async fn unconfigured_telegram_fails_only_when_an_alert_is_due() {
    let directory = TempDir::new();
    let sender = TelegramSender::new(None);
    // Nothing needs an alert, so the missing configuration is not reached.
    let outcome = run(
        &Fixed::new(vec![logged_in()]),
        &sender,
        &options(&directory, Mode::Monitor),
        at(0),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, Outcome::Checked { alerts: 0, .. }));

    let result = run(
        &Fixed::new(vec![candidate()]),
        &sender,
        &options(&directory, Mode::Monitor),
        at(0),
    )
    .await
    .map_err(|failure| failure.fault);
    assert_eq!(result, Err(Fault::DeliveryNotConfigured));
    assert_eq!(Fault::DeliveryNotConfigured.exit(), Exit::Precondition);
    assert!(
        read_state(&directory.state())["candidates"]
            .as_object()
            .unwrap()
            .is_empty(),
        "the refused alert is not recorded"
    );
}

#[tokio::test]
async fn dry_run_prints_alerts_and_touches_no_state_or_lock() {
    let directory = TempDir::new();
    let missing = directory.path().join("absent");
    let sender = Recorder::default();
    let outcome = run(
        &Fixed::new(vec![candidate(), logged_in_other()]),
        &sender,
        &Options {
            state_file: missing.join("state.json"),
            mode: Mode::DryRun,
            show_emails: true,
        },
        at(0),
    )
    .await
    .unwrap();
    let Outcome::DryRun { messages } = outcome else {
        panic!("dry run reports its messages");
    };
    assert_eq!(messages.len(), 1);
    assert!(messages[0].starts_with("Verdaxis onboarding needs attention\n"));
    assert!(sender.messages().is_empty(), "a dry run never sends");
    assert!(!missing.exists(), "a dry run creates nothing");
}

#[tokio::test]
async fn onboarding_state_is_atomic_0600_and_schema_v1() {
    let directory = TempDir::new();
    let sender = Recorder::default();
    monitor(&directory, vec![candidate()], &sender, at(0))
        .await
        .unwrap();

    let metadata = std::fs::metadata(directory.state()).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::read_to_string(directory.state()).unwrap(),
        r#"{"candidates":{"user:1":{"stage":"first_login_overdue","updated_at":"2026-08-04T08:00:00+00:00"}},"schema_version":1}"#
    );
    let text = std::fs::read_to_string(directory.state()).unwrap();
    assert!(!text.contains("person@example.com"), "identifiers only");
    let mut names: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["onboarding.lock", "state.json"],
        "no temp file stays"
    );
    assert_eq!(
        std::fs::metadata(directory.lock())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    // Malformed, foreign-version, oversized and linked state is refused before
    // any alert, and the file is left as it was.
    let refused: [(&str, String); 5] = [
        ("schema 2", r#"{"candidates":{},"schema_version":2}"#.into()),
        ("no schema", r#"{"candidates":{}}"#.into()),
        (
            "candidates not an object",
            r#"{"candidates":[],"schema_version":1}"#.into(),
        ),
        ("not json", "not json".into()),
        (
            "oversized",
            format!(
                r#"{{"candidates":{{"x":{{"pad":"{}"}}}},"schema_version":1}}"#,
                "a".repeat(1024 * 1024)
            ),
        ),
    ];
    for (label, document) in refused {
        write_private(&directory.state(), &document);
        let quiet = Recorder::default();
        let result = monitor(&directory, vec![candidate()], &quiet, at(0)).await;
        assert_eq!(result, Err(Fault::StateInvalid), "{label}");
        assert_eq!(Fault::StateInvalid.exit(), Exit::Precondition);
        assert!(
            quiet.messages().is_empty(),
            "{label}: no alert on bad state"
        );
        assert_eq!(
            std::fs::read_to_string(directory.state()).unwrap(),
            document
        );
    }
    std::fs::remove_file(directory.state()).unwrap();
    let target = directory.path().join("elsewhere.json");
    write_private(&target, PYTHON_STATE);
    std::os::unix::fs::symlink(&target, directory.state()).unwrap();
    let quiet = Recorder::default();
    assert_eq!(
        monitor(&directory, vec![candidate()], &quiet, at(0)).await,
        Err(Fault::StateInvalid),
        "a symlinked state file is refused"
    );
    assert!(quiet.messages().is_empty());
}

#[tokio::test]
async fn onboarding_stable_lock_busy_exits_without_send() {
    let directory = TempDir::new();
    let held = StateLock::try_acquire(&directory.lock())
        .expect("create the lock")
        .expect("take the lock");
    let source = Fixed::new(vec![candidate()]);
    let sender = Recorder::default();
    for mode in [Mode::Monitor, Mode::Bootstrap] {
        let outcome = run(&source, &sender, &options(&directory, mode), at(0))
            .await
            .expect("a busy lock is not a failure");
        assert_eq!(outcome, Outcome::LockBusy);
    }
    assert!(sender.messages().is_empty());
    assert_eq!(source.loads.load(Ordering::SeqCst), 0, "no query when busy");
    assert!(!directory.state().exists(), "no state when busy");

    // The lock is on its own stable file: a lock on the state file does not
    // block a run, and a run never replaces or removes the lock file.
    drop(held);
    let state_inode = {
        write_private(
            &directory.state(),
            r#"{"candidates":{},"schema_version":1}"#,
        );
        std::fs::metadata(directory.state()).unwrap().ino()
    };
    let lock_inode = std::fs::metadata(directory.lock()).unwrap().ino();
    let state_file = std::fs::File::open(directory.state()).unwrap();
    state_file.lock().expect("lock the state file itself");
    let outcome = run(&source, &sender, &options(&directory, Mode::Monitor), at(0))
        .await
        .unwrap();
    assert!(matches!(outcome, Outcome::Checked { alerts: 1, .. }));
    drop(state_file);
    assert_eq!(
        std::fs::metadata(directory.lock()).unwrap().ino(),
        lock_inode,
        "the lock file is stable"
    );
    assert_ne!(
        std::fs::metadata(directory.state()).unwrap().ino(),
        state_inode,
        "the state file is replaced by rename"
    );
}

#[tokio::test]
async fn onboarding_repeated_crashes_can_repeat_delivery() {
    // Alerts go out before the state is recorded, because a missed alert is
    // worse than a repeat. A crash between the two repeats the alert on every
    // later run until one run completes. The accepted behaviour, recorded here.
    let directory = TempDir::new();
    let state_directory = directory.path().to_path_buf();
    let away = state_directory.with_extension("away");
    let crash_directory = state_directory.clone();
    let crash_away = away.clone();
    let sender = Recorder {
        after_send: Some(Box::new(move || {
            // The state directory vanishes after the send: the state write
            // then fails exactly as it would after a process crash.
            std::fs::rename(&crash_directory, &crash_away).unwrap();
        })),
        ..Recorder::default()
    };
    for attempt in 1..=3 {
        let result = monitor(&directory, vec![candidate()], &sender, at(0)).await;
        assert_eq!(result, Err(Fault::StateWrite), "crash {attempt}");
        assert_eq!(sender.messages().len(), attempt);
        std::fs::rename(&away, &state_directory).unwrap();
        assert!(!directory.state().exists(), "no state survived the crash");
    }
    let messages = sender.messages();
    assert!(messages.iter().all(|message| message == &messages[0]));

    // One completed run records the stage and ends the repeats.
    let healthy = Recorder::default();
    monitor(&directory, vec![candidate()], &healthy, at(300))
        .await
        .unwrap();
    monitor(&directory, vec![candidate()], &healthy, at(600))
        .await
        .unwrap();
    assert_eq!(healthy.messages().len(), 1);
}

#[tokio::test]
async fn onboarding_partial_delivery_failure_repeats_the_delivered_alert() {
    let directory = TempDir::new();
    let flaky = Recorder::failing_after(1);
    let result = monitor(
        &directory,
        vec![candidate(), second_candidate()],
        &flaky,
        at(0),
    )
    .await;
    assert_eq!(result, Err(Fault::DeliveryFailed));
    assert_eq!(flaky.messages().len(), 1);
    assert!(
        !directory.state().exists(),
        "no state until every send ends"
    );

    let sender = Recorder::default();
    monitor(
        &directory,
        vec![candidate(), second_candidate()],
        &sender,
        at(300),
    )
    .await
    .unwrap();
    assert_eq!(sender.messages().len(), 2, "the first alert is sent again");
}

#[tokio::test]
async fn onboarding_handover_drains_and_fences_unlocked_python() {
    // Python takes no lock, so the monitor's lock does not exclude it: a
    // Python-style writer replaces the state while the lock is held. That is
    // why the cutover stops and drains Python first instead of relying on it.
    let directory = TempDir::new();
    write_private(
        &directory.state(),
        r#"{"candidates":{},"schema_version":1}"#,
    );
    {
        let _held = StateLock::try_acquire(&directory.lock())
            .unwrap()
            .expect("take the lock");
        python_style_write(&directory.state(), PYTHON_STATE);
        assert_eq!(
            std::fs::read_to_string(directory.state()).unwrap(),
            PYTHON_STATE,
            "the lock does not stop a writer that ignores it"
        );
    }

    // When such a writer replaces the state between the monitor's read and its
    // write, the monitor refuses to publish over it. The alert it already sent
    // can repeat on the next run; the other writer's state is not overwritten.
    let state_path = directory.state();
    let replacement =
        r#"{"candidates":{"user:9":{"stage":"rejected","updated_at":"x"}},"schema_version":1}"#;
    let sender = Recorder {
        after_send: Some(Box::new(move || {
            python_style_write(&state_path, replacement);
        })),
        ..Recorder::default()
    };
    let result = monitor(&directory, vec![second_candidate()], &sender, at(0)).await;
    assert_eq!(result, Err(Fault::StateChanged));
    assert_eq!(Fault::StateChanged.exit(), Exit::Precondition);
    assert_eq!(
        std::fs::read_to_string(directory.state()).unwrap(),
        replacement
    );

    // With Python drained, the same run publishes normally.
    let quiet = Recorder::default();
    monitor(&directory, vec![second_candidate()], &quiet, at(300))
        .await
        .unwrap();
    assert_eq!(
        read_state(&directory.state())["candidates"]["user:2"]["stage"],
        "first_login_overdue"
    );
    assert!(read_state(&directory.state())["candidates"]["user:9"].is_object());
}

/// The child half of the process tests. It does nothing unless the parent test
/// sets the scratch directory, so a normal run passes it as an empty test.
#[tokio::test(flavor = "multi_thread")]
async fn child_process_entry() {
    let Ok(directory) = std::env::var("ONBOARDING_CHILD_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let wait_for_busy = std::env::var("ONBOARDING_CHILD_WAIT_BUSY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let id = std::process::id();
    let sends = directory.join("sends.log");
    let busy_dir = directory.join("busy");
    struct ProcessSender {
        sends: PathBuf,
        busy_dir: PathBuf,
        wait_for_busy: usize,
        id: u32,
    }
    impl AlertSender for ProcessSender {
        async fn send(&self, _message: &str) -> Result<(), SendError> {
            use std::io::Write;
            let mut log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.sends)
                .unwrap();
            writeln!(log, "SEND {}", self.id).unwrap();
            // Hold the lock inside the delivery until the other processes have
            // found it busy, so the overlap is certain and not timing luck.
            let deadline = std::time::Instant::now() + StdDuration::from_secs(30);
            while std::fs::read_dir(&self.busy_dir).map_or(0, Iterator::count) < self.wait_for_busy
                && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(StdDuration::from_millis(20)).await;
            }
            Ok(())
        }
    }
    let sender = ProcessSender {
        sends,
        busy_dir: busy_dir.clone(),
        wait_for_busy,
        id,
    };
    let state = directory.join("state").join("state.json");
    let outcome = run(
        &Fixed::new(vec![candidate()]),
        &sender,
        &Options {
            state_file: state,
            mode: Mode::Monitor,
            show_emails: false,
        },
        at(0),
    )
    .await
    .expect("the child run succeeds");
    if outcome == Outcome::LockBusy {
        std::fs::write(busy_dir.join(format!("busy-{id}")), b"").unwrap();
        println!("CHILD_OUTCOME=lock_busy");
    } else {
        println!("CHILD_OUTCOME=checked");
    }
}

struct ChildScene {
    directory: TempDir,
}

impl ChildScene {
    fn new() -> Self {
        let directory = TempDir::new();
        for name in ["state", "busy"] {
            let path = directory.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self { directory }
    }

    fn spawn(&self, wait_for_busy: usize) -> std::process::Child {
        Process::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "child_process_entry", "--nocapture"])
            .env("ONBOARDING_CHILD_DIR", self.directory.path())
            .env("ONBOARDING_CHILD_WAIT_BUSY", wait_for_busy.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start a child process")
    }

    fn sends(&self) -> usize {
        std::fs::read_to_string(self.directory.path().join("sends.log"))
            .map_or(0, |log| log.lines().count())
    }

    fn state(&self) -> PathBuf {
        self.directory.path().join("state").join("state.json")
    }

    fn lock(&self) -> PathBuf {
        self.directory.path().join("state").join("onboarding.lock")
    }
}

fn outcome_of(child: std::process::Child) -> String {
    let output = child.wait_with_output().expect("wait for a child");
    assert!(output.status.success(), "child exit: {:?}", output.status);
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("CHILD_OUTCOME=").map(str::to_owned))
        .expect("the child reports an outcome")
}

#[test]
fn onboarding_second_process_exits_without_alert() {
    let scene = ChildScene::new();
    // The first process holds the lock inside its delivery until one other
    // process has reported the lock busy.
    let first = scene.spawn(1);
    let deadline = std::time::Instant::now() + StdDuration::from_secs(30);
    while scene.sends() == 0 {
        assert!(std::time::Instant::now() < deadline, "first never sent");
        std::thread::sleep(StdDuration::from_millis(20));
    }
    let second = scene.spawn(0);
    assert_eq!(outcome_of(second), "lock_busy", "second exits 0, no alert");
    assert_eq!(outcome_of(first), "checked");
    assert_eq!(scene.sends(), 1, "only the lock holder alerted");
}

#[test]
fn onboarding_three_processes_remain_exclusive_across_state_rename() {
    let scene = ChildScene::new();
    // An older state exists, so the winner's publish is a rename over it.
    write_private(&scene.state(), r#"{"candidates":{},"schema_version":1}"#);
    let state_inode = std::fs::metadata(scene.state()).unwrap().ino();
    let children: Vec<_> = (0..3).map(|_| scene.spawn(2)).collect();
    let outcomes: Vec<String> = children.into_iter().map(outcome_of).collect();
    assert_eq!(
        outcomes.iter().filter(|o| *o == "checked").count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes.iter().filter(|o| *o == "lock_busy").count(),
        2,
        "{outcomes:?}"
    );
    assert_eq!(scene.sends(), 1, "one alert in total");
    assert_ne!(
        std::fs::metadata(scene.state()).unwrap().ino(),
        state_inode,
        "the state was replaced by rename"
    );
    let lock_inode = std::fs::metadata(scene.lock()).unwrap().ino();

    // A later process finds the recorded stage and sends nothing more.
    let late = scene.spawn(0);
    assert_eq!(outcome_of(late), "checked");
    assert_eq!(scene.sends(), 1);
    assert_eq!(std::fs::metadata(scene.lock()).unwrap().ino(), lock_inode);
}

#[tokio::test]
async fn onboarding_command_fails_closed_without_a_database_context() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let base = Invocation {
        environment: Some(Environment::Production),
        release_sha: Some(sha.into()),
        state_file: PathBuf::from("/nonexistent/verdaxis/state.json"),
        dry_run: false,
        bootstrap: false,
        show_emails: false,
        telegram_disabled: false,
        other_monitor_flags: false,
    };
    // The monitor is production-only; refuse before any pool or file is touched.
    for refused in [
        Invocation {
            environment: Some(Environment::Staging),
            ..base.clone()
        },
        Invocation {
            environment: None,
            release_sha: None,
            ..base.clone()
        },
        Invocation {
            dry_run: true,
            bootstrap: true,
            ..base.clone()
        },
        // A disabled channel would record stages that were never delivered.
        Invocation {
            telegram_disabled: true,
            ..base.clone()
        },
        Invocation {
            show_emails: true,
            ..base.clone()
        },
        Invocation {
            other_monitor_flags: true,
            ..base.clone()
        },
    ] {
        assert_eq!(run_command(refused).await, Exit::Usage);
    }
    // No deployed context exists until the runtime projection lands.
    assert_eq!(run_command(base).await, Exit::Precondition);
}

fn nameless() -> Candidate {
    Candidate {
        key: "user:3".into(),
        email: "nameless@example.com".into(),
        first_name: None,
        last_name: Some("  ".into()),
        ..candidate()
    }
}

fn canary() -> Candidate {
    Candidate {
        key: "user:4".into(),
        email: "canary+prod@prod.canary.verdaxis.exchange".into(),
        ..candidate()
    }
}

async fn dry_run_messages(candidates: Vec<Candidate>, show_emails: bool) -> Vec<String> {
    let directory = TempDir::new();
    let outcome = run(
        &Fixed::new(candidates),
        &Recorder::default(),
        &Options {
            state_file: directory.state(),
            mode: Mode::DryRun,
            show_emails,
        },
        at(0),
    )
    .await
    .expect("a dry run succeeds");
    let Outcome::DryRun { messages } = outcome else {
        panic!("dry run reports its messages");
    };
    messages
}

#[test]
fn mask_email_keeps_the_first_character_and_the_domain() {
    assert_eq!(mask_email("person@example.com"), "p***@example.com");
    assert_eq!(mask_email("\u{e9}lan@example.com"), "\u{e9}***@example.com");
    assert_eq!(mask_email("a@b@example.com"), "a***@example.com");
    assert_eq!(mask_email("no-at-sign"), "***");
    assert_eq!(mask_email("\u{7}x@example.com"), "?***@example.com");
}

#[tokio::test]
async fn dry_run_masks_emails_and_the_name_fallback_by_default() {
    // The classifier still reads the real address: the canary is excluded.
    let messages = dry_run_messages(vec![candidate(), nameless(), canary()], false).await;
    assert_eq!(messages.len(), 2);
    for message in &messages {
        assert!(!message.contains("person@example.com"), "{message}");
        assert!(!message.contains("nameless@example.com"), "{message}");
    }
    assert!(
        messages[0].contains("Pat Lee \u{b7} p***@example.com"),
        "{}",
        messages[0]
    );
    // With no name the display name falls back to the address: both are masked.
    assert!(
        messages[1].contains("n***@example.com \u{b7} n***@example.com"),
        "{}",
        messages[1]
    );
}

#[tokio::test]
async fn dry_run_shows_the_alert_text_with_the_flag() {
    let now = at(0);
    let expected: Vec<String> = [candidate(), nameless()]
        .iter()
        .map(|candidate| format_message(candidate, &classify(candidate, now).unwrap(), now))
        .collect();
    assert_eq!(
        dry_run_messages(vec![candidate(), nameless(), canary()], true).await,
        expected
    );
}

#[tokio::test]
async fn telegram_alerts_keep_the_full_address() {
    // Only the dry-run print is masked; the operator's channel is not.
    let directory = TempDir::new();
    let sender = Recorder::default();
    monitor(&directory, vec![candidate()], &sender, at(0))
        .await
        .unwrap();
    assert!(sender.messages()[0].contains("Pat Lee \u{b7} person@example.com"));
}

#[tokio::test]
async fn an_empty_organization_name_shows_not_created() {
    let unnamed = Candidate {
        organization_name: Some(String::new()),
        ..candidate()
    };
    let directory = TempDir::new();
    let sender = Recorder::default();
    monitor(&directory, vec![unnamed.clone()], &sender, at(0))
        .await
        .unwrap();
    assert!(sender.messages()[0].contains("Organization: Not created\n"));
    assert!(
        recovery_message(&unnamed).contains("Organization: Not created\n"),
        "recovery text"
    );
    let messages = dry_run_messages(vec![unnamed], true).await;
    assert!(messages[0].contains("Organization: Not created\n"));
}

#[tokio::test]
async fn failed_runs_report_the_counts_already_sent() {
    // A failed send: the first alert went out before the second failed.
    let directory = TempDir::new();
    let failure = monitor_detailed(
        &directory,
        vec![candidate(), second_candidate()],
        &Recorder::failing_after(1),
        at(0),
    )
    .await
    .unwrap_err();
    assert_eq!(
        failure,
        Failure {
            fault: Fault::DeliveryFailed,
            alerts: 1,
            recoveries: 0
        }
    );

    // A recovery counts as its own kind.
    let directory = TempDir::new();
    write_private(
        &directory.state(),
        r#"{"candidates":{"user:1":{"stage":"first_login_overdue","updated_at":"x"}},"schema_version":1}"#,
    );
    let failure = monitor_detailed(
        &directory,
        vec![logged_in(), nameless()],
        &Recorder::failing_after(1),
        at(0),
    )
    .await
    .unwrap_err();
    assert_eq!(
        failure,
        Failure {
            fault: Fault::DeliveryFailed,
            alerts: 0,
            recoveries: 1
        }
    );

    // Another writer replaced the state after the alert.
    let directory = TempDir::new();
    write_private(
        &directory.state(),
        r#"{"candidates":{},"schema_version":1}"#,
    );
    let state_path = directory.state();
    let sender = Recorder {
        after_send: Some(Box::new(move || {
            python_style_write(&state_path, r#"{"candidates":{},"schema_version":1}"#);
        })),
        ..Recorder::default()
    };
    let failure = monitor_detailed(&directory, vec![candidate()], &sender, at(0))
        .await
        .unwrap_err();
    assert_eq!(
        failure,
        Failure {
            fault: Fault::StateChanged,
            alerts: 1,
            recoveries: 0
        }
    );

    // The state could not be published.
    let directory = TempDir::new();
    let (state_directory, away) = (
        directory.path().to_path_buf(),
        directory.path().with_extension("away"),
    );
    let (hook_directory, hook_away) = (state_directory.clone(), away.clone());
    let sender = Recorder {
        after_send: Some(Box::new(move || {
            std::fs::rename(&hook_directory, &hook_away).unwrap();
        })),
        ..Recorder::default()
    };
    let failure = monitor_detailed(&directory, vec![candidate()], &sender, at(0))
        .await
        .unwrap_err();
    std::fs::rename(&away, &state_directory).unwrap();
    assert_eq!(
        failure,
        Failure {
            fault: Fault::StateWrite,
            alerts: 1,
            recoveries: 0
        }
    );
}

const FULL_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn invocation() -> Invocation {
    Invocation {
        environment: Some(Environment::Production),
        release_sha: Some(FULL_SHA.into()),
        state_file: PathBuf::from("/nonexistent/verdaxis/state.json"),
        dry_run: false,
        bootstrap: false,
        show_emails: false,
        telegram_disabled: false,
        other_monitor_flags: false,
    }
}

fn deployed(directory: &Path) -> HashMap<String, String> {
    HashMap::from([
        ("ENVIRONMENT".to_owned(), "production".to_owned()),
        ("RELEASE_SHA".to_owned(), FULL_SHA.to_owned()),
        (
            "DEPLOY_STATE_DIR".to_owned(),
            directory.to_string_lossy().into_owned(),
        ),
    ])
}

#[test]
fn onboarding_preflight_checks_the_deployed_identity_and_deploy_state() {
    let directory = TempDir::new();
    let vars = deployed(directory.path());
    let plan = preflight(&invocation(), &vars).expect("a deployed production release passes");
    assert_eq!(plan.environment, Environment::Production);
    assert_eq!(plan.release_sha, FULL_SHA);
    assert_eq!(plan.mode, Mode::Monitor);
    let dry = Invocation {
        dry_run: true,
        ..invocation()
    };
    assert_eq!(preflight(&dry, &vars).unwrap().mode, Mode::DryRun);

    let refused = |label: &str, vars: &HashMap<String, String>| {
        assert_eq!(
            preflight(&invocation(), vars).map(|_| ()),
            Err(Exit::Precondition),
            "{label}"
        );
    };
    // The flag says production; the deployed identity decides.
    let mut staging = vars.clone();
    staging.insert("ENVIRONMENT".into(), "staging".into());
    refused("deployed staging with the production flag", &staging);
    let mut other_sha = vars.clone();
    other_sha.insert("RELEASE_SHA".into(), "f".repeat(40));
    refused("another release", &other_sha);
    for name in ["ENVIRONMENT", "RELEASE_SHA", "DEPLOY_STATE_DIR"] {
        let mut missing = vars.clone();
        missing.remove(name);
        refused(&format!("{name} unset"), &missing);
    }
    for path in ["", "relative/state-dir"] {
        let mut bad = vars.clone();
        bad.insert("DEPLOY_STATE_DIR".into(), path.into());
        refused(&format!("deploy state dir {path:?}"), &bad);
    }
    // A present state file, and a dangling link, are both a live deploy.
    let state = directory.path().join("production.state");
    std::fs::write(&state, b"blocked").unwrap();
    refused("deploy state present", &vars);
    std::fs::remove_file(&state).unwrap();
    std::os::unix::fs::symlink(directory.path().join("missing-target"), &state).unwrap();
    refused("dangling deploy state link", &vars);
    std::fs::remove_file(&state).unwrap();
    preflight(&invocation(), &vars).expect("absent state passes again");
}

#[test]
fn onboarding_preflight_refuses_misuse_as_usage() {
    let directory = TempDir::new();
    let vars = deployed(directory.path());
    for (label, refused) in [
        (
            "staging flag",
            Invocation {
                environment: Some(Environment::Staging),
                ..invocation()
            },
        ),
        (
            "no identity",
            Invocation {
                environment: None,
                release_sha: None,
                ..invocation()
            },
        ),
        (
            "dry run and bootstrap",
            Invocation {
                dry_run: true,
                bootstrap: true,
                ..invocation()
            },
        ),
        (
            "disabled channel",
            Invocation {
                telegram_disabled: true,
                ..invocation()
            },
        ),
        (
            "--show-emails without --dry-run",
            Invocation {
                show_emails: true,
                ..invocation()
            },
        ),
        (
            "--show-emails with --bootstrap",
            Invocation {
                show_emails: true,
                bootstrap: true,
                ..invocation()
            },
        ),
        (
            "flags of another monitor",
            Invocation {
                other_monitor_flags: true,
                ..invocation()
            },
        ),
    ] {
        assert_eq!(
            preflight(&refused, &vars).map(|_| ()),
            Err(Exit::Usage),
            "{label}"
        );
    }
    let shown = Invocation {
        dry_run: true,
        show_emails: true,
        ..invocation()
    };
    assert!(preflight(&shown, &vars).is_ok());
}

/// What the loopback Telegram server saw.
#[derive(Debug, Clone, Default)]
struct Seen {
    requests: Vec<(String, String, String)>,
}

/// Serve the given `(status, location, body)` answers, one per connection.
async fn fake_telegram(
    answers: Vec<(u16, Option<&'static str>, &'static str)>,
) -> (String, Arc<Mutex<Seen>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let record = Arc::clone(&seen);
    tokio::spawn(async move {
        for (status, location, body) in answers {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 4096];
            let (head, content) = loop {
                let read = stream.read(&mut chunk).await.unwrap_or(0);
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(split) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buffer[..split]).into_owned();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while buffer.len() < split + 4 + length {
                        let read = stream.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                    }
                    break (head, buffer[split + 4..].to_vec());
                }
                if read == 0 {
                    return;
                }
            };
            let request_line = head.lines().next().unwrap_or_default().to_owned();
            let content_type = head
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-type:")
                        .map(|value| value.trim().to_owned())
                })
                .unwrap_or_default();
            record.lock().unwrap().requests.push((
                request_line,
                content_type,
                String::from_utf8_lossy(&content).into_owned(),
            ));
            let location = location
                .map(|target| format!("Location: {target}\r\n"))
                .unwrap_or_default();
            let response = format!(
                "HTTP/1.1 {status} X\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (format!("http://{address}"), seen)
}

fn telegram_for(base: &str) -> TelegramSender {
    let variables = BTreeMap::from([
        ("TELEGRAM_BOT_TOKEN".to_owned(), " token-value ".to_owned()),
        ("TELEGRAM_CHAT_ID".to_owned(), " 4242 ".to_owned()),
        ("TELEGRAM_API_BASE".to_owned(), format!("{base}/")),
    ]);
    TelegramSender::new(TelegramConfig::from_environment(&variables))
}

#[tokio::test]
async fn telegram_posts_a_form_and_requires_ok_true() {
    let (base, seen) = fake_telegram(vec![
        (200, None, r#"{"ok":true}"#),
        (200, None, r#"{"ok":false}"#),
        (500, None, r#"{"ok":true}"#),
        (200, None, "not json"),
    ])
    .await;
    let sender = telegram_for(&base);

    assert_eq!(sender.send("Line one\nPat & Lee é").await, Ok(()));
    let (request_line, content_type, body) = seen.lock().unwrap().requests[0].clone();
    assert_eq!(request_line, "POST /bottoken-value/sendMessage HTTP/1.1");
    assert_eq!(content_type, "application/x-www-form-urlencoded");
    let pairs: BTreeMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(pairs["chat_id"], "4242");
    assert_eq!(pairs["text"], "Line one\nPat & Lee é");

    for label in ["ok false", "http 500", "not json"] {
        assert_eq!(
            sender.send("message").await,
            Err(SendError::Failed),
            "{label}"
        );
    }
}

#[tokio::test]
async fn telegram_does_not_follow_a_redirect_with_the_token() {
    let (base, seen) = fake_telegram(vec![
        (307, Some("/elsewhere"), ""),
        (200, None, r#"{"ok":true}"#),
    ])
    .await;
    let sender = telegram_for(&base);
    assert_eq!(sender.send("message").await, Err(SendError::Failed));
    assert_eq!(
        seen.lock().unwrap().requests.len(),
        1,
        "the redirect target is never requested"
    );
}

#[tokio::test]
async fn telegram_failure_to_connect_is_a_failed_send() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert_eq!(
        telegram_for(&base).send("message").await,
        Err(SendError::Failed)
    );
}

#[test]
fn telegram_configuration_comes_from_the_environment_and_hides_the_token() {
    let mut variables = BTreeMap::from([
        ("TELEGRAM_BOT_TOKEN".to_owned(), "secret-token".to_owned()),
        ("TELEGRAM_CHAT_ID".to_owned(), "99".to_owned()),
    ]);
    let config = TelegramConfig::from_environment(&variables).expect("both values present");
    assert!(!format!("{config:?}").contains("secret-token"));
    for missing in ["TELEGRAM_BOT_TOKEN", "TELEGRAM_CHAT_ID"] {
        let mut partial = variables.clone();
        partial.insert(missing.to_owned(), "  ".to_owned());
        assert!(TelegramConfig::from_environment(&partial).is_none());
        partial.remove(missing);
        assert!(TelegramConfig::from_environment(&partial).is_none());
    }
    variables.insert("TELEGRAM_API_BASE".into(), "http://127.0.0.1:1//".into());
    assert!(TelegramConfig::from_environment(&variables).is_some());
}
