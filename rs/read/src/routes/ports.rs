//! R01 and R02: the public port list and a single public port.
//!
//! This is Task 8's fixed pattern for a simple public read: one parameterized
//! statement per route, all database work inside `run_admitted`, and a wire
//! projection whose field order, nulls, and number formatting match the Python
//! oracle. The Python route takes no query parameters, ignores unknown ones,
//! and applies no filter and no ordering, so neither does this port.

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgConnection;

use crate::{ApiError, AppState, Json, database_error, run_admitted};

/// One `ports` row with its extracted coordinates and optional intelligence.
///
/// `intelligence_port_id` is the join key of the intelligence row: when it is
/// null the port has no intelligence and the response carries an explicit
/// `null` rather than an object of nulls.
#[derive(sqlx::FromRow)]
pub struct PortRow {
    pub id: String,
    pub name: String,
    pub country: String,
    pub timezone: Option<String>,
    pub is_active: bool,
    pub lng: Option<f64>,
    pub lat: Option<f64>,
    pub intelligence_port_id: Option<String>,
    pub congestion_level: Option<String>,
    pub methanol_price_avg: Option<f64>,
    pub biofuel_price_avg: Option<f64>,
    pub captured_at: Option<DateTime<Utc>>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/ports", get(list_ports))
        .route("/api/ports/{port_id}", get(get_port))
}

async fn list_ports(State(state): State<AppState>) -> Result<Response, ApiError> {
    let ports = run_admitted(&state, |connection| {
        Box::pin(async move { select_ports(connection).await })
    })
    .await?;
    Ok(axum::Json(Json::Array(ports)).into_response())
}

async fn get_port(
    State(state): State<AppState>,
    Path(port_id): Path<String>,
) -> Result<Response, ApiError> {
    let port = run_admitted(&state, |connection| {
        Box::pin(async move { select_port(connection, port_id).await })
    })
    .await?;
    match port {
        Some(port) => Ok(axum::Json(port).into_response()),
        None => Err(ApiError {
            status: StatusCode::NOT_FOUND,
            detail: json!("Port not found"),
            retry_after: None,
        }),
    }
}

async fn select_ports(connection: &mut PgConnection) -> Result<Vec<Json>, ApiError> {
    let rows = sqlx::query_as::<_, PortRow>(include_str!("../../sql/ports.sql"))
        .fetch_all(connection)
        .await
        .map_err(|error| query_error("/api/ports", &error))?;
    Ok(rows.iter().map(port_response).collect())
}

async fn select_port(
    connection: &mut PgConnection,
    port_id: String,
) -> Result<Option<Json>, ApiError> {
    let row = sqlx::query_as::<_, PortRow>(include_str!("../../sql/port.sql"))
        .bind(port_id)
        .fetch_optional(connection)
        .await
        .map_err(|error| query_error("/api/ports", &error))?;
    Ok(row.as_ref().map(port_response))
}

/// Project one row to the `PortResponse` wire contract.
///
/// Field order follows `PortBase` and `PortResponse` field order, because the
/// recorded oracle response body is compared byte for byte.
pub fn port_response(row: &PortRow) -> Json {
    let intelligence = if row.intelligence_port_id.is_some() {
        // `captured_at` is non-null whenever the intelligence row exists; the
        // optional form only keeps the projection total.
        json!({
            "congestion_level": row.congestion_level,
            "methanol_price_avg": row.methanol_price_avg,
            "biofuel_price_avg": row.biofuel_price_avg,
            "captured_at": row.captured_at.map(pydantic_datetime),
        })
    } else {
        Json::Null
    };
    json!({
        "id": row.id,
        "name": row.name,
        "country": row.country,
        // The geography column is never serialized.
        "location": Json::Null,
        "timezone": row.timezone,
        "is_active": row.is_active,
        "lat": row.lat,
        "lng": row.lng,
        "intelligence": intelligence,
    })
}

/// Format a captured UTC instant the way Pydantic v2 writes it: RFC 3339 with
/// an explicit `Z` for the UTC offset.
pub fn pydantic_datetime(value: DateTime<Utc>) -> String {
    utc_rfc3339(value, "Z")
}

/// Format a captured UTC instant for a route that calls `datetime.isoformat()`
/// in Python, which writes `+00:00` instead of `Z`.
///
/// Both Python formatters omit the fractional part when it is zero and always
/// write six digits when it is not, so the two callers share this body. Task 8
/// owns only these three route files, so the helper lives here rather than in a
/// new shared module.
pub fn utc_rfc3339(value: DateTime<Utc>, utc_suffix: &str) -> String {
    let seconds = value.format("%Y-%m-%dT%H:%M:%S");
    let micros = value.timestamp_subsec_micros();
    if micros == 0 {
        format!("{seconds}{utc_suffix}")
    } else {
        format!("{seconds}.{micros:06}{utc_suffix}")
    }
}

/// Convert a SQLx failure into the shared database error contract.
///
/// The three simple-read modules share this adapter so the market-path
/// classifier and the retry rule stay in one place.
pub(crate) fn query_error(path: &str, error: &sqlx::Error) -> ApiError {
    let sqlstate = match error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    };
    database_error(path, "GET", sqlstate.as_deref())
}
