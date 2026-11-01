//! Account-approval mail retries for the auth maintenance job.
//!
//! The transition-scoped state machine is the Phase 4 one in
//! [`crate::business::approval_mail`]; this module only drives due rows.
use super::{Delivery, MailLog};
use crate::business::approval_mail::{
    ApprovalMailBatchSize, ApprovalMailClaim, FrozenApprovalPayload, MailError,
    claim_approval_mail, due_approval_mail, finalize_approval_mail,
};
use crate::crud::referrals::invite::MailClient;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use std::future::Future;
use std::time::Duration as StdDuration;
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

/// Bounds one provider call, including an injected test sender. The Phase 2
/// client has the same 10-second request timeout.
const PROVIDER_DEADLINE: StdDuration = StdDuration::from_secs(10);

/// Deliver one due transition (`app/services/account_approval_email.py:41-103`).
///
/// 1. Lock the user row only while the transition is still due, then claim it
///    (five-minute lease) or discard it, and commit.
/// 2. Call the provider with no transaction open and no row lock held.
/// 3. Lock the row again; finalize only if this exact claim is still there.
///    A rejection, re-approval or expired-lease retry during the call leaves
///    the newer state alone.
pub async fn deliver_due<C, S, F>(
    pool: &PgPool,
    user_id: Uuid,
    transition_id: Uuid,
    clock: &C,
    send: &S,
) -> Result<ApprovalOutcome, MailError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    let claim = {
        let mut tx = pool.begin_with("BEGIN READ WRITE").await?;
        let now = clock();
        if !lock_due_transition(&mut tx, user_id, transition_id, now).await? {
            tx.rollback().await?;
            return Ok(ApprovalOutcome::Skipped);
        }
        // The row is locked and due, so the Phase 4 claim either claims it or
        // clears an ineligible account's transition.
        let claim = claim_approval_mail(&mut tx, user_id, transition_id, now).await?;
        tx.commit().await?;
        claim
    };
    let Some(claim) = claim else {
        return Ok(ApprovalOutcome::Discarded);
    };

    let accepted = tokio::time::timeout(
        PROVIDER_DEADLINE,
        send(claim.marker.payload.clone(), idempotency_key(transition_id)),
    )
    .await
    .unwrap_or(false);

    let mut tx = pool.begin_with("BEGIN READ WRITE").await?;
    if !lock_claim(&mut tx, &claim).await? {
        tx.rollback().await?;
        return Ok(ApprovalOutcome::Skipped);
    }
    finalize_approval_mail(&mut tx, &claim, accepted, clock()).await?;
    tx.commit().await?;
    Ok(if accepted {
        ApprovalOutcome::Sent
    } else {
        ApprovalOutcome::Deferred
    })
}

/// Lock the user row if `transition_id` is its pending transition and is due.
async fn lock_due_transition(
    conn: &mut PgConnection,
    user_id: Uuid,
    transition_id: Uuid,
    now: DateTime<Utc>,
) -> Result<bool, MailError> {
    let locked: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 AND pending_approval_email_transition_id=$2 \
         AND pending_approval_email_retry_at <= $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(transition_id)
    .bind(now)
    .fetch_optional(conn)
    .await?;
    Ok(locked.is_some())
}

/// Lock the user row if it still carries exactly this claim's lease.
async fn lock_claim(conn: &mut PgConnection, claim: &ApprovalMailClaim) -> Result<bool, MailError> {
    let locked: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 AND pending_approval_email_transition_id=$2 \
         AND pending_approval_email_retry_at=$3 FOR UPDATE",
    )
    .bind(claim.marker.user_id)
    .bind(claim.marker.transition_id)
    .bind(claim.claim_until)
    .fetch_optional(conn)
    .await?;
    Ok(locked.is_some())
}

/// Retry a bounded, fair snapshot of due transitions, oldest first
/// (`app/services/account_approval_email.py:106-149`). The scan holds no
/// transaction while the rows are delivered. A database failure stops the
/// run; a claimed row then waits for its lease to expire.
pub async fn retry_due<C, S, F>(
    pool: &PgPool,
    batch: ApprovalMailBatchSize,
    clock: C,
    send: S,
) -> Result<ApprovalCounts, MailError>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    let due = {
        let mut conn = pool.acquire().await?;
        due_approval_mail(&mut conn, clock(), batch).await?
    };
    let mut counts = ApprovalCounts {
        attempted: due.len() as u64,
        ..ApprovalCounts::default()
    };
    for row in due {
        match deliver_due(pool, row.user_id, row.transition_id, &clock, &send).await? {
            ApprovalOutcome::Sent => counts.completed += 1,
            ApprovalOutcome::Deferred => counts.deferred += 1,
            ApprovalOutcome::Discarded => counts.discarded += 1,
            ApprovalOutcome::Skipped => counts.skipped += 1,
        }
    }
    Ok(counts)
}

/// Production sender: the Phase 2 mail client with the transition key.
pub struct ApprovalMail {
    client: MailClient,
}

impl ApprovalMail {
    pub fn new(client: MailClient) -> Self {
        Self { client }
    }

    /// One provider call. The log names the recipient by digest only.
    pub async fn send(&self, payload: FrozenApprovalPayload, key: String) -> bool {
        let [recipient] = &payload.to;
        let accepted = match serde_json::to_value(&payload) {
            Ok(body) => self.client.send_with_idempotency(&body, Some(&key)).await,
            Err(_) => false,
        };
        let delivery = if accepted {
            Delivery::Sent
        } else {
            Delivery::Refused
        };
        MailLog::new(recipient, delivery).record();
        accepted
    }
}
