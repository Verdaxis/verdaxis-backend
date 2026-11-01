//! `verdaxis-read job reminders`: in-app order-expiry reminders and their mail.
//!
//! One run scans for orders that expire within 48 hours and creates one
//! reminder per real order, recipient and expiry instant, then delivers at
//! most one mail batch (`cli::mail::expiry`). It replaces
//! `python -m app.cli.order_expiry_reminders` and keeps its shared-database
//! contract (`app/services/order_expiry_reminders.py:25-144`):
//!
//! - the transaction advisory key `8246192048`; a busy key means another
//!   scan (Rust or Python) is running, and this scan creates nothing;
//! - the eligibility rule, the per-recipient duplicate check on
//!   `order_key` and `expiry_epoch`, and the notification JSON keys;
//! - the scan order `expires_at, id, user id` and its row bound.
//!
//! Lock order differs from Python on purpose: each order is handled in its own
//! short transaction that takes the recipients' user rows (`FOR KEY SHARE`)
//! before the order row (`FOR UPDATE`). The market cancel path locks the
//! actor's user row before the order row, so this order cannot form the
//! cycle in which a scan holds the order while its notification insert waits
//! for the user row that a waiting cancel holds.
//!
//! The job runs in production only. Every other environment is refused before
//! a pool is opened.
use super::super::context::{self, Purpose, RunContext};
use super::super::gate::require_apply;
use super::super::mail::expiry::{
    self, Counts, EVENT, ExpiryMailConfig, ExpirySender, LEAD_TIME, MAX_BATCH, ProviderSender,
    RETRY_WINDOW, TRANSACTION_BOUNDS, epoch_seconds, render_payload,
};
use super::super::{Environment, Exit, JobArgs, JobKind, retry_bounded};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::future::Future;
use uuid::Uuid;

/// Python `REMINDER_LOCK_ID`; unchanged so Python and Rust scans exclude each other.
pub const REMINDER_LOCK_ID: i64 = 8_246_192_048;
/// Default scan bound in recipient rows (Python `batch_size`).
pub const DEFAULT_SCAN_LIMIT: u32 = 1000;
/// Largest scan bound a caller may ask for.
pub const MAX_SCAN_LIMIT: u32 = 10_000;

/// Database errors a scan transaction may meet from a concurrent writer:
/// deadlock, serialization failure and lock timeout. Each is retried with the
/// shared bounded policy; anything else stops the run.
const RETRYABLE_SQLSTATES: [&str; 3] = ["40P01", "40001", "55P03"];

/// Test seams inside one order's scan transaction. Production uses `()`.
pub trait ScanObserver: Sync {
    /// The recipients' user rows are locked; the order row is not yet.
    fn users_locked(&self, _order_id: Uuid) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
    /// The order row is locked too; no reminder is written yet.
    fn order_locked(&self, _order_id: Uuid) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

impl ScanObserver for () {}

/// What one scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanCounts {
    pub created: u64,
    /// Another scan held the advisory key, so this one stopped.
    pub lock_busy: bool,
}

/// What one whole run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunCounts {
    pub scan: ScanCounts,
    pub mail: Counts,
}

/// The eligibility rule (Python `_eligible_order_clauses`); `$1` is the
/// reference instant and `$2` the reference plus 48 hours.
macro_rules! eligible {
    () => {
        "o.provenance = 'REAL' AND g.provenance = 'REAL' AND u.status = 'APPROVED' \
         AND (o.owner_user_id = u.id OR o.owner_user_id IS NULL) \
         AND o.status IN ('OPEN', 'PARTIALLY_FILLED') \
         AND o.remaining_quantity_mt > 0 \
         AND o.expires_at > $1 AND o.expires_at <= $2"
    };
}

