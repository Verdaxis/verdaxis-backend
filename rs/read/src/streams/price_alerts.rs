//! Price-alert evaluation over committed market events.
//!
//! An alert fires when a real trade executes through its threshold. The
//! evaluator reads the committed event stream; it never changes how events or
//! trades are written.
//!
//! # Which events count
//!
//! `trade_auto_matched` (a matched order pair, confirmed on creation) and
//! `trade_confirmed` (a counterparty confirmed a pending trade) are the two
//! moments a trade becomes an executed price. `trade_created` is only a
//! request, and `trade_delivered` / `trade_paid` repeat a price that an earlier
//! confirmation already carried. The event payload does not carry the product
//! and delivery point reliably, so the evaluator reads the trade row by id:
//! the row is the source of truth for the price, the slice and the provenance.
//!
//! # Which prints count
//!
//! The same evidence rule as the public trade tape and benchmarks: a formal
//! snapshot (`market_snapshot_version = 1`, status confirmed, delivered or
//! paid) whose buyer and seller are both `REAL`. DEMO, TEST, CANARY, unknown
//! and mixed pairs never fire an alert.
//!
//! # Once per alert, across workers
//!
//! Every worker of the service runs a dispatcher, so every worker sees every
//! event. The evaluator is idempotent instead of elected: candidate alerts are
//! locked in id order, then deactivated with a conditional `UPDATE ... WHERE
//! is_active RETURNING`. A second worker waits for the first one's commit, finds
//! no active row and does nothing. The notification rows and the outbox event
//! are written in the same transaction as the deactivation, so they exist
//! exactly once for each alert, and a failed transaction leaves the alert
//! active for the next attempt.
//!
//! # Never in the dispatch path
//!
//! The sink wrapper only offers a trade id to a bounded queue with
//! `try_send`. A separate task does the database work. A slow or failing
//! evaluation cannot delay delivery of other events or move the dispatcher's
//! cursor. A full queue drops the offer with a warning; the next qualifying
//! print evaluates the alert again, because the threshold test is a property of
//! the alert and the price, not of the event.
//!
//! # No loops
//!
//! The evaluator reacts only to the two trade event types above. The
//! `price_alert_triggered` event it appends is therefore never evaluated.

use std::{collections::VecDeque, future::Future, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, PgPool, Row};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use super::{
    bus::{HubError, PublicChannel, StoredEvent, Terminal},
    dispatch::DispatchSink,
    election::WAKE_CHANNEL,
};
use crate::market::provenance::{FORMAL_TRADE_STATUSES, MarketEvidenceScope};

/// Event types that carry a newly executed trade price.
pub const EXECUTION_EVENT_TYPES: [&str; 2] = ["trade_auto_matched", "trade_confirmed"];

/// The event this module appends for each triggered alert.
pub const TRIGGERED_EVENT_TYPE: &str = "price_alert_triggered";

/// Waiting trade ids between the dispatcher and the evaluator task.
const QUEUE_CAPACITY: usize = 1024;
/// Failed evaluations kept for another attempt.
const RETRY_CAPACITY: usize = 64;
/// Evaluations of one trade before it is given up.
const MAX_ATTEMPTS: u8 = 3;
/// Pause before a retry when no new event arrives.
const RETRY_DELAY: Duration = Duration::from_secs(5);

const TRADE_WRITE_TRANSACTION: &str = "BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE";

const PRINT_SQL: &str = "\
SELECT product_id, delivery_point_id, price_per_mt_usd::text AS price,
       product_name, delivery_point_name
FROM trades
WHERE id = $1
  AND status = ANY($2)
  AND confirmed_at IS NOT NULL
  AND market_snapshot_version = 1
  AND buyer_provenance = $3
  AND seller_provenance = $3
  AND product_id IS NOT NULL
  AND delivery_point_id IS NOT NULL";

/// Locks the alerts this price crosses, in id order, so that two workers never
/// wait for each other's rows in a different order. A row changed by the other
/// worker while this statement waited is checked again and drops out.
const CANDIDATES_SQL: &str = "\
SELECT id
FROM price_alerts
WHERE is_active
  AND product_id = $1
  AND (delivery_point_id IS NULL OR delivery_point_id = $2)
  AND ((direction = 'above' AND $3::text::numeric > threshold_usd)
    OR (direction = 'below' AND $3::text::numeric < threshold_usd))
ORDER BY id
FOR UPDATE";

