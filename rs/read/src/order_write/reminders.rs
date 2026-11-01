//! Order-expiry reminder locks: the only order-row locks and the only advisory
//! key that the reminder job (`cli::jobs::reminders`, `cli::mail::expiry`)
//! takes.
//!
//! Like [`super::invalidate_organization`], every function borrows the
//! caller's connection and never begins, commits or retries a transaction.
//! The caller sets the transaction bounds and writes the reminder rows; this
//! module only reads and locks.
//!
//! Lock order follows the market operation graph, which takes user rows before
//! order rows (cancel locks the actor's user row, then the order row):
//!
//! - scan: the reminder scan key (try only, never waits), then the recipients'
//!   user rows `FOR KEY SHARE` in id order, then the order row `FOR UPDATE`
//!   with the eligibility recheck. `KEY SHARE` is the lock the reminder's
//!   foreign key takes anyway; taking it first removes the cycle with a
//!   waiting cancel and still lets ordinary user updates proceed;
//! - delivery: the recipient's user row `FOR UPDATE`, then the order row
//!   `FOR UPDATE` with the delivery recheck.
//!
//! [`RecipientsLocked`] can only be obtained after the user rows are locked,
//! so the order row cannot be locked first.
//!
//! The eligibility rule and the duplicate check are the Python reminder's
//! (`app/services/order_expiry_reminders.py:25-69`), which may run against the
//! same rows while both exist.
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

/// Python `REMINDER_LOCK_ID`, unchanged so Python and Rust scans exclude each
/// other.
pub const REMINDER_SCAN_KEY: i64 = 8_246_192_048;

/// The reminder window: orders that expire after `reference` and no later
/// than `horizon` (the reference plus 48 hours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReminderWindow {
    pub reference: DateTime<Utc>,
    pub horizon: DateTime<Utc>,
}

/// One eligible order with the recipients found for it, before any lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderCandidate {
    pub order_id: Uuid,
    pub users: Vec<Uuid>,
}

/// One recipient row of a locked order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderRow {
    pub order_id: Uuid,
    pub side: String,
    pub expires_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub email_enabled: bool,
    pub inapp_enabled: bool,
}

/// Proof that this transaction holds the scan key and the recipients' user
/// rows of one order.
#[derive(Debug)]
pub struct RecipientsLocked {
    order_id: Uuid,
    users: Vec<Uuid>,
}

/// The eligibility rule (Python `_eligible_order_clauses`); `$1` is the
/// reference instant and `$2` the horizon.
macro_rules! eligible {
    () => {
        "o.provenance = 'REAL' AND g.provenance = 'REAL' AND u.status = 'APPROVED' \
         AND (o.owner_user_id = u.id OR o.owner_user_id IS NULL) \
         AND o.status IN ('OPEN', 'PARTIALLY_FILLED') \
         AND o.remaining_quantity_mt > 0 \
         AND o.expires_at > $1 AND o.expires_at <= $2"
    };
}

/// No reminder exists yet for this recipient, order and expiry instant: the
/// stored number read as `float8` against the expiry epoch, as in Python.
macro_rules! not_reminded {
    () => {
        "NOT EXISTS (SELECT 1 FROM notifications n WHERE n.recipient_id = u.id \
         AND (n.data ->> 'event') = 'order_expiring' \
         AND (n.data ->> 'order_key') = replace(CAST(o.id AS varchar), '-', '') \
         AND CAST(n.data ->> 'expiry_epoch' AS float8) = EXTRACT(epoch FROM o.expires_at))"
    };
}

const CANDIDATES: &str = concat!(
    "SELECT o.id AS order_id, u.id AS user_id FROM orderbook_orders o \
     JOIN organizations g ON g.id = o.organization_id \
     JOIN users u ON u.organization_id = o.organization_id WHERE ",
    eligible!(),
    " AND ",
    not_reminded!(),
    " ORDER BY o.expires_at, o.id, u.id LIMIT $3"
);

const TRY_SCAN_KEY: &str = "SELECT pg_try_advisory_xact_lock($1)";

const LOCK_RECIPIENTS: &str = "SELECT id FROM users WHERE id = ANY($1) ORDER BY id FOR KEY SHARE";

const LOCK_ORDER: &str = concat!(
    "SELECT o.id AS order_id, o.side AS side, o.expires_at AS expires_at, \
     u.id AS user_id, u.email AS email, u.email_verified AS email_verified, \
     COALESCE(CAST(p.value ->> 'email_trade_updates' AS boolean), true) AS email_enabled, \
     COALESCE(CAST(p.value ->> 'inapp_trade_updates' AS boolean), true) AS inapp_enabled \
     FROM orderbook_orders o \
     JOIN organizations g ON g.id = o.organization_id \
     JOIN users u ON u.organization_id = o.organization_id \
     LEFT JOIN user_preferences p ON p.user_id = u.id AND p.namespace = 'notifications' \
     WHERE o.id = $3 AND u.id = ANY($4) AND ",
    eligible!(),
    " AND ",
    not_reminded!(),
    " ORDER BY u.id FOR UPDATE OF o"
);

