//! W07 and W08: the watchlist event feed and its read state.
//!
//! Python is the event producer until the trusted Rust producer exists. This
//! module reads the rows it commits and applies the PD4-G read guard: a feed
//! that can carry a price signal must not expose an order outside the full
//! public order scope, so best-price events and events sourced from a hidden
//! order are filtered before `LIMIT`. That keeps pages full instead of
//! returning short pages whose hidden rows would have occupied the limit.
use super::http::{WatchlistState, not_found, run};
use super::reads::VisibleSnapshot;
use super::validation::{path_uuid, unprocessable};
use crate::ApiError;
use crate::runtime::http::{QueryFields, QueryKind, QueryValue};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

const DEFAULT_LIMIT: i64 = 20;
const MINIMUM_LIMIT: i64 = 1;
const MAXIMUM_LIMIT: i64 = 100;
const BEST_PRICE_EVENT: &str = "SLICE_BEST_PRICE_MOVED";

const SOURCE_KINDS: &[&str] = &[
    "CONFIRMED_TRADE",
    "LIVE_ORDER",
    "LIVE_INVENTORY",
    "DEMO_SEED",
    "BENCHMARK_REFERENCE",
    "MIXED_SOURCE",
    "NO_DATA",
    "UNKNOWN",
];
const SCOPES: &[&str] = &["DELIVERY_POINT", "REGION", "PRODUCT", "UNKNOWN"];
const DEMO_STATUSES: &[&str] = &[
    "REAL_ONLY",
    "DEMO_ONLY",
    "MIXED",
    "UNKNOWN",
    "NOT_APPLICABLE",
];

/// One feed cursor: the last row of the previous page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl EventCursor {
    pub fn new(created_at: DateTime<Utc>, id: Uuid) -> Self {
        Self { created_at, id }
    }
}

/// The validated query string. `cursor` is already decoded into its two parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventQuery {
    pub limit: i64,
    pub cursor: Option<EventCursor>,
}

/// The cursor spelling both stacks accept: RFC 3339 in UTC with microsecond
/// precision, then the row identifier. Postgres timestamps are microsecond
/// exact, so no precision is lost or invented.
pub fn encode_cursor(cursor: EventCursor) -> String {
    format!(
        "{}|{}",
        cursor
            .created_at
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Micros, false),
        cursor.id
    )
}

fn cursor_error() -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Value::String("Invalid watchlist event cursor".into()),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}

/// Parse a cursor. Python issues the same grammar, and while both stacks serve
/// a cursor from either stack must work: `datetime.fromisoformat` also accepts
/// `Z`, a numeric offset, a space separator and a naive timestamp, so accept
/// those too. Every deployed session runs in UTC, which makes a naive value
/// unambiguous.
pub fn decode_cursor(raw: &str) -> Result<EventCursor, ApiError> {
    let (timestamp, identifier) = raw.split_once('|').ok_or_else(cursor_error)?;
    let id = Uuid::parse_str(identifier).map_err(|_| cursor_error())?;
    let created_at = parse_cursor_instant(timestamp).ok_or_else(cursor_error)?;
    Ok(EventCursor::new(created_at, id))
}

fn parse_cursor_instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            let normalised = text.replacen(' ', "T", 1);
            NaiveDateTime::parse_from_str(&normalised, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|naive| Utc.from_utc_datetime(&naive))
                .ok()
        })
}

/// `cursor` is a plain string, so only `limit` has declared bounds.
pub fn parse_event_query(raw: &str) -> Result<EventQuery, ApiError> {
    let fields = QueryFields::parse(raw);
    let limit = match fields.validate(
        "limit",
        QueryKind::Integer {
            minimum: MINIMUM_LIMIT,
            maximum: MAXIMUM_LIMIT,
        },
        false,
    )? {
        Some(QueryValue::Integer(value)) => value,
        _ => DEFAULT_LIMIT,
    };
    let cursor = match fields.raw("cursor") {
        None | Some("") => None,
        Some(text) => Some(decode_cursor(text)?),
    };
    Ok(EventQuery { limit, cursor })
}

