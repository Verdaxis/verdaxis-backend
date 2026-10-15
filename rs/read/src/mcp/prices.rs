//! Anonymous MCP market snapshots over the shared public-order relation.
use crate::market::visibility::{
    PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
};
use crate::market::windows::normalize_window;
use crate::routes::curves_canonical::{benchmarks, curve_projection};
use crate::routes::orderbook_map::map_decimal;
use crate::{ApiError, AppState, Json, run_admitted};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use std::cmp::Ordering;

pub fn project_slice(
    product: &str,
    point: &str,
    window: &str,
    rows: &[Json],
    benchmark: Option<&str>,
    observed_at: &str,
) -> Result<Json, String> {
    let mut best_bid: Option<String> = None;
    let mut best_ask: Option<String> = None;
    let mut bid_count = 0;
    let mut ask_count = 0;
    let mut bid_quantity = "0".to_owned();
    let mut ask_quantity = "0".to_owned();
    let mut real = false;
    let mut demo = false;
    for row in rows {
        let price = row["price"].as_str().ok_or("missing price")?;
        let quantity = row["quantity"].as_str().ok_or("missing quantity")?;
        real |= row["provenance"] == "REAL";
        demo |= row["provenance"] == "DEMO";
        let (best, total, count, direction) = if row["side"] == "BID" {
            (
                &mut best_bid,
                &mut bid_quantity,
                &mut bid_count,
                Ordering::Greater,
            )
        } else {
            (
                &mut best_ask,
                &mut ask_quantity,
                &mut ask_count,
                Ordering::Less,
            )
        };
        if best
            .as_ref()
            .map(|previous| map_decimal::compare(price, previous))
            .transpose()?
            .is_none_or(|order| order == direction)
        {
            *best = Some(price.to_owned());
        }
        *total = map_decimal::add(total, quantity)?;
        *count += 1;
    }
    let (status, evidence) = match (real, demo) {
        (true, true) => (
            "MIXED",
            "a mix of live orders from real organizations and demo preview liquidity",
        ),
        (true, false) => ("REAL_ONLY", "live orders from real organizations"),
        (false, true) => (
            "DEMO_ONLY",
            "demo preview liquidity, which is never executable",
        ),
        (false, false) => ("NOT_APPLICABLE", "no open orders"),
    };
    let suffix = if benchmark.is_some() {
        "; the benchmark is an indicative reference, not an executable price"
    } else {
        ""
    };
    Ok(json!({
        "market_product":product,"delivery_point":point,"availability_window":window,
        "best_bid_usd_per_mt":best_bid,"best_ask_usd_per_mt":best_ask,
        "bid_order_count":bid_count,"ask_order_count":ask_count,
        "bid_quantity_mt":bid_quantity,"ask_quantity_mt":ask_quantity,
        "demo_status":status,"benchmark_usd_per_mt":benchmark,"observed_at":observed_at,
        "note":format!("This slice shows {evidence}{suffix}.")
    }))
}

fn invalid(message: String) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!(message),
        retry_after: None,
    }
}

pub async fn market_slice(
    state: &AppState,
    product: &str,
    point: &str,
    window: &str,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    let product = product.to_owned();
    let point = point.to_owned();
    let window = window.to_owned();
    let owned = state.clone();
    run_admitted(state, move |connection| Box::pin(async move {
        // Python validates these arguments inside tool admission, before SQL.
        let window = normalize_window(&window, now.date_naive()).map_err(invalid)?;
        let product_id = owned.policy.products.iter().find(|value| value.code == product).map(|value| value.id).ok_or_else(|| {
            let mut codes: Vec<_> = owned.policy.products.iter().map(|value| value.code.as_str()).collect();
            codes.sort();
            invalid(format!("Unsupported market product {product:?}. Supported products: {}", codes.join(", ")))
        })?;
        let point_id = owned.policy.points.iter().find(|value| value.name == point).map(|value| value.id).ok_or_else(|| invalid(format!("Unsupported delivery point {point:?}. Supported delivery points: {}", owned.policy.points.iter().map(|value| value.name.as_str()).collect::<Vec<_>>().join(", "))))?;
        let query = order_query_sql("SELECT side::text AS side, price_per_mt_usd::text AS price, remaining_quantity_mt::text AS quantity, provenance::text AS provenance FROM eligible_orders");
        let filters = PublicOrderFilters { product_id: Some(product_id), delivery_point_id: Some(point_id), availability_window: Some(&window), ..Default::default() };
        let rows = bind_order_query(&query, &owned.policy, &VisibilityClock::at(now), &owned.config, &filters)
            .map_err(|_| ApiError::internal())?.fetch_all(&mut *connection).await.map_err(|_| ApiError::internal())?;
        let rows: Vec<_> = rows.iter().map(|row| json!({"side":row.get::<String,_>("side"),"price":row.get::<String,_>("price"),"quantity":row.get::<String,_>("quantity"),"provenance":row.get::<String,_>("provenance")})).collect();
        let key = (product.clone(), point_id.to_string(), window.clone());
        let quotes = benchmarks::load(connection, &json!([{"market_product":product,"delivery_point_id":point_id,"availability_window":window}])).await?;
        let quote = benchmarks::board_quote(quotes.get(&key), &key, now);
        let observed = curve_projection::timestamp(now);
        project_slice(&product, &point, &window, &rows, quote["price"].as_str(), observed.as_str().ok_or_else(ApiError::internal)?).map_err(|_| ApiError::internal())
    })).await
}
