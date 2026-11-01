//! KYC grace notice mail: a permanent, at-most-once claim per grace row.
//!
//! This is the interface the tests pin. The behaviour lands in the next commit.
use super::super::Exit;
use super::super::context::RunContext;
use crate::crud::referrals::invite::MailClient;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// Rows selected per page.
pub const PAGE_SIZE: u32 = 200;
/// Rows one invocation may attempt in total.
pub const MAX_PER_INVOCATION: u32 = 1000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KycCounts {
    pub sent: u64,
    pub failed: u64,
    pub skipped: u64,
}

#[derive(Debug)]
pub struct ClaimedNotice {
    pub grace_id: Uuid,
    pub email: String,
    pub expires_at: DateTime<Utc>,
}

pub fn idempotency_key(_grace_id: Uuid) -> String {
    String::new()
}

pub fn render_payload(
    _sender: &str,
    _frontend: &str,
    _to: &str,
    _expires_at: DateTime<Utc>,
) -> Value {
    json!({})
}

pub async fn eligible_page(
    _conn: &mut PgConnection,
    _limit: u32,
) -> Result<Vec<Uuid>, sqlx::Error> {
    Ok(Vec::new())
}

pub async fn claim(
    _conn: &mut PgConnection,
    _grace_id: Uuid,
) -> Result<Option<ClaimedNotice>, sqlx::Error> {
    Ok(None)
}

pub struct KycNotice {
    _mail: MailClient,
}

impl KycNotice {
    pub fn new(mail: MailClient, _sender: String, _frontend: String, _database: String) -> Self {
        Self { _mail: mail }
    }

    pub async fn send_page(&self, _ctx: &RunContext, _limit: u32) -> Result<KycCounts, Exit> {
        Ok(KycCounts::default())
    }
}
