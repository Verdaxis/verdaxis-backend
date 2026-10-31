//! Session and reset-token rows shared with the Python writers.
//!
//! Every function runs on the caller's transaction and never commits. Callers
//! take locks in the order [`super::lock`] defines: [`lock_device`], then the
//! user row ([`lock_user`] or [`lock_user_by_email`]), then that user's
//! refresh-session or reset-token rows. Statements read and write the same
//! columns Python does, so a row written by either stack is valid for both.
//!
//! Rust never reads or writes `users.password_reset_token_hash` or
//! `users.password_reset_expires`: they belong to Python's invitation flow.
//! Reset tokens live only in `password_reset_tokens`.
//!
//! Token values never reach this module; callers pass SHA-256 hex digests, and
//! a lookup is a digest equality in PostgreSQL.

use std::fmt;

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::lock::{device_lock_key, is_sha256_hex};

/// Refuse a value that is not a digest before it reaches a statement, so a
/// raw token can never be stored or used as a lookup key by mistake.
fn validate_digest(digest: &str) -> Result<(), sqlx::Error> {
    if is_sha256_hex(digest) {
        return Ok(());
    }
    let message = "expected a lowercase SHA-256 hex digest";
    Err(sqlx::Error::InvalidArgument(message.into()))
}

/// Maximum expired rows removed by one independent cleanup transaction.
pub const EXPIRED_RESET_SWEEP_LIMIT: i64 = 1000;

/// The account projection a credential route reads under the user row lock.
#[derive(Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub status: String,
    pub role: Option<String>,
    pub organization_id: Option<Uuid>,
    pub email_verified: bool,
    pub must_change_password: bool,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub authentication_revoked_at: Option<DateTime<Utc>>,
    pub email_verification_token_hash: Option<String>,
    pub email_verification_token_expires_at: Option<DateTime<Utc>>,
    pub referral_code: Option<String>,
}

/// Credentials and token digests stay out of logs.
impl fmt::Debug for UserRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserRow")
            .field("id", &self.id)
            .field("status", &self.status)
            .field("role", &self.role)
            .field("organization_id", &self.organization_id)
            .field("email_verified", &self.email_verified)
            .field("must_change_password", &self.must_change_password)
            .field("password_changed_at", &self.password_changed_at)
            .field("authentication_revoked_at", &self.authentication_revoked_at)
            .finish_non_exhaustive()
    }
}

/// One `refresh_sessions` row. `device_id_hash` is absent only on revoked
/// families from before device binding.
#[derive(Clone, sqlx::FromRow)]
pub struct RefreshRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub jti_hash: String,
    pub device_id_hash: Option<String>,
    pub replaced_by_jti_hash: Option<String>,
    pub rotation_grace_until: Option<DateTime<Utc>>,
    pub revoked: bool,
    pub expires_at: DateTime<Utc>,
}

/// Token digests stay out of logs.
impl fmt::Debug for RefreshRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RefreshRow")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("family_id", &self.family_id)
            .field("rotation_grace_until", &self.rotation_grace_until)
            .field("revoked", &self.revoked)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// A new live refresh session. New rows are always bound to a device.
