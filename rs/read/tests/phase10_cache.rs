//! Phase 10 Task 24: `ops cache-authority`, offline.
//!
//! The record functions run against a scratch runtime directory and a fake
//! incarnation store that records the record's state at each step, so the
//! order (revoke, drain, bind, publish) is observed rather than assumed.
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use verdaxis_read::cli::ops::cache::{
    AuthorityError, FENCE_DRAIN, IncarnationRow, IncarnationStore, MAX_REVISION, RECORD_NAME,
    Record, activate, describe, new_identity, publish_record, read_record, render_record,
    revoke_and_drain,
};
use verdaxis_read::runtime::cache_authority::{AUTHORITY_LEASE, Authority, parse_record};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn state(directory: &Path) -> String {
    match read_record(directory).unwrap() {
        None => "absent".into(),
        Some(Record {
            revision,
            authority: None,
        }) => format!("revoked {revision}"),
        Some(Record {
            revision,
            authority: Some(_),
        }) => format!("active {revision}"),
    }
}

/// A store that logs every call with the record's state at that moment.
struct FakeStore {
    directory: PathBuf,
    row: IncarnationRow,
    log: Arc<Mutex<Vec<String>>>,
    fail_bind: bool,
    bound: Option<Authority>,
}

impl IncarnationStore for FakeStore {
    async fn read(&mut self) -> Result<IncarnationRow, AuthorityError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("read ({})", state(&self.directory)));
        Ok(self.row.clone())
    }

    async fn bind(&mut self, authority: &Authority) -> Result<(), AuthorityError> {
        self.log.lock().unwrap().push(format!(
            "bind {} ({})",
            authority.revision,
            state(&self.directory)
        ));
        if self.fail_bind {
            return Err(AuthorityError {
                message: "the cache incarnation row could not be read or bound",
                revoked: None,
            });
        }
        self.bound = Some(authority.clone());
        Ok(())
    }
}

fn store(directory: &Path, revision: u64, log: &Arc<Mutex<Vec<String>>>) -> FakeStore {
    FakeStore {
        directory: directory.to_path_buf(),
        row: IncarnationRow {
            database_epoch: new_identity(),
            primary_identity: new_identity(),
            authority_revision: revision,
            active: true,
            in_recovery: false,
        },
        log: log.clone(),
        fail_bind: false,
        bound: None,
    }
}

/// A sleep that records its length and the record's state when it starts.
fn recording_sleep(
    directory: &Path,
    log: &Arc<Mutex<Vec<String>>>,
) -> impl Fn(Duration) -> std::future::Ready<()> {
    let directory = directory.to_path_buf();
    let log = log.clone();
    move |length: Duration| {
        log.lock().unwrap().push(format!(
            "sleep {}s ({})",
            length.as_secs(),
            state(&directory)
        ));
        std::future::ready(())
    }
}

#[tokio::test]
async fn cache_revoke_publishes_revoked_before_the_drain_wait() {
    let scratch = Scratch::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let revision = revoke_and_drain(&scratch.0, 0, recording_sleep(&scratch.0, &log))
        .await
        .unwrap();
    assert_eq!(revision, 1);
    assert_eq!(*log.lock().unwrap(), ["sleep 10s (revoked 1)"]);
}

#[tokio::test]
async fn cache_activate_revokes_then_binds_the_row_then_publishes() {
    let scratch = Scratch::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut fake = store(&scratch.0, 7, &log);
    let authority = activate(&scratch.0, &mut fake, recording_sleep(&scratch.0, &log))
        .await
        .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        [
            "read (absent)",
            "sleep 10s (revoked 8)",
            "bind 9 (revoked 8)"
        ]
    );
    assert_eq!(state(&scratch.0), "active 9");
    assert_eq!(fake.bound.as_ref(), Some(&authority));
    let published = read_record(&scratch.0).unwrap().unwrap();
    assert_eq!(published.authority.as_ref(), Some(&authority));
    // Fresh identities: neither repeats the row's.
    assert_ne!(authority.database_epoch, fake.row.database_epoch);
    assert_ne!(authority.primary_identity, fake.row.primary_identity);
    // A replica is refused before anything is published.
    let scratch = Scratch::new();
    let mut replica = store(&scratch.0, 1, &log);
    replica.row.in_recovery = true;
    let error = activate(&scratch.0, &mut replica, recording_sleep(&scratch.0, &log))
        .await
        .unwrap_err();
    assert_eq!(
        (error.message, error.revoked),
        (
            "the database is a replica; promote it before activation",
            None
        )
    );
    assert_eq!(state(&scratch.0), "absent");
}

#[tokio::test]
async fn cache_failure_between_steps_leaves_the_record_revoked() {
    let scratch = Scratch::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut active = store(&scratch.0, 0, &log);
    activate(&scratch.0, &mut active, recording_sleep(&scratch.0, &log))
        .await
        .unwrap();
    assert_eq!(state(&scratch.0), "active 2");
    let mut failing = store(&scratch.0, 2, &log);
    failing.fail_bind = true;
    let error = activate(&scratch.0, &mut failing, recording_sleep(&scratch.0, &log))
        .await
        .unwrap_err();
    assert_eq!(
        error.revoked,
        Some(3),
        "the command reports the revoked revision"
    );
    assert_eq!(state(&scratch.0), "revoked 3", "caching stays off");
    // The command exits 4 and prints the revoked revision for this case
    // (`failure` in cache.rs); a refusal before revocation exits 3.
    let source = include_str!("../src/cli/ops/cache.rs");
    assert!(source.contains("\"outcome\": \"left_revoked\", \"authority_revision\": revision"));
    assert!(source.contains("Exit::Dependency\n        }\n        None => {"));
}

