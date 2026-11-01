//! The consumer transaction and the one-time watermark bootstrap.
//!
//! This is the only producer file with SQL. It reads committed, sequenced
//! outbox rows, inserts derived watchlist events and moves the watermark in
//! one transaction. It never writes the outbox, never takes the sequencer
//! lock and never changes a target snapshot.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Connection, PgConnection, Row};
use uuid::Uuid;

use super::derive::{DerivedEvent, derive};
use super::evidence::{MAX_EVIDENCE_BYTES, TargetKind, decode};
use super::{ProducerActivation, ProducerError, RUST_PRODUCER};
use crate::order_write::locks::{MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT};

/// At most this many outbox rows enter one transaction.
pub const MAX_SOURCES_PER_BATCH: i64 = 100;
/// A batch stops adding sources before it passes this many derived rows. One
/// source is never split, so a single larger source runs alone.
pub const MAX_ROWS_PER_BATCH: usize = 5_000;
/// One insert statement carries at most this many rows, so a large source
/// fits the statement deadline; all statements share the batch transaction.
pub const INSERT_CHUNK_ROWS: usize = 1_000;

/// What one committed batch did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Outbox rows the watermark now covers that it did not before.
    pub sources: usize,
    /// Watchlist event rows inserted (duplicates excluded).
    pub inserted: u64,
    pub watermark: i64,
    /// True when no further sequenced row waited at the time of the read.
    pub drained: bool,
}

/// What the explicit bootstrap found or wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapOutcome {
    Created { watermark: i64 },
    AlreadyPresent { watermark: i64 },
}

struct SourceRow {
    stream_seq: i64,
    trusted: bool,
    stored_bytes: i64,
    evidence: Option<Value>,
}

struct PendingEvent {
    source: Uuid,
    stream_seq: i64,
    attested_at: DateTime<Utc>,
    event: DerivedEvent,
}

