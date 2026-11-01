//! `verdaxis-read monitor backup-verify`: read-only checks of local backups.
//!
//! The monitor reads the producer's attempt records, its published metadata and
//! the newest archives, and publishes one private status document (see
//! [`super::status`]). It needs no credentials and no network. Nothing it reads
//! is written, created, moved or repaired, and a failure names a fixed class,
//! never a path or file content.
//!
//! The latest attempt decides first: a newer started or failed attempt keeps an
//! older success red. Only a newer success whose backup id and finish time match
//! the published metadata and the archives turns the check green again.
use super::status::{
    build_status, exit_for, failure_status, guarded, load_monitor_status, parse_bounded_timestamp,
    parse_strict_json, parse_utc_timestamp, publish, quarantine, report, report_refused, Admission,
    AdmitAll, Category, CheckResult, Fault, Previous, Status, MAX_STATUS_BYTES,
};
use crate::cli::Exit;
use chrono::{DateTime, Duration as Span, NaiveDate, Utc};
use flate2::read::MultiGzDecoder;
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const COMMAND: &str = "monitor backup-verify";
const MONITOR_NAME: &str = "backup_monitor";
/// Each database, its status name prefix and its archive file prefix.
const DATABASES: [(&str, &str); 3] = [
    ("verdaxis_prod", "verdaxis-prod"),
    ("verdaxis_staging", "verdaxis-staging"),
    ("umami", "umami"),
];
/// An archive must expand to at least this many bytes.
const MIN_UNCOMPRESSED_BYTES: u64 = 1024;
/// The dump marker must appear in this many leading bytes.
const MARKER_SCAN_BYTES: usize = 64 * 1024;
const SQL_MARKER: &[u8] = b"PostgreSQL database dump";
/// Streaming read size: the only buffer an archive is ever read into.
const GZIP_CHUNK_BYTES: usize = 1024 * 1024;
const ATTEMPT_ID_MAX_CHARS: usize = 128;
const BACKUP_ID_CHARS: usize = 15;

/// Everything the monitor reads and its limits. The default is the installed
/// layout; tests build their own values.
#[derive(Debug, Clone)]
pub struct BackupConfig {
    pub backup_status: PathBuf,
    pub backup_directory: PathBuf,
    pub attempt_directory: PathBuf,
    pub output: PathBuf,
    pub max_age_seconds: i64,
    pub future_tolerance_seconds: i64,
    pub max_timestamp_skew_seconds: i64,
    pub max_metadata_bytes: u64,
    pub max_attempt_records: usize,
    pub max_attempt_bytes: u64,
    pub max_artifacts: usize,
    pub max_artifact_bytes: u64,
    pub max_uncompressed_bytes: u64,
    pub total_timeout: Duration,
}

impl Default for BackupConfig {
    fn default() -> Self {
        const GIB: u64 = 1024 * 1024 * 1024;
        Self {
            backup_status: PathBuf::from("/home/verdaxis-prod/backups/status.json"),
            backup_directory: PathBuf::from("/home/verdaxis-prod/backups"),
            attempt_directory: PathBuf::from("/home/verdaxis-prod/backups/attempts"),
            output: PathBuf::from("/var/lib/verdaxis-backup-verify/status.json"),
            max_age_seconds: 30 * 60 * 60,
            future_tolerance_seconds: 300,
            max_timestamp_skew_seconds: 5,
            max_metadata_bytes: 16_384,
            max_attempt_records: 1_000,
            max_attempt_bytes: 4_096,
            max_artifacts: 1_000,
            max_artifact_bytes: 20 * GIB,
            max_uncompressed_bytes: 100 * GIB,
            total_timeout: Duration::from_secs(120),
        }
    }
}

// ---- small helpers ---------------------------------------------------------------

/// `YYYYMMDD-HHMMSS`: eight digits, a hyphen and six digits.
fn is_backup_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == BACKUP_ID_CHARS
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}

/// The UTC time a backup id names, or none for an impossible date or time.
fn backup_time(backup_id: &str) -> Option<DateTime<Utc>> {
    if !is_backup_id(backup_id) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| backup_id[range].parse::<u32>().ok();
    NaiveDate::from_ymd_opt(
        i32::try_from(number(0..4)?).ok()?,
        number(4..6)?,
        number(6..8)?,
    )?
    .and_hms_opt(number(9..11)?, number(11..13)?, number(13..15)?)
    .map(|naive| naive.and_utc())
}

