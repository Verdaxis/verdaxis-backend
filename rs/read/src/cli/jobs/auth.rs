//! `verdaxis-read job auth-maintenance`.
use crate::business::approval_mail::{ApprovalMailBatchSize, FrozenApprovalPayload};
use crate::cli::mail::approval::ApprovalCounts;
use crate::cli::{Exit, JobArgs};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::future::Future;

pub const DEFAULT_BATCH_SIZE: u32 = 1000;
pub const MAX_BATCH_SIZE: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupBatchSize(u32);

impl CleanupBatchSize {
    pub fn new(size: u32) -> Result<Self, Exit> {
        Ok(Self(size))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for CleanupBatchSize {
    fn default() -> Self {
        Self(DEFAULT_BATCH_SIZE)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupCounts {
    pub refresh_sessions_deleted: u64,
    pub pending_registrations_deleted: u64,
    pub password_reset_hashes_cleared: u64,
    pub email_verification_hashes_cleared: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthMaintenanceReport {
    pub cleanup: CleanupCounts,
    pub approval: ApprovalCounts,
}

impl AuthMaintenanceReport {
    pub fn counts_json(&self) -> Value {
        Value::Null
    }
}

pub async fn clean_expired_auth_state(
    _pool: &PgPool,
    _cutoff: DateTime<Utc>,
    _batch: CleanupBatchSize,
) -> Result<CleanupCounts, sqlx::Error> {
    Ok(CleanupCounts::default())
}

pub async fn run<C, S, F>(
    _pool: &PgPool,
    _batch: CleanupBatchSize,
    _email_batch: ApprovalMailBatchSize,
    _clock: C,
    _send: S,
) -> Result<AuthMaintenanceReport, Exit>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    Ok(AuthMaintenanceReport::default())
}

pub async fn execute(_args: &JobArgs) -> Exit {
    Exit::Precondition
}