async fn bounded(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    for statement in [
        format!("SET LOCAL lock_timeout = '{MARKET_LOCK_TIMEOUT}'"),
        format!("SET LOCAL statement_timeout = '{MARKET_STATEMENT_TIMEOUT}'"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// The retention fence must stay `blocked` and the replay floor must not pass
/// the watermark. Either failure stops production with no write.
async fn require_coverage(conn: &mut PgConnection, watermark: i64) -> Result<(), ProducerError> {
    let row = sqlx::query(
        "SELECT (SELECT mode FROM market_event_retention_control WHERE singleton) AS mode, \
         (SELECT discarded_through_seq FROM market_event_replay_state WHERE singleton) AS floor",
    )
    .fetch_one(&mut *conn)
    .await?;
    let mode: Option<String> = row.try_get("mode")?;
    let floor: Option<i64> = row.try_get("floor")?;
    if mode.as_deref() != Some("blocked") {
        return Err(ProducerError::RetentionNotBlocked);
    }
    match floor {
        Some(floor) if floor <= watermark => Ok(()),
        _ => Err(ProducerError::CoverageLost),
    }
}

/// Consume one bounded batch in one transaction.
///
/// The singleton watermark row is locked first, so concurrent consumers
/// serialize and each re-reads the committed watermark. A visible gap in the
/// sequence is a rolled-back allocation of the single sequencer and is passed;
/// a replay floor above the watermark is lost coverage and stops the batch.
pub async fn consume_batch(
    conn: &mut PgConnection,
    _activation: &ProducerActivation,
) -> Result<BatchOutcome, ProducerError> {
    let mut tx = conn
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await?;
    bounded(&mut tx).await?;
    let watermark: i64 = sqlx::query_scalar(
        "SELECT consumed_through_seq FROM watchlist_producer_state WHERE singleton FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ProducerError::WatermarkMissing)?;
    require_coverage(&mut tx, watermark).await?;
    let rows = sqlx::query(
        "SELECT stream_seq, producer_evidence IS NOT NULL AS trusted, \
         COALESCE(octet_length(producer_evidence::text), 0)::bigint AS stored_bytes, \
         CASE WHEN octet_length(producer_evidence::text) <= $2 THEN producer_evidence END AS evidence \
         FROM market_event_outbox WHERE stream_seq > $1 ORDER BY stream_seq LIMIT $3",
    )
    .bind(watermark)
    .bind(MAX_EVIDENCE_BYTES)
    .bind(MAX_SOURCES_PER_BATCH)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        Ok(SourceRow {
            stream_seq: row.try_get("stream_seq")?,
            trusted: row.try_get("trusted")?,
            stored_bytes: row.try_get("stored_bytes")?,
            evidence: row.try_get("evidence")?,
        })
    })
    .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let fetched = rows.len();
    let mut covered = watermark;
    let mut processed = 0usize;
    // One mutation may append several outbox rows with the same envelope; the
    // first row in sequence order is the source row of its derived events.
    let mut mutations: BTreeSet<Uuid> = BTreeSet::new();
    let mut pending: Vec<PendingEvent> = Vec::new();
    for row in rows {
        if row.trusted {
            let source = match decode(row.stored_bytes, row.evidence.as_ref()) {
                Ok(source) => source,
                Err(error) if processed == 0 => {
                    return Err(ProducerError::InvalidEvidence {
                        stream_seq: row.stream_seq,
                        reason: error.reason(),
                    });
                }
                // Commit the valid prefix; the next batch stops at this row.
                Err(_) => break,
            };
            if mutations.insert(source.mutation_id) {
                let events = derive(&source);
                if !pending.is_empty() && pending.len() + events.len() > MAX_ROWS_PER_BATCH {
                    break;
                }
                pending.extend(events.into_iter().map(|event| PendingEvent {
                    source: source.mutation_id,
                    stream_seq: row.stream_seq,
                    attested_at: source.captured_at,
                    event,
                }));
            }
        }
        // A row without evidence is another writer's: covered, nothing derived.
        covered = row.stream_seq;
        processed += 1;
    }
    // A retention delete that committed while the outbox was read is visible
    // to this new statement: read the fence and the floor again before writing.
    require_coverage(&mut tx, watermark).await?;
    let inserted = insert_events(&mut tx, pending).await?;
    if covered > watermark {
        sqlx::query(
            "UPDATE watchlist_producer_state SET consumed_through_seq = $1, updated_at = now() \
             WHERE singleton AND consumed_through_seq < $1",
        )
        .bind(covered)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(BatchOutcome {
        sources: processed,
        inserted,
        watermark: covered,
        drained: processed == fetched
            && i64::try_from(fetched).unwrap_or(i64::MAX) < MAX_SOURCES_PER_BATCH,
    })
}

/// Lock every captured target in ascending id order, keep the events whose
/// target still exists with its captured identity, then insert them.
async fn insert_events(
    conn: &mut PgConnection,
    pending: Vec<PendingEvent>,
) -> Result<u64, ProducerError> {
    if pending.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = pending
        .iter()
        .map(|pending| pending.event.target.target_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let current = sqlx::query(
        "SELECT id, watchlist_id, target_type, order_id, market_product_code, delivery_point_id, \
         availability_window_code FROM watchlist_targets WHERE id = ANY($1) ORDER BY id FOR KEY SHARE",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    let mut present = BTreeMap::new();
    for row in current {
        let id: Uuid = row.try_get("id")?;
        present.insert(id, row);
    }
    let mut target_ids = Vec::new();
    let mut classes = Vec::new();
    let mut payloads = Vec::new();
    let mut sources = Vec::new();
    let mut sequences = Vec::new();
    let mut instants = Vec::new();
    let mut keys = Vec::new();
    for pending in pending {
        let captured = &pending.event.target;
        let Some(row) = present.get(&captured.target_id) else {
            // Deleted since capture: never substituted, never an error.
            continue;
        };
        let watchlist: Uuid = row.try_get("watchlist_id")?;
        let kind: String = row.try_get("target_type")?;
        let same_identity = watchlist == captured.watchlist_id
            && match captured.kind {
                // A PIN's market codes follow its order's display; its order does not move.
                TargetKind::Pin => {
                    kind == "PIN"
                        && row.try_get::<Option<Uuid>, _>("order_id")? == captured.order_id
                }
                TargetKind::Slice => {
                    kind == "SLICE"
                        && row
                            .try_get::<Option<String>, _>("market_product_code")?
                            .as_deref()
                            == Some(captured.slice.market_product.as_str())
                        && row.try_get::<Option<Uuid>, _>("delivery_point_id")?
                            == Some(captured.slice.delivery_point_id)
                        && row
                            .try_get::<Option<String>, _>("availability_window_code")?
                            .as_deref()
                            == Some(captured.slice.availability_window.as_str())
                }
            };
        if !same_identity {
            continue;
        }
        target_ids.push(captured.target_id);
        classes.push(pending.event.class.as_str().to_owned());
        payloads.push(Value::Object(pending.event.payload).to_string());
        sources.push(pending.source);
        sequences.push(pending.stream_seq);
        instants.push(pending.attested_at);
        keys.push(pending.event.key);
    }
    if target_ids.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0;
    for start in (0..target_ids.len()).step_by(INSERT_CHUNK_ROWS) {
        let rows = start..(start + INSERT_CHUNK_ROWS).min(target_ids.len());
        let result = sqlx::query(
            "INSERT INTO watchlist_events (id, watchlist_id, watchlist_target_id, event_type, \
             event_payload, is_read, created_at, producer, source_event_id, source_stream_seq, \
             scope_attested_at, derived_event_key) \
             SELECT gen_random_uuid(), t.watchlist_id, t.id, d.class, d.payload::json, false, d.at, \
             $8, d.source, d.seq, d.at, d.key \
             FROM unnest($1::uuid[], $2::text[], $3::text[], $4::uuid[], $5::bigint[], \
             $6::timestamptz[], $7::text[]) AS d(target_id, class, payload, source, seq, at, key) \
             JOIN watchlist_targets t ON t.id = d.target_id \
             ON CONFLICT (producer, source_event_id, watchlist_target_id, derived_event_key) \
             WHERE source_event_id IS NOT NULL DO NOTHING",
        )
        .bind(&target_ids[rows.clone()])
        .bind(&classes[rows.clone()])
        .bind(&payloads[rows.clone()])
        .bind(&sources[rows.clone()])
        .bind(&sequences[rows.clone()])
        .bind(&instants[rows.clone()])
        .bind(&keys[rows])
        .bind(RUST_PRODUCER)
        .execute(&mut *conn)
        .await?;
        inserted += result.rows_affected();
    }
    Ok(inserted)
}

/// Create the watermark once, at the sequenced high-water barrier, before any
/// trusted source exists. Legacy history below the barrier is never derived.
/// An existing watermark is returned unchanged; a retained trusted source, or
/// a retention fence that is not `blocked`, refuses with no write.
pub async fn bootstrap_watermark(
    conn: &mut PgConnection,
) -> Result<BootstrapOutcome, ProducerError> {
    let mut tx = conn
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await?;
    bounded(&mut tx).await?;
    if let Some(watermark) = sqlx::query_scalar::<_, i64>(
        "SELECT consumed_through_seq FROM watchlist_producer_state WHERE singleton FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        return Ok(BootstrapOutcome::AlreadyPresent { watermark });
    }
    let row = sqlx::query(
        "SELECT (SELECT mode FROM market_event_retention_control WHERE singleton) AS mode, \
         (SELECT discarded_through_seq FROM market_event_replay_state WHERE singleton) AS floor, \
         (SELECT COALESCE(max(stream_seq), 0) FROM market_event_outbox) AS high_water, \
         EXISTS (SELECT 1 FROM market_event_outbox WHERE producer_evidence IS NOT NULL) AS trusted",
    )
    .fetch_one(&mut *tx)
    .await?;
    let mode: Option<String> = row.try_get("mode")?;
    if mode.as_deref() != Some("blocked") {
        return Err(ProducerError::RetentionNotBlocked);
    }
    if row.try_get::<bool, _>("trusted")? {
        return Err(ProducerError::TrustedEvidenceExists);
    }
    let floor: i64 = row
        .try_get::<Option<i64>, _>("floor")?
        .ok_or(ProducerError::CoverageLost)?;
    let barrier = floor.max(row.try_get("high_water")?);
    let created: Option<i64> = sqlx::query_scalar(
        "INSERT INTO watchlist_producer_state (singleton, consumed_through_seq, updated_at) \
         VALUES (true, $1, now()) ON CONFLICT (singleton) DO NOTHING RETURNING consumed_through_seq",
    )
    .bind(barrier)
    .fetch_optional(&mut *tx)
    .await?;
    let outcome = match created {
        Some(watermark) => BootstrapOutcome::Created { watermark },
        // A concurrent bootstrap won; report its committed value.
        None => BootstrapOutcome::AlreadyPresent {
            watermark: sqlx::query_scalar(
                "SELECT consumed_through_seq FROM watchlist_producer_state WHERE singleton",
            )
            .fetch_one(&mut *tx)
            .await?,
        },
    };
    tx.commit().await?;
    Ok(outcome)
}
