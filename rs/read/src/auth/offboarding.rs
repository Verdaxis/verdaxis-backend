//! Credential cutoff for an account whose access ends.
//!
//! [`revoke_account_credentials`] runs inside the caller's transaction, after
//! the caller has locked the target user row in its own lock order. It never
//! commits and never takes another lock before the user row: no device
//! advisory lock (session writers take that one *before* the user row), and
//! only the user's own refresh-session and reset-token rows afterwards, which
//! is the token step of the session lock order in [`super::lock`].
//!
//! The cutoff comes from the database clock (`clock_timestamp()`), read only
//! once the user row lock is held. A login or refresh that held the lock first
//! has therefore committed before the clock is read, so every token it issued
//! is at or before the cutoff. [`CUTOFF_SKEW_MARGIN_SECONDS`] moves the cutoff
//! a little further, so a token issued on an application host whose clock runs
//! ahead of the database by less than the margin is cut off too.
//!
//! Both stacks compare a token's issuance time with `password_changed_at` and
//! `authentication_revoked_at`, inclusive at microsecond precision. Both
//! columns move to at least the cutoff and never backwards. The password hash
//! is replaced by a well-formed bcrypt hash of a discarded random secret, which
//! the Rust and Python verifiers parse and reject for every presented password.
//! Every refresh family is revoked, and the reset, invitation, verification and
//! pending approval-mail secrets are cleared. History rows (audit, sent mail,
//! login days) and KYC evidence are left unchanged.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{store, token_bytes::generate_token};

/// Seconds added to the database clock reading. A few seconds covers ordinary
/// clock differences between application hosts and the database host; it is
/// small because an account cannot sign in while the cutoff is in the future.
pub const CUTOFF_SKEW_MARGIN_SECONDS: i64 = 5;

/// bcrypt's minimum cost. The unusable hash protects a discarded 256-bit
/// random secret, so its strength comes from that entropy, not from the cost,
/// and the hash stays cheap to compute while the user row is locked.
const UNUSABLE_HASH_COST: u32 = 4;

/// What one credential cutoff changed. It carries no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRevocation {
    /// The database clock after the user lock plus the skew margin.
    pub cutoff: DateTime<Utc>,
    /// The stored revocation cutoff: the cutoff, or a later stored value.
    pub authentication_revoked_at: DateTime<Utc>,
    /// The stored password cutoff: the cutoff, or a later stored value.
    pub password_changed_at: DateTime<Utc>,
    /// Refresh-session rows that were live and are now revoked.
    pub refresh_sessions_revoked: u64,
    /// Reset-token rows removed.
    pub reset_tokens_removed: u64,
}

/// A well-formed bcrypt hash of a fresh random secret that is never stored or
/// returned. No presented password matches it, and a new value is made for
/// every account, so no two accounts share one.
pub fn unusable_password_hash() -> Result<String, bcrypt::BcryptError> {
    bcrypt::hash(generate_token(), UNUSABLE_HASH_COST)
}

/// End every credential of one user inside the caller's transaction.
///
/// The caller must already hold the user row lock (`FOR UPDATE`) in this
/// transaction. The lock is confirmed without waiting: when another
/// transaction holds the row, PostgreSQL refuses with `55P03` and nothing is
/// written. An unknown user returns [`sqlx::Error::RowNotFound`]. The caller
/// commits or rolls back the whole operation, including after an error.
pub async fn revoke_account_credentials(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<CredentialRevocation, sqlx::Error> {
    confirm_user_lock(connection, user_id).await?;
    let unusable_hash =
        unusable_password_hash().map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    // A separate statement, run only after the lock is confirmed: a clock read
    // inside a locking statement could be evaluated before that lock is granted.
    let cutoff: DateTime<Utc> = sqlx::query_scalar(
        "SELECT clock_timestamp() + make_interval(secs => $1::double precision)",
    )
    .bind(CUTOFF_SKEW_MARGIN_SECONDS as f64)
    .fetch_one(&mut *connection)
    .await?;
    let (authentication_revoked_at, password_changed_at): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as(
            "UPDATE users SET \
             authentication_revoked_at = GREATEST(authentication_revoked_at, $2), \
             password_changed_at = GREATEST(password_changed_at, $2), \
             password_hash = $3, \
             password_reset_token_hash = NULL, password_reset_expires = NULL, \
             email_verification_token_hash = NULL, email_verification_token_expires_at = NULL, \
             pending_approval_email_transition_id = NULL, \
             pending_approval_email_payload = NULL, pending_approval_email_retry_at = NULL \
             WHERE id = $1 \
             RETURNING authentication_revoked_at, password_changed_at",
        )
        .bind(user_id)
        .bind(cutoff)
        .bind(&unusable_hash)
        .fetch_one(&mut *connection)
        .await?;
    let refresh_sessions_revoked = store::revoke_user_families(connection, user_id).await?;
    let reset_tokens_removed = sqlx::query("DELETE FROM password_reset_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *connection)
        .await?
        .rows_affected();
    Ok(CredentialRevocation {
        cutoff,
        authentication_revoked_at,
        password_changed_at,
        refresh_sessions_revoked,
        reset_tokens_removed,
    })
}

/// Confirm this transaction holds the user row lock. Re-locking a row the
/// transaction already holds returns at once; `NOWAIT` refuses instead of
/// waiting when another transaction holds it, so this helper never waits for
/// a lock outside the caller's lock order.
async fn confirm_user_lock(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(user_id)
        .fetch_optional(&mut *connection)
        .await?
        .map(|_| ())
        .ok_or(sqlx::Error::RowNotFound)
}
