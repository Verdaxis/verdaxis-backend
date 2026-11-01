//! `verdaxis-read job auth-maintenance`: bounded cleanup of expired
//! authentication state, then the approval-mail retry queue.
//!
//! Python source: `app/cli/auth_maintenance.py` (cleanup commits before the
//! mail retries, `:19-28`) and `app/services/auth_maintenance.py` (the cleanup
//! categories, `:27-136`).
//!
//! Cleanup differs from Python in one deliberate way. Python selects candidate
//! ids and then updates or deletes `WHERE id IN (...)`, so a token renewed
//! after the selection can be cleared by the update. Here each statement locks
//! its candidates with `SKIP LOCKED` in id order and changes only those locked
//! rows; for the token digests it also requires the exact digest and expiry it
//! read under the lock. The job therefore never waits on a row another request
//! holds, never clears a renewed token, and cannot join a lock cycle with the
//! writers that lock users in id order.
use crate::auth::store::sweep_expired_reset_tokens;
use crate::business::approval_mail::{ApprovalMailBatchSize, FrozenApprovalPayload};
use crate::cli::context::{self, Purpose};
use crate::cli::gate::require_apply;
use crate::cli::mail::approval::{ApprovalCounts, ApprovalMail, retry_due};
use crate::cli::{Exit, JobArgs, JobKind, retry_bounded};
use crate::crud::referrals::invite::MailClient;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::future::Future;

/// Rows per category per run (`app/services/auth_maintenance.py:13`).
pub const DEFAULT_BATCH_SIZE: u32 = 1000;
/// Largest batch an operator may ask for (`:34-35`).
pub const MAX_BATCH_SIZE: u32 = 10_000;
/// A revoked family stays this long so token replay is still detected (`:14`).
const REVOKED_FAMILY_RETENTION: Duration = Duration::hours(24);

/// Expired, or revoked for a day with no live member left in the family.
const DELETE_REFRESH_SESSIONS: &str = "\
    WITH candidates AS ( \
        SELECT session.id FROM refresh_sessions AS session \
        WHERE session.expires_at <= $1 \
           OR (session.revoked \
               AND session.created_at <= $2 \
               AND NOT EXISTS ( \
                   SELECT 1 FROM refresh_sessions AS successor \
                   WHERE successor.family_id = session.family_id \
                     AND NOT successor.revoked \
                     AND successor.expires_at > $1)) \
        ORDER BY session.id \
        LIMIT $3 \
        FOR UPDATE OF session SKIP LOCKED) \
    DELETE FROM refresh_sessions AS session \
    USING candidates WHERE session.id = candidates.id";

/// Expired or already used.
const DELETE_PENDING_REGISTRATIONS: &str = "\
    WITH candidates AS ( \
        SELECT id FROM pending_registrations \
        WHERE expires_at <= $1 OR used_at IS NOT NULL \
        ORDER BY id \
        LIMIT $2 \
        FOR UPDATE SKIP LOCKED) \
    DELETE FROM pending_registrations AS registration \
    USING candidates WHERE registration.id = candidates.id";

/// `FOR NO KEY UPDATE` is the lock the update itself takes, so concurrent
/// inserts that reference the user (key-share locks) are not blocked.
const CLEAR_PASSWORD_RESET_HASHES: &str = "\
    WITH candidates AS ( \
        SELECT id, password_reset_token_hash AS token_hash, \
               password_reset_expires AS token_expires \
        FROM users \
        WHERE password_reset_token_hash IS NOT NULL \
          AND (password_reset_expires IS NULL OR password_reset_expires <= $1) \
        ORDER BY id \
        LIMIT $2 \
        FOR NO KEY UPDATE SKIP LOCKED) \
    UPDATE users SET password_reset_token_hash = NULL, password_reset_expires = NULL \
    FROM candidates \
    WHERE users.id = candidates.id \
      AND users.password_reset_token_hash = candidates.token_hash \
      AND users.password_reset_expires IS NOT DISTINCT FROM candidates.token_expires";

const CLEAR_EMAIL_VERIFICATION_HASHES: &str = "\
    WITH candidates AS ( \
        SELECT id, email_verification_token_hash AS token_hash, \
               email_verification_token_expires_at AS token_expires \
        FROM users \
        WHERE email_verification_token_hash IS NOT NULL \
          AND (email_verification_token_expires_at IS NULL \
               OR email_verification_token_expires_at <= $1) \
        ORDER BY id \
        LIMIT $2 \
        FOR NO KEY UPDATE SKIP LOCKED) \
    UPDATE users SET email_verification_token_hash = NULL, \
                     email_verification_token_expires_at = NULL \
    FROM candidates \
    WHERE users.id = candidates.id \
      AND users.email_verification_token_hash = candidates.token_hash \
      AND users.email_verification_token_expires_at \
          IS NOT DISTINCT FROM candidates.token_expires";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupBatchSize(u32);

