//! Public, uncached port availability from one inventory snapshot.

use axum::{
    Router,
    extract::{RawQuery, State},
    routing::get,
};
use serde::Serialize;
use sqlx::PgConnection;

use crate::{ApiError, AppState, MarketPolicy, routes::market_data_error, runtime};

pub const PATH: &str = "/api/availability";
pub const MAX_GROUPS: u64 = 512;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AvailabilityRow {
    pub port_id: String,
    pub port_name: String,
    pub market_product: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub total_stock_mt: Option<String>,
    pub avg_price_per_mt: Option<String>,
    pub supplier_count: i64,
    pub unknown_count: i64,
    pub source_kind: String,
    pub demo_status: String,
    pub availability_level: String,
}

/// PostgreSQL owns NUMERIC aggregation and rounding. Decimal text is never
/// narrowed to a float or a fixed-width coefficient on the way to the client.
#[derive(Debug, Serialize)]
pub struct PortFuelAvailability {
    port_id: String,
    port_name: String,
    lat: f64,
    lng: f64,
    fuel_type: String,
    market_product_code: String,
    total_stock_mt: Option<String>,
    supplier_count: i64,
    availability_level: String,
    avg_price_per_mt: Option<String>,
    source_kind: String,
    scope: &'static str,
    demo_status: String,
    unknown_count: i64,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(availability).head(method_not_allowed))
}

async fn method_not_allowed() -> axum::http::StatusCode {
    axum::http::StatusCode::METHOD_NOT_ALLOWED
}

async fn availability(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> Result<axum::Json<Vec<PortFuelAvailability>>, ApiError> {
    let fields = runtime::http::QueryFields::parse(query.as_deref().unwrap_or_default());
    let fuel_type = fields.raw("fuel_type").map(str::to_owned);
    let policy = state.policy.clone();
    let rows = runtime::admission::run_market_read(&state, move |connection| {
        Box::pin(async move { select_availability(connection, &policy, fuel_type).await })
    })
    .await?;
    Ok(axum::Json(rows))
}

pub async fn select_availability(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
    fuel_type: Option<String>,
) -> Result<Vec<PortFuelAvailability>, ApiError> {
    let policy = serde_json::to_string(policy).map_err(|_| market_data_error::unavailable())?;
    let rows = sqlx::query_as::<_, AvailabilityRow>(include_str!("../../sql/availability.sql"))
        .bind(policy)
        .bind(fuel_type)
        .bind(market_data_error::probe_limit(MAX_GROUPS))
        .fetch_all(connection)
        .await
        .map_err(|error| market_data_error::database_failure(PATH, &error))?;
    project_rows(rows)
}

pub fn project_rows(rows: Vec<AvailabilityRow>) -> Result<Vec<PortFuelAvailability>, ApiError> {
    // Count groups before omitting map entries, so the sentinel cannot become a
    // successful partial response when a port has no coordinate pair.
    market_data_error::within_limit(rows.len() as u64, MAX_GROUPS)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let (Some(lat), Some(lng)) = (row.lat, row.lng) else {
                return None;
            };
            Some(PortFuelAvailability {
                port_id: row.port_id,
                port_name: row.port_name,
                lat,
                lng,
                fuel_type: row.market_product.clone(),
                market_product_code: row.market_product,
                total_stock_mt: row.total_stock_mt,
                supplier_count: row.supplier_count,
                availability_level: row.availability_level,
                avg_price_per_mt: row.avg_price_per_mt,
                source_kind: row.source_kind,
                scope: "DELIVERY_POINT",
                demo_status: row.demo_status,
                unknown_count: row.unknown_count,
            })
        })
        .collect())
}