/// The conditional update that makes an alert fire at most once.
const DEACTIVATE_SQL: &str = "\
UPDATE price_alerts
SET is_active = false, triggered_at = $2
WHERE id = ANY($1) AND is_active
RETURNING id, org_id, direction, threshold_usd::text AS threshold_usd";

const NOTIFY_MEMBERS_SQL: &str = "\
INSERT INTO notifications (id, recipient_id, type, title, message, data, is_read, created_at)
SELECT gen_random_uuid(), member.id, 'SYSTEM', $2, $3, CAST($4 AS json), false, $5
FROM users AS member
WHERE member.organization_id = $1 AND member.status = 'APPROVED'";

const APPEND_EVENT_SQL: &str = "\
INSERT INTO market_event_outbox (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload)
VALUES ($1, $2, 'price_alert', $3, $4, $5)";

const WAKE_SQL: &str = "SELECT pg_notify($1, '')";

/// A bounded failure class. It never carries database text or row data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvaluationError;

/// The executed price an evaluation compared against the alerts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedPrint {
    pub product_id: Uuid,
    pub delivery_point_id: Uuid,
    /// Exact decimal text of the trade price.
    pub price: String,
    pub product_name: Option<String>,
    pub delivery_point_name: Option<String>,
}

/// One alert this evaluation deactivated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggeredAlert {
    pub alert_id: Uuid,
    pub organization_id: Uuid,
    pub direction: String,
    /// Exact decimal text of the alert threshold.
    pub threshold_usd: String,
}

/// The trade an execution event refers to, or `None` for any other event.
///
/// `trade_auto_matched` names it `trade_id`; `trade_confirmed` names it `id`.
/// A payload that says it is not real-only market evidence is skipped here, so
/// demo activity does not cost a database read. The trade row is checked again
/// in the database: the payload only saves work and never grants a trigger.
pub fn executed_trade_id(event: &StoredEvent) -> Option<Uuid> {
    if !EXECUTION_EVENT_TYPES.contains(&event.event_type()) {
        return None;
    }
    let payload = event.payload();
    if payload
        .get("demo_status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "REAL_ONLY")
    {
        return None;
    }
    let key = if event.event_type() == "trade_auto_matched" {
        "trade_id"
    } else {
        "id"
    };
    payload.get(key)?.as_str()?.parse().ok()
}

/// The sending side of the evaluation queue.
#[derive(Clone)]
pub struct PriceAlertFeed {
    sender: mpsc::Sender<Uuid>,
}

impl PriceAlertFeed {
    /// A feed and the receiver an evaluator task drains.
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<Uuid>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }

    /// Offer an event to the evaluator. Never waits.
    pub fn offer(&self, event: &StoredEvent) {
        let Some(trade_id) = executed_trade_id(event) else {
            return;
        };
        match self.sender.try_send(trade_id) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => tracing::warn!(
                reason = "price_alert_queue_full",
                "price alert evaluation queue is full; the next qualifying print evaluates the alerts again"
            ),
            // The evaluator stopped with the runtime.
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

/// Delivers every event to the wrapped sink, and offers executions to the feed.
pub struct PriceAlertSink<S> {
    inner: S,
    feed: PriceAlertFeed,
}

impl<S> PriceAlertSink<S> {
    pub fn new(inner: S, feed: PriceAlertFeed) -> Self {
        Self { inner, feed }
    }
}

impl<S: DispatchSink> DispatchSink for PriceAlertSink<S> {
    fn private(&mut self, event: Arc<StoredEvent>) -> Result<(), HubError> {
        self.feed.offer(&event);
        self.inner.private(event)
    }

    fn public(&mut self, channel: PublicChannel) -> Result<(), HubError> {
        self.inner.public(channel)
    }

    fn terminate(&mut self, terminal: Terminal) {
        self.inner.terminate(terminal);
    }
}

/// Compare one executed trade with the active alerts and fire those it crosses.
///
/// Returns the alerts this call deactivated. A concurrent call for the same
/// trade waits for this one and then returns an empty list. All writes happen
/// in one transaction: on any error nothing is changed and the alerts stay
/// active.
pub async fn evaluate_trade(
    pool: &PgPool,
    trade_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<TriggeredAlert>, EvaluationError> {
    let mut connection = pool.acquire().await.map_err(|_| EvaluationError)?;
    // The service pools default to read-only sessions; this transaction asks
    // for write access explicitly.
    let mut transaction = connection
        .begin_with(TRADE_WRITE_TRANSACTION)
        .await
        .map_err(|_| EvaluationError)?;

    let statuses: Vec<String> = FORMAL_TRADE_STATUSES
        .iter()
        .map(|status| (*status).to_owned())
        .collect();
    let print = sqlx::query(PRINT_SQL)
        .bind(trade_id)
        .bind(&statuses)
        .bind(MarketEvidenceScope::Real.provenance())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| EvaluationError)?;
    let Some(print) = print else {
        transaction.commit().await.map_err(|_| EvaluationError)?;
        return Ok(Vec::new());
    };
    let print = ExecutedPrint {
        product_id: print.try_get("product_id").map_err(|_| EvaluationError)?,
        delivery_point_id: print
            .try_get("delivery_point_id")
            .map_err(|_| EvaluationError)?,
        price: print.try_get("price").map_err(|_| EvaluationError)?,
        product_name: print.try_get("product_name").map_err(|_| EvaluationError)?,
        delivery_point_name: print
            .try_get("delivery_point_name")
            .map_err(|_| EvaluationError)?,
    };

    let candidates: Vec<Uuid> = sqlx::query_scalar(CANDIDATES_SQL)
        .bind(print.product_id)
        .bind(print.delivery_point_id)
        .bind(&print.price)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| EvaluationError)?;
    if candidates.is_empty() {
        transaction.commit().await.map_err(|_| EvaluationError)?;
        return Ok(Vec::new());
    }

    let rows = sqlx::query(DEACTIVATE_SQL)
        .bind(&candidates)
        .bind(now)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| EvaluationError)?;
    let mut triggered = Vec::with_capacity(rows.len());
    for row in rows {
        triggered.push(TriggeredAlert {
            alert_id: row.try_get("id").map_err(|_| EvaluationError)?,
            organization_id: row.try_get("org_id").map_err(|_| EvaluationError)?,
            direction: row.try_get("direction").map_err(|_| EvaluationError)?,
            threshold_usd: row.try_get("threshold_usd").map_err(|_| EvaluationError)?,
        });
    }
    // Keep the order of delivery stable for a reader of the outbox.
    triggered.sort_by_key(|alert| alert.alert_id);

    for alert in &triggered {
        record_trigger(&mut transaction, alert, &print, now).await?;
    }
    if !triggered.is_empty() {
        sqlx::query(WAKE_SQL)
            .bind(WAKE_CHANNEL)
            .execute(&mut *transaction)
            .await
            .map_err(|_| EvaluationError)?;
    }
    transaction.commit().await.map_err(|_| EvaluationError)?;
    Ok(triggered)
}

