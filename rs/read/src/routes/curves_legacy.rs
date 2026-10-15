//! R07/R08: legacy REAL-only curve and exact LF CSV projection.
use super::ports::{query_error, utc_rfc3339};
use crate::market::{
    legacy_curve::legacy_curve_query,
    visibility::{PublicOrderFilters, VisibilityClock, bind_order_query},
    windows::{normalize_window, window_sort_key},
};
use crate::runtime::http::{QueryFields, QueryKind, QueryValue};
use crate::{ApiError, AppState, Json, run_admitted};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/curves/forward", get(curve))
        .route("/api/curves/forward/export", get(export))
}

pub fn midpoint(bid: Option<Decimal>, ask: Option<Decimal>) -> Option<Decimal> {
    bid.zip(ask).map(|(b, a)| {
        ((b + a) / Decimal::from(2))
            .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven)
    })
}

fn rounded(value: Decimal) -> String {
    let mut value = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    value.rescale(2);
    value.to_string()
}

/// An aggregate for one raw SQL window/side, before alias merging.
#[derive(Debug)]
pub struct CurveAggregate {
    pub window: String,
    pub side: String,
    pub max_price: Option<Decimal>,
    pub min_price: Option<Decimal>,
    pub volume: Decimal,
    pub count: i64,
}

pub fn curve_points(rows: Vec<CurveAggregate>, today: NaiveDate) -> Result<Vec<Json>, String> {
    let mut windows: std::collections::BTreeMap<
        String,
        (Option<Decimal>, Option<Decimal>, Decimal, i64),
    > = std::collections::BTreeMap::new();
    for row in rows {
        let window = normalize_window(&row.window, today)?;
        let bucket = windows
            .entry(window)
            .or_insert((None, None, Decimal::ZERO, 0));
        if row.side == "BID" {
            if let Some(price) = row.max_price {
                bucket.0 = Some(bucket.0.map_or(price, |old| old.max(price)));
            }
        } else if row.side == "ASK"
            && let Some(price) = row.min_price
        {
            bucket.1 = Some(bucket.1.map_or(price, |old| old.min(price)));
        }
        bucket.2 += row.volume;
        bucket.3 += row.count;
    }
    let mut ordered = Vec::new();
    for (window, (bid, ask, volume, count)) in windows {
        let key = window_sort_key(&window, today)?;
        ordered.push((
            key,
            json!({
                "availability_window": window,
                "best_bid": bid.map(|v| v.to_string()),
                "best_ask": ask.map(|v| v.to_string()),
                "mid_price": midpoint(bid, ask).map(rounded),
                "spread": bid.zip(ask).map(|(b, a)| rounded(a - b)),
                "volume_mt": volume.to_string(),
                "order_count": count
            }),
        ));
    }
    ordered.sort_by_key(|(key, _)| *key);
    Ok(ordered.into_iter().map(|(_, point)| point).collect())
}

pub fn curve_csv(points: &[Json]) -> Result<Vec<u8>, String> {
    let fields = [
        "availability_window",
        "best_bid",
        "best_ask",
        "mid_price",
        "spread",
        "volume_mt",
        "order_count",
    ];
    let mut text = fields.join(",") + "\n";
    for point in points {
        let mut cells = Vec::new();
        for field in fields {
            let value = point
                .get(field)
                .ok_or_else(|| format!("missing curve field {field}"))?;
            let cell = match value {
                Json::Null => String::new(),
                Json::String(value) => value.clone(),
                Json::Number(value) if field == "order_count" => value.to_string(),
                _ => return Err(format!("invalid curve field {field}")),
            };
            // Python's LF writer quotes LF, comma and quote, but not a lone CR.
            cells.push(if cell.contains([',', '"', '\n']) {
                format!("\"{}\"", cell.replace('"', "\"\""))
            } else {
                cell
            });
        }
        text.push_str(&cells.join(","));
        text.push('\n');
    }
    Ok(text.into_bytes())
}

pub fn curve_query(raw: &str) -> Result<(Uuid, Option<Uuid>), ApiError> {
    let fields = QueryFields::parse(raw);
    let product = fields.validate("product_id", QueryKind::Uuid, true);
    let point = fields.validate("delivery_point_id", QueryKind::Uuid, false);
    let mut errors = Vec::new();
    for result in [&product, &point] {
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
        });
    }
    let Some(QueryValue::Uuid(product)) = product? else {
        return Err(ApiError::internal());
    };
    let point = match point? {
        Some(QueryValue::Uuid(value)) => Some(value),
        None => None,
        _ => return Err(ApiError::internal()),
    };
    Ok((product, point))
}

async fn curve(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    response(state, raw.unwrap_or_default(), false).await
}
async fn export(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    response(state, raw.unwrap_or_default(), true).await
}
async fn response(state: AppState, raw: String, csv: bool) -> Result<Response, ApiError> {
    let (product, point) = curve_query(&raw)?;
    let clock = VisibilityClock::at(request_now()?);
    let policy = state.policy.clone();
    let config = state.config.clone();
    run_admitted(&state, move |connection| Box::pin(async move {
        let query = legacy_curve_query(include_str!("../../sql/curves/legacy_orders.sql"));
        let filters = PublicOrderFilters { product_id: Some(product), delivery_point_id: point, ..Default::default() };
        let rows = bind_order_query(&query, &policy, &clock, &config, &filters)
            .map_err(|e| query_error("/api/curves/forward", &e))?
            .fetch_all(connection).await.map_err(|e| query_error("/api/curves/forward", &e))?;
        let aggregates = rows.iter().map(|row| Ok(CurveAggregate {
            window: row.try_get("availability_window")?, side: row.try_get("side")?,
            max_price: row.try_get("max_price")?, min_price: row.try_get("min_price")?,
            volume: row.try_get::<Option<Decimal>, _>("total_volume")?.unwrap_or_default(), count: row.try_get("order_count")?,
        })).collect::<Result<Vec<_>, sqlx::Error>>().map_err(|e| query_error("/api/curves/forward", &e))?;
        let points = curve_points(aggregates, clock.singapore_date).map_err(|_| ApiError::internal())?;
        if csv {
            let body = curve_csv(&points).map_err(|_| ApiError::internal())?;
            let mut response = ([(header::CONTENT_TYPE, "text/csv; charset=utf-8"), (header::CONTENT_DISPOSITION, &format!("attachment; filename=\"forward_curve_{product}.csv\""))], body).into_response();
            response.headers_mut().remove(header::CONTENT_LENGTH);
            Ok(response)
        } else {
            Ok(axum::Json(json!({"product_id": product, "delivery_point_id": point, "curve": points, "generated_at": utc_rfc3339(clock.now, "Z")})).into_response())
        }
    })).await
}

fn request_now() -> Result<chrono::DateTime<Utc>, ApiError> {
    #[cfg(feature = "contract-tests")]
    if let Ok(value) = std::env::var("RUST_CONTRACT_CLOCK") {
        return value.parse().map_err(|_| ApiError::internal());
    }
    Ok(Utc::now())
}