#[test]
fn cache_record_is_bounded_atomic_and_0600() {
    let scratch = Scratch::new();
    let authority = Authority {
        database_epoch: "a".repeat(64),
        primary_identity: "b".repeat(64),
        revision: 4,
    };
    let record = Record {
        revision: 4,
        authority: Some(authority.clone()),
    };
    publish_record(&scratch.0, &record).unwrap();
    let path = scratch.0.join(RECORD_NAME);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        format!(
            "{{\"authority_revision\":4,\"database_epoch\":\"{}\",\"primary_identity\":\"{}\",\"state\":\"active\",\"version\":1}}\n",
            "a".repeat(64),
            "b".repeat(64)
        )
    );
    assert_eq!(render_record(&record), text);
    // The Rust reader accepts exactly what the owner writes.
    assert_eq!(parse_record(&text), Ok(Some(authority)));
    assert_eq!(
        parse_record(&render_record(&Record {
            revision: 5,
            authority: None
        })),
        Ok(None)
    );
    // Only the record remains: no temporary file.
    let names: Vec<String> = std::fs::read_dir(&scratch.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [RECORD_NAME]);
    // Oversized, linked or non-regular records are refused; a linked runtime
    // directory is refused for publication.
    std::fs::write(&path, vec![b' '; 4097]).unwrap();
    assert!(read_record(&scratch.0).is_err());
    std::fs::remove_file(&path).unwrap();
    symlink("/etc/hosts", &path).unwrap();
    assert!(read_record(&scratch.0).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(read_record(&scratch.0).is_err());
    let linked = scratch.0.join("linked");
    symlink(scratch.0.join(RECORD_NAME), &linked).unwrap();
    assert!(publish_record(&linked, &record).is_err());
}

#[test]
fn cache_status_never_prints_a_full_identity() {
    let scratch = Scratch::new();
    let authority = Authority {
        database_epoch: new_identity(),
        primary_identity: new_identity(),
        revision: 9,
    };
    publish_record(
        &scratch.0,
        &Record {
            revision: 9,
            authority: Some(authority.clone()),
        },
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .args(["ops", "cache-authority", "status", "--runtime-dir"])
        .arg(&scratch.0)
        .args([
            "--environment",
            "staging",
            "--release-sha",
            &"0".repeat(39),
            "--release-sha",
        ])
        .output()
        .unwrap();
    // A malformed command line is refused before anything is read.
    assert_eq!(output.status.code(), Some(2));
    let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .args(["ops", "cache-authority", "status", "--runtime-dir"])
        .arg(&scratch.0)
        .args([
            "--environment",
            "staging",
            "--release-sha",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        !text.contains(&authority.database_epoch) && !text.contains(&authority.primary_identity)
    );
    let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(summary["state"], "active");
    assert_eq!(summary["authority_revision"], 9);
    assert_eq!(
        summary["database_epoch_fingerprint"]
            .as_str()
            .unwrap()
            .len(),
        12
    );
    assert_eq!(describe(None), serde_json::json!({"state": "absent"}));
}

#[tokio::test]
async fn cache_revision_is_monotonic_and_rejects_out_of_range_values() {
    let scratch = Scratch::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sleep = recording_sleep(&scratch.0, &log);
    // Above both the floor and the published revision.
    assert_eq!(revoke_and_drain(&scratch.0, 41, &sleep).await.unwrap(), 42);
    assert_eq!(revoke_and_drain(&scratch.0, 3, &sleep).await.unwrap(), 43);
    // The BIGINT bound: a revoke that leaves no room for the active revision.
    let error = revoke_and_drain(&scratch.0, MAX_REVISION - 1, &sleep)
        .await
        .unwrap_err();
    assert_eq!(error.message, "cache authority revision space is exhausted");
    assert_eq!(state(&scratch.0), "revoked 43", "nothing was published");
    // Out-of-range and malformed records are refused by the shared parser.
    let epoch = "c".repeat(64);
    for text in [
        format!(
            "{{\"version\":1,\"state\":\"active\",\"authority_revision\":0,\"database_epoch\":\"{epoch}\",\"primary_identity\":\"{epoch}\"}}"
        ),
        "{\"version\":1,\"state\":\"revoked\",\"authority_revision\":-1}".to_owned(),
        "{\"version\":1,\"state\":\"revoked\",\"authority_revision\":18446744073709551616}"
            .to_owned(),
        "{\"version\":2,\"state\":\"revoked\",\"authority_revision\":1}".to_owned(),
        "{\"version\":1,\"state\":\"revoked\",\"authority_revision\":1,\"extra\":true}".to_owned(),
        format!(
            "{{\"version\":1,\"state\":\"active\",\"authority_revision\":1,\"database_epoch\":\"{}\",\"primary_identity\":\"{epoch}\"}}",
            "C".repeat(64)
        ),
    ] {
        assert!(parse_record(&text).is_err(), "{text}");
    }
}

#[test]
fn cache_lease_and_fence_match_the_reader() {
    assert_eq!(FENCE_DRAIN, AUTHORITY_LEASE * 2);
    // The Python owner the command replaces used the same numbers.
    let python = include_str!("../../../scripts/rust_cache_authority.py");
    assert!(python.contains("LEASE_SECONDS = 5\n"));
    assert!(python.contains("FENCE_DRAIN_SECONDS = 2 * LEASE_SECONDS\n"));
    assert_eq!(AUTHORITY_LEASE, Duration::from_secs(5));
    // Identities are 32 random bytes, distinct each call.
    let (first, second) = (new_identity(), new_identity());
    assert_eq!(first.len(), 64);
    assert!(
        first
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_ne!(first, second);
}
