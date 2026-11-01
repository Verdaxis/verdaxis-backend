//! Shared status document of the local monitors.
//!
//! Each local monitor publishes one small private JSON document that the alert
//! units read. This module owns that document: its bounded and strict reader,
//! its validity rules, the deterministic worst-category roll-up, quarantine of
//! an unsafe previous document, and the durable atomic publication.
//!
//! The document keeps the keys and codes the alert units already read:
//! `schema_version`, `generated_at`, `category` and `checks`, where each check
//! has `name`, `category`, `code`, `duration_ms` and `last_success_at`.
//!
//! Publication passes through an [`Admission`] first. The alert handoff
//! journal plugs in there: a monitor never publishes a status that the journal
//! refused to admit.
use crate::cli::Exit;
use crate::cli::state::write_atomic;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Largest status document the monitors read back.
pub const MAX_STATUS_BYTES: u64 = 65_536;
/// Most checks one status document may hold.
pub const MAX_CHECKS: usize = 128;
/// Longest check name or code, in characters.
pub const MAX_NAME_CHARS: usize = 128;
/// Longest accepted timestamp text, in characters.
pub const MAX_TIMESTAMP_CHARS: usize = 40;
/// Largest check duration a document may carry: one day in milliseconds.
const MAX_DURATION_MS: i64 = 86_400_000;
const SCHEMA_VERSION: u32 = 1;

/// Outcome of one check. The order is the roll-up order: the document takes the
/// greatest category of its checks, so a warning is never reported as healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Healthy,
    Warning,
    Failure,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Failure => "failure",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "healthy" => Some(Self::Healthy),
            "warning" => Some(Self::Warning),
            "failure" => Some(Self::Failure),
            _ => None,
        }
    }
}

/// One finished check, before it is placed in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub category: Category,
    pub code: String,
    pub duration_ms: u64,
}

impl CheckResult {
    /// A result whose duration is the time since `started`.
    pub fn new(name: &str, category: Category, code: &str, started: Instant) -> Self {
        Self {
            name: name.to_owned(),
            category,
            code: code.to_owned(),
            duration_ms: elapsed_ms(started),
        }
    }
}

/// Whole milliseconds since `started`.
pub fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// One check as published. The fields are in key order, so the compact JSON has
/// sorted keys, the same bytes as the document the previous monitor wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatus {
    pub category: Category,
    pub code: String,
    pub duration_ms: u64,
    pub last_success_at: Option<String>,
    pub name: String,
}

/// The published document. Fields are in key order, see [`CheckStatus`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub category: Category,
    pub checks: Vec<CheckStatus>,
    pub generated_at: String,
    pub schema_version: u32,
}

/// What the previous document on disk was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    /// No document: a first run.
    Absent,
    /// A document that passed every validity rule.
    Valid(Status),
    /// Something at the path that is not a valid document.
    Malformed,
}

/// A collection that could not finish. It carries no message on purpose: the
/// published status names only a fixed code, never a cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault;

/// The point where a monitor hands its observation to the alert journal.
///
/// `admit` runs after the status is built and before it is published. An error
/// means the observation was not admitted: the run fails with that exit code and
/// the published document stays as it was.
pub trait Admission {
    fn admit(&self, status: &Status) -> Result<(), Exit>;
}

/// The admission used until the alert journal is connected: it admits everything.
pub struct AdmitAll;

impl Admission for AdmitAll {
    fn admit(&self, _status: &Status) -> Result<(), Exit> {
        Ok(())
    }
}

/// UTC text with whole seconds and a `Z` suffix.
pub fn utc_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse a timestamp that carries a zone. Text without a zone is refused, so a
/// local time can never be mistaken for UTC.
pub fn parse_utc_timestamp(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%#z"))
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Like [`parse_utc_timestamp`], for the short timestamps of status documents
/// and attempt records: not empty and at most [`MAX_TIMESTAMP_CHARS`] long.
pub fn parse_bounded_timestamp(text: &str) -> Option<DateTime<Utc>> {
    if text.is_empty() || text.chars().count() > MAX_TIMESTAMP_CHARS {
        return None;
    }
    parse_utc_timestamp(text)
}

/// Parse JSON and refuse any object that repeats a key.
///
/// `serde_json::Value` keeps only the last duplicate, so a hostile document
/// could hide a field. The check runs over the raw bytes, then the value is
/// parsed normally. Nesting deeper than serde_json's limit is refused too.
pub fn parse_strict_json(bytes: &[u8]) -> Option<Value> {
    let mut checker = serde_json::Deserializer::from_slice(bytes);
    NoDuplicateKeys.deserialize(&mut checker).ok()?;
    checker.end().ok()?;
    serde_json::from_slice(bytes).ok()
}

/// Walks a JSON document and fails on the first repeated object key.
struct NoDuplicateKeys;

impl<'de> DeserializeSeed<'de> for NoDuplicateKeys {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for NoDuplicateKeys {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate keys")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        while sequence.next_element_seed(NoDuplicateKeys)?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(serde::de::Error::custom("duplicate key"));
            }
            map.next_value_seed(NoDuplicateKeys)?;
        }
        Ok(())
    }
}

