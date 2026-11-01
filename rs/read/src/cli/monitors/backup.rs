//! `monitor backup-verify` (stub: behaviour lands with the tests).
use super::status::{Admission, CheckResult, Fault, Status};
use crate::cli::Exit;
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Duration;

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
        Self {
            backup_status: PathBuf::new(),
            backup_directory: PathBuf::new(),
            attempt_directory: PathBuf::new(),
            output: PathBuf::new(),
            max_age_seconds: 0,
            future_tolerance_seconds: 0,
            max_timestamp_skew_seconds: 0,
            max_metadata_bytes: 0,
            max_attempt_records: 0,
            max_attempt_bytes: 0,
            max_artifacts: 0,
            max_artifact_bytes: 0,
            max_uncompressed_bytes: 0,
            total_timeout: Duration::ZERO,
        }
    }
}

pub fn verify_backups(_config: &BackupConfig, _now: DateTime<Utc>) -> Vec<CheckResult> {
    Vec::new()
}

pub fn collect(_config: &BackupConfig, _now: DateTime<Utc>) -> Result<Status, Fault> {
    Err(Fault)
}

pub fn run(_config: &BackupConfig, _admission: &dyn Admission) -> Exit {
    Exit::Ok
}

pub fn run_default() -> Exit {
    Exit::Ok
}