/// The notification rows and the outbox event of one triggered alert.
async fn record_trigger(
    transaction: &mut sqlx::PgConnection,
    alert: &TriggeredAlert,
    print: &ExecutedPrint,
    now: DateTime<Utc>,
) -> Result<(), EvaluationError> {
    let event_payload = triggered_payload(alert, print);
    let (title, message) = notification_text(alert, print);
    let mut data = event_payload.clone();
    data["type"] = json!(TRIGGERED_EVENT_TYPE);

    sqlx::query(NOTIFY_MEMBERS_SQL)
        .bind(alert.organization_id)
        .bind(title)
        .bind(message)
        .bind(data)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| EvaluationError)?;
    sqlx::query(APPEND_EVENT_SQL)
        .bind(Uuid::new_v4())
        .bind(TRIGGERED_EVENT_TYPE)
        .bind(alert.alert_id.to_string())
        .bind(json!([alert.organization_id]))
        .bind(event_payload)
        .execute(&mut *transaction)
        .await
        .map_err(|_| EvaluationError)?;
    Ok(())
}

/// The org-private event body. `delivery_point_id` is the point where the
/// price executed, which differs from the alert's own point for an alert that
/// watches every point.
pub fn triggered_payload(alert: &TriggeredAlert, print: &ExecutedPrint) -> Value {
    json!({
        "schema_version": 1,
        "alert_id": alert.alert_id,
        "product_id": print.product_id,
        "delivery_point_id": print.delivery_point_id,
        "direction": alert.direction,
        "threshold_usd": alert.threshold_usd,
        "price": print.price,
        "product_name": print.product_name,
        "delivery_point_name": print.delivery_point_name,
    })
}

/// A display price with cents. The exact text stays in the event payload.
fn display_amount(text: &str) -> String {
    text.parse::<Decimal>()
        .map(|amount| format!("{amount:.2}"))
        .unwrap_or_else(|_| text.to_owned())
}

