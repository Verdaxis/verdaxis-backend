//! Phase 10 Task 27: the backup verification monitor (`monitor backup-verify`).
//!
//! Every case builds a backup directory in a scratch directory. Archives and
//! file times are created per test, because a checkout does not keep file times.
#[path = "support/monitor_fixture.rs"]
mod support;

use chrono::{DateTime, Duration as Span, TimeZone, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use support::{TempDir, contract_fixture};
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::monitors::backup::{BackupConfig, collect, run_at, verify_backups};
use verdaxis_read::cli::monitors::status::{
    Admission, AdmitAll, Category, CheckResult, Status, failure_status,
};

const BACKUP_ID: &str = "20260720-030000";
const PREFIXES: [&str; 3] = ["verdaxis-prod", "verdaxis-staging", "umami"];

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).unwrap()
}

fn completed() -> DateTime<Utc> {
    now() - Span::hours(1)
}

fn dump() -> Vec<u8> {
    std::fs::read(contract_fixture("backup/sample-dump.sql")).unwrap()
}

fn gzip(content: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(content).unwrap();
    encoder.finish().unwrap()
}

fn set_time(path: &Path, at: DateTime<Utc>) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(at.timestamp() as u64);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn write_artifact(
    directory: &Path,
    prefix: &str,
    backup_id: &str,
    bytes: &[u8],
    at: DateTime<Utc>,
) -> PathBuf {
    let path = directory.join(format!("{prefix}-{backup_id}.sql.gz"));
    std::fs::write(&path, bytes).unwrap();
    set_time(&path, at);
    path
}

fn write_attempt(
    directory: &Path,
    attempt_id: &str,
    state: &str,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    backup_id: Option<&str>,
) {
    let attempts = directory.join("attempts");
    std::fs::create_dir_all(&attempts).unwrap();
    let document = json!({
        "schema_version": 1,
        "attempt_id": attempt_id,
        "state": state,
        "started_at": started_at.to_rfc3339(),
        "finished_at": finished_at.map(|at| at.to_rfc3339()),
        "backup_id": backup_id,
    });
    std::fs::write(
        attempts.join(format!("{attempt_id}.json")),
        document.to_string(),
    )
    .unwrap();
}

/// A complete, healthy backup generation for `backup_id` completed at `at`.
fn write_generation(directory: &Path, backup_id: &str, at: DateTime<Utc>) {
    let metadata = json!({
        "ok": true,
        "completed_at": at.to_rfc3339(),
        "backup_id": backup_id,
        "databases": ["verdaxis_prod", "verdaxis_staging", "umami"],
    });
    std::fs::write(directory.join("status.json"), metadata.to_string()).unwrap();
    let archive = gzip(&dump());
    for prefix in PREFIXES {
        write_artifact(directory, prefix, backup_id, &archive, at);
    }
    write_attempt(
        directory,
        &format!("attempt-{backup_id}"),
        "succeeded",
        at - Span::minutes(5),
        Some(at),
        Some(backup_id),
    );
}

fn fixture() -> TempDir {
    let scratch = TempDir::new();
    write_generation(scratch.path(), BACKUP_ID, completed());
    scratch
}