/// The PD4-G read guard.
///
/// Every best-price event is hidden: current rows carry no producer
/// attestation, so a public source order cannot vouch for the historical
/// aggregate prices in their payload. A PIN must point at a publicly visible
/// order. An event whose own payload names a source order is visible only when
/// that order is public. An event with no source order and a non-PIN target is
/// historical and stays visible.
pub fn event_visible(
    event_type: &str,
    source_order: Option<Uuid>,
    target: Option<&TargetColumns>,
    visible: &VisibleSnapshot,
) -> bool {
    if event_type == BEST_PRICE_EVENT {
        return false;
    }
    // A deleted target has no PIN context to check; W08 reports it as its own
    // missing-target case after committing the read.
    if let Some(target) = target
        && target.target_type == "PIN"
        && !target.order.is_some_and(|id| visible.contains(id))
    {
        return false;
    }
    if source_order.is_some_and(|id| !visible.contains(id)) {
        return false;
    }
    true
}

// ------------------------------------------------------------------ projection

/// The target columns an event response reads. `live_point_name` is null when
/// the point row is gone; the stored snapshot is the fallback.
#[derive(Debug, Clone)]
pub struct TargetColumns {
    pub target_type: String,
    pub product: Option<String>,
    pub point: Option<Uuid>,
    pub order: Option<Uuid>,
    pub window: Option<String>,
    pub snapshot_point_name: Option<String>,
    pub live_point_name: Option<String>,
}

/// One event row with the target columns and the guard input it needs.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub id: Uuid,
    pub watchlist_id: Uuid,
    pub target_id: Uuid,
    pub event_type: String,
    pub payload: Map<String, Value>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
    pub source_order: Option<Uuid>,
    pub target: Option<TargetColumns>,
}

