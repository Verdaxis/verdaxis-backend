//! Bounded read-only access to persisted market events.
//!
//! Two short read-only snapshots support private replay:
//!
//! - [`replay_boundary`] captures the durable coverage barrier and the current
//!   assigned high-water mark in one `REPEATABLE READ` transaction.
//! - [`replay_page`] returns one participant-scoped page together with the
//!   barrier from the same snapshot, so a caller can detect a delete that
//!   committed after the previous page.
//!
//! Persisted rows are validated from metadata before their JSON payload is
//! decoded. An oversized or malformed row is reported as
//! [`StoreError::InvalidEvent`]; it is never truncated, skipped or echoed.
//! No database connection is held while a caller waits on a client.

use std::time::Duration;

use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub use super::bus::StoredEvent;
pub use super::policy::{
    MAX_EVENT_BYTES as STREAM_EVENT_PAYLOAD_BYTES, MAX_PAGE_BYTES as STREAM_PAGE_PAYLOAD_BYTES,
    MAX_PAGE_ROWS as STREAM_PAGE_ROWS, MAX_PARTICIPANTS as STREAM_EVENT_PARTICIPANTS,
};

/// Stream statement bound in milliseconds.
const STREAM_STATEMENT_TIMEOUT_MS: u64 = 5_000;
/// Stream lock bound in milliseconds.
const STREAM_LOCK_TIMEOUT_MS: u64 = 500;
/// Connection acquisition cannot use the pool's longer REST deadline.
const STREAM_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
/// Canonical UUID strings plus JSON separators fit within this per-participant
/// budget. Bound raw text too, before decoding malformed stored values.
const PARTICIPANT_JSON_BYTES: i64 = 40;

const FLOOR_SQL: &str = "SELECT discarded_through_seq FROM public.market_event_replay_state";
const HIGH_WATER_SQL: &str =
    "SELECT COALESCE(MAX(stream_seq), 0)::bigint FROM public.market_event_outbox";
const METADATA_SQL: &str = r#"
SELECT stream_seq,
       octet_length(payload::text)::bigint AS payload_bytes,
       CASE
           WHEN json_typeof(participant_org_ids) = 'array'
            AND octet_length(participant_org_ids::text) <= $5
           THEN json_array_length(participant_org_ids)::bigint
           ELSE 0::bigint
       END AS participant_count
FROM public.market_event_outbox
WHERE stream_seq > $1
  AND stream_seq <= $2
  AND ($3::text IS NULL OR (participant_org_ids::jsonb) ? $3)
ORDER BY stream_seq
LIMIT $4
"#;
const PAGE_SQL: &str = r#"
SELECT stream_seq,
       event_type,
       octet_length(payload::text)::bigint AS payload_bytes,
       participant_org_ids::text AS participants,
       payload
FROM public.market_event_outbox
WHERE stream_seq = ANY($1)
ORDER BY stream_seq
"#;

/// Durable coverage barrier and the assigned high-water mark of one snapshot.
///
/// A cursor below `discarded_through` cannot prove complete coverage. A cursor
/// equal to it may replay rows above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBoundary {
    pub discarded_through: i64,
    pub high_water: i64,
}

/// One bounded replay page and the barrier observed in the same snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayPage {
    pub boundary: ReplayBoundary,
    pub events: Vec<StoredEvent>,
    pub more: bool,
}

/// Metadata fetched before any payload is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMetadata {
    pub seq: i64,
    pub payload_bytes: i64,
    pub participants: i64,
}

/// Page bounds. Values outside the documented ceilings are refused rather than
/// replaced with unbounded defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageLimits {
    pub rows: usize,
    pub payload_bytes: usize,
    pub payload_bytes_per_event: usize,
    pub participants: usize,
}

impl PageLimits {
    pub const DEFAULT: Self = Self {
        rows: STREAM_PAGE_ROWS,
        payload_bytes: STREAM_PAGE_PAYLOAD_BYTES,
        payload_bytes_per_event: STREAM_EVENT_PAYLOAD_BYTES,
        participants: STREAM_EVENT_PARTICIPANTS,
    };

    /// Refuse a page request that is unbounded or internally inconsistent.
    ///
    /// The per-event ceiling may not exceed the page budget: otherwise a single
    /// valid event could fill no page, and the caller would see an empty page
    /// that still reports more rows. Requiring
    /// `payload_bytes_per_event <= payload_bytes` keeps every non-empty page
    /// able to make progress.
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.rows == 0
            || self.rows > STREAM_PAGE_ROWS
            || self.payload_bytes == 0
            || self.payload_bytes > STREAM_PAGE_PAYLOAD_BYTES
            || self.payload_bytes_per_event == 0
            || self.payload_bytes_per_event > self.payload_bytes
            || self.payload_bytes_per_event > STREAM_EVENT_PAYLOAD_BYTES
            || self.participants == 0
            || self.participants > STREAM_EVENT_PARTICIPANTS
        {
            return Err(StoreError::InvalidPageRequest);
        }
        Ok(())
    }
}

