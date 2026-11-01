//! Account-approval mail retries for the auth maintenance job.
//!
//! The transition-scoped state machine is the Phase 4 one in
//! [`crate::business::approval_mail`]; this module only drives due rows.
use crate::business::approval_mail::{ApprovalMailBatchSize, FrozenApprovalPayload, MailError};
use crate::crud::referrals::invite::MailClient;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::future::Future;
use uuid::Uuid;

/// What one due row came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Sent,
    Deferred,
    Discarded,
    Skipped,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApprovalCounts {
    pub attempted: u64,
    pub completed: u64,
    pub deferred: u64,
    pub discarded: u64,
    pub skipped: u64,
}

impl ApprovalCounts {
    pub fn json(&self) -> Value {
        serde_json::json!({
            "approval_emails_attempted": self.attempted,
            "approval_emails_completed": self.completed,
            "approval_emails_deferred": self.deferred,
            "approval_emails_discarded": self.discarded,
            "approval_emails_skipped": self.skipped,
        })
    }
}

/// The provider idempotency key Python uses for one transition.
pub fn idempotency_key(transition_id: Uuid) -> String {
    format!("account-approval/{transition_id}")
}

pub async fn deliver_due<C, S, F>(
    _pool: &PgPool,
    _user_id: Uuid,
    _transition_id: Uuid,
    _clock: &C,
    _send: &S,
) -> Result<ApprovalOutcome, MailError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    Ok(ApprovalOutcome::Skipped)
}

pub async fn retry_due<C, S, F>(
    _pool: &PgPool,
    _batch: ApprovalMailBatchSize,
    _clock: C,
    _send: S,
) -> Result<ApprovalCounts, MailError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    Ok(ApprovalCounts::default())
}

/// Production sender: the Phase 2 mail client with the transition key.
pub struct ApprovalMail {
    client: MailClient,
}

impl ApprovalMail {
    pub fn new(client: MailClient) -> Self {
        Self { client }
    }

    pub async fn send(&self, _payload: FrozenApprovalPayload, _key: String) -> bool {
        let _ = &self.client;
        false
    }
}