fn failure(name: &str, code: &str, started: Instant) -> CheckResult {
    CheckResult::new(name, Category::Failure, code, started)
}

fn metadata_failure(code: &str, started: Instant) -> Vec<CheckResult> {
    vec![failure("backup_metadata", code, started)]
}

fn attempt_failure(code: &str, started: Instant) -> Vec<CheckResult> {
    vec![failure("backup_attempt", code, started)]
}

/// Read one bounded file from an open descriptor. None if it is not regular, is
/// larger than `max_bytes`, or cannot be read.
fn read_bounded_file(file: File, max_bytes: u64) -> Option<Vec<u8>> {
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut raw = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut raw)
        .ok()?;
    (raw.len() as u64 <= max_bytes).then_some(raw)
}

// ---- attempt records -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptState {
    Started,
    Failed,
    Succeeded,
}

/// One validated attempt record.
struct Attempt {
    started_at: DateTime<Utc>,
    state: AttemptState,
    finished_at: Option<DateTime<Utc>>,
    backup_id: Option<String>,
}

fn is_attempt_id(text: &str) -> bool {
    let mut characters = text.chars();
    text.chars().count() <= ATTEMPT_ID_MAX_CHARS
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|next| next.is_ascii_alphanumeric() || matches!(next, '.' | '_' | '-'))
}

/// Validate one record against the exact schema, the file name and the clock.
fn validate_attempt(
    payload: &Value,
    file_name: &str,
    now: DateTime<Utc>,
    future_tolerance: Span,
) -> Option<Attempt> {
    let record = payload.as_object()?;
    let keys = [
        "schema_version",
        "attempt_id",
        "state",
        "started_at",
        "finished_at",
        "backup_id",
    ];
    if record.len() != keys.len() || keys.iter().any(|key| !record.contains_key(*key)) {
        return None;
    }
    if record["schema_version"].as_i64() != Some(1) {
        return None;
    }
    let attempt_id = record["attempt_id"].as_str()?;
    if !is_attempt_id(attempt_id) || file_name != format!("{attempt_id}.json") {
        return None;
    }
    let state = match record["state"].as_str()? {
        "started" => AttemptState::Started,
        "failed" => AttemptState::Failed,
        "succeeded" => AttemptState::Succeeded,
        _ => return None,
    };
    let started_at = parse_bounded_timestamp(record["started_at"].as_str()?)?;
    if started_at - now > future_tolerance {
        return None;
    }
    let finished = &record["finished_at"];
    let backup_id = &record["backup_id"];
    if state == AttemptState::Started {
        return (finished.is_null() && backup_id.is_null()).then_some(Attempt {
            started_at,
            state,
            finished_at: None,
            backup_id: None,
        });
    }
    let finished_at = parse_bounded_timestamp(finished.as_str()?)?;
    if finished_at < started_at || finished_at - now > future_tolerance {
        return None;
    }
    let backup_id = if state == AttemptState::Failed {
        if !backup_id.is_null() {
            return None;
        }
        None
    } else {
        let id = backup_id.as_str().filter(|id| is_backup_id(id))?;
        Some(id.to_owned())
    };
    Some(Attempt {
        started_at,
        state,
        finished_at: Some(finished_at),
        backup_id,
    })
}

/// Whether a directory entry is a regular file, never following a link.
fn is_regular_entry(directory: &OwnedFd, name: &std::ffi::CStr, kind: FileType) -> bool {
    match kind {
        FileType::RegularFile => true,
        FileType::Unknown => rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile),
        _ => false,
    }
}

