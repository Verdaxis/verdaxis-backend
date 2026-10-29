//! M01: one canonical market benchmark quote.

use crate::routes::{curves_canonical::benchmarks, market_data_error};
use crate::{ApiError, AppState, MarketPolicy, run_market_read};
use axum::{
    Json, Router,
    extract::{RawQuery, State},
    routing::get,
};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

pub const PATH: &str = "/api/benchmarks";

#[derive(Debug)]
pub struct BenchmarkQuery {
    pub market_product: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkQuote {
    market_product: String,
    delivery_point_id: Uuid,
    delivery_point_name: String,
    availability_window: String,
    benchmark_price_per_mt_usd: String,
    source: String,
    generated_at: DateTime<Utc>,
    observed_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct BenchmarkResponse {
    items: Vec<BenchmarkQuote>,
    generated_at: DateTime<Utc>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(quote))
}

async fn quote(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<BenchmarkResponse>, ApiError> {
    let now = Utc::now();
    let query = parse_query(raw.as_deref(), &state.policy, now)?;
    if query.market_product.is_none() || query.delivery_point_id.is_none() {
        return Ok(Json(BenchmarkResponse {
            items: vec![],
            generated_at: now,
        }));
    }
    let policy = state.policy.clone();
    let item = run_market_read(&state, move |connection| {
        Box::pin(async move { select_quote(connection, &policy, &query, now).await })
    })
    .await?;
    Ok(Json(BenchmarkResponse {
        items: item.into_iter().collect(),
        generated_at: now,
    }))
}

pub fn parse_query(
    raw: Option<&str>,
    policy: &MarketPolicy,
    now: DateTime<Utc>,
) -> Result<BenchmarkQuery, ApiError> {
    let mut product = None;
    let mut point = None;
    let mut window = None;
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let slot = match name.as_ref() {
            "market_product" => &mut product,
            "delivery_point_id" => &mut point,
            "availability_window" => &mut window,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(market_data_error::invalid_input(
                "Query fields must occur at most once.",
            ));
        }
    }
    if let Some(code) = &product
        && !policy.products.iter().any(|p| p.code == *code)
    {
        return Err(market_data_error::invalid_input(
            "market_product must name a catalog product.",
        ));
    }
    let point = point
        .map(|value| {
            Uuid::parse_str(&value)
                .map_err(|_| market_data_error::invalid_input("delivery_point_id must be a UUID."))
        })
        .transpose()?;
    let window = crate::market::windows::normalize_window(
        window.as_deref().unwrap_or("SPOT"),
        now.date_naive(),
    )
    .map_err(|_| {
        market_data_error::invalid_input("availability_window must name a supported window.")
    })?;
    crate::market::windows::window_start_date(&window, now.date_naive()).map_err(|_| {
        market_data_error::invalid_input("availability_window must contain a valid date.")
    })?;
    Ok(BenchmarkQuery {
        market_product: product,
        delivery_point_id: point,
        availability_window: window,
    })
}

#[derive(sqlx::FromRow)]
struct QuoteRow {
    catalog_count: i64,
    quote_count: i64,
    point_name: Option<String>,
    price: Option<String>,
    source: Option<String>,
    observed_at: Option<DateTime<Utc>>,
}

/// Called inside the admitted READ ONLY transaction; there is one domain SELECT.
pub async fn select_quote(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
    query: &BenchmarkQuery,
    now: DateTime<Utc>,
) -> Result<Option<BenchmarkQuote>, ApiError> {
    let (Some(product), Some(point)) = (&query.market_product, query.delivery_point_id) else {
        return Ok(None);
    };
    let policy_json = serde_json::to_string(policy).map_err(|_| read_failed())?;
    let row = sqlx::query_as::<_, QuoteRow>(include_str!("../../sql/benchmark_quote.sql"))
        .bind(policy_json)
        .bind(product)
        .bind(point)
        .bind(&query.availability_window)
        .fetch_one(connection)
        .await
        .map_err(|error| market_data_error::database_failure("benchmark_quote", &error))?;
    if row.catalog_count > 1 || row.quote_count > 1 {
        return Err(read_failed());
    }
    if row.catalog_count == 0 {
        return Ok(None);
    }
    let (price, source, observed_at) = if row.quote_count == 1 {
        let price = row.price.as_deref().ok_or_else(read_failed)?;
        let price = Decimal::from_str_exact(price).map_err(|_| read_failed())?;
        if price.is_sign_negative() {
            return Err(read_failed());
        }
        (
            price
                .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven)
                .to_string(),
            row.source.ok_or_else(read_failed)?,
            row.observed_at,
        )
    } else {
        let key = (
            product.clone(),
            point.to_string(),
            query.availability_window.clone(),
        );
        let fallback = benchmarks::board_quote(None, &key, now);
        if fallback.is_null() {
            return Ok(None);
        }
        (
            fallback["price"]
                .as_str()
                .ok_or_else(read_failed)?
                .to_owned(),
            fallback["source"]
                .as_str()
                .ok_or_else(read_failed)?
                .to_owned(),
            None,
        )
    };
    Ok(Some(BenchmarkQuote {
        market_product: product.clone(),
        delivery_point_id: point,
        delivery_point_name: row.point_name.ok_or_else(read_failed)?,
        availability_window: query.availability_window.clone(),
        benchmark_price_per_mt_usd: price,
        source,
        generated_at: now,
        observed_at,
    }))
}

fn read_failed() -> ApiError {
    market_data_error::database_failure("benchmark_projection", &sqlx::Error::RowNotFound)
}