const LOCK_DELIVERY_RECIPIENT: &str = "SELECT u.id FROM notifications n \
     JOIN users u ON u.id = n.recipient_id WHERE n.id = $1 FOR UPDATE OF u";

/// The ownership, address, preference and expiry recheck of a claimed
/// reminder (Python lines 193-217). The expiry comparison reads the stored
/// number in PostgreSQL.
const LOCK_DELIVERY_ORDER: &str = concat!(
    "SELECT u.email FROM notifications n \
     JOIN users u ON u.id = n.recipient_id \
     JOIN organizations g ON g.id = u.organization_id \
     JOIN orderbook_orders o ON o.organization_id = g.id \
     LEFT JOIN user_preferences p ON p.user_id = u.id AND p.namespace = 'notifications' \
     WHERE n.id = $3 AND o.id = $4 \
       AND u.email_verified IS true \
       AND COALESCE(CAST(p.value ->> 'email_trade_updates' AS boolean), true) IS true \
       AND EXTRACT(epoch FROM o.expires_at) = CAST(n.data ->> 'expiry_epoch' AS float8) AND ",
    eligible!(),
    " FOR UPDATE OF o"
);

/// Eligible recipient rows without locks, in Python's order and row bound,
/// grouped by order.
pub async fn reminder_candidates(
    conn: &mut PgConnection,
    window: ReminderWindow,
    limit: i64,
) -> Result<Vec<ReminderCandidate>, sqlx::Error> {
    let rows = sqlx::query(CANDIDATES)
        .bind(window.reference)
        .bind(window.horizon)
        .bind(limit)
        .fetch_all(conn)
        .await?;
    let mut candidates: Vec<ReminderCandidate> = Vec::new();
    for row in rows {
        let order_id: Uuid = row.try_get("order_id")?;
        let user_id: Uuid = row.try_get("user_id")?;
        match candidates.last_mut() {
            Some(last) if last.order_id == order_id => last.users.push(user_id),
            _ => candidates.push(ReminderCandidate {
                order_id,
                users: vec![user_id],
            }),
        }
    }
    Ok(candidates)
}

/// Try the scan key, then lock the candidate's user rows. `None` means another
/// scan (Rust or Python) holds the key.
pub async fn lock_reminder_recipients(
    conn: &mut PgConnection,
    candidate: &ReminderCandidate,
) -> Result<Option<RecipientsLocked>, sqlx::Error> {
    let acquired: bool = sqlx::query_scalar(TRY_SCAN_KEY)
        .bind(REMINDER_SCAN_KEY)
        .fetch_one(&mut *conn)
        .await?;
    if !acquired {
        return Ok(None);
    }
    let mut users = candidate.users.clone();
    users.sort();
    users.dedup();
    sqlx::query(LOCK_RECIPIENTS)
        .bind(&users)
        .execute(&mut *conn)
        .await?;
    Ok(Some(RecipientsLocked {
        order_id: candidate.order_id,
        users,
    }))
}

impl RecipientsLocked {
    pub fn order_id(&self) -> Uuid {
        self.order_id
    }

    /// Lock the order row and return the recipients still due a reminder.
    /// Only recipients whose user rows this transaction holds are returned.
    pub async fn lock_order(
        self,
        conn: &mut PgConnection,
        window: ReminderWindow,
    ) -> Result<Vec<ReminderRow>, sqlx::Error> {
        let rows = sqlx::query(LOCK_ORDER)
            .bind(window.reference)
            .bind(window.horizon)
            .bind(self.order_id)
            .bind(&self.users)
            .fetch_all(conn)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ReminderRow {
                    order_id: row.try_get("order_id")?,
                    side: row.try_get("side")?,
                    expires_at: row.try_get("expires_at")?,
                    user_id: row.try_get("user_id")?,
                    email: row.try_get("email")?,
                    email_verified: row.try_get("email_verified")?,
                    email_enabled: row.try_get("email_enabled")?,
                    inapp_enabled: row.try_get("inapp_enabled")?,
                })
            })
            .collect()
    }
}

/// Lock a claimed reminder's recipient, then its order, and return the
/// recipient's current address when the reminder is still deliverable.
pub async fn lock_reminder_delivery(
    conn: &mut PgConnection,
    notification_id: Uuid,
    order_id: Uuid,
    window: ReminderWindow,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query(LOCK_DELIVERY_RECIPIENT)
        .bind(notification_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query_scalar(LOCK_DELIVERY_ORDER)
        .bind(window.reference)
        .bind(window.horizon)
        .bind(notification_id)
        .bind(order_id)
        .fetch_optional(conn)
        .await
}
