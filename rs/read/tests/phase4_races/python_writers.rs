//! Models of the Python writers that keep running after the Phase 4 routes
//! switch to Rust: the approval-mail retry timer and the authentication
//! maintenance command (both run by the one `auth_maintenance` job), and the
//! trade-lifecycle requests, which stay in Python until the market phases.
//!
//! Each function issues the statements its Python source issues, in the order
//! the source issues them, on a dedicated connection with the runtime role's
//! authority. The races interleave them with the Rust commands that write the
//! same rows. Python routes that move at the switch have no model here.

use chrono::{DateTime, SubsecRound, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

/// What the timer's first transaction found.
pub enum DueMail {
    /// The marker was missing, stale or not yet due: nothing was claimed.
    Skipped,
    /// The recipient is no longer eligible: the marker was cleared and committed.
    Discarded,
    /// The row is locked and its payload read; the lease is not yet written.
    Locked,
}

/// `app/services/account_approval_email.py`, `deliver_account_approval_email`,
/// first transaction up to the lock: select the due row `FOR UPDATE`.
pub async fn lock_due_approval_mail(
    conn: &mut PgConnection,
    user: Uuid,
    transition: Uuid,
    now: DateTime<Utc>,
) -> DueMail {
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    let row: Option<(String, Option<String>, bool, Option<Value>)> = sqlx::query_as(
        "SELECT status,role,email_verified,pending_approval_email_payload FROM users \
         WHERE id=$1 AND pending_approval_email_transition_id=$2 \
         AND pending_approval_email_retry_at <= $3 FOR UPDATE",
    )
    .bind(user)
    .bind(transition)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
    .unwrap();
    let Some((status, role, verified, payload)) = row else {
        sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
        return DueMail::Skipped;
    };
    let eligible = status == "APPROVED" && verified && role.as_deref() != Some("ADMIN");
    if !eligible || !payload.is_some_and(|value| value.is_object()) {
        sqlx::query("UPDATE users SET pending_approval_email_transition_id=NULL,pending_approval_email_payload=NULL,pending_approval_email_retry_at=NULL WHERE id=$1")
            .bind(user)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
        return DueMail::Discarded;
    }
    DueMail::Locked
}

/// The same transaction after the lock: write the five-minute lease and commit.
/// Returns the lease expiry exactly as the database stores it.
pub async fn commit_lease(
    conn: &mut PgConnection,
    user: Uuid,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let claim_until = (now + chrono::Duration::minutes(5)).trunc_subsecs(6);
    sqlx::query("UPDATE users SET pending_approval_email_retry_at=$2 WHERE id=$1")
        .bind(user)
        .bind(claim_until)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
    claim_until
}

/// How the second transaction ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Finalized {
    /// The lease was replaced during the send: nothing was changed.
    Skipped,
    Sent,
    Deferred,
}

/// The second transaction: reselect the row by the exact lease it wrote and
/// either clear the marker or push the retry back an hour.
pub async fn finalize_approval_mail(
    conn: &mut PgConnection,
    user: Uuid,
    transition: Uuid,
    claim_until: DateTime<Utc>,
    accepted: bool,
    now: DateTime<Utc>,
) -> Finalized {
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE id=$1 AND pending_approval_email_transition_id=$2 \
         AND pending_approval_email_retry_at=$3 FOR UPDATE",
    )
    .bind(user)
    .bind(transition)
    .bind(claim_until)
    .fetch_optional(&mut *conn)
    .await
    .unwrap();
    if found.is_none() {
        sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
        return Finalized::Skipped;
    }
    if accepted {
        sqlx::query("UPDATE users SET pending_approval_email_transition_id=NULL,pending_approval_email_payload=NULL,pending_approval_email_retry_at=NULL WHERE id=$1")
            .bind(user)
            .execute(&mut *conn)
            .await
            .unwrap();
    } else {
        let retry = (now + chrono::Duration::hours(1)).trunc_subsecs(6);
        sqlx::query("UPDATE users SET pending_approval_email_retry_at=$2 WHERE id=$1")
            .bind(user)
            .bind(retry)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
    if accepted {
        Finalized::Sent
    } else {
        Finalized::Deferred
    }
}

