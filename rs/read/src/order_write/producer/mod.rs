//! The trusted watchlist producer.
//!
//! The economic writer appends an internal evidence envelope to each outbox
//! row in its own transaction (`effects.rs`). This consumer reads committed,
//! sequenced rows in `stream_seq` order, derives each watchlist event from that
//! immutable evidence, and inserts the events together with its watermark in
//! one transaction. A restart resumes after the watermark; the producer
//! identity index makes a repeated source harmless.
//!
//! Source identity: one mutation can append several outbox rows that carry the
//! same envelope. `source_event_id` is the envelope's mutation id and
//! `source_stream_seq` the first row of that mutation the consumer read, so
//! the identity index removes the repeats.
//!
//! Activation: the consumer runs only when the release metadata file selects
//! the Rust producer and records the activation instant, in the same file as
//! the verified `RELEASE_SHA`. There is no default and no clock fallback.
//! Selecting the legacy producer again pauses consumption: evidence stays in
//! the outbox after the unchanged watermark, and reactivation drains it in order.
//!
//! The watermark row is created only by [`bootstrap_watermark`], an explicit
//! operator step; [`run_producer`] never creates it.

mod consume;
pub(crate) mod derive;
pub(crate) mod evidence;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub use consume::{BatchOutcome, BootstrapOutcome, bootstrap_watermark};

/// The producer identity of trusted rows.
pub const RUST_PRODUCER: &str = "rust-market-v1";
/// The producer identity of rows the unchanged legacy writers insert.
pub const LEGACY_PRODUCER: &str = "legacy-python";
/// The bounded poll that covers a missed or absent wake signal.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// The longest wait between retries after consecutive failed batches.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// The wait after `failures` consecutive failed batches: the poll interval,
/// doubled per further failure, at most [`MAX_RETRY_DELAY`]. A failure never
/// ends the consumer; it retries from the unchanged watermark.
pub fn retry_delay(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(5);
    POLL_INTERVAL
        .saturating_mul(1 << doublings)
        .min(MAX_RETRY_DELAY)
}
/// The first schema revision that carries the producer columns and watermark.
pub const PRODUCER_REVISION: &str = "p89_20261022_watchlist_producer";

/// Why the consumer is not active. Each reason is a fixed text for one log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationRefusal {
    ProducerMissing,
    LegacyProducer,
    UnknownProducer,
    ActivatedAtMissing,
    ActivatedAtInvalid,
    ReleaseInvalid,
    SchemaTooOld,
}

impl ActivationRefusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::ProducerMissing => "producer_not_selected",
            Self::LegacyProducer => "legacy_producer_selected",
            Self::UnknownProducer => "producer_unknown",
            Self::ActivatedAtMissing => "activation_instant_missing",
            Self::ActivatedAtInvalid => "activation_instant_invalid",
            Self::ReleaseInvalid => "release_identity_invalid",
            Self::SchemaTooOld => "producer_schema_absent",
        }
    }
}

/// Proof that the release metadata activates the trusted producer. The
/// consumer transaction requires one, so an inactive configuration cannot run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProducerActivation {
    activated_at: DateTime<Utc>,
    release_sha: String,
}