/// A document the monitors may read back as the previous status.
///
/// The exact key set, `schema_version` 1 as an integer, a zoned `generated_at`,
/// one to 128 uniquely named checks with bounded fields, and a top-level
/// category equal to the worst check category.
pub fn valid_monitor_status(value: &Value) -> bool {
    let Some(document) = value.as_object() else {
        return false;
    };
    let keys = ["schema_version", "generated_at", "category", "checks"];
    if document.len() != keys.len() || keys.iter().any(|key| !document.contains_key(*key)) {
        return false;
    }
    if document["schema_version"].as_i64() != Some(i64::from(SCHEMA_VERSION)) {
        return false;
    }
    let Some(category) = document["category"].as_str().and_then(Category::parse) else {
        return false;
    };
    let generated_at = document["generated_at"]
        .as_str()
        .and_then(parse_bounded_timestamp);
    if generated_at.is_none() {
        return false;
    }
    let Some(checks) = document["checks"].as_array() else {
        return false;
    };
    if checks.is_empty() || checks.len() > MAX_CHECKS {
        return false;
    }
    let mut names = HashSet::new();
    let mut worst = Category::Healthy;
    for check in checks {
        let Some(check_category) = valid_check(check, &mut names) else {
            return false;
        };
        worst = worst.max(check_category);
    }
    category == worst
}

/// Validate one check entry and return its category.
fn valid_check<'a>(check: &'a Value, names: &mut HashSet<&'a str>) -> Option<Category> {
    let check = check.as_object()?;
    let keys = ["name", "category", "code", "duration_ms", "last_success_at"];
    if check.len() != keys.len() || keys.iter().any(|key| !check.contains_key(*key)) {
        return None;
    }
    let name = check["name"].as_str()?;
    let code = check["code"].as_str()?;
    let duration = check["duration_ms"].as_i64()?;
    let category = Category::parse(check["category"].as_str()?)?;
    let bounded = |text: &str| !text.is_empty() && text.chars().count() <= MAX_NAME_CHARS;
    if !bounded(name) || !bounded(code) || !names.insert(name) {
        return None;
    }
    if !(0..=MAX_DURATION_MS).contains(&duration) {
        return None;
    }
    match &check["last_success_at"] {
        Value::Null => {}
        Value::String(text) if parse_bounded_timestamp(text).is_some() => {}
        _ => return None,
    }
    Some(category)
}

/// Read the previous document.
///
/// A missing file is [`Previous::Absent`]. A link, a non-regular file, an
/// oversize or unreadable file, repeated keys, or a document that fails
/// [`valid_monitor_status`] is [`Previous::Malformed`]. The path is opened with
/// `O_NOFOLLOW`, and the size bound applies while reading.
pub fn load_monitor_status(path: &Path, max_bytes: u64) -> Previous {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Previous::Absent,
        Err(error) => {
            let refused_by_policy = matches!(
                error.raw_os_error(),
                Some(libc::ELOOP | libc::EACCES | libc::EPERM)
            );
            return if refused_by_policy || path.exists() {
                Previous::Malformed
            } else {
                Previous::Absent
            };
        }
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Previous::Malformed;
    }
    let mut raw = Vec::new();
    if file
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut raw)
        .is_err()
        || raw.len() as u64 > max_bytes
    {
        return Previous::Malformed;
    }
    let Some(value) = parse_strict_json(&raw) else {
        return Previous::Malformed;
    };
    if !valid_monitor_status(&value) {
        return Previous::Malformed;
    }
    match serde_json::from_value(value) {
        Ok(status) => Previous::Valid(status),
        Err(_) => Previous::Malformed,
    }
}