fn config(directory: &Path) -> BackupConfig {
    BackupConfig {
        backup_status: directory.join("status.json"),
        backup_directory: directory.to_path_buf(),
        attempt_directory: directory.join("attempts"),
        output: directory.join("state/status.json"),
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

fn verify(directory: &Path) -> Vec<CheckResult> {
    verify_backups(&config(directory), now())
}

fn codes(results: &[CheckResult]) -> Vec<&str> {
    let mut codes: Vec<&str> = results.iter().map(|result| result.code.as_str()).collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

fn read_metadata(directory: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(directory.join("status.json")).unwrap()).unwrap()
}

fn rewrite_metadata(directory: &Path, change: impl FnOnce(&mut Value)) {
    let mut metadata = read_metadata(directory);
    change(&mut metadata);
    std::fs::write(directory.join("status.json"), metadata.to_string()).unwrap();
}

fn artifact(directory: &Path, prefix: &str) -> PathBuf {
    directory.join(format!("{prefix}-{BACKUP_ID}.sql.gz"))
}

// ---- the happy path and the default configuration ---------------------------

#[test]
fn backup_verifies_metadata_and_the_newest_listed_artifacts() {
    let scratch = fixture();
    let results = verify(scratch.path());
    assert!(
        results
            .iter()
            .all(|result| result.category == Category::Healthy),
        "{results:?}"
    );
    let names: Vec<&str> = results.iter().map(|result| result.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "backup_attempt",
            "backup_metadata",
            "verdaxis_prod_artifact",
            "verdaxis_staging_artifact",
            "umami_artifact"
        ]
    );

    // The static fixture files are the producer's documents as the verifier reads them.
    let copy = TempDir::new();
    let valid = contract_fixture("backup/valid");
    std::fs::copy(valid.join("status.json"), copy.join("status.json")).unwrap();
    std::fs::create_dir(copy.join("attempts")).unwrap();
    std::fs::copy(
        valid.join("attempts/attempt-20260720-030000.json"),
        copy.join("attempts/attempt-20260720-030000.json"),
    )
    .unwrap();
    let archive = gzip(&dump());
    for prefix in PREFIXES {
        write_artifact(copy.path(), prefix, BACKUP_ID, &archive, completed());
    }
    let results = verify(copy.path());
    assert!(
        results
            .iter()
            .all(|result| result.category == Category::Healthy),
        "{results:?}"
    );
}

#[test]
fn backup_default_configuration_matches_the_replaced_monitor() {
    let defaults = BackupConfig::default();
    assert_eq!(
        defaults.backup_status,
        Path::new("/home/verdaxis-prod/backups/status.json")
    );
    assert_eq!(
        defaults.backup_directory,
        Path::new("/home/verdaxis-prod/backups")
    );
    assert_eq!(
        defaults.attempt_directory,
        Path::new("/home/verdaxis-prod/backups/attempts")
    );
    assert_eq!(
        defaults.output,
        Path::new("/var/lib/verdaxis-backup-verify/status.json")
    );
    assert_eq!(defaults.max_age_seconds, 30 * 60 * 60);
    assert_eq!(defaults.future_tolerance_seconds, 300);
    assert_eq!(defaults.max_timestamp_skew_seconds, 5);
    assert_eq!(defaults.max_metadata_bytes, 16_384);
    assert_eq!(defaults.max_attempt_records, 1_000);
    assert_eq!(defaults.max_attempt_bytes, 4_096);
    assert_eq!(defaults.max_artifacts, 1_000);
    assert_eq!(defaults.max_artifact_bytes, 20 * 1024 * 1024 * 1024);
    assert_eq!(defaults.max_uncompressed_bytes, 100 * 1024 * 1024 * 1024);
    assert_eq!(defaults.total_timeout, Duration::from_secs(120));
    // The monitor needs no credentials, database or network.
    let source = include_str!("../src/cli/monitors/backup.rs").to_lowercase();
    for forbidden in [
        "sqlx",
        "pg_dump",
        "psql",
        "backup-env",
        "database_url",
        "std::env",
        "reqwest",
        "process::command",
        "tokio::net",
    ] {
        assert!(
            !source.contains(forbidden),
            "backup.rs must not mention {forbidden}"
        );
    }
}

// ---- attempt evidence --------------------------------------------------------

#[test]
fn backup_missing_attempt_evidence_fails_closed() {
    let scratch = fixture();
    std::fs::remove_dir_all(scratch.join("attempts")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_missing"]);
    // An existing but empty directory is missing evidence too.
    std::fs::create_dir(scratch.join("attempts")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_missing"]);
    // Files that are not records do not count.
    std::fs::write(scratch.join("attempts/readme.txt"), b"x").unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_missing"]);
    // A linked attempt directory is not followed.
    let outside = TempDir::new();
    std::fs::remove_dir_all(scratch.join("attempts")).unwrap();
    symlink(outside.path(), scratch.join("attempts")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_invalid"]);
}

#[test]
fn backup_newer_crashed_or_failed_attempt_keeps_an_older_success_red() {
    for (state, finished, expected) in [
        ("started", None, "backup_attempt_incomplete"),
        (
            "failed",
            Some(now() - Span::minutes(20)),
            "backup_attempt_failed",
        ),
    ] {
        let scratch = fixture();
        write_attempt(
            scratch.path(),
            &format!("attempt-newer-{state}"),
            state,
            now() - Span::minutes(30),
            finished,
            None,
        );
        let results = verify(scratch.path());
        assert_eq!(codes(&results), [expected]);
        assert_eq!(
            results.len(),
            1,
            "one failed attempt result, no artifact checks"
        );
        assert_eq!(results[0].name, "backup_attempt");
    }
}

#[test]
fn backup_only_a_newer_verified_success_recovers_from_a_failed_attempt() {
    let scratch = fixture();
    write_attempt(
        scratch.path(),
        "attempt-failed",
        "failed",
        now() - Span::minutes(30),
        Some(now() - Span::minutes(20)),
        None,
    );
    assert_eq!(codes(&verify(scratch.path())), ["backup_attempt_failed"]);
    let newer = now() - Span::minutes(5);
    let newer_id = newer.format("%Y%m%d-%H%M%S").to_string();
    write_generation(scratch.path(), &newer_id, newer);
    // The older generation's artifacts remain on disk and are not the newest.
    let results = verify(scratch.path());
    assert!(
        results
            .iter()
            .all(|result| result.category == Category::Healthy),
        "{results:?}"
    );
}

#[test]
fn backup_latest_success_must_match_the_published_generation() {
    let scratch = fixture();
    write_attempt(
        scratch.path(),
        "attempt-new-success",
        "succeeded",
        now() - Span::minutes(20),
        Some(now() - Span::minutes(10)),
        Some("20260720-035000"),
    );
    assert_eq!(codes(&verify(scratch.path())), ["attempt_status_mismatch"]);
    // A finish time apart from the metadata time is a mismatch too.
    let scratch = fixture();
    write_attempt(
        scratch.path(),
        &format!("attempt-{BACKUP_ID}"),
        "succeeded",
        completed() - Span::minutes(5),
        Some(completed() + Span::seconds(30)),
        Some(BACKUP_ID),
    );
    assert_eq!(codes(&verify(scratch.path())), ["attempt_status_mismatch"]);
    // Metadata that predates the attempt start is a mismatch.
    let scratch = fixture();
    write_attempt(
        scratch.path(),
        &format!("attempt-{BACKUP_ID}"),
        "succeeded",
        completed() + Span::minutes(2),
        Some(completed() + Span::minutes(3)),
        Some(BACKUP_ID),
    );
    assert_eq!(codes(&verify(scratch.path())), ["attempt_status_mismatch"]);
}

#[test]
fn backup_malformed_attempt_journal_fails_closed() {
    let valid_time = "2026-07-20T03:30:00Z";
    let bad_records: Vec<(&str, Vec<u8>)> = vec![
        ("array", b"[]".to_vec()),
        ("missing keys", br#"{"schema_version":1,"state":"failed"}"#.to_vec()),
        (
            "boolean schema version",
            json!({"schema_version":true,"attempt_id":"bad","state":"failed","started_at":valid_time,"finished_at":"2026-07-20T03:31:00Z","backup_id":null}).to_string().into_bytes(),
        ),
        (
            "schema version two",
            json!({"schema_version":2,"attempt_id":"bad","state":"started","started_at":valid_time,"finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "name differs from the file",
            json!({"schema_version":1,"attempt_id":"other","state":"started","started_at":valid_time,"finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "unknown state",
            json!({"schema_version":1,"attempt_id":"bad","state":"paused","started_at":valid_time,"finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "started with a finish time",
            json!({"schema_version":1,"attempt_id":"bad","state":"started","started_at":valid_time,"finished_at":valid_time,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "failed with a backup id",
            json!({"schema_version":1,"attempt_id":"bad","state":"failed","started_at":valid_time,"finished_at":"2026-07-20T03:31:00Z","backup_id":BACKUP_ID}).to_string().into_bytes(),
        ),
        (
            "succeeded without a backup id",
            json!({"schema_version":1,"attempt_id":"bad","state":"succeeded","started_at":valid_time,"finished_at":"2026-07-20T03:31:00Z","backup_id":null}).to_string().into_bytes(),
        ),
        (
            "finished before it started",
            json!({"schema_version":1,"attempt_id":"bad","state":"failed","started_at":valid_time,"finished_at":"2026-07-20T03:00:00Z","backup_id":null}).to_string().into_bytes(),
        ),
        (
            "starts in the future",
            json!({"schema_version":1,"attempt_id":"bad","state":"started","started_at":"2026-07-20T04:06:00Z","finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "naive time",
            json!({"schema_version":1,"attempt_id":"bad","state":"started","started_at":"2026-07-20T03:30:00","finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        (
            "unsafe identifier",
            json!({"schema_version":1,"attempt_id":"../bad","state":"started","started_at":valid_time,"finished_at":null,"backup_id":null}).to_string().into_bytes(),
        ),
        ("not json", b"nope".to_vec()),
        ("over the record bound", vec![b' '; 4_097]),
    ];
    for (label, bytes) in bad_records {
        let scratch = fixture();
        std::fs::write(scratch.join("attempts/bad.json"), &bytes).unwrap();
        assert_eq!(
            codes(&verify(scratch.path())),
            ["attempt_evidence_invalid"],
            "{label}"
        );
    }
    // Duplicate keys, from the static fixture.
    let scratch = fixture();
    std::fs::copy(
        contract_fixture("backup/malformed/attempt-duplicate-key.json"),
        scratch.join("attempts/bad.json"),
    )
    .unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_invalid"]);
    // A linked record and a directory named like a record.
    let scratch = fixture();
    let external = scratch.join("external-attempt.json");
    std::fs::write(&external, b"{}").unwrap();
    symlink(&external, scratch.join("attempts/symlink.json")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_invalid"]);
    let scratch = fixture();
    std::fs::create_dir(scratch.join("attempts/dir.json")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_invalid"]);
    // Two records that started at the same instant cannot be ordered.
    let scratch = fixture();
    write_attempt(
        scratch.path(),
        "attempt-twin",
        "succeeded",
        completed() - Span::minutes(5),
        Some(completed()),
        Some(BACKUP_ID),
    );
    assert_eq!(codes(&verify(scratch.path())), ["attempt_evidence_invalid"]);
    // The record count is bounded.
    let scratch = fixture();
    let mut limited = config(scratch.path());
    limited.max_attempt_records = 1;
    write_attempt(
        scratch.path(),
        "attempt-extra",
        "succeeded",
        completed() - Span::hours(9),
        Some(completed() - Span::hours(8)),
        Some("20260719-190000"),
    );
    assert_eq!(
        codes(&verify_backups(&limited, now())),
        ["attempt_evidence_invalid"]
    );
}

// ---- metadata -----------------------------------------------------------------

#[test]
fn backup_corrupt_future_stale_and_inventory_metadata_fail_safely() {
    type Change = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Change, &str)> = vec![
        (
            "one database",
            Box::new(|m| m["databases"] = json!(["verdaxis_prod"])),
            "unexpected_inventory",
        ),
        (
            "inventory is text",
            Box::new(|m| m["databases"] = json!("verdaxis,verdaxis_staging,umami")),
            "unexpected_inventory",
        ),
        (
            "inventory is null",
            Box::new(|m| m["databases"] = Value::Null),
            "unexpected_inventory",
        ),
        (
            "object in inventory",
            Box::new(|m| {
                m["databases"] = json!([{"name":"verdaxis_prod"},"verdaxis_staging","umami"])
            }),
            "unexpected_inventory",
        ),
        (
            "list in inventory",
            Box::new(|m| m["databases"] = json!([["verdaxis_prod"], "verdaxis_staging", "umami"])),
            "unexpected_inventory",
        ),
        (
            "repeated database",
            Box::new(|m| m["databases"] = json!(["umami", "umami", "verdaxis_staging"])),
            "unexpected_inventory",
        ),
        (
            "unparsable time",
            Box::new(|m| m["completed_at"] = json!("not-a-time")),
            "invalid_timestamp",
        ),
        (
            "time without zone",
            Box::new(|m| m["completed_at"] = json!("2026-07-20T03:00:00")),
            "invalid_timestamp",
        ),
        (
            "time is a number",
            Box::new(|m| m["completed_at"] = json!(5)),
            "invalid_timestamp",
        ),
        (
            "future time",
            Box::new(|m| m["completed_at"] = json!((now() + Span::minutes(6)).to_rfc3339())),
            "future_timestamp",
        ),
        (
            "stale time",
            Box::new(|m| m["completed_at"] = json!((now() - Span::hours(31)).to_rfc3339())),
            "stale_status",
        ),
        (
            "traversal id",
            Box::new(|m| m["backup_id"] = json!("../../secret")),
            "invalid_backup_id",
        ),
        (
            "id is a number",
            Box::new(|m| m["backup_id"] = json!(20260720)),
            "invalid_backup_id",
        ),
        (
            "impossible date id",
            Box::new(|m| m["backup_id"] = json!("20261399-030000")),
            "invalid_backup_id",
        ),
        (
            "not ok",
            Box::new(|m| m["ok"] = json!(false)),
            "backup_incomplete",
        ),
        (
            "ok is text",
            Box::new(|m| m["ok"] = json!("true")),
            "backup_incomplete",
        ),
        (
            "ok is one",
            Box::new(|m| m["ok"] = json!(1)),
            "backup_incomplete",
        ),
        (
            "extra key",
            Box::new(|m| m["extra"] = json!(1)),
            "metadata_schema_mismatch",
        ),
        (
            "id differs from time",
            Box::new(|m| m["backup_id"] = json!("20260720-020000")),
            "backup_id_timestamp_mismatch",
        ),
    ];
    for (label, change, expected) in cases {
        let scratch = fixture();
        rewrite_metadata(scratch.path(), |m| change(m));
        let results = verify(scratch.path());
        assert_eq!(codes(&results), [expected], "{label}");
        assert_eq!(results.len(), 1, "{label}: a metadata failure ends the run");
        assert_eq!(results[0].name, "backup_metadata", "{label}");
    }
    // A missing key is a schema mismatch.
    let scratch = fixture();
    rewrite_metadata(scratch.path(), |m| {
        m.as_object_mut().unwrap().remove("databases");
    });
    assert_eq!(codes(&verify(scratch.path())), ["metadata_schema_mismatch"]);
    // The time is read with or without a zone designator and with fractions.
    for text in [
        "2026-07-20T03:00:00Z",
        "2026-07-20T03:00:00+00:00",
        "2026-07-20T03:00:00.000000+00:00",
    ] {
        let scratch = fixture();
        rewrite_metadata(scratch.path(), |m| m["completed_at"] = json!(text));
        let results = verify(scratch.path());
        assert!(
            results.iter().all(|r| r.category == Category::Healthy),
            "{text}: {results:?}"
        );
    }
}

#[test]
fn backup_metadata_file_is_bounded_regular_and_unique_keyed() {
    let scratch = fixture();
    std::fs::write(
        scratch.join("status.json"),
        json!({"secret": "do-not-copy".repeat(100)}).to_string(),
    )
    .unwrap();
    let mut limited = config(scratch.path());
    limited.max_metadata_bytes = 32;
    let results = verify_backups(&limited, now());
    assert_eq!(codes(&results), ["metadata_too_large"]);
    assert!(!format!("{results:?}").contains("do-not-copy"));

    let scratch = fixture();
    std::fs::copy(
        contract_fixture("backup/malformed/status-duplicate-key.json"),
        scratch.join("status.json"),
    )
    .unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["metadata_invalid_json"]);
    for bytes in [&b"[]"[..], b"{", b"", b"\xff\xfe"] {
        std::fs::write(scratch.join("status.json"), bytes).unwrap();
        assert_eq!(codes(&verify(scratch.path())), ["metadata_invalid_json"]);
    }
    std::fs::remove_file(scratch.join("status.json")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["metadata_missing"]);
    // A link to a good document is refused.
    let scratch = fixture();
    let external = scratch
        .path()
        .parent()
        .unwrap()
        .join(format!("external-{}.json", uuid::Uuid::new_v4().simple()));
    std::fs::copy(scratch.join("status.json"), &external).unwrap();
    std::fs::remove_file(scratch.join("status.json")).unwrap();
    symlink(&external, scratch.join("status.json")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["metadata_invalid_type"]);
    std::fs::remove_file(&external).unwrap();
    // A directory in its place is the wrong type too.
    std::fs::remove_file(scratch.join("status.json")).unwrap();
    std::fs::create_dir(scratch.join("status.json")).unwrap();
    assert_eq!(codes(&verify(scratch.path())), ["metadata_invalid_type"]);
}

// ---- artifacts ----------------------------------------------------------------

#[test]
fn backup_ignores_unlisted_future_and_duplicate_generations() {
    let scratch = fixture();
    // Older generations of the same databases are not examined, even when corrupt.
    for prefix in PREFIXES {
        write_artifact(
            scratch.path(),
            prefix,
            "20260719-030000",
            b"corrupt older generation",
            completed() - Span::hours(24),
        );
    }
    // Names outside the three expected databases and the archive pattern are not listed.
    for name in [
        "other-20260720-030000.sql.gz",
        "verdaxis-prod-20260720-030000.sql.gz.tmp",
        "verdaxis-prod-20260720-0300.sql.gz",
        "verdaxis-prod-20260721-030000.sql",
        "notes.txt",
    ] {
        std::fs::write(scratch.join(name), b"unlisted").unwrap();
    }
    // A directory or a link that carries an archive name is not an archive.
    std::fs::create_dir(scratch.join("verdaxis-prod-20260722-030000.sql.gz")).unwrap();
    symlink(
        artifact(scratch.path(), "umami"),
        scratch.join("umami-20260722-030000.sql.gz"),
    )
    .unwrap();
    let results = verify(scratch.path());
    assert!(
        results.iter().all(|r| r.category == Category::Healthy),
        "{results:?}"
    );

    // A newer archive than the published generation means the metadata is not the newest.
    write_artifact(
        scratch.path(),
        "verdaxis-prod",
        "20260720-033000",
        &gzip(&dump()),
        completed(),
    );
    let results = verify(scratch.path());
    assert_eq!(codes(&results), ["ok", "status_not_newest"]);
    let metadata = results
        .iter()
        .find(|r| r.name == "backup_metadata")
        .unwrap();
    assert_eq!(
        (metadata.category, metadata.code.as_str()),
        (Category::Failure, "status_not_newest")
    );
    // The three artifact checks still run, for the published generation.
    assert_eq!(results.len(), 5);

    // The listing is bounded.
    let mut limited = config(scratch.path());
    limited.max_artifacts = 3;
    let results = verify_backups(&limited, now());
    let metadata = results
        .iter()
        .find(|r| r.name == "backup_metadata")
        .unwrap();
    assert_eq!(metadata.code, "artifact_entry_limit");
    // An unreadable backup directory is reported by class, not by path.
    let mut missing = config(scratch.path());
    missing.backup_directory = scratch.join("absent");
    let results = verify_backups(&missing, now());
    let metadata = results
        .iter()
        .find(|r| r.name == "backup_metadata")
        .unwrap();
    assert_eq!(metadata.code, "backup_directory_unreadable");
    assert!(!format!("{results:?}").contains("absent"));
}

#[test]
fn backup_artifact_must_exist_be_regular_and_match_the_completion_time() {
    let scratch = fixture();
    std::fs::remove_file(artifact(scratch.path(), "umami")).unwrap();
    let results = verify(scratch.path());
    let umami = results.iter().find(|r| r.name == "umami_artifact").unwrap();
    assert_eq!(umami.code, "artifact_missing");
    // The metadata check sees no newer archive, so only the artifact fails.
    assert_eq!(
        results
            .iter()
            .find(|r| r.name == "backup_metadata")
            .unwrap()
            .code,
        "ok"
    );

    let scratch = fixture();
    set_time(&artifact(scratch.path(), "umami"), now() - Span::hours(2));
    let results = verify(scratch.path());
    assert!(codes(&results).contains(&"artifact_timestamp_mismatch"));
    // Within the skew is accepted; just beyond it is not.
    let scratch = fixture();
    set_time(
        &artifact(scratch.path(), "umami"),
        completed() + Span::seconds(4),
    );
    assert!(
        verify(scratch.path())
            .iter()
            .all(|r| r.category == Category::Healthy)
    );
    set_time(
        &artifact(scratch.path(), "umami"),
        completed() + Span::seconds(7),
    );
    assert!(codes(&verify(scratch.path())).contains(&"artifact_timestamp_mismatch"));

    // A linked artifact is refused, not followed.
    let scratch = fixture();
    let path = artifact(scratch.path(), "umami");
    let external = scratch
        .path()
        .parent()
        .unwrap()
        .join(format!("external-{}.sql.gz", uuid::Uuid::new_v4().simple()));
    std::fs::rename(&path, &external).unwrap();
    symlink(&external, &path).unwrap();
    let results = verify(scratch.path());
    // The link is not a listed file, so the generation also has no regular archive.
    let umami = results.iter().find(|r| r.name == "umami_artifact").unwrap();
    assert_eq!(umami.code, "artifact_invalid_type");
    std::fs::remove_file(&external).unwrap();
}

#[test]
fn backup_missing_empty_and_corrupt_gzip_artifacts_fail() {
    for (label, expected) in [
        ("empty", "artifact_empty"),
        ("corrupt", "gzip_invalid"),
        ("truncated", "gzip_invalid"),
    ] {
        let scratch = fixture();
        let path = artifact(scratch.path(), "umami");
        match label {
            "empty" => std::fs::write(&path, b"").unwrap(),
            "corrupt" => std::fs::write(&path, b"not gzip: do-not-copy").unwrap(),
            _ => {
                let full = gzip(&dump());
                std::fs::write(&path, &full[..full.len() / 2]).unwrap();
            }
        }
        set_time(&path, completed());
        let results = verify(scratch.path());
        assert!(codes(&results).contains(&expected), "{label}: {results:?}");
        assert!(!format!("{results:?}").contains("do-not-copy"));
    }
    // Trailing bytes after a complete stream are not a valid archive.
    let scratch = fixture();
    let path = artifact(scratch.path(), "umami");
    let mut bytes = gzip(&dump());
    bytes.extend_from_slice(b"trailing garbage");
    std::fs::write(&path, bytes).unwrap();
    set_time(&path, completed());
    assert!(codes(&verify(scratch.path())).contains(&"gzip_invalid"));
    // Two concatenated members are read as one stream.
    let scratch = fixture();
    let path = artifact(scratch.path(), "umami");
    let mut bytes = gzip(&dump());
    bytes.extend_from_slice(&gzip(&dump()));
    std::fs::write(&path, bytes).unwrap();
    set_time(&path, completed());
    assert!(
        verify(scratch.path())
            .iter()
            .all(|r| r.category == Category::Healthy)
    );
}

#[test]
fn backup_valid_gzip_without_a_plausible_dump_fails() {
    let marker_only = b"-- PostgreSQL database dump\n".to_vec();
    let late_marker = {
        let mut content = vec![b'x'; 64 * 1024];
        content.extend_from_slice(b"\n-- PostgreSQL database dump\n");
        content.extend_from_slice(&[b'y'; 2048]);
        content
    };
    for (label, content) in [
        ("empty", Vec::new()),
        ("marker only, under 1 KiB", marker_only),
        ("no marker", vec![b'x'; 2048]),
        ("marker after the scanned prefix", late_marker),
    ] {
        let scratch = fixture();
        let path = artifact(scratch.path(), "umami");
        std::fs::write(&path, gzip(&content)).unwrap();
        set_time(&path, completed());
        let results = verify(scratch.path());
        let umami = results.iter().find(|r| r.name == "umami_artifact").unwrap();
        assert_eq!(umami.code, "gzip_implausible", "{label}");
    }
}

#[test]
fn backup_rejects_an_oversize_uncompressed_artifact() {
    let scratch = fixture();
    let mut small = config(scratch.path());
    small.max_artifact_bytes = 4;
    assert!(codes(&verify_backups(&small, now())).contains(&"artifact_too_large"));
    let mut small = config(scratch.path());
    small.max_uncompressed_bytes = 4;
    let results = verify_backups(&small, now());
    assert!(codes(&results).contains(&"uncompressed_too_large"));
    // A small archive that expands far beyond the bound stops at the bound.
    let scratch = fixture();
    let path = artifact(scratch.path(), "umami");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&dump()).unwrap();
    let block = vec![0_u8; 1024 * 1024];
    for _ in 0..64 {
        encoder.write_all(&block).unwrap();
    }
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    set_time(&path, completed());
    let mut bounded = config(scratch.path());
    bounded.max_uncompressed_bytes = 8 * 1024 * 1024;
    let results = verify_backups(&bounded, now());
    let umami = results.iter().find(|r| r.name == "umami_artifact").unwrap();
    assert_eq!(umami.code, "uncompressed_too_large");
    // An expired total deadline stops the work with its own class.
    let mut late = config(scratch.path());
    late.total_timeout = Duration::ZERO;
    let results = verify_backups(&late, now());
    assert_eq!(codes(&results), ["total_deadline_exceeded"]);
}

/// Peak memory of a child test process that verifies a large archive.
///
/// The child runs this test binary again with the scratch directory in the
/// environment, so the measurement belongs to that process alone.
#[test]
fn backup_streams_gzip_within_the_128_mib_unit_limit() {
    const UNIT_LIMIT_BYTES: u64 = 128 * 1024 * 1024;
    const EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
    let scratch = fixture();
    let path = artifact(scratch.path(), "umami");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&dump()).unwrap();
    let block = vec![b'z'; 1024 * 1024];
    for _ in 0..(EXPANDED_BYTES / block.len() as u64) {
        encoder.write_all(&block).unwrap();
    }
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    set_time(&path, completed());
    assert!(std::fs::metadata(&path).unwrap().len() < 16 * 1024 * 1024);

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "backup_peak_memory_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BACKUP_PEAK_MEMORY_DIRECTORY", scratch.path())
        .output()
        .expect("run the measuring child");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed: {text} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let peak: u64 = text
        .lines()
        .find_map(|line| line.split_once("PEAK_RSS_BYTES=").map(|(_, value)| value))
        .expect("child reports its peak")
        .trim()
        .parse()
        .unwrap();
    assert!(
        peak < UNIT_LIMIT_BYTES,
        "peak {peak} must stay under the 128 MiB unit limit while expanding {EXPANDED_BYTES} bytes"
    );
}

/// Runs only when the measuring parent starts it; otherwise it does nothing.
#[test]
fn backup_peak_memory_child() {
    let Some(directory) = std::env::var_os("BACKUP_PEAK_MEMORY_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let mut large = config(&directory);
    large.max_uncompressed_bytes = 1024 * 1024 * 1024;
    let results = verify_backups(&large, now());
    assert!(
        results.iter().all(|r| r.category == Category::Healthy),
        "{results:?}"
    );
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage fills the struct it is given for the current process.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: the call above succeeded, so the struct is initialised.
    let usage = unsafe { usage.assume_init() };
    // Linux reports kibibytes; macOS reports bytes.
    let peak = if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64
    } else {
        usage.ru_maxrss as u64 * 1024
    };
    println!("PEAK_RSS_BYTES={peak}");
}

// ---- publication --------------------------------------------------------------

fn read_status(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("status file")).expect("status JSON")
}

#[test]
fn backup_status_is_private_and_key_compatible() {
    let scratch = fixture();
    let config = config(scratch.path());
    assert_eq!(run_at(&config, &AdmitAll, now()), Exit::Ok);
    let mode = std::fs::metadata(&config.output)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let published = read_status(&config.output);
    let mut keys: Vec<&String> = published.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["category", "checks", "generated_at", "schema_version"]
    );
    assert_eq!(published["category"], "healthy");
    assert_eq!(published["schema_version"], 1);
    let names: Vec<&str> = published["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "backup_attempt",
            "backup_metadata",
            "verdaxis_prod_artifact",
            "verdaxis_staging_artifact",
            "umami_artifact"
        ]
    );
    for check in published["checks"].as_array().unwrap() {
        let mut keys: Vec<&String> = check.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["category", "code", "duration_ms", "last_success_at", "name"]
        );
        assert!(
            check["last_success_at"].is_string(),
            "a healthy check has a success time"
        );
    }
    let leftovers: Vec<String> = std::fs::read_dir(config.output.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "status.json")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // A failing run exits 2, keeps the earlier success time, and names a class, not a path.
    std::fs::remove_file(artifact(scratch.path(), "umami")).unwrap();
    assert_eq!(run_at(&config, &AdmitAll, now()), Exit::Usage);
    let failed = read_status(&config.output);
    assert_eq!(failed["category"], "failure");
    let umami = failed["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "umami_artifact")
        .unwrap();
    assert_eq!(umami["code"], "artifact_missing");
    assert_eq!(
        umami["last_success_at"],
        published["checks"][4]["last_success_at"]
    );
    assert!(
        !failed
            .to_string()
            .contains(&scratch.path().to_string_lossy().into_owned())
    );
}

#[test]
fn backup_quarantines_a_malformed_previous_status() {
    for bytes in [&br#"{"schema_version":1,"checks":null}"#[..], b"[]", b"{"] {
        let scratch = fixture();
        let config = config(scratch.path());
        std::fs::create_dir_all(config.output.parent().unwrap()).unwrap();
        std::fs::write(&config.output, bytes).unwrap();
        let status = collect(&config, now()).expect("still a status");
        assert_eq!(status.category, Category::Failure);
        assert_eq!(status.checks[0].code, "previous_status_invalid");
        assert_eq!(status.checks[0].name, "backup_monitor");
        assert!(!config.output.exists());
        let quarantined = std::fs::read_dir(config.output.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("status.json.invalid-")
            })
            .count();
        assert_eq!(quarantined, 1);
    }
}

struct Refusing;

impl Admission for Refusing {
    fn admit(&self, _status: &Status) -> Result<(), Exit> {
        Err(Exit::Dependency)
    }
}

#[test]
fn backup_publication_waits_for_handoff_admission() {
    let scratch = fixture();
    let config = config(scratch.path());
    assert_eq!(run_at(&config, &AdmitAll, now()), Exit::Ok);
    let before = std::fs::read(&config.output).unwrap();
    std::fs::remove_file(artifact(scratch.path(), "umami")).unwrap();
    assert_eq!(run_at(&config, &Refusing, now()), Exit::Dependency);
    assert_eq!(std::fs::read(&config.output).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(config.output.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn backup_fatal_error_status_is_fixed_and_named() {
    let status = failure_status("fatal_error", now(), "backup_monitor");
    assert_eq!(status.checks[0].name, "backup_monitor");
    // An output directory that cannot be created is a dependency failure, not a panic.
    let scratch = fixture();
    let mut config = config(scratch.path());
    std::fs::write(scratch.join("blocker"), b"file").unwrap();
    config.output = scratch.join("blocker/inner/status.json");
    assert_eq!(run_at(&config, &AdmitAll, now()), Exit::Dependency);
}
