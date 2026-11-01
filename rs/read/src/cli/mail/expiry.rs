//! Order-expiry reminder mail: the notification and lease contract.
//!
//! The reminder scan (`cli::jobs::reminders`) freezes each provider payload
//! into the reminder's `notifications.data` when it creates the reminder. This
//! module delivers those frozen payloads in three separate steps, so no order
//! or user row lock is ever held across the provider call:
//!
//! 1. claim: one transaction locks one due reminder with its recipient's user
//!    row (user first, `SKIP LOCKED`), then the write boundary locks the order
//!    row and rechecks ownership, address, preference and expiry
//!    (`order_write::lock_reminder_delivery`); the claim then either discards
//!    the reminder or writes a 15-minute lease (`_email_retry_at`);
//! 2. send: the provider call, with the notification UUID as the idempotency
//!    key, outside every transaction;
//! 3. finish: one transaction marks an accepted reminder `sent`.
//!
//! A refused or interrupted send keeps the lease; a later run claims the same
//! reminder again after the lease and sends the identical payload under the
//! same key, until the deadline (`min(expiry, creation + 23 h)`) passes. The
//! provider keeps idempotency keys for 24 hours, so a retry never sends twice.
//!
//! The JSON keys and their meaning are shared with the Python implementation
//! (`app/services/order_expiry_reminders.py:147-236`), which may run against
//! the same rows during the switch. Number fields in `notifications.data` are
//! compared and rewritten by PostgreSQL only (its float parsing and printing
//! are exact), never parsed and printed again here.
use super::{Delivery, MailLog};
use crate::cli::{Exit, retry_bounded};
use crate::crud::referrals::invite::MailClient;
use crate::order_write::{ReminderWindow, lock_reminder_delivery};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::future::Future;
use uuid::Uuid;

/// The `event` value that marks an order-expiry reminder.
pub const EVENT: &str = "order_expiring";
/// The longest delivery batch per run (Python `deliver_expiry_reminder_emails`).
pub const MAX_BATCH: u32 = 20;
/// How long one claim keeps other workers away from a reminder.
pub const LEASE: Duration = Duration::minutes(15);
/// Retry window after creation. The provider keeps idempotency keys for 24
/// hours; stopping one hour sooner never replays an uncertain send.
pub const RETRY_WINDOW: Duration = Duration::hours(23);
/// The lead time before expiry inside which an order gets a reminder.
pub const LEAD_TIME: Duration = Duration::hours(48);

/// Python's session defaults (`app/config.py` `DB_LOCK_TIMEOUT_MS`,
/// `DB_STATEMENT_TIMEOUT_MS`), set on every reminder transaction.
pub(crate) const TRANSACTION_BOUNDS: &str =
    "SET LOCAL lock_timeout = '3000ms'; SET LOCAL statement_timeout = '30000ms'";

const SUPPORT: &str = "admin@verdaxis.exchange";
const DEFAULT_SENDER: &str = "Verdaxis <noreply@verdaxis.exchange>";
const DEFAULT_FRONTEND: &str = "https://app.verdaxis.exchange";

/// Rendering settings, with the Python defaults (`app/config.py:443-444`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiryMailConfig {
    pub sender: String,
    pub frontend: String,
}

impl Default for ExpiryMailConfig {
    fn default() -> Self {
        Self {
            sender: DEFAULT_SENDER.into(),
            frontend: DEFAULT_FRONTEND.into(),
        }
    }
}

impl ExpiryMailConfig {
    /// Read `EMAIL_FROM` and `FRONTEND_URL` as Python does; an absent or empty
    /// value keeps the default.
    pub fn from_environment() -> Self {
        let read = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.to_owned())
        };
        Self {
            sender: read("EMAIL_FROM", DEFAULT_SENDER),
            frontend: read("FRONTEND_URL", DEFAULT_FRONTEND),
        }
    }
}