impl CleanupBatchSize {
    pub fn new(size: u32) -> Result<Self, Exit> {
        if (1..=MAX_BATCH_SIZE).contains(&size) {
            Ok(Self(size))
        } else {
            Err(Exit::Usage)
        }
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

/// Rows changed per category. Python's fifth category, the legacy plaintext
/// `users.email_verification_token` column, was dropped by the
/// `sec_20260720_boundaries` migration (`:80`) and is not ported.
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
    /// Expired rows removed from the Rust-owned `password_reset_tokens`.
    pub reset_tokens_deleted: u64,
    pub approval: ApprovalCounts,
}

impl AuthMaintenanceReport {
    pub fn counts_json(&self) -> Value {
        let mut counts = serde_json::json!({
            "refresh_sessions_deleted": self.cleanup.refresh_sessions_deleted,
            "pending_registrations_deleted": self.cleanup.pending_registrations_deleted,
            "password_reset_hashes_cleared": self.cleanup.password_reset_hashes_cleared,
            "email_verification_hashes_cleared": self.cleanup.email_verification_hashes_cleared,
            "password_reset_tokens_deleted": self.reset_tokens_deleted,
        });
        if let (Value::Object(all), Value::Object(approval)) = (&mut counts, self.approval.json()) {
            all.extend(approval);
        }
        counts
    }
}

/// Remove at most `batch` rows per category in one transaction.
pub async fn clean_expired_auth_state(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    batch: CleanupBatchSize,
) -> Result<CleanupCounts, sqlx::Error> {
    let limit = i64::from(batch.get());
    let retention_cutoff = cutoff - REVOKED_FAMILY_RETENTION;
    let mut tx = pool.begin_with("BEGIN READ WRITE").await?;
    let refresh_sessions_deleted = sqlx::query(DELETE_REFRESH_SESSIONS)
        .bind(cutoff)
        .bind(retention_cutoff)
        .bind(limit)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let pending_registrations_deleted = sqlx::query(DELETE_PENDING_REGISTRATIONS)
        .bind(cutoff)
        .bind(limit)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let password_reset_hashes_cleared = sqlx::query(CLEAR_PASSWORD_RESET_HASHES)
        .bind(cutoff)
        .bind(limit)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let email_verification_hashes_cleared = sqlx::query(CLEAR_EMAIL_VERIFICATION_HASHES)
        .bind(cutoff)
        .bind(limit)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(CleanupCounts {
        refresh_sessions_deleted,
        pending_registrations_deleted,
        password_reset_hashes_cleared,
        email_verification_hashes_cleared,
    })
}

/// Commit the cleanup, sweep expired reset tokens, then work the
/// approval-mail queue.
///
/// The cleanup and sweep transactions are idempotent, so they run under the
/// bounded job retry. Mail delivery is never retried here: a claimed row keeps its lease.
pub async fn run<C, S, F>(
    pool: &PgPool,
    batch: CleanupBatchSize,
    email_batch: ApprovalMailBatchSize,
    clock: C,
    send: S,
) -> Result<AuthMaintenanceReport, Exit>
where
    C: Fn() -> DateTime<Utc>,
    S: Fn(FrozenApprovalPayload, String) -> F,
    F: Future<Output = bool>,
{
    let cleanup = retry_bounded(|| clean_expired_auth_state(pool, clock(), batch))
        .await
        .map_err(|_| Exit::Dependency)?;
    // Rust-owned reset tokens (no Python counterpart). The store function
    // runs its own bounded `SKIP LOCKED` transaction after the cleanup commit.
    let reset_tokens_deleted = retry_bounded(|| sweep_expired_reset_tokens(pool, clock()))
        .await
        .map_err(|_| Exit::Dependency)?;
    let approval = retry_due(pool, email_batch, clock, send)
        .await
        .map_err(|_| Exit::Dependency)?;
    Ok(AuthMaintenanceReport {
        cleanup,
        reset_tokens_deleted,
        approval,
    })
}

/// The command entry point. It writes, so `--apply` is required; the
/// purpose-bound context then checks identity, target and deploy state before
/// any pool is opened.
pub async fn execute(args: &JobArgs) -> Exit {
    if let Err(exit) = require_apply(args.apply) {
        report("usage_refused", None);
        return exit;
    }
    let ctx = match context::open(
        Purpose::Job(JobKind::AuthMaintenance),
        args.identity.environment,
        &args.identity.release_sha,
    )
    .await
    {
        Ok(ctx) => ctx,
        Err(exit) => {
            report("context_refused", None);
            return exit;
        }
    };
    // The provider key is the same setting the Python job reads; without it
    // every send is refused and the transition is deferred one hour.
    let client = match MailClient::production(std::env::var("RESEND_API_KEY").ok()) {
        Ok(client) => client,
        Err(_) => {
            report("mail_unavailable", None);
            return Exit::Dependency;
        }
    };
    let mail = ApprovalMail::new(client);
    let outcome = run(
        &ctx.pool,
        CleanupBatchSize::default(),
        ApprovalMailBatchSize::default(),
        Utc::now,
        |payload, key| mail.send(payload, key),
    )
    .await;
    match outcome {
        Ok(result) => {
            report("completed", Some(result.counts_json()));
            Exit::Ok
        }
        Err(exit) => {
            report("failed", None);
            exit
        }
    }
}

/// The one JSON line every command prints on stderr. Counts only; no address,
/// token or database detail.
fn report(outcome: &str, counts: Option<Value>) {
    eprintln!(
        "{}",
        serde_json::json!({
            "command": "job auth-maintenance",
            "outcome": outcome,
            "counts": counts.unwrap_or_else(|| serde_json::json!({})),
        })
    );
}
