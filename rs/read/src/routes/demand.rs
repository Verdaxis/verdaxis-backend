//! Anonymous, ranked demand groups from current canonical bids.

use axum::{Json, Router, extract::{RawQuery, State}, routing::get};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{ApiError, AppState, VisibilityClock};
use crate::market::{visibility::expand_shared_markers, windows};
use crate::routes::market_data_error::{database_failure, invalid_input, result_limit};

pub const PATH: &str = "/api/demand";
const MAX_LIMIT: i64 = 512;
const DEFAULT_LIMIT: i64 = 256;

#[derive(Debug)]
pub struct DemandQuery {
    pub fuel_type: Option<String>,
    pub region: Option<String>,
    pub limit: i64,
}

impl DemandQuery {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        let mut query = Self { fuel_type: None, region: None, limit: DEFAULT_LIMIT };
        for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "fuel_type" => query.fuel_type = (!value.is_empty()).then(|| value.into_owned()),
                "region" => query.region = (!value.is_empty()).then(|| value.into_owned()),
                "limit" => {
                    query.limit = value.parse().ok().filter(|limit| (1..=MAX_LIMIT).contains(limit))
                        .ok_or_else(|| invalid_input("limit must be an integer from 1 to 512."))?;
                }
                _ => {}
            }
        }
        Ok(query)
    }
}

#[derive(Debug, Serialize)]
pub struct DemandSignal {
    fuel_type: String,
    region: String,
    market_product_code: String,
    delivery_point_id: Uuid,
    delivery_point_name: String,
    availability_window_code: String,
    volume_mt: Option<String>,
    max_price_per_mt: Option<String>,
    urgency: &'static str,
    bid_count: i64,
    earliest_delivery: String,
    created_at: DateTime<Utc>,
    source_kind: &'static str,
    scope: &'static str,
    demo_status: &'static str,
    unknown_count: i64,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(demand))
}

async fn demand(State(state): State<AppState>, RawQuery(raw): RawQuery) -> Result<Json<Vec<DemandSignal>>, ApiError> {
    let query = DemandQuery::parse(raw.as_deref())?;
    let clock = VisibilityClock::at(Utc::now());
    let owned = state.clone();
    crate::runtime::admission::run_market_read(&state, move |connection| {
        Box::pin(async move { load(connection, &owned, &clock, &query).await.map(Json) })
    }).await
}

pub fn urgency(window: &str, today: NaiveDate) -> Result<&'static str, String> {
    let days = (windows::window_start_date(window, today)? - today).num_days();
    Ok(if days <= 30 { "HIGH" } else if days <= 90 { "MEDIUM" } else { "LOW" })
}

/// One domain statement owns the group guard, exact aggregates and final rank.
/// Only accepted output rows (or a single status row) cross the SQL boundary.
pub async fn load(connection: &mut PgConnection, state: &AppState, clock: &VisibilityClock, query: &DemandQuery) -> Result<Vec<DemandSignal>, ApiError> {
    static SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        expand_shared_markers(&include_str!("../../sql/demand.sql").replace(
            "/* current_order_rule */", include_str!("../../sql/current_order_rule.sql")
        ), "$5")
    });
    let policy = serde_json::to_string(state.policy.as_ref())
        .map_err(|_| database_failure("demand_policy", &sqlx::Error::WorkerCrashed))?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(SQL.clone()))
        .bind(policy).bind(clock.now).bind(clock.singapore_date.to_string())
        .bind(query.fuel_type.clone()).bind(state.config.kyc_execution_gate_enabled)
        .bind(clock.now.date_naive()).bind(query.region.clone()).bind(query.limit)
        .fetch_all(connection).await.map_err(|error| database_failure("demand", &error))?;
    let project = || -> Result<Vec<DemandSignal>, sqlx::Error> {
        let mut signals = Vec::with_capacity(rows.len());
        for row in &rows {
            if row.try_get::<bool, _>("exceeded")? {
                // Checked separately below so overflow retains its typed response.
                continue;
            }
            let Some(product) = row.try_get::<Option<String>, _>("market_product")? else { continue };
            let window: String = row.try_get("availability_window")?;
            let provenance: String = row.try_get("provenance")?;
            let count: i64 = row.try_get("order_count")?;
            let (source_kind, demo_status) = match provenance.as_str() {
                "REAL" => ("LIVE_ORDER", "REAL_ONLY"),
                "DEMO" => ("DEMO_SEED", "DEMO_ONLY"),
                _ => ("UNKNOWN", "UNKNOWN"),
            };
            let label = windows::window_display_label(&window).map_err(|_| sqlx::Error::WorkerCrashed)?;
            let urgency = urgency(&window, clock.now.date_naive()).map_err(|_| sqlx::Error::WorkerCrashed)?;
            signals.push(DemandSignal {
                fuel_type: product.clone(), market_product_code: product,
                region: row.try_get("region")?, delivery_point_id: row.try_get("delivery_point_id")?,
                delivery_point_name: row.try_get("delivery_point_name")?, availability_window_code: window,
                volume_mt: row.try_get("volume_mt")?, max_price_per_mt: row.try_get("max_price_per_mt")?,
                urgency, bid_count: if provenance == "UNKNOWN" { 0 } else { count },
                earliest_delivery: label, created_at: row.try_get("created_at")?,
                source_kind, scope: "DELIVERY_POINT", demo_status,
                unknown_count: if provenance == "UNKNOWN" { count } else { 0 },
            });
        }
        Ok(signals)
    };
    for row in &rows {
        if row.try_get::<bool, _>("exceeded").map_err(|error| database_failure("demand_projection", &error))? {
            return Err(result_limit());
        }
    }
    project().map_err(|error| database_failure("demand_projection", &error))
}