/// Seconds since the Unix epoch, as the Python reminder stores them.
///
/// Python's `datetime.timestamp()` divides the exact microsecond count by
/// 10^6 with correct rounding. Every microsecond count of this era is exact
/// in an `f64` and IEEE division is correctly rounded, so this is the same
/// double, and PostgreSQL's `EXTRACT(epoch ...)` cast to `float8` rounds the
/// same exact value to the same double.
pub fn epoch_seconds(instant: DateTime<Utc>) -> f64 {
    instant.timestamp_micros() as f64 / 1_000_000.0
}

/// The provider idempotency key: the notification UUID (Python
/// `send_order_expiry_email`, `app/services/email.py:356-361`).
pub fn idempotency_key(notification_id: Uuid) -> String {
    format!("order-expiry/{notification_id}")
}

/// Python's `html.escape(value, quote=True)`.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// The frozen provider payload (Python `build_order_expiry_email_payload`,
/// `app/services/email.py:337-353`). Keys keep Python's order.
pub fn render_payload(config: &ExpiryMailConfig, recipient: &str, message: &str) -> Value {
    let url = format!("{}/app/home", config.frontend.trim_end_matches('/'));
    let values = [
        ("TITLE", escape("Your order is nearing expiry.")),
        ("PREVIEW", escape(message)),
        ("LABEL", escape("ORDER EXPIRY REMINDER")),
        ("BODY", format!("<p>{}</p>", escape(message))),
        ("URL", escape(&url)),
        ("ACTION", escape("Review your orders")),
        (
            "NOTE",
            escape("Your order will expire normally unless you take action."),
        ),
        ("SUPPORT", SUPPORT.to_owned()),
    ];
    // One replacement pass: message text cannot introduce another token.
    let mut html = String::new();
    let mut rest = include_str!("expiry.html");
    while let Some((prefix, tail)) = rest.split_once("{{") {
        html.push_str(prefix);
        let (key, suffix) = tail.split_once("}}").expect("static email template");
        let value = values
            .iter()
            .find(|(name, _)| *name == key)
            .expect("static email token");
        html.push_str(&value.1);
        rest = suffix;
    }
    html.push_str(rest);
    json!({
        "from": config.sender,
        "reply_to": SUPPORT,
        "to": [recipient],
        "subject": "Your Verdaxis order expires within 48 hours",
        "html": html,
    })
}

/// The payload the provider receives (Python `_canonical_email_payload`,
/// `app/services/email.py:50-74`): exactly one recipient, string fields, and
/// no field beyond `from`, `to`, `subject`, `html` and an optional `reply_to`.
pub fn canonical_payload(payload: &Value) -> Option<Value> {
    let object = payload.as_object()?;
    let from = object.get("from")?.as_str()?;
    let recipients = object.get("to")?.as_array()?;
    let [recipient] = recipients.as_slice() else {
        return None;
    };
    let recipient = recipient.as_str()?;
    let subject = object.get("subject")?.as_str()?;
    let html = object.get("html")?.as_str()?;
    let mut canonical = Map::new();
    canonical.insert("from".into(), from.into());
    canonical.insert("to".into(), json!([recipient]));
    canonical.insert("subject".into(), subject.into());
    canonical.insert("html".into(), html.into());
    if let Some(reply_to) = object.get("reply_to") {
        canonical.insert("reply_to".into(), reply_to.as_str()?.into());
    }
    Some(Value::Object(canonical))
}

/// One provider call for a claimed reminder.
pub trait ExpirySender: Sync {
    fn send(&self, payload: &Value, idempotency_key: &str)
    -> impl Future<Output = Delivery> + Send;
}

/// The production sender: the Phase 2 [`MailClient`] with the idempotency key.
pub struct ProviderSender {
    client: MailClient,
}

impl ProviderSender {
    pub fn new(client: MailClient) -> Self {
        Self { client }
    }

    /// The Resend key comes from `RESEND_API_KEY`, as in Python. Without a key
    /// every send is refused, so reminders keep their lease until the deadline.
    pub fn from_environment() -> Result<Self, Exit> {
        let key = std::env::var("RESEND_API_KEY")
            .ok()
            .filter(|key| !key.is_empty());
        MailClient::production(key)
            .map(Self::new)
            .map_err(|_| Exit::Dependency)
    }
}

