//! Phase 10 Task 3: private state files, job gates, bounded retry and mail audit.
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use verdaxis_read::cli::gate::{
    require_apply, require_deploy_state_absent, require_identity, require_job_preflight,
};
use verdaxis_read::cli::mail::{Delivery, MailLog, MailTransport, recipient_digest};
use verdaxis_read::cli::state::{PendingWrite, StateLock, read_bounded, write_atomic};
use verdaxis_read::cli::{Environment, Exit, retry_bounded};
use verdaxis_read::crud::referrals::invite::MailClient;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    version: u64,
    state: String,
}

fn record(version: u64, state: &str) -> Record {
    Record {
        version,
        state: state.into(),
    }
}

/// One private scratch directory, removed with its contents.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "verdaxis-phase10-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&path).expect("create a private scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn write_state_is_atomic_and_0600() {
    let directory = TempDir::new();
    let path = directory.path().join("state.json");
    write_atomic(&path, &record(1, "old")).expect("publish the first state");

    // Simulate a crash after the temp file write and before the rename: the
    // process never commits, so the old state must remain authoritative.
    let pending = PendingWrite::create(&path, &record(2, "new")).expect("prepare a replacement");
    let temporary = pending.temporary_path().to_path_buf();
    std::mem::forget(pending);
    assert!(
        std::fs::symlink_metadata(&temporary)
            .expect("the crashed temp file remains")
            .file_type()
            .is_file()
    );
    assert_eq!(
        read_bounded::<Record>(&path, 4096).expect("old state is intact"),
        record(1, "old")
    );

    // A later writer must not reuse the crashed temp name.
    let second = PendingWrite::create(&path, &record(3, "other")).expect("prepare again");
    let second_temporary = second.temporary_path().to_path_buf();
    assert_ne!(second_temporary, temporary);
    drop(second);
    assert!(
        std::fs::symlink_metadata(&second_temporary).is_err(),
        "an uncommitted writer removes its own temp file"
    );
    assert_eq!(
        read_bounded::<Record>(&path, 4096).expect("old state survives both writers"),
        record(1, "old")
    );

    write_atomic(&path, &record(4, "final")).expect("publish the final state");
    let metadata = std::fs::metadata(&path).expect("read the published metadata");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        read_bounded::<Record>(&path, 4096)
            .expect("read the final state")
            .version,
        4
    );
}