/// No reminder exists yet for this recipient, order and expiry instant. The
/// same comparison as Python: the stored number read as `float8` against the
/// expiry epoch.
macro_rules! not_reminded {
    () => {
        "NOT EXISTS (SELECT 1 FROM notifications n WHERE n.recipient_id = u.id \
         AND (n.data ->> 'event') = 'order_expiring' \
         AND (n.data ->> 'order_key') = replace(CAST(o.id AS varchar), '-', '') \
         AND CAST(n.data ->> 'expiry_epoch' AS float8) = EXTRACT(epoch FROM o.expires_at))"
    };
}

/// Candidate rows without locks, in Python's order and bound.
const CANDIDATES: &str = concat!(
    "SELECT o.id AS order_id, u.id AS user_id FROM orderbook_orders o \
     JOIN organizations g ON g.id = o.organization_id \
     JOIN users u ON u.organization_id = o.organization_id WHERE ",
    eligible!(),
    " AND ",
    not_reminded!(),
    " ORDER BY o.expires_at, o.id, u.id LIMIT $3"
);

const TRY_SCAN_LOCK: &str = "SELECT pg_try_advisory_xact_lock($1)";

/// The recipients' user rows, in id order, before the order row. `KEY SHARE`
/// is the lock the notification foreign key takes anyway; taking it first is
/// what removes the cycle, and it still allows ordinary user updates.
const LOCK_RECIPIENTS: &str = "SELECT id FROM users WHERE id = ANY($1) ORDER BY id FOR KEY SHARE";

/// The rows of one order, rechecked under its row lock. Only recipients whose
/// user rows this transaction already holds are written.
const LOCKED_ROWS: &str = concat!(
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

const INSERT_REMINDER: &str = "INSERT INTO notifications \
     (id, recipient_id, type, title, message, data, is_read, created_at) \
     VALUES ($1, $2, 'ORDER_UPDATE', 'Order expires within 48 hours', $3, CAST($4 AS json), false, $5)";

/// One eligible order with the recipients found for it.
struct Candidate {
    order_id: Uuid,
    users: Vec<Uuid>,
}

/// One locked row to remind.
struct Recipient {
    order_id: Uuid,
    side: String,
    expires_at: DateTime<Utc>,
    user_id: Uuid,
    email: String,
    email_verified: bool,
    email_enabled: bool,
    inapp_enabled: bool,
}

/// The reminder text (Python `remind_expiring_orders`, lines 105-113).
pub fn reminder_message(side: &str, order_id: Uuid, expiry: DateTime<Utc>) -> String {
    let order = order_id.to_string();
    format!(
        "Your {side} order {} expires on {}. Review it before expiry if you want to keep trading.",
        &order[..8],
        expiry.format("%d %b %Y at %H:%M UTC")
    )
}

/// Python `datetime.isoformat()` for a UTC instant: microseconds only when
/// nonzero, and a `+00:00` offset.
pub fn python_isoformat(instant: DateTime<Utc>) -> String {
    let format = if instant.timestamp_subsec_micros() == 0 {
        SecondsFormat::Secs
    } else {
        SecondsFormat::Micros
    };
    instant.to_rfc3339_opts(format, false)
}

/// The frozen notification data (Python lines 120-139), in Python's key order.
///
/// `deliverable_to` is the recipient address when the address is verified and
/// email updates are on; otherwise the reminder is created already discarded.
pub fn reminder_data(
    config: &ExpiryMailConfig,
    order_id: Uuid,
    expiry: DateTime<Utc>,
    deliverable_to: Option<&str>,
    inapp_enabled: bool,
    message: &str,
    reference: DateTime<Utc>,
) -> Value {
    let deadline = expiry.min(reference + RETRY_WINDOW);
    json!({
        "event": EVENT,
        "order_id": order_id.to_string(),
        "order_key": order_id.simple().to_string(),
        "expiry_epoch": epoch_seconds(expiry),
        "expires_at": python_isoformat(expiry),
        "_email_only": !inapp_enabled,
        "_email_status": if deliverable_to.is_some() { "pending" } else { "discarded" },
        "_email_payload": deliverable_to
            .map_or(Value::Null, |email| render_payload(config, email, message)),
        "_email_retry_at": epoch_seconds(reference),
        "_email_deadline": epoch_seconds(deadline),
    })
}

fn sqlstate(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned())
}