impl ExpirySender for ProviderSender {
    async fn send(&self, payload: &Value, idempotency_key: &str) -> Delivery {
        let Some(canonical) = canonical_payload(payload) else {
            return Delivery::Refused;
        };
        let accepted = self
            .client
            .send_with_idempotency(&canonical, Some(idempotency_key))
            .await;
        let delivery = if accepted {
            Delivery::Sent
        } else {
            Delivery::Refused
        };
        let recipient = canonical["to"][0].as_str().unwrap_or_default();
        MailLog::new(recipient, delivery).record();
        delivery
    }
}

/// What one delivery run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// The provider accepted the payload and the reminder is marked `sent`.
    pub sent: u64,
    /// The provider refused; the lease stays and a later run retries.
    pub deferred: u64,
    /// The reminder was discarded: deadline passed, no payload, or the
    /// owner, address, preference or expiry changed.
    pub discarded: u64,
}

enum Claim {
    Nothing,
    Discarded,
    Claimed { id: Uuid, payload: Value },
}

/// Deliver at most `batch` due reminders, one claim at a time.
///
/// `clock` is read once per claim, as Python reads the time once per loop.
pub async fn deliver<S, C>(pool: &PgPool, sender: &S, clock: &C, batch: u32) -> Result<Counts, Exit>
where
    S: ExpirySender,
    C: Fn() -> DateTime<Utc> + Sync,
{
    if !(1..=MAX_BATCH).contains(&batch) {
        return Err(Exit::Usage);
    }
    let mut counts = Counts::default();
    for _ in 0..batch {
        match claim(pool, clock()).await? {
            Claim::Nothing => break,
            Claim::Discarded => counts.discarded += 1,
            Claim::Claimed { id, payload } => {
                match sender.send(&payload, &idempotency_key(id)).await {
                    Delivery::Sent => {
                        finish_sent(pool, id).await?;
                        counts.sent += 1;
                    }
                    Delivery::Refused => counts.deferred += 1,
                }
            }
        }
    }
    Ok(counts)
}

/// One due reminder with its recipient's user row. The user row is locked
/// before the reminder row, the order the notification routes use (actor user
/// row, then that user's notification rows); with `SKIP LOCKED` neither lock
/// waits, so a busy user or reminder is left for a later run.
const DUE_REMINDER: &str = "SELECT n.id AS id, n.data ->> 'order_id' AS order_id, \
     json_typeof(n.data -> '_email_payload') = 'object' AS has_payload, \
     (n.data -> '_email_payload')::text AS payload, \
     COALESCE(CAST(n.data ->> '_email_deadline' AS float8) <= $2, true) AS past_deadline \
     FROM notifications n JOIN users u ON u.id = n.recipient_id \
     WHERE (n.data ->> 'event') = $1 \
       AND (n.data ->> '_email_status') = 'pending' \
       AND CAST(n.data ->> '_email_retry_at' AS float8) <= $2 \
     ORDER BY n.created_at, n.id LIMIT 1 FOR UPDATE OF u, n SKIP LOCKED";

const WRITE_LEASE: &str = "UPDATE notifications SET data = CAST(CAST(data AS jsonb) \
     || jsonb_build_object('_email_retry_at', CAST($2 AS float8)) AS json) WHERE id = $1";

/// Python `_finish_email`: set the terminal status and drop the delivery keys.
const FINISH: &str = "UPDATE notifications SET data = CAST((CAST(data AS jsonb) \
     - '_email_payload' - '_email_retry_at' - '_email_deadline') \
     || jsonb_build_object('_email_status', CAST($2 AS text)) AS json) WHERE id = $1";

async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>, Exit> {
    let mut transaction = pool.begin().await.map_err(|_| Exit::Dependency)?;
    sqlx::raw_sql(TRANSACTION_BOUNDS)
        .execute(&mut *transaction)
        .await
        .map_err(|_| Exit::Dependency)?;
    Ok(transaction)
}