/// `app/services/auth_maintenance.py`, refresh-session cleanup: delete a bounded
/// batch of expired sessions and of old revoked families without a live
/// successor. Runs inside the caller's open transaction.
pub async fn delete_stale_refresh_sessions(
    conn: &mut PgConnection,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> u64 {
    sqlx::query(
        "DELETE FROM refresh_sessions WHERE refresh_sessions.id IN (\
         SELECT refresh_sessions.id FROM refresh_sessions \
         WHERE refresh_sessions.expires_at <= $1 \
         OR (refresh_sessions.revoked IS true \
             AND refresh_sessions.created_at <= $1 - interval '24 hours' \
             AND NOT EXISTS (SELECT a.id FROM refresh_sessions a \
                 WHERE a.family_id = refresh_sessions.family_id AND a.revoked IS false \
                 AND a.expires_at > $1)) \
         LIMIT $2)",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(&mut *conn)
    .await
    .unwrap()
    .rows_affected()
}

/// The same job's invitation-digest cleanup: clear the digest and expiry of
/// every user (up to a batch) whose expiry has passed. One statement, no
/// organization lock, and no user-id order between the rows it locks.
pub async fn clear_expired_reset_hashes(
    conn: &mut PgConnection,
    cutoff: DateTime<Utc>,
    batch: i64,
) -> u64 {
    sqlx::query(
        "UPDATE users SET password_reset_token_hash=NULL,password_reset_expires=NULL \
         WHERE users.id IN (SELECT users.id FROM users \
         WHERE users.password_reset_token_hash IS NOT NULL \
         AND (users.password_reset_expires IS NULL OR users.password_reset_expires <= $1) \
         LIMIT $2)",
    )
    .bind(cutoff)
    .bind(batch)
    .execute(&mut *conn)
    .await
    .unwrap()
    .rows_affected()
}

/// One row of the digest cleanup. The statement above locks its rows one at a
/// time in storage order, which is not user-id order; a race that needs two
/// rows in a chosen order issues this once per row.
pub async fn clear_reset_hash_of(conn: &mut PgConnection, user: Uuid) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE users SET password_reset_token_hash=NULL,password_reset_expires=NULL WHERE id=$1",
    )
    .bind(user)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// `app/routers/trades.py`, a deliver or pay request on an existing trade: the
/// market slice's advisory lock first, then the trade and its order, then the
/// two parties' user rows and organization rows each in id order
/// (`_revalidate_trade_parties`), then the lifecycle write and the commit.
/// Python takes the slice and market rows before any user or organization row;
/// the Rust organization commands take organizations and users first.
pub struct TradeLifecycle {
    pub slice_key: i64,
    pub trade: Uuid,
    pub order: Uuid,
    pub users: Vec<Uuid>,
    pub organizations: Vec<Uuid>,
}

impl TradeLifecycle {
    pub async fn deliver(self, mut conn: PgConnection) -> Result<(), sqlx::Error> {
        sqlx::query("BEGIN").execute(&mut conn).await?;
        sqlx::query(include_str!(
            "../fixtures/market_invalidation_advisory_lock.sql"
        ))
        .bind(self.slice_key)
        .execute(&mut conn)
        .await?;
        sqlx::query("SELECT id FROM trades WHERE id=$1 FOR UPDATE")
            .bind(self.trade)
            .execute(&mut conn)
            .await?;
        sqlx::query(include_str!(
            "../fixtures/phase4_races_python_order_lock.sql"
        ))
        .bind(self.order)
        .execute(&mut conn)
        .await?;
        sqlx::query("SELECT id FROM users WHERE id = ANY($1) ORDER BY id FOR UPDATE")
            .bind(&self.users)
            .execute(&mut conn)
            .await?;
        sqlx::query("SELECT id FROM organizations WHERE id = ANY($1) ORDER BY id FOR UPDATE")
            .bind(&self.organizations)
            .execute(&mut conn)
            .await?;
        sqlx::query("UPDATE trades SET status='DELIVERED' WHERE id=$1")
            .bind(self.trade)
            .execute(&mut conn)
            .await?;
        sqlx::query("COMMIT").execute(&mut conn).await?;
        Ok(())
    }
}