#[test]
fn state_reader_refuses_oversize_symlink_and_invalid_json() {
    let directory = TempDir::new();
    let path = directory.path().join("state.json");

    assert_eq!(
        read_bounded::<Record>(&path, 4096),
        Err(Exit::Precondition),
        "a missing state file is refused"
    );

    std::fs::write(&path, b"{}").expect("write an invalid record");
    assert_eq!(
        read_bounded::<Record>(&path, 4096),
        Err(Exit::Precondition),
        "invalid JSON and a missing field are refused"
    );

    std::fs::write(&path, br#"{"version":1,"state":"ok"}"#).expect("write a valid record");
    let exact = std::fs::metadata(&path).expect("measure the record").len();
    assert!(exact > 0);
    assert!(
        read_bounded::<Record>(&path, exact).is_ok(),
        "a file exactly at the bound is accepted"
    );
    assert_eq!(
        read_bounded::<Record>(&path, exact - 1),
        Err(Exit::Precondition),
        "a file one byte over the bound is refused"
    );

    std::fs::write(&path, vec![b'x'; 128]).expect("write an oversize record");
    assert_eq!(
        read_bounded::<Record>(&path, 64),
        Err(Exit::Precondition),
        "an oversize file is refused before parsing"
    );

    let link = directory.path().join("state-link.json");
    std::os::unix::fs::symlink(&path, &link).expect("create a symlink");
    assert_eq!(
        read_bounded::<Record>(&link, 4096),
        Err(Exit::Precondition),
        "a symlink is refused even when its target is valid"
    );

    let directory_path = directory.path().join("state-directory");
    std::fs::create_dir(&directory_path).expect("create a directory");
    assert_eq!(
        read_bounded::<Record>(&directory_path, 4096),
        Err(Exit::Precondition),
        "a directory is refused"
    );
}

#[test]
fn state_lock_refuses_symlink_and_other_entries() {
    let directory = TempDir::new();

    // A symlink at the lock path is refused, and the entry it names is neither
    // locked nor re-moded through the link.
    let target = directory.path().join("real.lock");
    std::fs::write(&target, b"").expect("write the link target");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
        .expect("widen the link target");
    let link = directory.path().join("job.lock");
    std::os::unix::fs::symlink(&target, &link).expect("plant a lock symlink");

    assert_eq!(
        StateLock::try_acquire(&link).err(),
        Some(Exit::Precondition),
        "a symlinked lock path is refused"
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("the link remains")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::metadata(&target)
            .expect("read the link target")
            .permissions()
            .mode()
            & 0o777,
        0o644,
        "the lock path never changes the mode of the file it names"
    );

    // A directory or a device entry at the lock path is refused.
    let as_directory = directory.path().join("directory.lock");
    std::fs::create_dir(&as_directory).expect("create a directory");
    assert_eq!(
        StateLock::try_acquire(&as_directory).err(),
        Some(Exit::Precondition),
        "a non-regular lock path is refused"
    );
}

#[test]
fn state_lock_does_not_change_existing_mode() {
    let directory = TempDir::new();
    let path = directory.path().join("job.lock");
    std::fs::write(&path, b"").expect("write an existing lock");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
        .expect("set an owner-writable mode");

    let lock = StateLock::try_acquire(&path)
        .expect("open the lock")
        .expect("take an existing regular lock");
    drop(lock);

    assert_eq!(
        std::fs::metadata(&path)
            .expect("read the lock mode")
            .permissions()
            .mode()
            & 0o777,
        0o640,
        "an existing lock keeps its mode"
    );
}

#[test]
fn state_lock_is_nonblocking() {
    let directory = TempDir::new();
    let path = directory.path().join("job.lock");

    let first = StateLock::try_acquire(&path)
        .expect("open the lock")
        .expect("take the first lock");
    assert!(
        StateLock::try_acquire(&path)
            .expect("open the same lock")
            .is_none(),
        "a second holder must not wait"
    );

    drop(first);
    let second = StateLock::try_acquire(&path)
        .expect("open the released lock")
        .expect("reacquire after release");
    drop(second);

    let metadata = std::fs::metadata(&path).expect("read the lock metadata");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn state_lock_wait_is_bounded() {
    let directory = TempDir::new();
    let path = directory.path().join("job.lock");
    let first = StateLock::try_acquire(&path)
        .expect("open the lock")
        .expect("take the first lock");

    let deadline = std::time::Instant::now() + Duration::from_millis(120);
    assert_eq!(
        StateLock::acquire_until(&path, deadline).await.err(),
        Some(Exit::Dependency),
        "a held lock is a bounded dependency failure, not a wait forever"
    );

    drop(first);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    assert!(
        StateLock::acquire_until(&path, deadline).await.is_ok(),
        "the released lock is acquired"
    );
}

#[test]
fn job_preflight_refuses_dangling_state_and_live_deploy() {
    let directory = TempDir::new();
    let state = directory.path().join("deploy.state");
    let sha = "0123456789abcdef0123456789abcdef01234567";

    require_job_preflight("staging", sha, Environment::Staging, sha, &state)
        .expect("absent state passes the preflight");

    // A dangling symlink is present state. `Path::exists` follows the link and
    // would report it absent, which is the Python guard defect R2.
    std::os::unix::fs::symlink(directory.path().join("absent-target"), &state)
        .expect("plant a dangling state link");
    assert!(
        !state.exists(),
        "the refusal is about the link, not its missing target"
    );
    assert_eq!(require_deploy_state_absent(&state), Err(Exit::Precondition));
    assert_eq!(
        require_job_preflight("staging", sha, Environment::Staging, sha, &state),
        Err(Exit::Precondition)
    );
    std::fs::remove_file(&state).expect("remove the dangling link");

    std::fs::write(&state, b"restart-authorized").expect("write live deploy state");
    assert_eq!(
        require_deploy_state_absent(&state),
        Err(Exit::Precondition),
        "any present state path refuses an ordinary job"
    );
    std::fs::remove_file(&state).expect("remove the state file");

    assert_eq!(
        require_identity("production", sha, Environment::Staging, sha),
        Err(Exit::Precondition)
    );
    assert_eq!(
        require_identity("staging", &"f".repeat(40), Environment::Staging, sha),
        Err(Exit::Precondition)
    );
    require_identity("staging", sha, Environment::Staging, sha).expect("matching identity");
    assert_eq!(require_apply(false), Err(Exit::Usage));
    require_apply(true).expect("apply is accepted");
}

#[tokio::test(start_paused = true)]
async fn retry_stops_after_three_attempts() {
    let start = tokio::time::Instant::now();
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&attempts);
    let result: Result<(), &'static str> = retry_bounded(move || {
        let recorder = Arc::clone(&recorder);
        async move {
            recorder
                .lock()
                .expect("attempt log")
                .push(tokio::time::Instant::now() - start);
            Err("unavailable")
        }
    })
    .await;

    assert_eq!(result, Err("unavailable"));
    assert_eq!(
        *attempts.lock().expect("attempt log"),
        vec![
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(4)
        ],
        "one immediate attempt, then 1 s and 3 s delays, with no fourth attempt"
    );
}

#[tokio::test(start_paused = true)]
async fn retry_stops_at_the_first_success() {
    let attempts = Arc::new(Mutex::new(0_u32));
    let counter = Arc::clone(&attempts);
    let result: Result<u32, &'static str> = retry_bounded(move || {
        let counter = Arc::clone(&counter);
        async move {
            let mut count = counter.lock().expect("attempt log");
            *count += 1;
            if *count == 2 {
                Ok(*count)
            } else {
                Err("retry")
            }
        }
    })
    .await;

    assert_eq!(result, Ok(2));
    assert_eq!(*attempts.lock().expect("attempt log"), 2);
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn mail_transport_logs_recipient_digest_never_address() {
    let address = "Alice.Example@Example.invalid";
    let sink = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(sink.clone())
        .with_max_level(tracing::Level::INFO)
        .finish();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a single-threaded runtime");

    let delivery = tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(async {
            let transport = MailTransport::new(MailClient::disabled());
            transport
                .send(
                    address,
                    &serde_json::json!({"to": [address], "subject": "notice"}),
                )
                .await
        })
    });

    assert_eq!(delivery, Delivery::Refused);
    let logged =
        String::from_utf8(sink.0.lock().expect("log buffer").clone()).expect("utf-8 log output");
    assert!(!logged.is_empty(), "the transport logs one delivery record");
    assert!(
        !logged.contains(address) && !logged.contains("Example.invalid"),
        "the recipient address must never be logged: {logged}"
    );
    let digest = recipient_digest(address);
    assert_eq!(digest.len(), 32, "16 bytes keep a guessable address hidden");
    assert!(
        logged.contains(&digest),
        "the log names the recipient by digest: {logged}"
    );

    let record = MailLog::new(address, Delivery::Refused);
    assert_eq!(record.recipient_digest, digest);
    assert!(!record.json().to_string().contains(address));
    assert_eq!(
        recipient_digest(" alice.example@example.invalid "),
        digest,
        "the digest is stable across case and surrounding whitespace"
    );
}
