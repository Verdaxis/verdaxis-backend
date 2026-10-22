//! R03 and R04: the public product and delivery-point catalogs.
//!
//! Both routes read the mutable catalog rows from the database, join them to
//! the attested policy tuples in SQL, and project the response with the shared
//! [`project_catalog`], so neither product identity nor display order is
//! written in Rust. Unit, minimum lot, and description come from the row; every
//! other field comes from the policy.

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use sqlx::PgConnection;

use crate::market::policy::project_catalog;
use crate::routes::ports::query_error;
use crate::{ApiError, AppState, Json, MarketPolicy, run_admitted};

/// A canonical `products` row as the policy projection reads it.
///
/// The minimum lot stays a text value at its stored scale, which is exactly the
/// decimal string the Python response model emits.
#[derive(sqlx::FromRow, serde::Serialize)]
struct ProductRow {
    id: String,
    name: String,
    fuel_type: String,
    fuel_grade: String,
    unit: Option<String>,
    min_lot_size: Option<String>,
    spec_description: Option<String>,
    is_active: bool,
}

/// A canonical `delivery_points` row as the policy projection reads it.
#[derive(sqlx::FromRow, serde::Serialize)]
struct PointRow {
    id: String,
    name: String,
    region: String,
    timezone: Option<String>,
    is_active: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/catalog/products", get(list_products))
        .route("/api/catalog/delivery-points", get(list_delivery_points))
}

async fn list_products(State(state): State<AppState>) -> Result<Response, ApiError> {
    let key = crate::runtime::cache::ReferenceKey::Products;
    if let Some(payload) = crate::runtime::cache::cached_payload(&state, &key) {
        return Ok(axum::Json(payload).into_response());
    }
    let policy = state.policy.clone();
    let catalog = run_admitted(&state, move |connection| {
        Box::pin(async move { select_products(connection, &policy).await })
    })
    .await?;
    crate::runtime::cache::store_payload(&state, key, &catalog);
    Ok(axum::Json(catalog).into_response())
}

async fn list_delivery_points(State(state): State<AppState>) -> Result<Response, ApiError> {
    let key = crate::runtime::cache::ReferenceKey::DeliveryPoints;
    if let Some(payload) = crate::runtime::cache::cached_payload(&state, &key) {
        return Ok(axum::Json(payload).into_response());
    }
    let policy = state.policy.clone();
    let catalog = run_admitted(&state, move |connection| {
        Box::pin(async move { select_points(connection, &policy).await })
    })
    .await?;
    crate::runtime::cache::store_payload(&state, key, &catalog);
    Ok(axum::Json(catalog).into_response())
}

async fn select_products(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
) -> Result<Json, ApiError> {
    let rows = sqlx::query_as::<_, ProductRow>(include_str!("../../sql/catalog_products.sql"))
        .bind(policy_json(policy)?)
        .fetch_all(connection)
        .await
        .map_err(|error| query_error("/api/catalog/products", &error))?;
    project_rows(policy, &rows)
}

async fn select_points(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
) -> Result<Json, ApiError> {
    let rows = sqlx::query_as::<_, PointRow>(include_str!("../../sql/catalog_points.sql"))
        .bind(policy_json(policy)?)
        .fetch_all(connection)
        .await
        .map_err(|error| query_error("/api/catalog/delivery-points", &error))?;
    project_rows(policy, &rows)
}

/// Project rows through the reviewed shared projection.
///
/// A row the projection refuses (a missing required column, or a mix of product
/// and point rows) is a server error, exactly as a Python response-model
/// failure is.
fn project_rows<T: serde::Serialize>(policy: &MarketPolicy, rows: &[T]) -> Result<Json, ApiError> {
    let rows: Vec<Json> = rows
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::internal())?;
    project_catalog(policy, &rows).map_err(|_| ApiError::internal())
}

/// Serialize the policy for the statement's `$1::jsonb` parameter.
fn policy_json(policy: &MarketPolicy) -> Result<String, ApiError> {
    serde_json::to_string(policy).map_err(|_| ApiError::internal())
}