fn notification_text(alert: &TriggeredAlert, print: &ExecutedPrint) -> (String, String) {
    let slice = match (&print.product_name, &print.delivery_point_name) {
        (Some(product), Some(point)) => format!("{product} at {point}"),
        (Some(product), None) => product.clone(),
        _ => "Your watched market".to_owned(),
    };
    let side = if alert.direction == "above" {
        "above"
    } else {
        "below"
    };
    (
        "Price alert triggered".to_owned(),
        format!(
            "{slice} traded at ${}/MT, {side} your ${} alert.",
            display_amount(&print.price),
            display_amount(&alert.threshold_usd),
        ),
    )
}

/// The database work the worker needs. A trait so that the retry policy is
/// testable without a database.
pub trait AlertEvaluator: Send {
    /// Number of alerts this evaluation fired.
    fn evaluate(
        &mut self,
        trade_id: Uuid,
    ) -> impl Future<Output = Result<usize, EvaluationError>> + Send;
}

/// Evaluates trades against the database.
pub struct PgAlertEvaluator {
    pool: PgPool,
}

impl PgAlertEvaluator {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl AlertEvaluator for PgAlertEvaluator {
    async fn evaluate(&mut self, trade_id: Uuid) -> Result<usize, EvaluationError> {
        evaluate_trade(&self.pool, trade_id, Utc::now())
            .await
            .map(|triggered| triggered.len())
    }
}

/// Failed trades waiting for another attempt.
#[derive(Default)]
struct RetryQueue {
    waiting: VecDeque<(Uuid, u8)>,
}

impl RetryQueue {
    fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    fn take(&mut self) -> Vec<(Uuid, u8)> {
        self.waiting.drain(..).collect()
    }

    /// Keep a failed trade unless it used its attempts. The oldest entry makes
    /// room when the queue is full.
    fn keep(&mut self, trade_id: Uuid, attempts: u8) {
        if attempts >= MAX_ATTEMPTS {
            tracing::error!(
                reason = "price_alert_evaluation_abandoned",
                %trade_id,
                "price alert evaluation used all attempts"
            );
            return;
        }
        if self.waiting.len() >= RETRY_CAPACITY {
            self.waiting.pop_front();
            tracing::warn!(
                reason = "price_alert_retry_overflow",
                "oldest failed price alert evaluation dropped"
            );
        }
        self.waiting.push_back((trade_id, attempts));
    }
}

async fn attempt(
    evaluator: &mut impl AlertEvaluator,
    retry: &mut RetryQueue,
    trade_id: Uuid,
    attempts: u8,
) {
    match evaluator.evaluate(trade_id).await {
        Ok(0) => {}
        Ok(fired) => tracing::info!(
            reason = "price_alerts_triggered",
            %trade_id,
            fired,
            "price alerts triggered"
        ),
        Err(EvaluationError) => {
            tracing::warn!(
                reason = "price_alert_evaluation_failed",
                %trade_id,
                "price alert evaluation failed"
            );
            retry.keep(trade_id, attempts + 1);
        }
    }
}

/// Drain the queue until it closes or shutdown is requested. A failed trade is
/// tried again with the next event, or after a short pause when none comes.
pub async fn run_worker(
    mut evaluator: impl AlertEvaluator,
    mut queue: mpsc::Receiver<Uuid>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut retry = RetryQueue::default();
    loop {
        let next = tokio::select! {
            biased;
            _ = shutdown.wait_for(|stop| *stop) => return,
            offered = queue.recv() => match offered {
                Some(trade_id) => Some(trade_id),
                None => return,
            },
            () = tokio::time::sleep(RETRY_DELAY), if !retry.is_empty() => None,
        };
        for (trade_id, attempts) in retry.take() {
            attempt(&mut evaluator, &mut retry, trade_id, attempts).await;
        }
        if let Some(trade_id) = next {
            attempt(&mut evaluator, &mut retry, trade_id, 0).await;
        }
    }
}

/// Start the evaluator for the serving process. It stops with `shutdown`.
pub fn spawn(
    pool: PgPool,
    shutdown: watch::Receiver<bool>,
) -> (PriceAlertFeed, tokio::task::JoinHandle<()>) {
    let (feed, queue) = PriceAlertFeed::channel(QUEUE_CAPACITY);
    let task = tokio::spawn(run_worker(PgAlertEvaluator::new(pool), queue, shutdown));
    (feed, task)
}
