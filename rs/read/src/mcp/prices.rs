//! Anonymous MCP market snapshots over the shared public-order relation.
use crate::market::visibility::{
    PublicOrderFilters, VisibilityClock, bind_order_query, curve_candidate_sql,
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

pub async fn voyage(state: &AppState, request: Json, now: DateTime<Utc>) -> Result<Json, ApiError> {
    let owned = state.clone();
    run_admitted(state, move |connection| {
        Box::pin(async move {
            let lookups = super::voyage::required_price_lookups(&request).map_err(invalid)?;
            let prices = resolve_prices(connection, &owned, &lookups, now).await?;
            super::voyage::voyage_plan(&request, &prices, now.date_naive()).map_err(invalid)
        })
    })
    .await
}

async fn resolve_prices(
    connection: &mut sqlx::PgConnection,
    state: &AppState,
    lookups: &Json,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    use chrono::Datelike;
    let lookups = lookups.as_array().ok_or_else(ApiError::internal)?;
    if lookups.is_empty() {
        return Ok(json!([]));
    }
    let mut targets = Vec::new();
    for lookup in lookups {
        let product = state
            .policy
            .products
            .iter()
            .find(|product| lookup["fuel"] == product.code);
        let point = state
            .policy
            .points
            .iter()
            .find(|point| lookup["port"] == point.name);
        if let (Some(product), Some(point)) = (product, point) {
            let date: chrono::NaiveDate = lookup["departure_date"]
                .as_str()
                .ok_or_else(ApiError::internal)?
                .parse()
                .map_err(|_| ApiError::internal())?;
            let month = format!("{}-{:02}", date.year(), date.month());
            let mut windows = vec![
                month.clone(),
                format!("{}-Q{}", date.year(), (date.month() - 1) / 3 + 1),
            ];
            if date <= now.date_naive() + chrono::Duration::days(30) {
                windows.insert(0, "SPOT".into());
            }
            targets.push((lookup.clone(), product.id, point.id, month, windows));
        }
    }
    let keys: Vec<_> = targets.iter().flat_map(|(lookup,product,point,_,windows)|windows.iter().map(move |window|json!({"product_id":product,"delivery_point_id":point,"availability_window":window,"market_product":lookup["fuel"]}))).collect();
    // The requested key set is materialised once and matched as one composite
    // text key through `= ANY(ARRAY(SELECT ...))`, the same measured shape as
    // the curve tails: a per-row EXISTS over jsonb_to_recordset let the planner
    // choose a nested loop and compare three columns per candidate order
    // (measured at the large size as a uniform ~0.6 s tool call against
    // Python's ~0.09 s). The predicate and the row multiset are unchanged.
    let query = curve_candidate_sql(
        ", slice_orders AS (SELECT product_id, delivery_point_id, availability_window, price_per_mt_usd::text AS price, provenance::text AS provenance, carbon_intensity_gco2_mj::text AS ci, energy_density_mj_kg::text AS energy FROM eligible_orders ORDER BY price_per_mt_usd, acceptance_ordinal) SELECT * FROM slice_orders",
    );
    let filters = PublicOrderFilters {
        side: Some("ASK"),
        ..Default::default()
    };
    let rows = bind_order_query(
        &query,
        &state.policy,
        &VisibilityClock::at(now),
        &state.config,
        &filters,
    )
    .map_err(|_| ApiError::internal())?
    .bind(json!(keys).to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| ApiError::database())?;
    let benchmark_keys: Vec<_> = targets.iter().map(|(lookup,_,point,month,_)|json!({"market_product":lookup["fuel"],"delivery_point_id":point,"availability_window":month})).collect();
    let quotes = benchmarks::load(connection, &json!(benchmark_keys)).await?;
    let mut output = Vec::new();
    for (lookup, product, point, month, windows) in targets {
        let row = windows.iter().find_map(|window| {
            rows.iter().find(|row| {
                row.get::<uuid::Uuid, _>("product_id") == product
                    && row.get::<uuid::Uuid, _>("delivery_point_id") == point
                    && row.get::<String, _>("availability_window") == *window
            })
        });
        let price = if let Some(row) = row {
            let real = row.get::<String, _>("provenance") == "REAL";
            json!({"fuel":lookup["fuel"],"port":lookup["port"],"price_usd_per_mt":crate::market::amount::Amount::parse(&row.get::<String,_>("price")).and_then(|v|v.quantize(2)).map_err(|_|ApiError::internal())?,"source":if real{"LIVE_ORDER"}else{"DEMO_ORDER"},"executable":real,"availability_window":row.get::<String,_>("availability_window"),"observed_at":curve_projection::timestamp(now),"carbon_intensity_gco2e_per_mj":row.get::<Option<String>,_>("ci"),"energy_density_mj_per_kg":row.get::<Option<String>,_>("energy")})
        } else {
            let key = (
                lookup["fuel"]
                    .as_str()
                    .ok_or_else(ApiError::internal)?
                    .to_owned(),
                point.to_string(),
                month.clone(),
            );
            let quote = benchmarks::board_quote(quotes.get(&key), &key, now);
            if quote.is_null() {
                continue;
            }
            json!({"fuel":lookup["fuel"],"port":lookup["port"],"price_usd_per_mt":quote["price"],"source":"BENCHMARK","executable":false,"availability_window":month,"observed_at":curve_projection::timestamp(now),"carbon_intensity_gco2e_per_mj":null,"energy_density_mj_per_kg":null})
        };
        output.push(json!({"lookup":lookup,"price":price}));
    }
    Ok(json!(output))
}

fn invalid(message: String) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!(message),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
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
            invalid(format!("Unsupported market product {}. Supported products: {}", super::transport::python_string_repr(&product), codes.join(", ")))
        })?;
        let point_id = owned.policy.points.iter().find(|value| value.name == point).map(|value| value.id).ok_or_else(|| invalid(format!("Unsupported delivery point {}. Supported delivery points: {}", super::transport::python_string_repr(&point), owned.policy.points.iter().map(|value| value.name.as_str()).collect::<Vec<_>>().join(", "))))?;
        let query = curve_candidate_sql("SELECT side::text AS side, price_per_mt_usd::text AS price, remaining_quantity_mt::text AS quantity, provenance::text AS provenance FROM eligible_orders");
        let filters = PublicOrderFilters { product_id: Some(product_id), delivery_point_id: Some(point_id), availability_window: Some(&window), ..Default::default() };
        let rows = bind_order_query(&query, &owned.policy, &VisibilityClock::at(now), &owned.config, &filters)
            .map_err(|_| ApiError::internal())?.fetch_all(&mut *connection).await.map_err(|_| ApiError::database())?;
        let rows: Vec<_> = rows.iter().map(|row| json!({"side":row.get::<String,_>("side"),"price":row.get::<String,_>("price"),"quantity":row.get::<String,_>("quantity"),"provenance":row.get::<String,_>("provenance")})).collect();
        let key = (product.clone(), point_id.to_string(), window.clone());
        let quotes = benchmarks::load(connection, &json!([{"market_product":product,"delivery_point_id":point_id,"availability_window":window}])).await?;
        let quote = benchmarks::board_quote(quotes.get(&key), &key, now);
        let observed = curve_projection::timestamp(now);
        project_slice(&product, &point, &window, &rows, quote["price"].as_str(), observed.as_str().ok_or_else(ApiError::internal)?).map_err(|_| ApiError::internal())
    })).await
}