impl ProducerActivation {
    /// Read the two release keys recorded beside `release_sha`.
    pub fn from_release(
        producer: Option<&str>,
        activated_at: Option<&str>,
        release_sha: &str,
    ) -> Result<Self, ActivationRefusal> {
        match producer.map(str::trim) {
            None | Some("") => return Err(ActivationRefusal::ProducerMissing),
            Some(LEGACY_PRODUCER) => return Err(ActivationRefusal::LegacyProducer),
            Some(RUST_PRODUCER) => {}
            Some(_) => return Err(ActivationRefusal::UnknownProducer),
        }
        let recorded = activated_at
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ActivationRefusal::ActivatedAtMissing)?;
        let activated_at = DateTime::parse_from_rfc3339(recorded)
            .map_err(|_| ActivationRefusal::ActivatedAtInvalid)?
            .with_timezone(&Utc);
        if release_sha.len() != 40
            || !release_sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ActivationRefusal::ReleaseInvalid);
        }
        Ok(Self {
            activated_at,
            release_sha: release_sha.to_owned(),
        })
    }

    /// The serving configuration's release keys and schema checkpoint.
    pub fn from_config(config: &crate::Config) -> Result<Self, ActivationRefusal> {
        let activation = Self::from_release(
            config.phase2.watchlist_event_producer.as_deref(),
            config.phase2.watchlist_producer_activated_at.as_deref(),
            &config.release_sha,
        )?;
        if !schema_has_producer(&config.migration_revision) {
            return Err(ActivationRefusal::SchemaTooOld);
        }
        Ok(activation)
    }

    pub fn activated_at(&self) -> DateTime<Utc> {
        self.activated_at
    }

    pub fn release_sha(&self) -> &str {
        &self.release_sha
    }
}

/// True when the database revision is the producer checkpoint or a later one.
pub fn schema_has_producer(revision: &str) -> bool {
    let order = crate::runtime::artifact::ALLOWED_REVISIONS;
    let position = |wanted: &str| order.iter().position(|known| *known == wanted);
    matches!(
        (position(PRODUCER_REVISION), position(revision)),
        (Some(producer), Some(current)) if current >= producer
    )
}

/// Why one batch did not run or stopped. No variant carries row data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProducerError {
    /// The watermark row does not exist; the operator bootstrap has not run.
    WatermarkMissing,
    /// The replay floor is above the watermark: evidence the producer never read is gone.
    CoverageLost,
    /// The outbox retention fence is not `blocked`.
    RetentionNotBlocked,
    /// A trusted envelope cannot be read; consumption stops at this row.
    InvalidEvidence {
        stream_seq: i64,
        reason: &'static str,
    },
    /// Bootstrap refused: a trusted source already exists.
    TrustedEvidenceExists,
    /// Lock, serialization, deadlock or timeout contention; the next poll retries.
    Busy,
    Database,
}

impl ProducerError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::WatermarkMissing => "watermark_missing",
            Self::CoverageLost => "replay_floor_above_watermark",
            Self::RetentionNotBlocked => "retention_not_blocked",
            Self::InvalidEvidence { reason, .. } => reason,
            Self::TrustedEvidenceExists => "trusted_evidence_exists",
            Self::Busy => "busy",
            Self::Database => "database_error",
        }
    }
}

impl From<sqlx::Error> for ProducerError {
    fn from(error: sqlx::Error) -> Self {
        let code = error
            .as_database_error()
            .and_then(|error| error.code())
            .map(|code| code.into_owned());
        match code.as_deref() {
            Some("40001" | "40P01" | "55P03" | "57014") => Self::Busy,
            _ => Self::Database,
        }
    }
}

