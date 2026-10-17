//! A bounded public order book from one admitted, repeatable database view.
use std::collections::HashMap;

use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::Connection;
use uuid::Uuid;

use super::{
    ports::{pydantic_datetime, query_error},
    prices::{captured_now, fields, uuid},
};
use crate::market::{
    live_benchmarks::{LiveBenchmark, LiveBenchmarkCache, LiveBenchmarkKey},
    order_projection::PublicOrder,
    visibility::{
        OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
    },
    windows::normalize_window,
};
use crate::{ApiError, ApiResult, AppState, Json, run_admitted};

const PATH: &str = "/api/orderbook/snapshot";

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(snapshot))
}

#[derive(Debug)]
pub struct SnapshotQuery {
    pub market_product: String,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
}

/// Preserve the Python enum's declaration order, not catalog display order.
/// The source-attested enum is data, separate from executable product policy.
pub fn snapshot_query(raw: &str, now: DateTime<Utc>) -> Result<SnapshotQuery, ApiError> {
    let values = fields(raw);
    let mut errors = Vec::new();
    let enum_data: Json = serde_json::from_str(include_str!("../market/orderbook_query.json"))
        .map_err(|_| ApiError::internal())?;
    let variants = enum_data["market_product_enum"]
        .as_array()
        .ok_or_else(ApiError::internal)?;
    for name in ["market_product", "delivery_point_id", "availability_window"] {
        let Some(value) = values.get(name) else {
            errors.push(
                json!({"type":"missing","loc":["query",name],"msg":"Field required","input":null}),
            );
            continue;
        };
        if name == "market_product"
            && !variants
                .iter()
                .any(|variant| variant.as_str() == Some(value))
        {
            let labels: Vec<_> = variants
                .iter()
                .filter_map(Json::as_str)
                .map(|code| format!("'{code}'"))
                .collect();
            let (last, rest) = labels.split_last().ok_or_else(ApiError::internal)?;
            let expected = format!("{} or {last}", rest.join(", "));
            errors.push(json!({"type":"enum","loc":["query",name],"msg":format!("Input should be {expected}"),"input":value,"ctx":{"expected":expected}}));
        }
        if name == "delivery_point_id"
            && let Err(error) = uuid(raw, name)
        {
            errors.extend(error.detail.as_array().into_iter().flatten().cloned());
        }
    }
    if !errors.is_empty() {
        return Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: Json::Array(errors),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        });
    }
    let availability_window = normalize_window(&values["availability_window"], now.date_naive())
        .map_err(|message| ApiError {
            status: StatusCode::BAD_REQUEST,
            detail: json!(message),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        })?;
    Ok(SnapshotQuery {
        market_product: values["market_product"].clone(),
        delivery_point_id: uuid(raw, "delivery_point_id")?.ok_or_else(ApiError::internal)?,
        availability_window,
    })
}

pub async fn snapshot(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult {
    let now = captured_now(&state)?;
    let query = snapshot_query(raw.as_deref().unwrap_or_default(), now)?;
    let policy = state.policy.clone();
    let config = state.config.clone();
    let response = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let mut tx = connection
                .begin()
                .await
                .map_err(|error| query_error(PATH, &error))?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(|error| query_error(PATH, &error))?;
            let sql = order_query_sql(include_str!("../../sql/orderbook/snapshot.sql"));
            let filters = PublicOrderFilters {
                surface: OrderSurface::Orderbook,
                market_product: Some(&query.market_product),
                delivery_point_id: Some(query.delivery_point_id),
                availability_window: Some(&query.availability_window),
                ..Default::default()
            };
            let rows =
                bind_order_query(&sql, &policy, &VisibilityClock::at(now), &config, &filters)
                    .map_err(|error| query_error(PATH, &error))?
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|error| query_error(PATH, &error))?;
            // A separate disposable test server can stop exactly between the
            // two reads. This seam is absent from production builds.
            #[cfg(feature = "contract-tests")]
            if config.environment == "test"
                && let Ok(key) = std::env::var("RUST_SNAPSHOT_TEST_BARRIER")
            {
                let key: i64 = key.parse().map_err(|_| ApiError::internal())?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(key)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| query_error(PATH, &error))?;
            }
            let orders: Vec<_> = rows
                .iter()
                .map(PublicOrder::from_row)
                .collect::<Result<_, _>>()?;
            let keys: Vec<_> = orders
                .iter()
                .filter_map(|order| order.benchmark_key(now).transpose())
                .collect::<Result<_, _>>()?;
            let mut cache = LiveBenchmarkCache::default();
            let benchmarks = cache.load(&mut tx, &policy, &config, &keys, now).await?;
            let response = project_snapshot(&query, now, &orders, &benchmarks)?;
            tx.commit()
                .await
                .map_err(|error| query_error(PATH, &error))?;
            Ok(response)
        })
    })
    .await?;
    Ok(axum::Json(response).into_response())
}

fn project_snapshot(
    query: &SnapshotQuery,
    now: DateTime<Utc>,
    orders: &[PublicOrder],
    benchmarks: &HashMap<LiveBenchmarkKey, Option<LiveBenchmark>>,
) -> Result<Json, ApiError> {
    // The query restricts product/point/window; opposing prices remain separated
    // by provenance, and use only the returned depth, as Python does.
    let mut best_bids: HashMap<&str, Decimal> = HashMap::new();
    let mut best_asks: HashMap<&str, Decimal> = HashMap::new();
    for order in orders {
        let best = if order.side == "BID" {
            &mut best_bids
        } else {
            &mut best_asks
        };
        best.entry(&order.provenance)
            .and_modify(|price| {
                *price = if order.side == "BID" {
                    (*price).max(order.price)
                } else {
                    (*price).min(order.price)
                };
            })
            .or_insert(order.price);
    }
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for order in orders {
        let opposing = if order.side == "BID" {
            &best_asks
        } else {
            &best_bids
        };
        let crossed = opposing
            .get(order.provenance.as_str())
            .is_some_and(|price| {
                if order.side == "BID" {
                    order.price >= *price
                } else {
                    order.price <= *price
                }
            });
        let key = order.benchmark_key(now)?;
        let benchmark = key
            .as_ref()
            .and_then(|key| benchmarks.get(key))
            .and_then(Option::as_ref);
        let item = order.public(crossed, benchmark)?;
        if order.side == "BID" {
            bids.push(item)
        } else {
            asks.push(item)
        }
    }
    let real = orders.iter().any(|order| order.provenance == "REAL");
    let demo = orders.iter().any(|order| order.provenance == "DEMO");
    let (source, status) = match (real, demo) {
        (true, true) => ("MIXED_SOURCE", "MIXED"),
        (true, false) => ("LIVE_ORDER", "REAL_ONLY"),
        (false, true) => ("DEMO_SEED", "DEMO_ONLY"),
        (false, false) => ("NO_DATA", "NOT_APPLICABLE"),
    };
    Ok(
        json!({"market_product":query.market_product,"delivery_point_id":query.delivery_point_id,"availability_window":query.availability_window,"generated_at":pydantic_datetime(now),"source_kind":source,"scope":"DELIVERY_POINT","demo_status":status,"bids":bids,"asks":asks}),
    )
}
