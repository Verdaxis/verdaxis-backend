//! Start guards for the service units: the prepared runtime identity and the
//! exact readiness corpus.
//!
//! These functions read deploy-written files and stdin only. They open no
//! database, socket or secret, so they stay valid before the service starts.
//! The deploy-start guard is the separate shell helper, because it must run
//! before any Rust artifact exists.

use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Prepared connection budget; deploy writes it atomically under this bound.
pub const MAX_BUDGET_BYTES: u64 = 64 * 1024;
/// Migration revision record; exactly two short fields.
pub const MAX_REVISION_BYTES: u64 = 8 * 1024;
/// Readiness body; the service emits four short strings.
pub const MAX_HEALTH_BYTES: u64 = 64 * 1024;

const MAX_STATE_FIELD_CHARS: usize = 256;

// `std` has no portable `O_NOFOLLOW` constant. The guard refuses symlinks
// before opening as well, so this flag only closes the replace-after-check gap.
// Linux `open(2)`: O_NOFOLLOW is 0x20000 and O_NONBLOCK is 0x800.
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400000;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
// Darwin `fcntl.h`: O_NOFOLLOW is 0x0100 and O_NONBLOCK is 0x0004.
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;

/// Environments that have a deployed unit and a deploy state file.
pub fn check_environment(environment: &str) -> Result<(), &'static str> {
    match environment {
        "staging" | "production" => Ok(()),
        _ => Err("environment must be staging or production"),
    }
}

/// Full lowercase release identity; the deploy and readiness bodies use it.
pub fn check_release_sha(release_sha: &str) -> Result<(), &'static str> {
    if release_sha.len() == 40
        && release_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("release SHA must be 40 lowercase hexadecimal characters")
    }
}

/// Verify the prepared budget and the published migration revision before the
/// service starts. Both files are deploy-written, private and single-link.
pub fn check_rust_start(
    expected_release_sha: &str,
    budget_file: &Path,
    expected_revision: &str,
    revision_file: &Path,
) -> Result<(), &'static str> {
    check_release_sha(expected_release_sha)?;
    if expected_revision.is_empty() || expected_revision.len() > MAX_STATE_FIELD_CHARS {
        return Err("expected revision is not a bounded literal");
    }
    let budget: Value = serde_json::from_slice(&read_private(
        budget_file,
        MAX_BUDGET_BYTES,
        "budget is missing",
    )?)
    .map_err(|_| "budget is not JSON")?;
    let budget = budget.as_object().ok_or("budget must be a JSON object")?;
    if budget.get("version") != Some(&Value::from(1)) {
        return Err("budget version differs");
    }
    if budget.get("release_sha") != Some(&Value::from(expected_release_sha)) {
        return Err("budget release differs");
    }

    let revision: Value = serde_json::from_slice(&read_private(
        revision_file,
        MAX_REVISION_BYTES,
        "revision is missing",
    )?)
    .map_err(|_| "revision is not JSON")?;
    let revision = revision
        .as_object()
        .ok_or("revision must be a JSON object")?;
    if revision.len() != 2
        || revision.get("release_sha") != Some(&Value::from(expected_release_sha))
        || revision.get("revision") != Some(&Value::from(expected_revision))
    {
        return Err("revision identity differs");
    }
    Ok(())
}

/// Accept the readiness corpus only when it carries exactly the four deployed
/// keys with the expected values. Python's deploy consumer allowed extra keys
/// and the local monitor did not; the deploy path adopts the stricter rule.
pub fn check_health(
    input: &mut dyn Read,
    environment: &str,
    release_sha: &str,
) -> Result<(), &'static str> {
    check_environment(environment)?;
    check_release_sha(release_sha)?;
    let mut body = Vec::new();
    input
        .take(MAX_HEALTH_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|_| "readiness response is unreadable")?;
    if body.len() as u64 > MAX_HEALTH_BYTES {
        return Err("readiness response is too large");
    }
    let document: Value =
        serde_json::from_slice(&body).map_err(|_| "readiness response is not valid JSON")?;
    let document = document
        .as_object()
        .ok_or("readiness response must be a JSON object")?;
    if document.len() != 4 {
        return Err("readiness response must carry exactly four keys");
    }
    for (field, expected) in [
        ("status", "ok"),
        ("db", "ok"),
        ("environment", environment),
        ("release_sha", release_sha),
    ] {
        if document.get(field) != Some(&Value::from(expected)) {
            return Err("readiness response has an unexpected value");
        }
    }
    Ok(())
}

/// `guard rust-start` entry point. Every option is required; a missing option
/// is a usage refusal, never an implicit default.
pub fn rust_start(
    release_sha: Option<&str>,
    budget_file: Option<&Path>,
    expected_revision: Option<&str>,
    revision_file: Option<&Path>,
) -> Result<(), &'static str> {
    check_rust_start(
        release_sha.ok_or("--expected-release-sha is required")?,
        budget_file.ok_or("--budget-file is required")?,
        expected_revision.ok_or("--expected-revision is required")?,
        revision_file.ok_or("--revision-file is required")?,
    )
}

/// `guard health` entry point: the readiness body arrives on stdin.
pub fn health(
    environment: Option<&str>,
    release_sha: Option<&str>,
    input: &mut dyn Read,
) -> Result<(), &'static str> {
    check_health(
        input,
        environment.ok_or("--environment is required")?,
        release_sha.ok_or("--release-sha is required")?,
    )
}

/// Read a bounded private regular file without following a symlink. The file
/// must not be group- or other-accessible and must have one link, so a second
/// path cannot change what the guard checked.
fn read_private(
    path: &Path,
    max_bytes: u64,
    missing: &'static str,
) -> Result<Vec<u8>, &'static str> {
    let before = std::fs::symlink_metadata(path).map_err(|_| missing)?;
    if !before.file_type().is_file() {
        return Err("identity file must be a regular file");
    }
    if before.nlink() != 1 {
        return Err("identity file must not be linked");
    }
    if before.mode() & 0o077 != 0 {
        return Err("identity file must be private");
    }
    if before.size() > max_bytes {
        return Err("identity file is too large");
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(|_| "identity file is unreadable")?;
    let opened = file.metadata().map_err(|_| "identity file is unreadable")?;
    let current = std::fs::symlink_metadata(path).map_err(|_| "identity file changed")?;
    if (opened.dev(), opened.ino()) != (before.dev(), before.ino())
        || (current.dev(), current.ino()) != (opened.dev(), opened.ino())
        || current.mtime() != opened.mtime()
        || current.mtime_nsec() != opened.mtime_nsec()
    {
        return Err("identity file changed during validation");
    }
    let mut data = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut data)
        .map_err(|_| "identity file is unreadable")?;
    if data.len() as u64 > max_bytes || data.len() as u64 != opened.size() {
        return Err("identity file changed during validation");
    }
    Ok(data)
}