impl Default for PageLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Bounded store failure classes. Database text and payload bytes never leave
/// this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// The durable barrier is absent or duplicated, or the database is
    /// unreachable. The stream subsystem must report itself unavailable.
    Unavailable,
    /// One persisted row exceeds a documented bound or is malformed.
    InvalidEvent(i64),
    /// The caller asked for a page outside the documented bounds.
    InvalidPageRequest,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => write!(formatter, "replay state unavailable"),
            Self::InvalidEvent(seq) => write!(formatter, "invalid persisted event {seq}"),
            Self::InvalidPageRequest => write!(formatter, "invalid replay page request"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Interpret the barrier singleton. Exactly one non-negative row is required;
/// a missing, duplicated or negative value fails closed.
pub fn floor_from_rows(rows: &[i64]) -> Result<i64, StoreError> {
    match rows {
        [floor] if *floor >= 0 => Ok(*floor),
        _ => Err(StoreError::Unavailable),
    }
}

/// A cursor at the barrier may replay rows above it.
pub fn cursor_is_retained(cursor: i64, boundary: ReplayBoundary) -> bool {
    cursor >= boundary.discarded_through
}

/// Choose the sequence numbers of one page from bounded metadata.
///
/// A row outside the per-event bounds is invalid, not skippable. The returned
/// flag reports whether more retained rows remain after this page.
pub fn plan_page(
    rows: &[EventMetadata],
    limits: PageLimits,
) -> Result<(Vec<i64>, bool), StoreError> {
    limits.validate()?;
    let mut included = Vec::new();
    let mut total_bytes = 0usize;
    let mut more = false;

    for row in rows {
        if row.payload_bytes < 0
            || row.payload_bytes as usize > limits.payload_bytes_per_event
            || row.participants <= 0
            || row.participants as usize > limits.participants
        {
            return Err(StoreError::InvalidEvent(row.seq));
        }
        if included.len() >= limits.rows {
            more = true;
            break;
        }
        let bytes = row.payload_bytes as usize;
        if total_bytes.saturating_add(bytes) > limits.payload_bytes {
            more = true;
            break;
        }
        total_bytes += bytes;
        included.push(row.seq);
    }

    Ok((included, more))
}

/// Decode the participant array. Writers store a JSON array of UUID strings.
pub fn decode_participants(seq: i64, raw: &str, maximum: usize) -> Result<Vec<Uuid>, StoreError> {
    let parsed: Value = serde_json::from_str(raw).map_err(|_| StoreError::InvalidEvent(seq))?;
    let values = parsed.as_array().ok_or(StoreError::InvalidEvent(seq))?;
    if values.is_empty() || values.len() > maximum {
        return Err(StoreError::InvalidEvent(seq));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or(StoreError::InvalidEvent(seq))
        })
        .collect()
}

/// Capture the durable barrier and the assigned high-water mark together.
pub async fn replay_boundary(pool: &PgPool) -> Result<ReplayBoundary, StoreError> {
    let mut transaction = begin_snapshot(pool).await?;
    let discarded_through = read_floor(&mut transaction).await?;
    let high_water: i64 = sqlx::query_scalar(HIGH_WATER_SQL)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| StoreError::Unavailable)?;
    transaction
        .commit()
        .await
        .map_err(|_| StoreError::Unavailable)?;
    Ok(ReplayBoundary {
        discarded_through,
        // Deleting the last retained row must not make a current cursor look
        // ahead of the stream. The durable barrier is also assigned history.
        high_water: high_water.max(discarded_through),
    })
}

/// Read one participant-scoped page plus the barrier from the same snapshot.
///
/// `through` is the high-water mark captured when the subscription registered.
pub async fn replay_page(
    pool: &PgPool,
    organization: Uuid,
    after: i64,
    through: i64,
    limits: PageLimits,
) -> Result<ReplayPage, StoreError> {
    read_page(pool, Some(organization), after, Some(through), limits).await
}

/// Read a global dispatch page and its coverage boundary in one snapshot.
pub async fn dispatch_page(pool: &PgPool, after: i64) -> Result<ReplayPage, StoreError> {
    read_page(pool, None, after, None, PageLimits::default()).await
}