/// Move an unsafe document aside as `<name>.invalid-<nanoseconds>` and make the
/// rename durable. A missing path is not an error.
pub fn quarantine(path: &Path) -> std::io::Result<Option<PathBuf>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let (parent, name) = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => (parent, name.to_string_lossy().into_owned()),
        _ => return Err(std::io::ErrorKind::InvalidInput.into()),
    };
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let destination = parent.join(format!("{name}.invalid-{nanoseconds}"));
    std::fs::rename(path, &destination)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(Some(destination))
}

/// A document with one failed check that names the fixed `code`.
pub fn failure_status(code: &str, at: DateTime<Utc>, name: &str) -> Status {
    Status {
        category: Category::Failure,
        checks: vec![CheckStatus {
            category: Category::Failure,
            code: code.to_owned(),
            duration_ms: 0,
            last_success_at: None,
            name: name.to_owned(),
        }],
        generated_at: utc_text(at),
        schema_version: SCHEMA_VERSION,
    }
}

/// Roll the check results up into a document.
///
/// A healthy check records `at` as its success time. Any other check keeps the
/// success time the previous document held for the same name, or `null`. No
/// checks at all is a failure, never a healthy document.
pub fn build_status(checks: Vec<CheckResult>, previous: &Previous, at: DateTime<Utc>) -> Status {
    if checks.is_empty() {
        return failure_status("no_checks", at, "monitor");
    }
    let now = utc_text(at);
    let earlier = |name: &str| match previous {
        Previous::Valid(document) => document
            .checks
            .iter()
            .find(|check| check.name == name)
            .and_then(|check| check.last_success_at.clone()),
        _ => None,
    };
    let checks: Vec<CheckStatus> = checks
        .into_iter()
        .map(|check| CheckStatus {
            last_success_at: if check.category == Category::Healthy {
                Some(now.clone())
            } else {
                earlier(&check.name)
            },
            category: check.category,
            code: check.code,
            duration_ms: check.duration_ms,
            name: check.name,
        })
        .collect();
    let category = checks
        .iter()
        .map(|check| check.category)
        .max()
        .unwrap_or(Category::Failure);
    Status {
        category,
        checks,
        generated_at: now,
        schema_version: SCHEMA_VERSION,
    }
}

/// Exit code of a finished run: 0 only when healthy. A warning exits like a
/// failure, so the failure unit reports both.
pub fn exit_for(category: Category) -> Exit {
    if category == Category::Healthy {
        Exit::Ok
    } else {
        Exit::Usage
    }
}

/// Admit, then durably publish `status` at `path`.
///
/// The private state directory is created when missing. The document is written
/// to a temp file in the same directory (0600), synced, renamed into place and
/// the directory synced. A refused admission or a failed write leaves the
/// previous document in place.
pub fn publish(path: &Path, status: &Status, admission: &dyn Admission) -> Result<(), Exit> {
    admission.admit(status)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|_| Exit::Dependency)?;
    }
    write_atomic(path, status)
}

/// Run one collection. A refusal or a panic becomes the fixed `fatal_error`
/// status; neither the cause nor any panic text reaches the document.
pub fn guarded<F>(name: &str, at: DateTime<Utc>, collect: F) -> Status
where
    F: FnOnce() -> Result<Status, Fault>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(collect)) {
        Ok(Ok(status)) => status,
        Ok(Err(Fault)) | Err(_) => failure_status("fatal_error", at, name),
    }
}

/// Print the one-line run report on stderr: the command, the overall outcome and
/// the number of checks per category. It names no path, address or cause.
pub fn report(command: &str, status: &Status) {
    let count = |category: Category| {
        status
            .checks
            .iter()
            .filter(|check| check.category == category)
            .count()
    };
    eprintln!(
        "{}",
        serde_json::json!({
            "command": command,
            "outcome": status.category.as_str(),
            "counts": {
                "healthy": count(Category::Healthy),
                "warning": count(Category::Warning),
                "failure": count(Category::Failure),
            },
        })
    );
}

/// Print the run report for a run that could not publish.
pub fn report_refused(command: &str) {
    eprintln!(
        "{}",
        serde_json::json!({"command": command, "outcome": "publish_refused", "counts": {}})
    );
}