/// Consume until `stop` turns true: drain every waiting batch, then wait for a
/// wake signal or the poll interval. A failed batch writes nothing and moves
/// no watermark; it is logged once per change of reason and retried after
/// [`retry_delay`], so contention or a statement deadline never ends the
/// consumer.
pub async fn run_producer(
    pool: sqlx::PgPool,
    activation: ProducerActivation,
    wake: Arc<Notify>,
    mut stop: watch::Receiver<bool>,
) {
    let mut reported: Option<&'static str> = None;
    let mut failures: u32 = 0;
    loop {
        if *stop.borrow() {
            return;
        }
        let result = match pool.acquire().await {
            Ok(mut connection) => consume::consume_batch(&mut connection, &activation).await,
            Err(error) => Err(error.into()),
        };
        // `None`: run the next batch at once; `Some`: wait this long first.
        let wait = match result {
            Ok(outcome) => {
                failures = 0;
                if reported.take().is_some() {
                    tracing::info!(watermark = outcome.watermark, "watchlist producer resumed");
                }
                outcome.drained.then_some(POLL_INTERVAL)
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if reported != Some(error.reason()) {
                    reported = Some(error.reason());
                    match error {
                        ProducerError::InvalidEvidence { stream_seq, .. } => tracing::warn!(
                            reason = error.reason(),
                            stream_seq,
                            "watchlist producer waiting"
                        ),
                        _ => tracing::warn!(reason = error.reason(), "watchlist producer waiting"),
                    }
                }
                Some(retry_delay(failures))
            }
        };
        let Some(wait) = wait else {
            continue;
        };
        // After a failure the wake signal does not shorten the backoff.
        let backing_off = failures > 0;
        tokio::select! {
            biased;
            changed = stop.changed() => {
                // A dropped sender also means stop.
                if changed.is_err() {
                    return;
                }
            }
            () = wake.notified(), if !backing_off => {}
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// The one-shot operator bootstrap behind
/// `verdaxis-read --bootstrap-watchlist-producer --budget-file <path>`.
///
/// It refuses before the producer checkpoint, opens the deployed application
/// pool (the same attestation as a serving start), runs
/// [`bootstrap_watermark`] once and closes the pool. A refusal is one fixed
/// reason text; no variant carries row data.
pub async fn bootstrap_command(config: &crate::Config) -> Result<BootstrapOutcome, &'static str> {
    if !schema_has_producer(&config.migration_revision) {
        return Err(ActivationRefusal::SchemaTooOld.reason());
    }
    let pools = crate::runtime::db::connect(config)
        .await
        .map_err(|_| "database_unavailable")?;
    let outcome = match pools.read.acquire().await {
        Ok(mut connection) => bootstrap_watermark(&mut connection)
            .await
            .map_err(ProducerError::reason),
        Err(_) => Err("database_unavailable"),
    };
    pools.read.close().await;
    pools.health.close().await;
    outcome
}

impl BootstrapOutcome {
    /// The one line the operator command prints.
    pub fn report(&self) -> String {
        match self {
            Self::Created { watermark } => format!("outcome=created watermark={watermark}"),
            Self::AlreadyPresent { watermark } => {
                format!("outcome=already_present watermark={watermark}")
            }
        }
    }
}

/// The running consumer of the serving process.
pub struct ProducerTask {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl ProducerTask {
    /// Stop after the current batch commits or rolls back.
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        let _ = self.task.await;
    }
}

/// Start the consumer when the release activates it. `wake` is the stream
/// dispatcher's committed-assignment signal when streams run; without it the
/// poll alone bounds the delay. No new database connection is opened.
pub fn start(state: &crate::AppState, wake: Option<Arc<Notify>>) -> Option<ProducerTask> {
    let activation = match ProducerActivation::from_config(&state.config) {
        Ok(activation) => activation,
        Err(refusal) => {
            tracing::info!(reason = refusal.reason(), "watchlist producer inactive");
            return None;
        }
    };
    tracing::info!(
        activated_at = %activation.activated_at(),
        "watchlist producer active"
    );
    let (stop, receiver) = watch::channel(false);
    let task = tokio::spawn(run_producer(
        state.pools.read.clone(),
        activation,
        wake.unwrap_or_default(),
        receiver,
    ));
    Some(ProducerTask { stop, task })
}

/// Feature-only ports for the producer test targets.
#[cfg(feature = "contract-tests")]
pub mod contract {
    pub use super::consume::{
        BatchOutcome, BootstrapOutcome, INSERT_CHUNK_ROWS, MAX_ROWS_PER_BATCH,
        MAX_SOURCES_PER_BATCH, bootstrap_watermark, consume_batch,
    };
    pub use super::derive::{DerivedEvent, EventClass, derive, number, price_move_is_material};
    pub use super::evidence::{
        CapturedTarget, EvidenceError, MAX_EVIDENCE_BYTES, MAX_ORDERS, OrderEvidence, OrderStatus,
        OrderTransition, Side, SideAggregate, SliceIdentity, SourceEvidence, TargetKind, decode,
    };
    pub use super::{
        ActivationRefusal, LEGACY_PRODUCER, ProducerActivation, ProducerError, RUST_PRODUCER,
        retry_delay, run_producer, schema_has_producer,
    };
}
