//! `monitor health` (stub: behaviour lands with the tests).
use super::status::{Admission, CheckResult, Fault, Status};
use crate::cli::Exit;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAX_IDENTITY_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub environment: String,
    pub release_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityRefused;

pub fn read_identity(
    _path: &Path,
    _expected_environment: &str,
) -> Result<RuntimeIdentity, IdentityRefused> {
    Err(IdentityRefused)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessRefusal {
    ExpectedIdentity,
    Schema,
    Unhealthy,
    Identity,
}

pub fn validate_readiness_payload(
    _payload: &Value,
    _expected_environment: &str,
    _expected_release_sha: &str,
) -> Result<(), ReadinessRefusal> {
    Ok(())
}

pub fn check_readiness(
    name: &str,
    _url: &str,
    _expected_environment: &str,
    _expected_release_sha: &str,
    _timeout: Duration,
    _max_body_bytes: usize,
) -> CheckResult {
    CheckResult::new(name, super::status::Category::Healthy, "ok", Instant::now())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    pub free: u64,
}

pub fn system_disk_usage(_path: &Path) -> io::Result<DiskUsage> {
    Ok(DiskUsage { total: 1, free: 1 })
}

pub fn check_filesystem(
    name: &str,
    _path: &Path,
    _warning_free_percent: f64,
    _critical_free_percent: f64,
    _disk_usage: &dyn Fn(&Path) -> io::Result<DiskUsage>,
) -> CheckResult {
    CheckResult::new(name, super::status::Category::Healthy, "ok", Instant::now())
}

pub fn check_directory_size(
    name: &str,
    _path: &Path,
    _max_bytes: u64,
    _deadline: Instant,
    _max_entries: usize,
) -> CheckResult {
    CheckResult::new(name, super::status::Category::Healthy, "ok", Instant::now())
}

pub type Check<'a> = Box<dyn FnOnce(Duration) -> CheckResult + 'a>;

pub fn execute_checks(
    _checks: Vec<Check<'_>>,
    _total: Duration,
    _clock: &dyn Fn() -> Duration,
) -> Vec<CheckResult> {
    Vec::new()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: &'static str,
    pub url: String,
    pub expected_environment: &'static str,
    pub identity_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HealthConfig {
    pub status_file: PathBuf,
    pub endpoints: Vec<Endpoint>,
    pub filesystem: PathBuf,
    pub warning_free_percent: f64,
    pub critical_free_percent: f64,
    pub analytics_directory: Option<PathBuf>,
    pub analytics_max_bytes: u64,
    pub analytics_max_entries: usize,
    pub request_timeout: Duration,
    pub total_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            status_file: PathBuf::new(),
            endpoints: Vec::new(),
            filesystem: PathBuf::new(),
            warning_free_percent: 0.0,
            critical_free_percent: 0.0,
            analytics_directory: None,
            analytics_max_bytes: 0,
            analytics_max_entries: 0,
            request_timeout: Duration::ZERO,
            total_timeout: Duration::ZERO,
            max_body_bytes: 0,
        }
    }
}

pub fn collect(_config: &HealthConfig, _now: DateTime<Utc>) -> Result<Status, Fault> {
    Err(Fault)
}

pub fn run(_config: &HealthConfig, _admission: &dyn Admission) -> Exit {
    Exit::Ok
}

pub fn run_default() -> Exit {
    Exit::Ok
}
