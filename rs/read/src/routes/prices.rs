#[path = "prices_date.rs"]
mod date_parser;
#[path = "prices_query.rs"]
pub mod query;
// Immutable price summaries and REAL-only daily reference prices.
use crate::market::visibility::{
    TradeEvidenceFilters, bind_trade_query, price_quarantine::price_quarantine_query,
    trade_query_sql,
};
use crate::market::windows::normalize_window;
use crate::routes::ports::{query_error, utc_rfc3339};
use crate::runtime::http::{QueryFields, QueryKind, QueryValue};
use crate::{ApiError, AppState, Json, run_admitted};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::json;
use sqlx::{Row, postgres::PgRow};
use std::{collections::HashMap, str::FromStr};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/prices", get(summary))
        .route("/api/prices/reference", get(reference))
        .route("/api/prices/reference/export", get(export))
}

pub(crate) fn captured_now(state: &AppState) -> Result<DateTime<Utc>, ApiError> {
    #[cfg(feature = "contract-tests")]
    if state.config.environment == "test"
        && let Ok(clock) = std::env::var("RUST_CONTRACT_CLOCK")
    {
        return DateTime::parse_from_rfc3339(&clock)
            .map(|v| v.with_timezone(&Utc))
            .map_err(|_| ApiError::internal());
    }
    let _ = state;
    Ok(Utc::now())
}
pub(crate) fn invalid(message: impl Into<String>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Json::String(message.into()),
        retry_after: None,
    }
}
pub(crate) fn fields(raw: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}
pub(crate) fn integer(
    raw: &str,
    name: &str,
    min: i64,
    max: i64,
    default: i64,
) -> Result<i64, ApiError> {
    match QueryFields::parse(raw).validate(
        name,
        QueryKind::Integer {
            minimum: min,
            maximum: max,
        },
        false,
    )? {
        Some(QueryValue::Integer(v)) => Ok(v),
        None => Ok(default),
        _ => Err(ApiError::internal()),
    }
}
pub(crate) fn uuid(raw: &str, name: &str) -> Result<Option<Uuid>, ApiError> {
    let result = QueryFields::parse(raw)
        .validate(name, QueryKind::Uuid, false)
        .map_err(|mut error| {
            // The pinned Python UUID parser reports one-based character offsets;
            // Rust's uuid crate reports zero-based offsets, including prefixes.
            if let Some(cause) = error.detail[0]["ctx"]["error"].as_str()
                && cause.starts_with("invalid character:")
                && let Some((prefix, position)) = cause.rsplit_once(" at ")
                && let Ok(position) = position.parse::<usize>()
            {
                let cause = format!("{prefix} at {}", position + 1);
                error.detail[0]["msg"] = json!(format!("Input should be a valid UUID, {cause}"));
                error.detail[0]["ctx"]["error"] = json!(cause);
            }
            if let Some(value) = fields(raw).get(name) {
                let simple = value.strip_prefix("urn:uuid:").unwrap_or(value);
                let simple = simple
                    .strip_prefix('{')
                    .and_then(|v| v.strip_suffix('}'))
                    .unwrap_or(simple);
                if simple.bytes().all(|c| c.is_ascii_hexdigit()) && simple.len() != 32 {
                    let cause = format!(
                        "invalid length: expected length 32 for simple format, found {}",
                        simple.len()
                    );
                    error.detail[0]["msg"] =
                        json!(format!("Input should be a valid UUID, {cause}"));
                    error.detail[0]["ctx"]["error"] = json!(cause);
                }
            }
            error
        })?;
    match result {
        Some(QueryValue::Uuid(v)) => Ok(Some(v)),
        None => Ok(None),
        _ => Err(ApiError::internal()),
    }
}
#[derive(Default)]
struct PriceFilters {
    product: Option<Uuid>,
    point: Option<Uuid>,
    fuel: Option<String>,
    region: Option<String>,
    market: Option<String>,
    window: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}
