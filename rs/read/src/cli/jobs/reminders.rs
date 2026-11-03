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
//! short transaction in which the write boundary (`order_write::reminders`)
//! takes the scan key, the recipients' user rows (`FOR KEY SHARE`) and then
//! the order row (`FOR UPDATE`), as the market operation graph does. This
//! order cannot form the cycle in which a scan holds the order while its
//! notification insert waits for the user row that a waiting cancel holds.
//! This module writes only the reminder rows.
//!
//! The job runs in production only. Every other environment is refused before
//! a pool is opened.
use super::super::context::{self, Purpose, RunContext};
use super::super::gate::{require_apply, require_unit_deploy_state_absent};
use super::super::mail::expiry::{
    self, Counts, EVENT, ExpiryMailConfig, ExpirySender, LEAD_TIME, MAX_BATCH, ProviderSender,
    RETRY_WINDOW, TRANSACTION_BOUNDS, classify, epoch_seconds, render_payload,
};
use super::super::{Environment, Exit, JobArgs, JobKind, retry_bounded};
use crate::order_write::{
    REMINDER_SCAN_KEY, ReminderCandidate, ReminderWindow, lock_reminder_recipients,
    reminder_candidates,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use std::future::Future;
use uuid::Uuid;

/// Python `REMINDER_LOCK_ID`; unchanged so Python and Rust scans exclude each other.
pub const REMINDER_LOCK_ID: i64 = REMINDER_SCAN_KEY;
/// Default scan bound in recipient rows (Python `batch_size`).
pub const DEFAULT_SCAN_LIMIT: u32 = 1000;
/// Largest scan bound a caller may ask for.
pub const MAX_SCAN_LIMIT: u32 = 10_000;

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

const INSERT_REMINDER: &str = "INSERT INTO notifications \
     (id, recipient_id, type, title, message, data, is_read, created_at) \
     VALUES ($1, $2, 'ORDER_UPDATE', 'Order expires within 48 hours', $3, CAST($4 AS json), false, $5)";

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
    let window = ReminderWindow {
        reference,
        horizon: reference + LEAD_TIME,
    };
    let candidates = {
        let mut connection = pool.acquire().await.map_err(|_| Exit::Dependency)?;
        reminder_candidates(&mut connection, window, i64::from(limit))
            .await
            .map_err(|_| Exit::Dependency)?
    };
    let mut counts = ScanCounts::default();
    for candidate in &candidates {
        let outcome = retry_bounded(|| async {
            match remind_order(pool, config, window, candidate, observer).await {
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

/// One order's transaction: the write boundary takes the scan key, the user
/// rows and then the order row; this module writes the reminders.
async fn remind_order<O: ScanObserver>(
    pool: &PgPool,
    config: &ExpiryMailConfig,
    window: ReminderWindow,
    candidate: &ReminderCandidate,
    observer: &O,
) -> Result<OrderOutcome, sqlx::Error> {
    let mut transaction: Transaction<'_, Postgres> = pool.begin().await?;
    sqlx::raw_sql(TRANSACTION_BOUNDS)
        .execute(&mut *transaction)
        .await?;
    let Some(locked) = lock_reminder_recipients(&mut transaction, candidate).await? else {
        transaction.rollback().await?;
        return Ok(OrderOutcome::LockBusy);
    };
    observer.users_locked(locked.order_id()).await;
    let rows = locked.lock_order(&mut transaction, window).await?;
    observer.order_locked(candidate.order_id).await;
    let mut created = 0;
    for row in rows {
        let message = reminder_message(&row.side, row.order_id, row.expires_at);
        let data = reminder_data(
            config,
            row.order_id,
            row.expires_at,
            (row.email_verified && row.email_enabled).then_some(row.email.as_str()),
            row.inapp_enabled,
            &message,
            window.reference,
        );
        sqlx::query(INSERT_REMINDER)
            .bind(Uuid::new_v4())
            .bind(row.user_id)
            .bind(&message)
            .bind(data.to_string())
            .bind(window.reference)
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
        // Plan 6.3: an ordinary job refuses while a deploy is in progress. The
        // unit's condition skips the start too; this check covers a manual run.
        require_unit_deploy_state_absent(environment)?;
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
