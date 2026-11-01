//! Shared status document for the local monitors (stub: behaviour lands with the tests).
use crate::cli::Exit;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const MAX_STATUS_BYTES: u64 = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Healthy,
    Warning,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub category: Category,
    pub code: String,
    pub duration_ms: u64,
}

impl CheckResult {
    pub fn new(name: &str, category: Category, code: &str, _started: Instant) -> Self {
        Self {
            name: name.to_owned(),
            category,
            code: code.to_owned(),
            duration_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStatus {
    pub category: Category,
    pub code: String,
    pub duration_ms: u64,
    pub last_success_at: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub category: Category,
    pub checks: Vec<CheckStatus>,
    pub generated_at: String,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    Absent,
    Valid(Status),
    Malformed,
}

pub trait Admission {
    fn admit(&self, status: &Status) -> Result<(), Exit>;
}

pub struct AdmitAll;

impl Admission for AdmitAll {
    fn admit(&self, _status: &Status) -> Result<(), Exit> {
        Ok(())
    }
}

pub fn utc_text(_at: DateTime<Utc>) -> String {
    String::new()
}

pub fn parse_utc_timestamp(_text: &str) -> Option<DateTime<Utc>> {
    None
}

pub fn parse_strict_json(_bytes: &[u8]) -> Option<Value> {
    None
}

pub fn failure_status(_code: &str, at: DateTime<Utc>, _name: &str) -> Status {
    Status {
        category: Category::Healthy,
        checks: Vec::new(),
        generated_at: utc_text(at),
        schema_version: 0,
    }
}

pub fn build_status(_checks: Vec<CheckResult>, _previous: &Previous, at: DateTime<Utc>) -> Status {
    failure_status("stub", at, "stub")
}

pub fn valid_monitor_status(_value: &Value) -> bool {
    true
}

pub fn load_monitor_status(_path: &Path, _max_bytes: u64) -> Previous {
    Previous::Absent
}

pub fn quarantine(_path: &Path) -> std::io::Result<Option<PathBuf>> {
    Ok(None)
}

pub fn exit_for(_category: Category) -> Exit {
    Exit::Ok
}

pub fn publish(_path: &Path, _status: &Status, _admission: &dyn Admission) -> Result<(), Exit> {
    Ok(())
}

/// Run one collection; a refusal or a panic becomes the fixed `fatal_error` status.
pub fn guarded<F>(_name: &str, at: DateTime<Utc>, _collect: F) -> Status
where
    F: FnOnce() -> Result<Status, Fault>,
{
    failure_status("stub", at, "stub")
}

/// A collection that could not finish. It carries no message on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault;

pub fn report(_command: &str, _status: &Status) {}