/// The attempt with the latest start, or a fixed refusal code.
///
/// Every `*.json` entry must be a bounded regular file (no link) and a valid
/// record; records that started at the same instant cannot be ordered and are
/// refused. The directory is opened without following a link.
fn latest_attempt(
    config: &BackupConfig,
    now: DateTime<Utc>,
    deadline: Instant,
) -> Result<Attempt, &'static str> {
    let directory = match rustix::fs::open(
        &config.attempt_directory,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(rustix::io::Errno::NOENT) => return Err("attempt_evidence_missing"),
        Err(_) => return Err("attempt_evidence_invalid"),
    };
    let invalid = "attempt_evidence_invalid";
    let listing = Dir::read_from(&directory).map_err(|_| invalid)?;
    let tolerance = Span::seconds(config.future_tolerance_seconds);
    let mut records: Vec<Attempt> = Vec::new();
    let mut count = 0_usize;
    for entry in listing {
        let entry = entry.map_err(|_| invalid)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        count += 1;
        if count > config.max_attempt_records {
            return Err(invalid);
        }
        if Instant::now() >= deadline {
            return Err("total_deadline_exceeded");
        }
        if !name.to_bytes().ends_with(b".json") {
            continue;
        }
        if !is_regular_entry(&directory, name, entry.file_type()) {
            return Err(invalid);
        }
        let file_name = name.to_str().map_err(|_| invalid)?;
        let descriptor = rustix::fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| invalid)?;
        let payload = read_bounded_file(File::from(descriptor), config.max_attempt_bytes)
            .and_then(|raw| parse_strict_json(&raw))
            .ok_or(invalid)?;
        records.push(validate_attempt(&payload, file_name, now, tolerance).ok_or(invalid)?);
    }
    if records.is_empty() {
        return Err("attempt_evidence_missing");
    }
    let starts: HashSet<DateTime<Utc>> = records.iter().map(|record| record.started_at).collect();
    if starts.len() != records.len() {
        return Err(invalid);
    }
    records
        .into_iter()
        .max_by_key(|record| record.started_at)
        .ok_or(invalid)
}

// ---- metadata ---------------------------------------------------------------------------

/// Read the published metadata: a bounded regular file, not a link, with unique
/// keys, holding one JSON object.
fn read_metadata(config: &BackupConfig, started: Instant) -> Result<Value, Vec<CheckResult>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&config.backup_status)
        .map_err(|error| {
            metadata_failure(
                match (error.kind(), error.raw_os_error()) {
                    (std::io::ErrorKind::NotFound, _) => "metadata_missing",
                    (_, Some(libc::ELOOP)) => "metadata_invalid_type",
                    _ => "metadata_unreadable",
                },
                started,
            )
        })?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(metadata_failure("metadata_invalid_type", started));
    }
    let mut raw = Vec::new();
    file.take(config.max_metadata_bytes.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(|_| metadata_failure("metadata_unreadable", started))?;
    if raw.len() as u64 > config.max_metadata_bytes {
        return Err(metadata_failure("metadata_too_large", started));
    }
    match parse_strict_json(&raw) {
        Some(payload) if payload.is_object() => Ok(payload),
        _ => Err(metadata_failure("metadata_invalid_json", started)),
    }
}

/// Check the metadata's schema, inventory, freshness and the coupling between
/// its completion time and its backup id. Returns the id and completion time.
fn validate_metadata(
    payload: &Value,
    config: &BackupConfig,
    now: DateTime<Utc>,
    started: Instant,
) -> Result<(String, DateTime<Utc>), Vec<CheckResult>> {
    let refuse = |code: &str| Err(metadata_failure(code, started));
    let Some(document) = payload.as_object() else {
        return refuse("metadata_invalid_json");
    };
    let keys = ["ok", "completed_at", "backup_id", "databases"];
    if document.len() != keys.len() || keys.iter().any(|key| !document.contains_key(*key)) {
        return refuse("metadata_schema_mismatch");
    }
    if document["ok"] != Value::Bool(true) {
        return refuse("backup_incomplete");
    }
    let inventory_ok = document["databases"].as_array().is_some_and(|list| {
        let names: HashSet<&str> = list.iter().filter_map(Value::as_str).collect();
        list.len() == DATABASES.len()
            && names.len() == list.len()
            && DATABASES
                .iter()
                .all(|(database, _)| names.contains(database))
    });
    if !inventory_ok {
        return refuse("unexpected_inventory");
    }
    let Some(backup_id) = document["backup_id"].as_str().filter(|id| is_backup_id(id)) else {
        return refuse("invalid_backup_id");
    };
    let Some(completed_at) = document["completed_at"]
        .as_str()
        .and_then(parse_utc_timestamp)
    else {
        return refuse("invalid_timestamp");
    };
    let age = now - completed_at;
    if age < -Span::seconds(config.future_tolerance_seconds) {
        return refuse("future_timestamp");
    }
    if age > Span::seconds(config.max_age_seconds) {
        return refuse("stale_status");
    }
    let Some(named_time) = backup_time(backup_id) else {
        return refuse("invalid_backup_id");
    };
    if (completed_at - named_time).abs() > Span::seconds(config.max_timestamp_skew_seconds) {
        return refuse("backup_id_timestamp_mismatch");
    }
    Ok((backup_id.to_owned(), completed_at))
}

