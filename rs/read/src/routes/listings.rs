//! Compatibility listings retain their own visibility and dictionary contract.
use axum::{
    Router,
    extract::{RawQuery, State},
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Row, postgres::PgRow};

use super::{
    ports::{query_error, utc_rfc3339},
    prices::{captured_now, integer},
};
use crate::market::{
    terms::redact_terms,
    visibility::{
        OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
    },
};
use crate::{ApiError, ApiResult, AppState, Json, run_admitted};
#[path = "../market/listing_terms.rs"]
mod listing_terms;

const PATH: &str = "/api/listings";

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(listings))
}

async fn listings(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult {
    let raw = raw.as_deref().unwrap_or_default();
    let skip = integer(raw, "skip", 0, i64::MAX, 0);
    let limit = integer(raw, "limit", 1, 100, 50);
    let mut errors = Vec::new();
    for value in [&skip, &limit] {
        if let Err(error) = value {
            errors.extend(error.detail.as_array().into_iter().flatten().cloned());
        }
    }
    if !errors.is_empty() {
        return Err(ApiError {
            status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            detail: Json::Array(errors),
            retry_after: None,
        });
    }
    let (skip, limit) = (skip?, limit?);
    let now = captured_now(&state)?;
    let owned = state.clone();
    let result = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let sql = order_query_sql(include_str!("../../sql/orderbook/listings.sql"));
            let filters = PublicOrderFilters {
                surface: OrderSurface::Listings,
                ..Default::default()
            };
            let rows = bind_order_query(
                &sql,
                &owned.policy,
                &VisibilityClock::at(now),
                &owned.config,
                &filters,
            )
            .map_err(|error| query_error(PATH, &error))?
            .bind(skip)
            .bind(limit)
            .fetch_all(connection)
            .await
            .map_err(|error| query_error(PATH, &error))?;
            rows.iter()
                .map(listing_payload)
                .collect::<Result<Vec<_>, ApiError>>()
        })
    })
    .await?;
    Ok(axum::Json(result).into_response())
}

fn listing_payload(row: &PgRow) -> Result<Json, ApiError> {
    let private: Json = row
        .try_get("private_digest_input")
        .map_err(|_| ApiError::internal())?;
    let text = |name| {
        row.try_get::<String, _>(name)
            .map_err(|_| ApiError::internal())
    };
    let mut out = json!({
        "id": private["id"],
        "product_name": text("product_name")?,
        "market_product": text("market_product")?,
        "fuel_type": text("product_fuel_type")?,
        "fuel_grade": text("product_fuel_grade")?,
        "quantity_mt": private["quantity_mt"],
        "price_per_mt_usd": private["price_per_mt_usd"],
        "region": text("delivery_point_region")?,
        "availability_window": private["availability_window"],
        "certifications": private["certifications"].as_array().cloned().unwrap_or_default(),
    });
    for field in [
        "certification_declared",
        "certification_scheme",
        "specification_standard",
        "msds_available",
        "carbon_intensity_gco2_mj",
        "carbon_intensity_method",
        "feedstock",
        "origin",
        "off_spec",
        "off_spec_notes",
    ] {
        out[field] = private[field].clone();
    }
    // Unlike OrderResponse, this legacy helper validates full declarations
    // before redaction. Invalid private fields can therefore fail this route.
    for (name, profile, schema) in [
        ("lng_terms", "lng_v1", "lng_ask"),
        ("fame_terms", "fame_v1", "fame_ask"),
    ] {
        let public = redact_terms(profile, &private[name]).map_err(|_| ApiError::internal())?;
        out[name] = listing_terms::ordered(&public, schema)?;
    }
    out["status"] = private["status"].clone();
    out["match_count"] = json!(0);
    for (output, column) in [("expires_at", "expires_at"), ("observed_at", "created_at")] {
        let value: Option<DateTime<Utc>> = row.try_get(column).map_err(|_| ApiError::internal())?;
        out[output] = json!(value.map(|value| utc_rfc3339(value, "+00:00")));
    }
    let real = text("provenance")? == "REAL";
    out["source_kind"] = json!(if real { "LIVE_ORDER" } else { "DEMO_SEED" });
    out["scope"] = json!("DELIVERY_POINT");
    out["demo_status"] = json!(if real { "REAL_ONLY" } else { "DEMO_ONLY" });
    out["is_demo_listing"] = json!(!real);
    out["unknown_count"] = json!(0);
    out["delivery_point_id"] = private["delivery_point_id"].clone();
    Ok(out)
}