pub struct NewRefreshRow {
    pub user_id: Uuid,
    pub family_id: Uuid,
    pub jti_hash: String,
    pub device_id_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// A new reset token for one user. Issuing one replaces the user's previous
/// token, so only the latest link works.
pub struct NewResetToken {
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}

/// Take the transaction-scoped advisory lock for one browser device. This is
/// the first lock of every login, refresh, logout and password change.
pub async fn lock_device(
    connection: &mut PgConnection,
    device_hash: &str,
) -> Result<(), sqlx::Error> {
    let key = device_lock_key(device_hash).ok_or_else(|| {
        sqlx::Error::InvalidArgument("device hash is not a lowercase SHA-256 hex digest".into())
    })?;
    sqlx::query("SELECT pg_catalog.pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// The columns of [`UserRow`]. Status and role are stored as text-backed enums.
macro_rules! select_user_for_update {
    ($filter:literal) => {
        concat!(
            "SELECT id, email, password_hash, status::text AS status, role::text AS role, ",
            "organization_id, email_verified, must_change_password, password_changed_at, ",
            "authentication_revoked_at, email_verification_token_hash, ",
            "email_verification_token_expires_at, referral_code ",
            "FROM users WHERE ",
            $filter,
            " FOR UPDATE"
        )
    };
}

/// Lock one user row by id.
pub async fn lock_user(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as(select_user_for_update!("id = $1"))
        .bind(user_id)
        .fetch_optional(&mut *connection)
        .await
}

/// Lock one user row by email. The caller passes the normalized address
/// (trimmed and lowercased), the form `users.email` stores.
pub async fn lock_user_by_email(
    connection: &mut PgConnection,
    email: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as(select_user_for_update!("email = $1"))
        .bind(email)
        .fetch_optional(&mut *connection)
        .await
}

/// Lock one refresh-session row by the digest of its token id.
pub async fn lock_refresh_by_jti(
    connection: &mut PgConnection,
    jti_hash: &str,
) -> Result<Option<RefreshRow>, sqlx::Error> {
    validate_digest(jti_hash)?;
    sqlx::query_as(
        "SELECT id, user_id, family_id, jti_hash, device_id_hash, replaced_by_jti_hash, \
         rotation_grace_until, revoked, expires_at \
         FROM refresh_sessions WHERE jti_hash = $1 FOR UPDATE",
    )
    .bind(jti_hash)
    .fetch_optional(&mut *connection)
    .await
}

/// Revoke every live family bound to one device, and also the presented
/// family when the browser sent a refresh token: families from before device
/// binding have no device hash, and the presented token is the only way to
/// find them. Returns the number of rows newly revoked. The caller holds the
/// device lock.
pub async fn revoke_device_families(
    connection: &mut PgConnection,
    device_hash: &str,
    presented_family: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    validate_digest(device_hash)?;
    let revoked = sqlx::query(
        "UPDATE refresh_sessions SET revoked = true \
         WHERE NOT revoked AND (device_id_hash = $1 OR family_id = $2)",
    )
    .bind(device_hash)
    .bind(presented_family)
    .execute(&mut *connection)
    .await?;
    Ok(revoked.rows_affected())
}

/// Revoke one authenticated family after its user's row is locked.
pub async fn revoke_family(
    connection: &mut PgConnection,
    user_id: Uuid,
    family_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE user_id=$1 AND family_id=$2")
        .bind(user_id)
        .bind(family_id)
        .execute(connection)
        .await?;
    Ok(())
}

/// Revoke every live refresh session of one user. Returns the number of rows
/// newly revoked. The caller holds the user row lock.
pub async fn revoke_user_families(
    connection: &mut PgConnection,
    user_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let revoked = sqlx::query(
        "UPDATE refresh_sessions SET revoked = true WHERE NOT revoked AND user_id = $1",
    )
    .bind(user_id)
    .execute(&mut *connection)
    .await?;
    Ok(revoked.rows_affected())
}

/// Advance the user's cutoff and revoke all their refresh families in the
/// caller's transaction. Takes the user row lock before session rows; callers
/// must not already hold later locks. An unknown user returns `RowNotFound`.
/// The caller must commit or roll back the whole operation, including on error.
pub async fn revoke_user_authentication(
    conn: &mut PgConnection,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, sqlx::Error> {
    let user = lock_user(conn, user_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    // PostgreSQL timestamps store microseconds. Compare at that precision so
    // sub-microsecond input cannot return a cutoff that fails to advance in SQL.
    let now = now.trunc_subsecs(6);
    let cutoff = match user.authentication_revoked_at {
        Some(stored) if now <= stored => stored
            .checked_add_signed(TimeDelta::microseconds(1))
            .ok_or_else(|| sqlx::Error::InvalidArgument("cutoff exceeds timestamp range".into()))?,
        _ => now,
    };
    sqlx::query("UPDATE users SET authentication_revoked_at = $2 WHERE id = $1")
        .bind(user_id)
        .bind(cutoff)
        .execute(&mut *conn)
        .await?;
    revoke_user_families(conn, user_id).await?;
    Ok(cutoff)
}

/// Insert one live refresh session. `created_at` takes the column default.
pub async fn insert_refresh(
    connection: &mut PgConnection,
    row: &NewRefreshRow,
) -> Result<(), sqlx::Error> {
    validate_digest(&row.jti_hash)?;
    validate_digest(&row.device_id_hash)?;
    sqlx::query(
        "INSERT INTO refresh_sessions \
         (id, user_id, family_id, jti_hash, device_id_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(row.user_id)
    .bind(row.family_id)
    .bind(&row.jti_hash)
    .bind(&row.device_id_hash)
    .bind(row.expires_at)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Store a new reset token for a user whose row the caller has locked, and
/// replace any earlier token of that user.
///
/// This statement touches only that user's row. Expired-row cleanup uses
/// [`sweep_expired_reset_tokens`] in its own transaction.
pub async fn upsert_reset_token(
    connection: &mut PgConnection,
    token: &NewResetToken,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    validate_digest(&token.token_hash)?;
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at, created_at, used_at) \
         VALUES ($1, $2, $3, $4, NULL) \
         ON CONFLICT (user_id) DO UPDATE SET token_hash = EXCLUDED.token_hash, \
         expires_at = EXCLUDED.expires_at, created_at = EXCLUDED.created_at, used_at = NULL",
    )
    .bind(token.user_id)
    .bind(&token.token_hash)
    .bind(token.expires_at)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Remove a bounded batch of expired reset tokens in a separate transaction.
/// Accepting a pool prevents cleanup from retaining token-row locks inside a
/// caller's user/session transaction. Cleanup never acquires user-row locks.
/// Call this before or after, never while holding, a credential transaction.
pub async fn sweep_expired_reset_tokens(
    pool: &sqlx::PgPool,
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin_with("BEGIN READ WRITE").await?;
    let deleted = sqlx::query(
        "DELETE FROM password_reset_tokens WHERE user_id IN ( \
             SELECT user_id FROM password_reset_tokens WHERE expires_at <= $1 \
             ORDER BY expires_at, user_id LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(now)
    .bind(EXPIRED_RESET_SWEEP_LIMIT)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(deleted.rows_affected())
}

/// The user a reset-token digest belongs to, read without a lock. The caller
/// then locks that user and calls [`consume_reset_token`], which checks the
/// token again under the lock.
pub async fn reset_token_owner(
    connection: &mut PgConnection,
    token_hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    validate_digest(token_hash)?;
    sqlx::query_scalar("SELECT user_id FROM password_reset_tokens WHERE token_hash = $1")
        .bind(token_hash)
        .fetch_optional(&mut *connection)
        .await
}

/// Mark a user's reset token used if it is still that user's token, unused
/// and unexpired at `now`. Returns whether this call consumed it; a token is
/// consumed at most once. The caller holds the user row lock.
pub async fn consume_reset_token(
    connection: &mut PgConnection,
    user_id: Uuid,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    validate_digest(token_hash)?;
    let consumed = sqlx::query(
        "UPDATE password_reset_tokens SET used_at = $3 \
         WHERE user_id = $1 AND token_hash = $2 AND used_at IS NULL AND expires_at > $3",
    )
    .bind(user_id)
    .bind(token_hash)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(consumed.rows_affected() == 1)
}

/// Revoke all rows of a family after the device and user locks.
pub async fn revoke_refresh_family(
    conn: &mut sqlx::PgConnection,
    family: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE family_id=$1")
        .bind(family)
        .execute(conn)
        .await?;
    Ok(())
}
/// Link a locked predecessor to its successor and record its rotation time.
pub async fn mark_refresh_rotated(
    conn: &mut sqlx::PgConnection,
    jti: &str,
    next: &str,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refresh_sessions SET revoked=true,replaced_by_jti_hash=$2,rotation_grace_until=$3,last_used_at=$4 WHERE jti_hash=$1").bind(jti).bind(next).bind(now+TimeDelta::seconds(5)).bind(now).execute(conn).await?;
    Ok(())
}