// ---- archives -------------------------------------------------------------------------------

/// The newest backup id of each listed database in the backup directory.
///
/// Only regular files named `<prefix>-<id>.sql.gz` for the three known prefixes
/// are listed; everything else, including directories and links, is ignored.
fn newest_artifacts(
    config: &BackupConfig,
    deadline: Instant,
) -> Result<Vec<(usize, String)>, &'static str> {
    let listing =
        std::fs::read_dir(&config.backup_directory).map_err(|_| "backup_directory_unreadable")?;
    let mut newest: Vec<Option<String>> = vec![None; DATABASES.len()];
    for (count, entry) in listing.enumerate() {
        if count + 1 > config.max_artifacts {
            return Err("artifact_entry_limit");
        }
        if Instant::now() >= deadline {
            return Err("total_deadline_exceeded");
        }
        let entry = entry.map_err(|_| "backup_directory_unreadable")?;
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some((index, id)) = name.to_str().and_then(listed_artifact) else {
            continue;
        };
        if newest[index].as_deref() < Some(id) {
            newest[index] = Some(id.to_owned());
        }
    }
    Ok(newest
        .into_iter()
        .enumerate()
        .filter_map(|(index, id)| id.map(|id| (index, id)))
        .collect())
}

/// The database index and backup id of a listed archive name.
fn listed_artifact(name: &str) -> Option<(usize, &str)> {
    let stem = name.strip_suffix(".sql.gz")?;
    DATABASES
        .iter()
        .enumerate()
        .find_map(|(index, (_, prefix))| {
            let id = stem.strip_prefix(prefix)?.strip_prefix('-')?;
            is_backup_id(id).then_some((index, id))
        })
}

/// Verify one archive: a regular file (no link), not empty, within the size
/// bound, finished at the metadata's completion time, valid gzip, and a plausible
/// dump once expanded.
///
/// The archive is streamed through a fixed 1 MiB buffer. The expanded size is
/// bounded as it is produced, so an archive that expands without limit stops at
/// the bound. The check stops at `deadline`.
fn check_archive(
    name: &str,
    path: &Path,
    config: &BackupConfig,
    completed_at: DateTime<Utc>,
    deadline: Instant,
) -> CheckResult {
    let started = Instant::now();
    let fail = |code: &str| failure(name, code, started);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return fail("artifact_missing");
        }
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return fail("artifact_invalid_type");
        }
        Err(_) => return fail("artifact_unreadable"),
    };
    let Ok(metadata) = file.metadata() else {
        return fail("artifact_unreadable");
    };
    if !metadata.is_file() {
        return fail("artifact_invalid_type");
    }
    if metadata.len() == 0 {
        return fail("artifact_empty");
    }
    if metadata.len() > config.max_artifact_bytes {
        return fail("artifact_too_large");
    }
    let Ok(modified) = metadata.modified() else {
        return fail("artifact_unreadable");
    };
    let modified = DateTime::<Utc>::from(modified);
    if (completed_at - modified).abs() > Span::seconds(config.max_timestamp_skew_seconds) {
        return fail("artifact_timestamp_mismatch");
    }
    let mut decoder = MultiGzDecoder::new(file);
    let mut buffer = vec![0_u8; GZIP_CHUNK_BYTES];
    let mut prefix: Vec<u8> = Vec::new();
    let mut expanded = 0_u64;
    loop {
        if Instant::now() >= deadline {
            return fail("total_deadline_exceeded");
        }
        let read = match decoder.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => return fail("gzip_invalid"),
        };
        expanded += read as u64;
        if prefix.len() < MARKER_SCAN_BYTES {
            let room = MARKER_SCAN_BYTES - prefix.len();
            prefix.extend_from_slice(&buffer[..read.min(room)]);
        }
        if expanded > config.max_uncompressed_bytes {
            return fail("uncompressed_too_large");
        }
    }
    let has_marker = prefix
        .windows(SQL_MARKER.len())
        .any(|window| window == SQL_MARKER);
    if expanded < MIN_UNCOMPRESSED_BYTES || !has_marker {
        return fail("gzip_implausible");
    }
    CheckResult::new(name, Category::Healthy, "ok", started)
}