/// Split a database error into retryable (outer `Err`) and final (`Ok(Err)`).
fn classify(error: sqlx::Error) -> Result<Exit, sqlx::Error> {
    match sqlstate(&error) {
        Some(code) if RETRYABLE_SQLSTATES.contains(&code.as_str()) => Err(error),
        _ => Ok(Exit::Dependency),
    }
}

/// Create the due reminders, at most `limit` recipient rows.
pub async fn scan<O: ScanObserver>(
    pool: &PgPool,
    config: &ExpiryMailConfig,
    reference: DateTime<Utc>,
    limit: u32,
    observer: &O,
) -> Result<ScanCounts, Exit> {
    if !(1..=MAX_SCAN_LIMIT).contains(&limit) {
        return Err(Exit::Usage);
    }
    let horizon = reference + LEAD_TIME;
    let rows = sqlx::query(CANDIDATES)
        .bind(reference)
        .bind(horizon)
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await
        .map_err(|_| Exit::Dependency)?;
    let mut candidates: Vec<Candidate> = Vec::new();
    for row in rows {
        let order_id: Uuid = row.try_get("order_id").map_err(|_| Exit::Dependency)?;
        let user_id: Uuid = row.try_get("user_id").map_err(|_| Exit::Dependency)?;
        match candidates.last_mut() {
            Some(last) if last.order_id == order_id => last.users.push(user_id),
            _ => candidates.push(Candidate {
                order_id,
                users: vec![user_id],
            }),
        }
    }

    let mut counts = ScanCounts::default();
    for candidate in &candidates {
        let outcome = retry_bounded(|| async {
            match remind_order(pool, config, reference, candidate, observer).await {
                Ok(outcome) => Ok(Ok(outcome)),
                Err(error) => classify(error).map(Err),
            }
        })
        .await
        .map_err(|_| Exit::Dependency)??;
        match outcome {
            OrderOutcome::LockBusy => {
                counts.lock_busy = true;
                break;
            }
            OrderOutcome::Created(created) => counts.created += created,
        }
    }
    Ok(counts)
}

enum OrderOutcome {
    LockBusy,
    Created(u64),
}

/// One order's transaction: scan key, user rows, order row, reminders.
async fn remind_order<O: ScanObserver>(
    pool: &PgPool,
    config: &ExpiryMailConfig,
    reference: DateTime<Utc>,
    candidate: &Candidate,
    observer: &O,
) -> Result<OrderOutcome, sqlx::Error> {
    let mut transaction: Transaction<'_, Postgres> = pool.begin().await?;
    sqlx::raw_sql(TRANSACTION_BOUNDS)
        .execute(&mut *transaction)
        .await?;
    let acquired: bool = sqlx::query_scalar(TRY_SCAN_LOCK)
        .bind(REMINDER_LOCK_ID)
        .fetch_one(&mut *transaction)
        .await?;
    if !acquired {
        transaction.rollback().await?;
        return Ok(OrderOutcome::LockBusy);
    }
    let mut users = candidate.users.clone();
    users.sort();
    users.dedup();
    sqlx::query(LOCK_RECIPIENTS)
        .bind(&users)
        .execute(&mut *transaction)
        .await?;
    observer.users_locked(candidate.order_id).await;
    let rows = sqlx::query(LOCKED_ROWS)
        .bind(reference)
        .bind(reference + LEAD_TIME)
        .bind(candidate.order_id)
        .bind(&users)
        .fetch_all(&mut *transaction)
        .await?;
    observer.order_locked(candidate.order_id).await;
    let mut created = 0;
    for row in rows {
        let recipient = Recipient {
            order_id: row.try_get("order_id")?,
            side: row.try_get("side")?,
            expires_at: row.try_get("expires_at")?,
            user_id: row.try_get("user_id")?,
            email: row.try_get("email")?,
            email_verified: row.try_get("email_verified")?,
            email_enabled: row.try_get("email_enabled")?,
            inapp_enabled: row.try_get("inapp_enabled")?,
        };
        let message = reminder_message(&recipient.side, recipient.order_id, recipient.expires_at);
        let data = reminder_data(
            config,
            recipient.order_id,
            recipient.expires_at,
            (recipient.email_verified && recipient.email_enabled)
                .then_some(recipient.email.as_str()),
            recipient.inapp_enabled,
            &message,
            reference,
        );
        sqlx::query(INSERT_REMINDER)
            .bind(Uuid::new_v4())
            .bind(recipient.user_id)
            .bind(&message)
            .bind(data.to_string())
            .bind(reference)
            .execute(&mut *transaction)
            .await?;
        created += 1;
    }
    transaction.commit().await?;
    Ok(OrderOutcome::Created(created))
}