/// The client contract for one event. Field names, nullability and types are
/// what `src/types.ts` and the MCP clients read; unknown payload enum values
/// fall back to the schema default instead of failing the page.
#[derive(Debug, Serialize)]
pub struct WatchlistEvent {
    pub id: Uuid,
    pub watchlist_id: Uuid,
    pub watchlist_target_id: Uuid,
    pub target_type: String,
    pub event_type: String,
    pub event_payload: Map<String, Value>,
    pub source_kind: String,
    pub scope: String,
    pub demo_status: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub market_product_code: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub delivery_point_name: Option<String>,
    pub availability_window_code: Option<String>,
    pub order_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct EventsPage {
    pub items: Vec<WatchlistEvent>,
    pub next_cursor: Option<String>,
}

/// A non-empty string payload value, the only form the string fields accept.
fn text<'a>(payload: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// An enum field: a member of the schema's values, else the default.
fn payload_enum(
    payload: &Map<String, Value>,
    field: &str,
    allowed: &[&str],
    default: &str,
) -> String {
    text(payload, field)
        .filter(|value| allowed.contains(value))
        .unwrap_or(default)
        .to_owned()
}

/// A UUID payload value; any other spelling falls through to the target.
fn payload_uuid(payload: &Map<String, Value>, field: &str) -> Option<Uuid> {
    text(payload, field).and_then(|value| Uuid::parse_str(value).ok())
}

/// `observed_at` as an ISO timestamp or Unix seconds; anything else is absent.
fn payload_instant(payload: &Map<String, Value>, field: &str) -> Option<DateTime<Utc>> {
    match payload.get(field)? {
        Value::String(value) => DateTime::parse_from_rfc3339(value)
            .map(|value| value.with_timezone(&Utc))
            .ok()
            .or_else(|| {
                NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                    .map(|naive| Utc.from_utc_datetime(&naive))
                    .ok()
            }),
        Value::Number(value) => value
            .as_i64()
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
        _ => None,
    }
}

impl StoredEvent {
    pub fn project(&self) -> Result<WatchlistEvent, ApiError> {
        let target = self.target.as_ref().ok_or_else(ApiError::database)?;
        let payload = &self.payload;
        let scope_default = if target.point.is_some() {
            "DELIVERY_POINT"
        } else {
            "UNKNOWN"
        };
        Ok(WatchlistEvent {
            id: self.id,
            watchlist_id: self.watchlist_id,
            watchlist_target_id: self.target_id,
            target_type: target.target_type.clone(),
            event_type: self.event_type.clone(),
            event_payload: payload.clone(),
            source_kind: payload_enum(payload, "source_kind", SOURCE_KINDS, "UNKNOWN"),
            scope: payload_enum(payload, "scope", SCOPES, scope_default),
            demo_status: payload_enum(payload, "demo_status", DEMO_STATUSES, "UNKNOWN"),
            observed_at: payload_instant(payload, "observed_at"),
            market_product_code: text(payload, "market_product_code")
                .map(str::to_owned)
                .or_else(|| target.product.clone()),
            delivery_point_id: payload_uuid(payload, "delivery_point_id").or(target.point),
            delivery_point_name: text(payload, "delivery_point_name")
                .map(str::to_owned)
                .or_else(|| target.live_point_name.clone())
                .or_else(|| target.snapshot_point_name.clone()),
            availability_window_code: text(payload, "availability_window_code")
                .map(str::to_owned)
                .or_else(|| target.window.clone()),
            order_id: payload_uuid(payload, "order_id").or(target.order),
            is_read: self.is_read,
            created_at: self.created_at,
        })
    }
}

// ----------------------------------------------------------------------- reads

// A source order is the payload `order_id` only when it is a valid UUID; the
// cast must not fail the feed, so `pg_input_is_valid` gates both uses. The
// guard runs inside the query, before `LIMIT`, so pages stay full.
const FEED_SQL: &str = concat!(
    "SELECT * FROM (",
    "SELECT e.id,e.watchlist_id,e.watchlist_target_id,",
    "e.event_type::text AS event_type,e.event_payload,e.is_read,e.created_at,",
    "t.target_type::text AS target_type,t.market_product_code AS target_product,",
    "t.delivery_point_id AS target_point,t.order_id AS target_order,",
    "t.availability_window_code AS target_window,",
    "t.snapshot_delivery_point_name AS snapshot_point_name,",
    "p.name AS live_point_name,",
    "CASE WHEN pg_input_is_valid(e.event_payload->>'order_id','uuid') ",
    "THEN (e.event_payload->>'order_id')::uuid END AS source_order ",
    "FROM watchlist_events e ",
    "JOIN watchlist_targets t ON t.id=e.watchlist_target_id ",
    "LEFT JOIN delivery_points p ON p.id=t.delivery_point_id ",
    "WHERE e.watchlist_id=$1) feed ",
    "WHERE event_type<>$2 ",
    "AND (target_type<>'PIN' OR target_order=ANY($3)) ",
    "AND (source_order IS NULL OR source_order=ANY($3)) ",
    "AND ($4::timestamptz IS NULL OR created_at<$4 ",
    "     OR (created_at=$4 AND id<$5)) ",
    "ORDER BY created_at DESC,id DESC LIMIT $6"
);

const SINGLE_SQL: &str = concat!(
    "SELECT e.id,e.watchlist_id,e.watchlist_target_id,",
    "e.event_type::text AS event_type,e.event_payload,e.is_read,e.created_at,",
    "t.target_type::text AS target_type,t.market_product_code AS target_product,",
    "t.delivery_point_id AS target_point,t.order_id AS target_order,",
    "t.availability_window_code AS target_window,",
    "t.snapshot_delivery_point_name AS snapshot_point_name,",
    "p.name AS live_point_name,",
    "CASE WHEN pg_input_is_valid(e.event_payload->>'order_id','uuid') ",
    "THEN (e.event_payload->>'order_id')::uuid END AS source_order ",
    "FROM watchlist_events e ",
    "LEFT JOIN watchlist_targets t ON t.id=e.watchlist_target_id ",
    "LEFT JOIN delivery_points p ON p.id=t.delivery_point_id ",
    "WHERE e.id=$1 AND e.watchlist_id=$2"
);

/// One row to `StoredEvent`. The target is absent only on the single-event
/// read, where the target row can have been deleted after the event.
fn stored(row: &sqlx::postgres::PgRow) -> Result<StoredEvent, sqlx::Error> {
    let target_type: Option<String> = row.try_get("target_type")?;
    let target = match target_type {
        Some(target_type) => Some(TargetColumns {
            target_type,
            product: row.try_get("target_product")?,
            point: row.try_get("target_point")?,
            order: row.try_get("target_order")?,
            window: row.try_get("target_window")?,
            snapshot_point_name: row.try_get("snapshot_point_name")?,
            live_point_name: row.try_get("live_point_name")?,
        }),
        None => None,
    };
    let payload: Value = row.try_get("event_payload")?;
    let payload = match payload {
        Value::Null => Map::new(),
        Value::Object(map) => map,
        _ => {
            return Err(sqlx::Error::Decode(
                "event payload is not a JSON object".into(),
            ));
        }
    };
    Ok(StoredEvent {
        id: row.try_get("id")?,
        watchlist_id: row.try_get("watchlist_id")?,
        target_id: row.try_get("watchlist_target_id")?,
        event_type: row.try_get("event_type")?,
        payload,
        is_read: row.try_get("is_read")?,
        created_at: row.try_get("created_at")?,
        source_order: row.try_get("source_order")?,
        target,
    })
}

/// One page of the container feed. The guard runs in SQL before `LIMIT`, so a
/// page is full whenever visible rows remain.
pub async fn feed_page(
    connection: &mut PgConnection,
    watchlist: Uuid,
    visible: &VisibleSnapshot,
    query: EventQuery,
) -> Result<EventsPage, ApiError> {
    let (after, after_id) = match query.cursor {
        Some(cursor) => (Some(cursor.created_at), Some(cursor.id)),
        None => (None, None),
    };
    let rows = sqlx::query(FEED_SQL)
        .bind(watchlist)
        .bind(BEST_PRICE_EVENT)
        .bind(visible.visible_ids())
        .bind(after)
        .bind(after_id)
        .bind(query.limit + 1)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| ApiError::database())?;
    let mut events = rows
        .iter()
        .map(stored)
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(|_| ApiError::database())?;
    let limit = usize::try_from(query.limit).map_err(|_| ApiError::database())?;
    let next_cursor = if events.len() > limit {
        events
            .get(limit - 1)
            .map(|event| encode_cursor(EventCursor::new(event.created_at, event.id)))
    } else {
        None
    };
    events.truncate(limit);
    let items = events
        .iter()
        .map(StoredEvent::project)
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(EventsPage { items, next_cursor })
}

