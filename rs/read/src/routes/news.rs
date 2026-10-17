//! R05: the public news feed.
//!
//! Python has no response model here: the ten keys of a plain dict are the wire
//! contract, the dates come from `datetime.isoformat()`, and the query is
//! validated by FastAPI's parameter declarations. `limit` and `min_relevance`
//! therefore go through the shared Python-compatible validator, which produces
//! the same 422 envelope; `category` is read from the raw query text because
//! the validator has no text kind.

use axum::{
    Router,
    extract::{RawQuery, State},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgConnection;

use crate::routes::ports::{query_error, utc_rfc3339};
use crate::runtime::http::{QueryFields, QueryKind, QueryValue};
use crate::{ApiError, AppState, Json, run_admitted};

/// The news query after Python-compatible validation.
///
/// `category` keeps the empty string Python passes through: the statement turns
/// it into "no filter".
#[derive(Debug)]
pub struct NewsQuery {
    pub limit: i64,
    pub category: Option<String>,
    pub min_relevance: i32,
}

/// A `news_items` row. Only `summary` and `source_url` are nullable in the
/// model, but every field is projected as the response writes it.
#[derive(sqlx::FromRow)]
pub struct NewsRow {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub source: String,
    pub source_url: Option<String>,
    pub url: String,
    pub category: String,
    pub relevance: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: Option<DateTime<Utc>>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/news", get(list_news))
}

/// Validate the news query exactly as the Python handler declares it.
///
/// Defaults are `limit=20`, `min_relevance=1`, and no category. FastAPI keeps
/// the last value of a repeated scalar query key, which is what the shared
/// parser and the text lookup below both do.
pub fn news_query(raw_query: &str) -> Result<NewsQuery, ApiError> {
    let fields = QueryFields::parse(raw_query);
    let limit = integer(&fields, "limit", 1, 100, 20);
    let min_relevance = integer(&fields, "min_relevance", 1, 5, 1);
    let mut errors = Vec::new();
    for result in [&limit, &min_relevance] {
        if let Err(error) = result
            && let Some(entries) = error.detail.as_array()
        {
            errors.extend(entries.iter().cloned());
        }
    }
    if !errors.is_empty() {
        return Err(ApiError {
            status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            detail: Json::Array(errors),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        });
    }
    let limit = limit?;
    let min_relevance = min_relevance?;
    let category = url::form_urlencoded::parse(raw_query.as_bytes())
        .filter(|(key, _)| key == "category")
        .map(|(_, value)| value.into_owned())
        .last();
    Ok(NewsQuery {
        limit,
        category,
        min_relevance: min_relevance as i32,
    })
}

fn integer(
    fields: &QueryFields,
    name: &str,
    minimum: i64,
    maximum: i64,
    default: i64,
) -> Result<i64, ApiError> {
    match fields.validate(name, QueryKind::Integer { minimum, maximum }, false)? {
        None => Ok(default),
        Some(QueryValue::Integer(value)) => Ok(value),
        Some(QueryValue::Uuid(_)) => Err(ApiError::internal()),
    }
}

async fn list_news(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let query = news_query(&raw.unwrap_or_default())?;
    let items = run_admitted(&state, move |connection| {
        Box::pin(async move { select_news(connection, query).await })
    })
    .await?;
    Ok(axum::Json(Json::Array(items)).into_response())
}

async fn select_news(
    connection: &mut PgConnection,
    query: NewsQuery,
) -> Result<Vec<Json>, ApiError> {
    let rows = sqlx::query_as::<_, NewsRow>(include_str!("../../sql/news.sql"))
        .bind(query.min_relevance)
        .bind(query.category)
        .bind(query.limit)
        .fetch_all(connection)
        .await
        .map_err(|error| query_error("/api/news", &error))?;
    Ok(rows.iter().map(news_response).collect())
}

/// Project one row to the ten-key wire contract, in Python's dict order.
pub fn news_response(row: &NewsRow) -> Json {
    json!({
        "id": row.id,
        "title": row.title,
        "summary": row.summary,
        "source": row.source,
        "source_url": row.source_url,
        "url": row.url,
        "category": row.category,
        "relevance": row.relevance,
        "published_at": row.published_at.map(isoformat_utc),
        "fetched_at": row.fetched_at.map(isoformat_utc),
    })
}

/// Format a captured UTC instant the way Python's `datetime.isoformat()` does.
pub fn isoformat_utc(value: DateTime<Utc>) -> String {
    utc_rfc3339(value, "+00:00")
}
