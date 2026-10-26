//! Transition-scoped approval mail state. Borrowed helpers never commit.
use std::{fmt, future::Future, time::Duration as StdDuration};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct FrozenApprovalPayload {
    pub from: String,
    pub to: [String; 1],
    pub subject: String,
    pub html: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

impl fmt::Debug for FrozenApprovalPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrozenApprovalPayload(REDACTED)")
    }
}

#[derive(Clone, Debug)]
pub struct ApprovalMailMarker {
    pub user_id: Uuid,
    pub transition_id: Uuid,
    pub payload: FrozenApprovalPayload,
    pub retry_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct ApprovalMailClaim {
    pub marker: ApprovalMailMarker,
    pub claim_until: DateTime<Utc>,
}

/// Database errors retain their SQLSTATE for callers, but never their message.
pub enum MailError {
    Database(sqlx::Error),
    ClockRange,
    InvalidBatchSize,
}

impl fmt::Debug for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Database(_) => "MailError::Database",
            Self::ClockRange => "MailError::ClockRange",
            Self::InvalidBatchSize => "MailError::InvalidBatchSize",
        })
    }
}

impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
impl std::error::Error for MailError {}
impl From<sqlx::Error> for MailError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ApprovalMailBatchSize(u32);
impl ApprovalMailBatchSize {
    pub fn new(size: u32) -> Result<Self, MailError> {
        if (1..=100).contains(&size) {
            Ok(Self(size))
        } else {
            Err(MailError::InvalidBatchSize)
        }
    }
    pub fn get(self) -> u32 {
        self.0
    }
}
impl Default for ApprovalMailBatchSize {
    fn default() -> Self {
        Self(20)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DueApprovalMail {
    pub user_id: Uuid,
    pub transition_id: Uuid,
}

pub async fn freeze_approval_mail(
    conn: &mut PgConnection,
    marker: &ApprovalMailMarker,
) -> Result<(), MailError> {
    // Caller holds the target user lock and appends this transition atomically.
    sqlx::query(
        "UPDATE users SET pending_approval_email_transition_id=$2, \
         pending_approval_email_payload=$3, pending_approval_email_retry_at=$4 WHERE id=$1",
    )
    .bind(marker.user_id)
    .bind(marker.transition_id)
    .bind(sqlx::types::Json(&marker.payload))
    .bind(marker.retry_at)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn due_approval_mail(
    conn: &mut PgConnection,
    now: DateTime<Utc>,
    batch: ApprovalMailBatchSize,
) -> Result<Vec<DueApprovalMail>, MailError> {
    Ok(sqlx::query_as(
        "SELECT id AS user_id,pending_approval_email_transition_id AS transition_id \
         FROM users WHERE pending_approval_email_transition_id IS NOT NULL \
         AND pending_approval_email_retry_at <= $1 \
         ORDER BY pending_approval_email_retry_at,id LIMIT $2",
    )
    .bind(now)
    .bind(i64::from(batch.get()))
    .fetch_all(conn)
    .await?)
}

pub async fn claim_approval_mail(
    conn: &mut PgConnection,
    user_id: Uuid,
    transition_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<ApprovalMailClaim>, MailError> {
    #[derive(sqlx::FromRow)]
    struct Pending {
        status: String,
        role: String,
        email_verified: bool,
        pending_approval_email_payload: Option<serde_json::Value>,
    }
    let pending: Option<Pending> = sqlx::query_as(
        "SELECT status::text,role::text,email_verified,pending_approval_email_payload \
         FROM users WHERE id=$1 AND pending_approval_email_transition_id=$2 \
         AND pending_approval_email_retry_at <= $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(transition_id)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(pending) = pending else {
        return Ok(None);
    };
    let payload = pending
        .pending_approval_email_payload
        .and_then(|value| serde_json::from_value::<FrozenApprovalPayload>(value).ok());
    if pending.status != "APPROVED"
        || pending.role == "ADMIN"
        || !pending.email_verified
        || payload.is_none()
    {
        sqlx::query(
            "UPDATE users SET pending_approval_email_transition_id=NULL, \
             pending_approval_email_payload=NULL,pending_approval_email_retry_at=NULL WHERE id=$1",
        )
        .bind(user_id)
        .execute(conn)
        .await?;
        return Ok(None);
    }
    // Store and return PostgreSQL's microsecond precision for exact finalization.
    let claim_until = now
        .checked_add_signed(Duration::minutes(5))
        .ok_or(MailError::ClockRange)?;
    let (claim_until,): (DateTime<Utc>,) = sqlx::query_as(
        "UPDATE users SET pending_approval_email_retry_at=$2 WHERE id=$1 \
         RETURNING pending_approval_email_retry_at",
    )
    .bind(user_id)
    .bind(claim_until)
    .fetch_one(conn)
    .await?;
    Ok(Some(ApprovalMailClaim {
        marker: ApprovalMailMarker {
            user_id,
            transition_id,
            payload: payload.expect("validated frozen payload"),
            retry_at: now,
        },
        claim_until,
    }))
}

pub async fn finalize_approval_mail(
    conn: &mut PgConnection,
    claim: &ApprovalMailClaim,
    accepted: bool,
    now: DateTime<Utc>,
) -> Result<(), MailError> {
    let retry_at = if accepted {
        None
    } else {
        Some(
            now.checked_add_signed(Duration::hours(1))
                .ok_or(MailError::ClockRange)?,
        )
    };
    sqlx::query(
        "UPDATE users SET \
         pending_approval_email_transition_id=CASE WHEN $4 THEN NULL ELSE pending_approval_email_transition_id END, \
         pending_approval_email_payload=CASE WHEN $4 THEN NULL ELSE pending_approval_email_payload END, \
         pending_approval_email_retry_at=$5 \
         WHERE id=$1 AND pending_approval_email_transition_id=$2 AND pending_approval_email_retry_at=$3",
    )
    .bind(claim.marker.user_id)
    .bind(claim.marker.transition_id)
    .bind(claim.claim_until)
    .bind(accepted)
    .bind(retry_at)
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApprovalDelivery {
    Skipped,
    Sent,
    Deferred,
}

/// Post-commit owner. `send` is the existing mail transport adapter, not a queue.
/// One attempt only; cancellation/uncertain delivery leaves the committed lease.
/// Callers must not retry this owner as an approval transaction.
pub async fn deliver_account_approval_mail<C, S, F>(
    pool: &PgPool,
    user_id: Uuid,
    transition_id: Uuid,
    clock: C,
    send: S,
) -> Result<ApprovalDelivery, MailError>
where
    C: Fn() -> DateTime<Utc>,
    S: FnOnce(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    let claim = {
        let mut tx = pool.begin().await?;
        let claim = claim_approval_mail(&mut tx, user_id, transition_id, clock()).await?;
        tx.commit().await?;
        claim
    };
    let Some(claim) = claim else {
        return Ok(ApprovalDelivery::Skipped);
    };
    let key = format!("account-approval/{transition_id}");
    let accepted = tokio::time::timeout(
        StdDuration::from_secs(10),
        send(claim.marker.payload.clone(), key),
    )
    .await
    .unwrap_or(false);
    let mut tx = pool.begin().await?;
    finalize_approval_mail(&mut tx, &claim, accepted, clock()).await?;
    tx.commit().await?;
    Ok(if accepted {
        ApprovalDelivery::Sent
    } else {
        ApprovalDelivery::Deferred
    })
}
