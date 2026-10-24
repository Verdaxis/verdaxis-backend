//! Start guards. Implementation lands in the next commit.

use std::io::Read;
use std::path::Path;

pub const MAX_BUDGET_BYTES: u64 = 64 * 1024;
pub const MAX_REVISION_BYTES: u64 = 8 * 1024;
pub const MAX_HEALTH_BYTES: u64 = 64 * 1024;

pub fn check_environment(_environment: &str) -> Result<(), &'static str> {
    Ok(())
}

pub fn check_release_sha(_release_sha: &str) -> Result<(), &'static str> {
    Ok(())
}

pub fn check_rust_start(
    _expected_release_sha: &str,
    _budget_file: &Path,
    _expected_revision: &str,
    _revision_file: &Path,
) -> Result<(), &'static str> {
    Ok(())
}

pub fn check_health(
    _input: &mut dyn Read,
    _environment: &str,
    _release_sha: &str,
) -> Result<(), &'static str> {
    Ok(())
}