async fn load_event(
    connection: &mut PgConnection,
    watchlist: Uuid,
    event_id: Uuid,
) -> Result<Option<StoredEvent>, ApiError> {
    let row = sqlx::query(SINGLE_SQL)
        .bind(event_id)
        .bind(watchlist)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|_| ApiError::database())?;
    row.as_ref()
        .map(stored)
        .transpose()
        .map_err(|_| ApiError::database())
}

/// Mark one event read.
///
/// A guarded event is refused before any write, so the read guard cannot be
/// bypassed with a write. An already-read event writes nothing. `None` means
/// the read was committed but the target row is gone: the route reports
/// `Event target not found` after the commit, as the route contract states.
pub async fn acknowledge(
    connection: &mut PgConnection,
    watchlist: Uuid,
    event_id: Uuid,
    visible: &VisibleSnapshot,
) -> Result<Option<WatchlistEvent>, ApiError> {
    let Some(event) = load_event(connection, watchlist, event_id).await? else {
        return Err(not_found("Event not found"));
    };
    if !event_visible_ws(&event, visible) {
        return Err(not_found("Event not found"));
    }
    if !event.is_read {
        // `AND is_read=false` re-checks the row after a concurrent reader's
        // lock is released: under READ COMMITTED the loser writes nothing.
        sqlx::query("UPDATE watchlist_events SET is_read=true WHERE id=$1 AND is_read=false")
            .bind(event_id)
            .execute(&mut *connection)
            .await
            .map_err(|_| ApiError::database())?;
    }
    if event.target.is_none() {
        return Ok(None);
    }
    let mut event = event;
    event.is_read = true;
    event.project().map(Some)
}