// ---- verification ----------------------------------------------------------------------------

/// Verify the backups and return the check results.
///
/// The run ends early, with one result, when the attempt evidence or the
/// metadata fails. Otherwise it returns the attempt, the metadata and one result
/// per database archive, in that order.
pub fn verify_backups(config: &BackupConfig, now: DateTime<Utc>) -> Vec<CheckResult> {
    let started = Instant::now();
    let deadline = started + config.total_timeout;
    let attempt = match latest_attempt(config, now, deadline) {
        Ok(attempt) => attempt,
        Err(code) => return attempt_failure(code, started),
    };
    match attempt.state {
        AttemptState::Started => return attempt_failure("backup_attempt_incomplete", started),
        AttemptState::Failed => return attempt_failure("backup_attempt_failed", started),
        AttemptState::Succeeded => {}
    }
    let payload = match read_metadata(config, started) {
        Ok(payload) => payload,
        Err(results) => return results,
    };
    let (backup_id, completed_at) = match validate_metadata(&payload, config, now, started) {
        Ok(identity) => identity,
        Err(results) => return results,
    };
    let skew = Span::seconds(config.max_timestamp_skew_seconds);
    let matches_attempt = attempt.backup_id.as_deref() == Some(backup_id.as_str())
        && attempt.finished_at.is_some_and(|finished| {
            completed_at >= attempt.started_at && (completed_at - finished).abs() <= skew
        });
    if !matches_attempt {
        return attempt_failure("attempt_status_mismatch", started);
    }
    let mut results = vec![CheckResult::new(
        "backup_attempt",
        Category::Healthy,
        "ok",
        started,
    )];
    let metadata_code = match newest_artifacts(config, deadline) {
        Err(code) => code,
        Ok(newest) if newest.iter().any(|(_, id)| *id != backup_id) => "status_not_newest",
        Ok(_) => "ok",
    };
    results.push(CheckResult::new(
        "backup_metadata",
        if metadata_code == "ok" {
            Category::Healthy
        } else {
            Category::Failure
        },
        metadata_code,
        started,
    ));
    for (database, prefix) in DATABASES {
        results.push(check_archive(
            &format!("{database}_artifact"),
            &config
                .backup_directory
                .join(format!("{prefix}-{backup_id}.sql.gz")),
            config,
            completed_at,
            deadline,
        ));
    }
    results
}

/// Verify and build the new status.
///
/// An unsafe previous document is quarantined first and reported as one
/// `previous_status_invalid` failure with no backup read.
pub fn collect(config: &BackupConfig, now: DateTime<Utc>) -> Result<Status, Fault> {
    let previous = load_monitor_status(&config.output, MAX_STATUS_BYTES);
    if previous == Previous::Malformed {
        quarantine(&config.output).map_err(|_| Fault)?;
        return Ok(failure_status("previous_status_invalid", now, MONITOR_NAME));
    }
    Ok(build_status(verify_backups(config, now), &previous, now))
}

/// Verify, admit and publish one status. Returns 0 for a healthy status, 2 for a
/// warning or failure, and the admission or write error when the status could
/// not be published.
pub fn run(config: &BackupConfig, admission: &dyn Admission) -> Exit {
    run_at(config, admission, Utc::now())
}

/// [`run`] with the observation time given, for tests that use a fixed clock.
pub fn run_at(config: &BackupConfig, admission: &dyn Admission, now: DateTime<Utc>) -> Exit {
    let status = guarded(MONITOR_NAME, now, || collect(config, now));
    match publish(&config.output, &status, admission) {
        Ok(()) => {
            report(COMMAND, &status);
            exit_for(status.category)
        }
        Err(exit) => {
            report_refused(COMMAND);
            exit
        }
    }
}

/// The installed command: default layout, no journal yet.
pub fn run_default() -> Exit {
    run(&BackupConfig::default(), &AdmitAll)
}