/// One run: the scan, then one bounded mail batch, as the Python CLI does.
/// A busy scan key still lets the mail batch run.
pub async fn run_once<S, C>(
    pool: &PgPool,
    config: &ExpiryMailConfig,
    sender: &S,
    clock: &C,
    limit: u32,
    batch: u32,
) -> Result<RunCounts, Exit>
where
    S: ExpirySender,
    C: Fn() -> DateTime<Utc> + Sync,
{
    let scan = scan(pool, config, clock(), limit, &()).await?;
    let mail = expiry::deliver(pool, sender, clock, batch).await?;
    Ok(RunCounts { scan, mail })
}

fn report(outcome: &str, counts: Option<RunCounts>) {
    let counts = counts.map_or_else(
        || json!({}),
        |counts| {
            json!({
                "reminders_created": counts.scan.created,
                "scan_lock_busy": counts.scan.lock_busy,
                "emails_sent": counts.mail.sent,
                "emails_deferred": counts.mail.deferred,
                "emails_discarded": counts.mail.discarded,
            })
        },
    );
    eprintln!(
        "{}",
        json!({"command": "job reminders", "outcome": outcome, "counts": counts})
    );
}

/// The command entry point.
pub async fn command(args: &JobArgs) -> Exit {
    command_with(args, |environment, release_sha| async move {
        context::open(Purpose::Job(JobKind::Reminders), environment, &release_sha).await
    })
    .await
}

/// The command with an injectable context opener, so the refusals can be
/// proven to happen before any pool exists.
pub async fn command_with<F, Fut>(args: &JobArgs, open: F) -> Exit
where
    F: FnOnce(Environment, String) -> Fut,
    Fut: Future<Output = Result<RunContext, Exit>>,
{
    if args.identity.environment != Environment::Production {
        report("refused_environment", None);
        return Exit::Usage;
    }
    if let Err(exit) = require_apply(args.apply) {
        report("refused_without_apply", None);
        return exit;
    }
    let context = match open(args.identity.environment, args.identity.release_sha.clone()).await {
        Ok(context) => context,
        Err(exit) => {
            report("context_refused", None);
            return exit;
        }
    };
    let _ = tracing_subscriber::fmt().json().try_init();
    let sender = match ProviderSender::from_environment() {
        Ok(sender) => sender,
        Err(exit) => {
            report("mail_unavailable", None);
            return exit;
        }
    };
    let config = ExpiryMailConfig::from_environment();
    match run_once(
        &context.pool,
        &config,
        &sender,
        &Utc::now,
        DEFAULT_SCAN_LIMIT,
        MAX_BATCH,
    )
    .await
    {
        Ok(counts) => {
            report("completed", Some(counts));
            Exit::Ok
        }
        Err(exit) => {
            report("failed", None);
            exit
        }
    }
}