fn event_visible_ws(event: &StoredEvent, visible: &VisibleSnapshot) -> bool {
    event_visible(
        &event.event_type,
        event.source_order,
        event.target.as_ref(),
        visible,
    )
}

// -------------------------------------------------------------------- handlers

fn page_response(page: EventsPage) -> Response {
    Json(page).into_response()
}

/// W07: one page of the actor's own feed.
pub(super) async fn list(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let now = Utc::now();
    let policy = state.app.policy.clone();
    let config = state.app.config.clone();
    let raw_query = raw_query.unwrap_or_default();
    let result = run(
        &state,
        &headers,
        "GET",
        "/api/watchlists/{watchlist_id}/events",
        move |transaction| {
            Box::pin(async move {
                let watchlist =
                    path_uuid("watchlist_id", &raw).map_err(|item| unprocessable(vec![item]))?;
                let query = parse_event_query(&raw_query)?;
                let actor = transaction.actor().id;
                let connection = transaction.connection();
                let owns: Option<Uuid> =
                    sqlx::query_scalar("SELECT id FROM watchlists WHERE id=$1 AND user_id=$2")
                        .bind(watchlist)
                        .bind(actor)
                        .fetch_optional(&mut *connection)
                        .await
                        .map_err(|_| ApiError::database())?;
                if owns.is_none() {
                    return Err(not_found("Watchlist not found"));
                }
                let visible = super::capture_visible(
                    connection,
                    &policy,
                    &crate::market::visibility::VisibilityClock::at(now),
                    &config,
                )
                .await
                .map_err(|_| ApiError::database())?;
                feed_page(connection, watchlist, &visible, query).await
            })
        },
    )
    .await;
    match result {
        Ok(page) => page_response(page),
        Err(response) => response,
    }
}

/// W08: mark one event read.
pub(super) async fn mark_read(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path((container, event)): Path<(String, String)>,
) -> Response {
    let now = Utc::now();
    let policy = state.app.policy.clone();
    let config = state.app.config.clone();
    let result = run(
        &state,
        &headers,
        "PATCH",
        "/api/watchlists/{watchlist_id}/events/{event_id}",
        move |transaction| {
            Box::pin(async move {
                let watchlist = path_uuid("watchlist_id", &container);
                let event_id = path_uuid("event_id", &event);
                let errors: Vec<Value> = [watchlist.as_ref().err(), event_id.as_ref().err()]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                if !errors.is_empty() {
                    return Err(unprocessable(errors));
                }
                let watchlist = watchlist.expect("checked");
                let event_id = event_id.expect("checked");
                let actor = transaction.actor().id;
                let connection = transaction.connection();
                let owns: Option<Uuid> =
                    sqlx::query_scalar("SELECT id FROM watchlists WHERE id=$1 AND user_id=$2")
                        .bind(watchlist)
                        .bind(actor)
                        .fetch_optional(&mut *connection)
                        .await
                        .map_err(|_| ApiError::database())?;
                if owns.is_none() {
                    return Err(not_found("Watchlist not found"));
                }
                let visible = super::capture_visible(
                    connection,
                    &policy,
                    &crate::market::visibility::VisibilityClock::at(now),
                    &config,
                )
                .await
                .map_err(|_| ApiError::database())?;
                // Ok commits the read; TargetMissing is then reported as 404.
                acknowledge(connection, watchlist, event_id, &visible).await
            })
        },
    )
    .await;
    match result {
        Ok(Some(event)) => Json(event).into_response(),
        Ok(None) => not_found("Event target not found").into_response(),
        Err(response) => response,
    }
}