async fn read_page(
    pool: &PgPool,
    organization: Option<Uuid>,
    after: i64,
    through: Option<i64>,
    limits: PageLimits,
) -> Result<ReplayPage, StoreError> {
    limits.validate()?;
    if after < 0 || through.is_some_and(|through| through < after) {
        return Err(StoreError::InvalidPageRequest);
    }

    let mut transaction = begin_snapshot(pool).await?;
    let discarded_through = read_floor(&mut transaction).await?;
    let through = match through {
        Some(through) => through,
        None => sqlx::query_scalar::<_, i64>(HIGH_WATER_SQL)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| StoreError::Unavailable)?
            .max(discarded_through),
    };

    let metadata_rows = sqlx::query(METADATA_SQL)
        .bind(after)
        .bind(through)
        .bind(organization.map(|organization| organization.to_string()))
        .bind((limits.rows + 1) as i64)
        .bind(limits.participants as i64 * PARTICIPANT_JSON_BYTES + 2)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| StoreError::Unavailable)?;
    let metadata = metadata_rows
        .iter()
        .map(read_metadata)
        .collect::<Result<Vec<_>, StoreError>>()?;
    let (sequences, more) = plan_page(&metadata, limits)?;

    let events = if sequences.is_empty() {
        Vec::new()
    } else {
        let rows = sqlx::query(PAGE_SQL)
            .bind(&sequences)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|_| StoreError::Unavailable)?;
        if rows.len() != sequences.len() {
            return Err(StoreError::Unavailable);
        }
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            events.push(read_event(row, limits.participants)?);
        }
        if events
            .iter()
            .map(|event| event.seq())
            .ne(sequences.iter().copied())
        {
            return Err(StoreError::Unavailable);
        }
        events
    };

    transaction
        .commit()
        .await
        .map_err(|_| StoreError::Unavailable)?;
    Ok(ReplayPage {
        boundary: ReplayBoundary {
            discarded_through,
            high_water: through,
        },
        events,
        more,
    })
}

async fn begin_snapshot(pool: &PgPool) -> Result<Transaction<'_, Postgres>, StoreError> {
    let mut transaction = tokio::time::timeout(STREAM_ACQUIRE_TIMEOUT, pool.begin())
        .await
        .map_err(|_| StoreError::Unavailable)?
        .map_err(|_| StoreError::Unavailable)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(|_| StoreError::Unavailable)?;
    // Both values are compile-time constants, so the interpolated statements
    // carry no caller-controlled text. A bind parameter is not accepted in
    // SET, hence the explicit audit marker used elsewhere in this workspace.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = {STREAM_STATEMENT_TIMEOUT_MS}"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|_| StoreError::Unavailable)?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL lock_timeout = {STREAM_LOCK_TIMEOUT_MS}"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|_| StoreError::Unavailable)?;
    Ok(transaction)
}

async fn read_floor(transaction: &mut Transaction<'_, Postgres>) -> Result<i64, StoreError> {
    let rows: Vec<i64> = sqlx::query_scalar(FLOOR_SQL)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| StoreError::Unavailable)?;
    floor_from_rows(&rows)
}

fn read_metadata(row: &PgRow) -> Result<EventMetadata, StoreError> {
    Ok(EventMetadata {
        seq: row
            .try_get("stream_seq")
            .map_err(|_| StoreError::Unavailable)?,
        payload_bytes: row
            .try_get("payload_bytes")
            .map_err(|_| StoreError::Unavailable)?,
        participants: row
            .try_get("participant_count")
            .map_err(|_| StoreError::Unavailable)?,
    })
}

fn read_event(row: &PgRow, maximum_participants: usize) -> Result<StoredEvent, StoreError> {
    let seq: i64 = row
        .try_get("stream_seq")
        .map_err(|_| StoreError::Unavailable)?;
    let event_type: String = row
        .try_get("event_type")
        .map_err(|_| StoreError::Unavailable)?;
    if event_type.is_empty()
        || event_type.len() > 80
        || !event_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(StoreError::InvalidEvent(seq));
    }
    let participants_raw: String = row
        .try_get("participants")
        .map_err(|_| StoreError::Unavailable)?;
    let payload: Value = row
        .try_get("payload")
        .map_err(|_| StoreError::Unavailable)?;
    let payload_bytes: i64 = row
        .try_get("payload_bytes")
        .map_err(|_| StoreError::Unavailable)?;
    let payload_bytes =
        usize::try_from(payload_bytes).map_err(|_| StoreError::InvalidEvent(seq))?;
    StoredEvent::from_persisted(
        seq,
        decode_participants(seq, &participants_raw, maximum_participants)?,
        &event_type,
        payload,
        payload_bytes,
    )
    .map_err(|_| StoreError::InvalidEvent(seq))
}