async fn finish(
    transaction: &mut Transaction<'static, Postgres>,
    id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(FINISH)
        .bind(id)
        .bind(status)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// Database errors a reminder transaction may meet from a concurrent writer:
/// deadlock, serialization failure and lock timeout.
const RETRYABLE_SQLSTATES: [&str; 3] = ["40P01", "40001", "55P03"];

/// Split a database error into retryable (outer `Err`) and final (`Ok(Exit)`),
/// the shape [`retry_bounded`] retries on.
pub(crate) fn classify(error: sqlx::Error) -> Result<Exit, sqlx::Error> {
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    match code {
        Some(code) if RETRYABLE_SQLSTATES.contains(&code.as_str()) => Err(error),
        _ => Ok(Exit::Dependency),
    }
}

/// Claim one due reminder, retrying a deadlock, serialization failure or
/// lock timeout with the shared bounded policy.
async fn claim(pool: &PgPool, reference: DateTime<Utc>) -> Result<Claim, Exit> {
    retry_bounded(|| async {
        match claim_once(pool, reference).await {
            Ok(claim) => Ok(Ok(claim)),
            Err(error) => classify(error).map(Err),
        }
    })
    .await
    .map_err(|_| Exit::Dependency)?
}

/// One claim transaction (Python `deliver_expiry_reminder_emails` loop body,
/// `app/services/order_expiry_reminders.py:168-226`): the recipient's user
/// row and the reminder row, then the order row through the write boundary.
///
/// A reminder whose stored order id or delivery fields are malformed is
/// discarded, where Python would stop the whole run on it every time.
async fn claim_once(pool: &PgPool, reference: DateTime<Utc>) -> Result<Claim, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(TRANSACTION_BOUNDS)
        .execute(&mut *transaction)
        .await?;
    let Some(row) = sqlx::query(DUE_REMINDER)
        .bind(EVENT)
        .bind(epoch_seconds(reference))
        .fetch_optional(&mut *transaction)
        .await?
    else {
        transaction.commit().await?;
        return Ok(Claim::Nothing);
    };
    let id: Uuid = row.try_get("id")?;
    let has_payload: Option<bool> = row.try_get("has_payload")?;
    let past_deadline: bool = row.try_get("past_deadline")?;
    let order_id: Option<String> = row.try_get("order_id")?;
    let payload: Option<String> = row.try_get("payload")?;
    let payload = payload.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let order_id = order_id.and_then(|text| Uuid::parse_str(&text).ok());

    let (Some(order_id), Some(payload), Some(true), false) =
        (order_id, payload, has_payload, past_deadline)
    else {
        finish(&mut transaction, id, "discarded").await?;
        transaction.commit().await?;
        return Ok(Claim::Discarded);
    };

    let window = ReminderWindow {
        reference,
        horizon: reference + LEAD_TIME,
    };
    let email = lock_reminder_delivery(&mut transaction, id, order_id, window).await?;
    let addressed = |email: &str| payload.get("to") == Some(&json!([email]));
    if !email.as_deref().is_some_and(addressed) {
        finish(&mut transaction, id, "discarded").await?;
        transaction.commit().await?;
        return Ok(Claim::Discarded);
    }
    // The lease survives a crash: a retry keeps the same payload and key.
    sqlx::query(WRITE_LEASE)
        .bind(id)
        .bind(epoch_seconds(reference + LEASE))
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Claim::Claimed { id, payload })
}

/// Mark an accepted reminder `sent`. A failure here leaves the lease; the
/// retry after it reuses the same idempotency key, so the provider does not
/// send again.
async fn finish_sent(pool: &PgPool, id: Uuid) -> Result<(), Exit> {
    let mut transaction = begin(pool).await?;
    let present: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM notifications WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| Exit::Dependency)?;
    if present.is_some() {
        finish(&mut transaction, id, "sent")
            .await
            .map_err(|_| Exit::Dependency)?;
    }
    transaction.commit().await.map_err(|_| Exit::Dependency)
}