impl PriceFilters {
    fn parse(raw: &str, state: &AppState, now: DateTime<Utc>) -> Result<Self, ApiError> {
        let mut values = fields(raw);
        let mut text = |name| values.remove(name).filter(|v| !v.is_empty());
        let fuel = text("fuel_type");
        let region = text("region");
        let market = text("market_product").map(|v| v.trim().to_uppercase());
        if let Some(market) = &market
            && !market.is_empty()
            && !state
                .policy
                .products
                .iter()
                .any(|p| p.execution_mode == "ORDERBOOK" && p.code == *market)
        {
            return Err(invalid(format!(
                "market_product must be one of {}",
                state
                    .policy
                    .products
                    .iter()
                    .filter(|p| p.execution_mode == "ORDERBOOK")
                    .map(|p| p.code.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let window = text("availability_window")
            .map(|v| normalize_window(&v, now.date_naive()).map_err(invalid))
            .transpose()?;
        Ok(Self {
            product: uuid(raw, "product_id")?,
            point: uuid(raw, "delivery_point_id")?,
            fuel,
            region,
            market: market.filter(|s| !s.is_empty()),
            window,
            ..Self::default()
        })
    }
    fn evidence(&self, since: Option<DateTime<Utc>>) -> TradeEvidenceFilters<'_> {
        TradeEvidenceFilters {
            product_id: self.product,
            delivery_point_id: self.point,
            availability_window: self.window.as_deref(),
            confirmed_since: since,
            ..Default::default()
        }
    }
}

fn date_query(values: &HashMap<String, String>, name: &str) -> Result<Option<NaiveDate>, ApiError> {
    let Some(value) = values.get(name) else {
        return Ok(None);
    };
    date_parser::parse(value, name).map(Some)
}
fn date_range(raw: &str, filters: &mut PriceFilters, export: bool) -> Result<(), ApiError> {
    let values = fields(raw);
    let (from, to) = if export {
        ("from_date", "to_date")
    } else {
        ("date_from", "date_to")
    };
    filters.from = date_query(&values, from)?;
    filters.to = date_query(&values, to)?;
    if !export {
        for (preferred, alias, target) in [
            (from, "from", &mut filters.from),
            (to, "to", &mut filters.to),
        ] {
            let old = date_query(&values, alias)?;
            if target.is_some() && old.is_some() && *target != old {
                return Err(invalid(format!(
                    "{preferred} and {alias} must match when both are supplied"
                )));
            }
            *target = target.or(old);
        }
    }
    if let (Some(a), Some(b)) = (filters.from, filters.to)
        && a > b
    {
        return Err(invalid(format!("{from} must be before or equal to {to}")));
    }
    Ok(())
}
pub fn rounded_price(value: &str) -> Result<String, String> {
    let mut value = Decimal::from_str(value)
        .map_err(|e| e.to_string())?
        .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    value.rescale(2);
    Ok(value.to_string())
}
fn text(row: &PgRow, column: &str) -> Result<String, ApiError> {
    row.try_get(column).map_err(|_| ApiError::internal())
}
fn identity(row: &PgRow) -> Result<Json, ApiError> {
    Ok(
        json!({"product_id":row.try_get::<Uuid,_>("product_id").map_err(|_|ApiError::internal())?,"product_name":text(row,"product_name")?,"market_product":text(row,"market_product")?,"fuel_type":text(row,"fuel_type")?,"delivery_point_id":row.try_get::<Uuid,_>("delivery_point_id").map_err(|_|ApiError::internal())?,"delivery_point_name":text(row,"delivery_point_name")?,"availability_window":text(row,"availability_window")?,"region":text(row,"delivery_point_region")?}),
    )
}
fn observed(row: &PgRow) -> Result<String, ApiError> {
    Ok(utc_rfc3339(
        row.try_get("observed_at")
            .map_err(|_| ApiError::internal())?,
        "Z",
    ))
}
fn key(row: &PgRow) -> Result<(Uuid, String, Uuid, String), ApiError> {
    Ok((
        row.try_get("product_id")
            .map_err(|_| ApiError::internal())?,
        text(row, "market_product")?,
        row.try_get("delivery_point_id")
            .map_err(|_| ApiError::internal())?,
        text(row, "availability_window")?,
    ))
}
async fn summary(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let raw = raw.unwrap_or_default();
    let now = captured_now(&state)?;
    query::validate(&raw, query::Surface::Summary)?;
    let hours = integer(&raw, "hours", 1, 168, 24)?;
    let filters = PriceFilters::parse(&raw, &state, now)?;
    let policy = state.policy.clone();
    let items = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let evidence = filters.evidence(Some(now - chrono::Duration::hours(hours)));
            let mut items = Vec::new();
            for class in ["REAL", "DEMO"] {
                let sql = trade_query_sql(include_str!("../../sql/prices/summary.sql"));
                let rows = bind_trade_query(&sql, &policy, &evidence)
                    .map_err(|_| ApiError::internal())?
                    .bind(&filters.fuel)
                    .bind(&filters.region)
                    .bind(&filters.market)
                    .bind(class)
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(|e| query_error("/api/prices", &e))?;
                let sql = trade_query_sql(include_str!("../../sql/prices/latest.sql"));
                let latest = bind_trade_query(&sql, &policy, &evidence)
                    .map_err(|_| ApiError::internal())?
                    .bind(&filters.fuel)
                    .bind(&filters.region)
                    .bind(&filters.market)
                    .bind(class)
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(|e| query_error("/api/prices", &e))?;
                let latest = latest
                    .iter()
                    .map(|r| Ok((key(r)?, r)))
                    .collect::<Result<HashMap<_, _>, ApiError>>()?;
                for row in rows {
                    let last = latest.get(&key(&row)?).ok_or_else(ApiError::internal)?;
                    let mut item = identity(&row)?;
                    let count: i64 = row.try_get("count").map_err(|_| ApiError::internal())?;
                    let time = observed(last)?;
                    item["last_price"] = json!(text(last, "last_price")?);
                    item["avg_price_24h"] = json!(
                        rounded_price(&text(&row, "average")?).map_err(|_| ApiError::internal())?
                    );
                    item["high_24h"] = json!(text(&row, "high")?);
                    item["low_24h"] = json!(text(&row, "low")?);
                    item["volume_24h"] = json!(text(&row, "volume")?);
                    item["trade_count_24h"] = json!(count);
                    item["price_change_pct"] = Json::Null;
                    item["last_trade_at"] = json!(time);
                    item["source_kind"] = json!(if class == "REAL" {
                        "CONFIRMED_TRADE"
                    } else {
                        "DEMO_SEED"
                    });
                    item["scope"] = json!("DELIVERY_POINT");
                    item["demo_status"] = json!(if class == "REAL" {
                        "REAL_ONLY"
                    } else {
                        "DEMO_ONLY"
                    });
                    item["is_reference"] = json!(false);
                    item["observed_at"] = json!(time);
                    item["real_trade_count_24h"] = json!(if class == "REAL" { count } else { 0 });
                    item["demo_trade_count_24h"] = json!(if class == "DEMO" { count } else { 0 });
                    item["unknown_trade_count_24h"] = json!(0);
                    items.push(item);
                }
            }
            let sql = price_quarantine_query(include_str!("../../sql/prices/quarantine.sql"));
            let rows = bind_trade_query(&sql, &policy, &evidence)
                .map_err(|_| ApiError::internal())?
                .bind(&filters.fuel)
                .bind(&filters.region)
                .bind(&filters.market)
                .fetch_all(connection)
                .await
                .map_err(|e| query_error("/api/prices", &e))?;
            for row in rows {
                let mut item = identity(&row)?;
                for name in ["last_price", "avg_price_24h", "high_24h", "low_24h"] {
                    item[name] = Json::Null;
                }
                item["volume_24h"] = json!("0");
                item["trade_count_24h"] = json!(0);
                item["price_change_pct"] = Json::Null;
                item["last_trade_at"] = Json::Null;
                item["source_kind"] = json!("UNKNOWN");
                item["scope"] = json!("DELIVERY_POINT");
                item["demo_status"] = json!("UNKNOWN");
                item["is_reference"] = json!(false);
                item["observed_at"] = json!(observed(&row)?);
                item["real_trade_count_24h"] = json!(0);
                item["demo_trade_count_24h"] = json!(0);
                item["unknown_trade_count_24h"] = json!(
                    row.try_get::<i64, _>("count")
                        .map_err(|_| ApiError::internal())?
                );
                items.push(item);
            }
            Ok(items)
        })
    })
    .await?;
    Ok(axum::Json(json!({"summaries":items,"generated_at":utc_rfc3339(now,"Z")})).into_response())
}
async fn reference(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    reference_response(state, raw.unwrap_or_default(), false).await
}
async fn export(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    reference_response(state, raw.unwrap_or_default(), true).await
}
async fn reference_response(
    state: AppState,
    raw: String,
    export: bool,
) -> Result<Response, ApiError> {
    let now = captured_now(&state)?;
    query::validate(
        &raw,
        if export {
            query::Surface::Export
        } else {
            query::Surface::Reference
        },
    )?;
    let mut filters = PriceFilters::parse(&raw, &state, now)?;
    date_range(&raw, &mut filters, export)?;
    let visibility = fields(&raw)
        .remove("visibility")
        .unwrap_or_else(|| "external".into());
    if !export && !["internal", "external"].contains(&visibility.as_str()) {
        return Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: json!([{"type":"literal_error","loc":["query","visibility"],"msg":"Input should be 'internal' or 'external'","input":visibility,"ctx":{"expected":"'internal' or 'external'"}}]),
            retry_after: None,
        });
    }
    let policy = state.policy.clone();
    run_admitted(&state, move |connection| {
        Box::pin(async move {
            let sql = trade_query_sql(include_str!("../../sql/prices/reference.sql"));
            let rows = bind_trade_query(&sql, &policy, &filters.evidence(None))
                .map_err(|_| ApiError::internal())?
                .bind(&filters.fuel)
                .bind(&filters.region)
                .bind(&filters.market)
                .bind(filters.from)
                .bind(filters.to)
                .fetch_all(connection)
                .await
                .map_err(|e| query_error("/api/prices/reference", &e))?;
            let mut items = Vec::new();
            for row in rows {
                let mut item = identity(&row)?;
                let volume = text(&row, "volume")?;
                let quantity = Decimal::from_str(&volume).map_err(|_| ApiError::internal())?;
                let weighted = Decimal::from_str(&text(&row, "weighted_sum")?)
                    .map_err(|_| ApiError::internal())?;
                let vwap = if quantity > Decimal::ZERO {
                    rounded_price(&(weighted / quantity).to_string())
                        .map_err(|_| ApiError::internal())?
                } else {
                    "0".into()
                };
                item["vwap_usd"] = json!(vwap);
                item["total_volume_mt"] = json!(if quantity == Decimal::ZERO {
                    "0".into()
                } else {
                    volume
                });
                item["trade_count"] = json!(
                    row.try_get::<i64, _>("count")
                        .map_err(|_| ApiError::internal())?
                );
                item["date"] = json!(
                    row.try_get::<NaiveDate, _>("trade_date")
                        .map_err(|_| ApiError::internal())?
                        .to_string()
                );
                item["visibility"] = json!(visibility);
                item["source_kind"] = json!("CONFIRMED_TRADE");
                item["scope"] = json!("DELIVERY_POINT");
                item["demo_status"] = json!("REAL_ONLY");
                item["is_reference"] = json!(true);
                items.push(item);
            }
            if export {
                Ok((
                    [
                        ("content-type", "text/csv; charset=utf-8"),
                        (
                            "content-disposition",
                            "attachment; filename=\"verdaxis_reference_prices.csv\"",
                        ),
                    ],
                    reference_csv(&items).map_err(|_| ApiError::internal())?,
                )
                    .into_response())
            } else {
                Ok(
                    axum::Json(json!({"prices":items,"generated_at":utc_rfc3339(now,"Z")}))
                        .into_response(),
                )
            }
        })
    })
    .await
}

/// Python csv.DictWriter's column order, minimal quoting and CRLF termination.
pub fn reference_csv(rows: &[Json]) -> Result<Vec<u8>, String> {
    let columns = [
        "date",
        "product_name",
        "market_product",
        "availability_window",
        "fuel_type",
        "delivery_point_name",
        "region",
        "vwap_usd",
        "volume_mt",
        "trade_count",
    ];
    let mut output = columns.join(",");
    output.push_str("\r\n");
    for row in rows {
        let mut values = Vec::new();
        for column in columns {
            let key = if column == "volume_mt" {
                "total_volume_mt"
            } else {
                column
            };
            let value = row
                .get(key)
                .ok_or_else(|| format!("missing CSV field {key}"))?;
            let value = match value {
                Json::Null => String::new(),
                Json::String(s) => s.clone(),
                Json::Number(n) => n.to_string(),
                _ => return Err("invalid CSV field".into()),
            };
            values.push(if value.contains([',', '"', '\r', '\n']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value
            });
        }
        output.push_str(&values.join(","));
        output.push_str("\r\n");
    }
    Ok(output.into_bytes())
}
