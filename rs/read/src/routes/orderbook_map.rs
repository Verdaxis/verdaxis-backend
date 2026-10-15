//! Public map projections over the shared eligible-order relation.
use std::collections::{BTreeMap, HashMap};

use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
#[path = "../market/map_decimal.rs"]
pub mod map_decimal;
use serde_json::json;
use sqlx::{Connection, Row, postgres::PgRow};
use uuid::Uuid;

use super::{
    ports::{pydantic_datetime, query_error},
    prices::{captured_now, fields, integer, uuid},
};
use crate::market::{
    visibility::{PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql},
    windows::{normalize_window, window_sort_key},
};
use crate::{ApiError, ApiResult, AppState, Json, run_admitted};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/orderbook/aggregated", get(aggregate))
        .route("/api/orderbook/product-counts", get(counts))
        .route("/api/orderbook/map-summary", get(summary))
        .route("/api/orderbook/map-summary/compact", get(compact))
        .route("/api/orderbook/products", get(products))
        .route("/api/orderbook/regions", get(regions))
        .route("/api/orderbook/fuel-types", get(fuel_types))
}

pub fn product_counts(codes: &[String], rows: &[(String, u64)]) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<_, _> = codes.iter().map(|code| (code.clone(), 0)).collect();
    for (code, count) in rows {
        if let Some(value) = counts.get_mut(code) {
            *value = *count;
        }
    }
    counts
}

#[derive(Default)]
struct Filters {
    product: Option<Uuid>,
    point: Option<Uuid>,
    side: Option<String>,
    fuel: Option<String>,
    market: Option<String>,
    region: Option<String>,
    window: Option<String>,
    off_spec: bool,
    limit: Option<i64>,
}

fn validation(errors: Vec<Json>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Json::Array(errors),
        retry_after: None,
    }
}

impl Filters {
    fn parse(raw: &str, now: DateTime<Utc>, count: bool) -> Result<Self, ApiError> {
        let values = fields(raw);
        let mut errors = Vec::new();
        let mut result = Self::default();
        let mut collect = |error: ApiError| {
            errors.extend(error.detail.as_array().into_iter().flatten().cloned());
        };
        if !count {
            match uuid(raw, "product_id") {
                Ok(value) => result.product = value,
                Err(error) => collect(error),
            }
        }
        match uuid(raw, "delivery_point_id") {
            Ok(value) => result.point = value,
            Err(error) => collect(error),
        }
        if !count {
            match integer(raw, "limit", 1, 2048, 1024) {
                Ok(value) => result.limit = Some(value),
                Err(error) => collect(error),
            }
        }
        if count {
            match values.get("side") {
                None => errors.insert(0, json!({"type":"missing","loc":["query","side"],"msg":"Field required","input":null})),
                Some(value) if value == "BID" || value == "ASK" => result.side = Some(value.clone()),
                Some(value) => errors.insert(0, json!({"type":"enum","loc":["query","side"],"msg":"Input should be 'BID' or 'ASK'","input":value,"ctx":{"expected":"'BID' or 'ASK'"}})),
            }
        } else if let Some(value) = values.get("market_product") {
            let data: Json = serde_json::from_str(include_str!("../market/orderbook_query.json"))
                .map_err(|_| ApiError::internal())?;
            let variants = data["market_product_enum"]
                .as_array()
                .ok_or_else(ApiError::internal)?;
            if !variants
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
                errors.push(json!({"type":"enum","loc":["query","market_product"],"msg":format!("Input should be {expected}"),"input":value,"ctx":{"expected":expected}}));
            } else {
                result.market = Some(value.clone());
            }
        }
        if let Some(value) = values.get("include_off_spec") {
            match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "t" | "on" | "yes" | "y" => result.off_spec = true,
                "0" | "false" | "f" | "off" | "no" | "n" => {},
                _ => errors.push(json!({"type":"bool_parsing","loc":["query","include_off_spec"],"msg":"Input should be a valid boolean, unable to interpret input","input":value})),
            }
        }
        // FastAPI reports field errors in declaration order, before handler validation.
        let order = if count {
            vec!["side", "delivery_point_id", "include_off_spec"]
        } else {
            vec![
                "product_id",
                "delivery_point_id",
                "market_product",
                "include_off_spec",
                "limit",
            ]
        };
        errors.sort_by_key(|error| {
            order
                .iter()
                .position(|name| Some(*name) == error["loc"][1].as_str())
        });
        if !errors.is_empty() {
            return Err(validation(errors));
        }
        result.region = values.get("region").filter(|v| !v.is_empty()).cloned();
        if !count {
            result.fuel = values.get("fuel_type").filter(|v| !v.is_empty()).cloned();
        }
        result.window = values
            .get("availability_window")
            .filter(|v| !v.is_empty())
            .map(|value| normalize_window(value, now.date_naive()))
            .transpose()
            .map_err(|message| ApiError {
                status: StatusCode::BAD_REQUEST,
                detail: json!(message),
                retry_after: None,
            })?;
        Ok(result)
    }
    fn shared(&self) -> PublicOrderFilters<'_> {
        PublicOrderFilters {
            product_id: self.product,
            delivery_point_id: self.point,
            side: self.side.as_deref(),
            fuel_type: self.fuel.as_deref(),
            market_product: self.market.as_deref(),
            region: self.region.as_deref(),
            availability_window: self.window.as_deref(),
            include_off_spec: self.off_spec,
            ..Default::default()
        }
    }
}

fn text(row: &PgRow, name: &str) -> Result<String, ApiError> {
    row.try_get(name).map_err(|_| ApiError::internal())
}
fn number(row: &PgRow, name: &str) -> Result<i64, ApiError> {
    row.try_get(name).map_err(|_| ApiError::internal())
}
fn timestamp(row: &PgRow, name: &str) -> Result<String, ApiError> {
    row.try_get::<DateTime<Utc>, _>(name)
        .map(pydantic_datetime)
        .map_err(|_| ApiError::internal())
}
fn identity(row: &PgRow) -> Result<Json, ApiError> {
    let product: Uuid = row
        .try_get("product_id")
        .map_err(|_| ApiError::internal())?;
    let point: Uuid = row
        .try_get("delivery_point_id")
        .map_err(|_| ApiError::internal())?;
    Ok(
        json!({"product_id":product,"product_name":text(row,"product_name")?,"market_product":text(row,"market_product")?,"fuel_type":text(row,"product_fuel_type")?,"delivery_point_id":point,"delivery_point_name":text(row,"delivery_point_name")?}),
    )
}
fn evidence(out: &mut Json, class: &str) {
    out["evidence_class"] = json!(class);
    out["source_kind"] = json!(if class == "REAL" {
        "LIVE_ORDER"
    } else {
        "DEMO_SEED"
    });
    out["scope"] = json!("DELIVERY_POINT");
    out["demo_status"] = json!(if class == "REAL" {
        "REAL_ONLY"
    } else {
        "DEMO_ONLY"
    });
}
fn aggregate_row(row: &PgRow) -> Result<Json, ApiError> {
    let mut out = identity(row)?;
    out["availability_window"] = json!(text(row, "availability_window")?);
    out["region"] = json!(text(row, "delivery_point_region")?);
    out["side"] = json!(text(row, "side")?);
    for name in ["min_price", "max_price", "total_quantity"] {
        out[name] = json!(text(row, &format!("{name}_text"))?);
    }
    for name in ["order_count", "product_total_order_count"] {
        out[name] = json!(number(row, name)?);
    }
    evidence(&mut out, &text(row, "provenance")?);
    out["observed_at"] = json!(timestamp(row, "observed_at")?);
    Ok(out)
}
async fn aggregates(
    connection: &mut sqlx::PgConnection,
    state: &AppState,
    now: DateTime<Utc>,
    filters: &Filters,
    path: &str,
) -> Result<Vec<Json>, ApiError> {
    let sql = order_query_sql(include_str!("../../sql/orderbook/aggregate.sql"));
    let rows = bind_order_query(
        &sql,
        &state.policy,
        &VisibilityClock::at(now),
        &state.config,
        &filters.shared(),
    )
    .map_err(|error| query_error(path, &error))?
    .bind(filters.limit)
    .fetch_all(connection)
    .await
    .map_err(|error| query_error(path, &error))?;
    rows.iter().map(aggregate_row).collect()
}
async fn recent(
    connection: &mut sqlx::PgConnection,
    state: &AppState,
    now: DateTime<Utc>,
    path: &str,
) -> Result<Vec<Json>, ApiError> {
    let sql = order_query_sql(include_str!("../../sql/orderbook/recent_asks.sql"));
    let rows = bind_order_query(
        &sql,
        &state.policy,
        &VisibilityClock::at(now),
        &state.config,
        &PublicOrderFilters {
            side: Some("ASK"),
            ..Default::default()
        },
    )
    .map_err(|error| query_error(path, &error))?
    .fetch_all(connection)
    .await
    .map_err(|error| query_error(path, &error))?;
    rows.iter()
        .map(|row| {
            let mut out = identity(row)?;
            out["region"] = json!(text(row, "delivery_point_region")?);
            out["price_per_mt_usd"] = json!(text(row, "price_text")?);
            out["remaining_quantity_mt"] = json!(text(row, "quantity_text")?);
            out["created_at"] = json!(timestamp(row, "created_at")?);
            evidence(&mut out, &text(row, "provenance")?);
            Ok(out)
        })
        .collect()
}
async fn aggregate(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult {
    let now = captured_now(&state)?;
    let filters = Filters::parse(raw.as_deref().unwrap_or_default(), now, false)?;
    let owned = state.clone();
    let result = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let path = "/api/orderbook/aggregated";
            let mut tx = map_transaction(connection, path).await?;
            let result = aggregates(&mut tx, &owned, now, &filters, path).await?;
            tx.commit().await.map_err(|e| query_error(path, &e))?;
            Ok(result)
        })
    })
    .await?;
    Ok(axum::Json(result).into_response())
}
async fn counts(State(state): State<AppState>, RawQuery(raw): RawQuery) -> ApiResult {
    let now = captured_now(&state)?;
    let filters = Filters::parse(raw.as_deref().unwrap_or_default(), now, true)?;
    let owned = state.clone();
    let result = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let path = "/api/orderbook/product-counts";
            let mut tx = map_transaction(connection, path).await?;
            let sql = order_query_sql(include_str!("../../sql/orderbook/counts.sql"));
            let rows = bind_order_query(
                &sql,
                &owned.policy,
                &VisibilityClock::at(now),
                &owned.config,
                &filters.shared(),
            )
            .map_err(|error| query_error(path, &error))?
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| query_error(path, &error))?;
            tx.commit().await.map_err(|e| query_error(path, &e))?;
            let values: Vec<_> = rows
                .iter()
                .map(|row| {
                    Ok((
                        text(row, "market_product")?,
                        number(row, "order_count")? as u64,
                    ))
                })
                .collect::<Result<_, ApiError>>()?;
            let codes: Vec<_> = owned
                .policy
                .products
                .iter()
                .filter(|product| product.execution_mode == "ORDERBOOK")
                .map(|product| product.code.clone())
                .collect();
            let counts = product_counts(&codes, &values);
            // Python initializes the dict in canonical catalog order, not lexical order.
            let ordered: serde_json::Map<_, _> = codes
                .iter()
                .map(|code| (code.clone(), json!(counts[code])))
                .collect();
            Ok(json!({"counts":ordered,"total":counts.values().sum::<u64>()}))
        })
    })
    .await?;
    Ok(axum::Json(result).into_response())
}
async fn summary(State(state): State<AppState>) -> ApiResult {
    map_response(state, false).await
}
async fn compact(State(state): State<AppState>) -> ApiResult {
    map_response(state, true).await
}
async fn map_response(state: AppState, compact: bool) -> ApiResult {
    let now = captured_now(&state)?;
    let owned = state.clone();
    let result = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let path = "/api/orderbook/map-summary";
            let mut tx = map_transaction(connection, path).await?;
            let groups = aggregates(&mut tx, &owned, now, &Filters::default(), path).await?;
            let (markets, demo_groups) = if compact {
                match compact_groups(&groups, now.date_naive()) {
                    Ok(groups) => groups,
                    Err(_) => return Ok(crate::runtime::http::unhandled_server_error()),
                }
            } else {
                (Vec::new(), Vec::new())
            };
            let asks = recent(&mut tx, &owned, now, path).await?;
            tx.commit().await.map_err(|e| query_error(path, &e))?;
            Ok(axum::Json(if compact {
                json!({"markets":markets,"demo_groups":demo_groups,"recent_asks":asks})
            } else {
                json!({"groups":groups,"recent_asks":asks})
            })
            .into_response())
        })
    })
    .await?;
    Ok(result)
}

fn compare_amount(left: &Json, right: &Json) -> Result<std::cmp::Ordering, ApiError> {
    map_decimal::compare(
        left.as_str().ok_or_else(ApiError::internal)?,
        right.as_str().ok_or_else(ApiError::internal)?,
    )
    .map_err(|_| ApiError::internal())
}

fn observed(value: &Json) -> Result<DateTime<chrono::FixedOffset>, ApiError> {
    DateTime::parse_from_rfc3339(value.as_str().ok_or_else(ApiError::internal)?)
        .map_err(|_| ApiError::internal())
}

/// Reduce all windows while keeping each evidence class separate.
pub fn compact_groups(
    groups: &[Json],
    today: NaiveDate,
) -> Result<(Vec<Json>, Vec<Json>), ApiError> {
    let mut markets: BTreeMap<(String, String, String), Json> = BTreeMap::new();
    type DemoChoice = ((u8, NaiveDate, u8), String, Vec<Json>);
    let mut demo: HashMap<(String, String), DemoChoice> = HashMap::new();
    for group in groups {
        let s = |name| group[name].as_str().ok_or_else(ApiError::internal);
        let key = (
            s("market_product")?.to_owned(),
            s("delivery_point_name")?.to_owned(),
            s("evidence_class")?.to_owned(),
        );
        let market = markets.entry(key).or_insert_with(|| {
            let mut out = json!({});
            for name in [
                "product_id",
                "product_name",
                "market_product",
                "fuel_type",
                "delivery_point_id",
                "delivery_point_name",
                "region",
                "evidence_class",
                "source_kind",
                "scope",
                "demo_status",
            ] {
                out[name] = group[name].clone();
            }
            for side in ["bid", "ask"] {
                for field in ["min_price", "max_price"] {
                    out[format!("{side}_{field}")] = Json::Null;
                }
                out[format!("{side}_total_quantity")] = json!("0");
                out[format!("{side}_order_count")] = json!(0);
            }
            out["spot_best_bid"] = Json::Null;
            out["spot_best_ask"] = Json::Null;
            out["observed_at"] = group["observed_at"].clone();
            out
        });
        if observed(&group["observed_at"])? > observed(&market["observed_at"])? {
            market["observed_at"] = group["observed_at"].clone();
        }
        let side = match s("side")? {
            "BID" => "bid",
            "ASK" => "ask",
            _ => return Err(ApiError::internal()),
        };
        for field in ["min_price", "max_price"] {
            let name = format!("{side}_{field}");
            if market[&name].is_null()
                || (field == "min_price" && compare_amount(&group[field], &market[&name])?.is_lt())
                || (field == "max_price" && compare_amount(&group[field], &market[&name])?.is_gt())
            {
                market[&name] = group[field].clone();
            }
        }
        let quantity = format!("{side}_total_quantity");
        market[&quantity] = json!(
            map_decimal::add(
                market[&quantity].as_str().ok_or_else(ApiError::internal)?,
                group["total_quantity"]
                    .as_str()
                    .ok_or_else(ApiError::internal)?,
            )
            .map_err(|_| ApiError::internal())?
        );
        let count = format!("{side}_order_count");
        market[&count] = json!(
            market[&count].as_u64().ok_or_else(ApiError::internal)?
                + group["order_count"]
                    .as_u64()
                    .ok_or_else(ApiError::internal)?
        );
        if s("availability_window")? == "SPOT" {
            let name = format!("spot_best_{side}");
            let price = &group[if side == "bid" {
                "max_price"
            } else {
                "min_price"
            }];
            if market[&name].is_null()
                || (side == "bid" && compare_amount(price, &market[&name])?.is_gt())
                || (side == "ask" && compare_amount(price, &market[&name])?.is_lt())
            {
                market[&name] = price.clone();
            }
        }
        if s("evidence_class")? == "DEMO" {
            let window = s("availability_window")?;
            let window_key = window_sort_key(window, today).map_err(|_| ApiError::internal())?;
            let key = (
                s("product_id")?.to_owned(),
                s("delivery_point_id")?.to_owned(),
            );
            match demo.get_mut(&key) {
                Some(selected) if window_key < selected.0 => {
                    *selected = (window_key, window.into(), vec![group.clone()])
                }
                Some(selected) if window_key == selected.0 && window == selected.1 => {
                    selected.2.push(group.clone())
                }
                None => {
                    demo.insert(key, (window_key, window.into(), vec![group.clone()]));
                }
                _ => {}
            }
        }
    }
    let mut demo_groups: Vec<_> = demo.into_values().flat_map(|choice| choice.2).collect();
    demo_groups.sort_by(|a, b| {
        for field in [
            "market_product",
            "delivery_point_name",
            "evidence_class",
            "side",
        ] {
            let order = a[field].as_str().cmp(&b[field].as_str());
            if !order.is_eq() {
                return order;
            }
        }
        std::cmp::Ordering::Equal
    });
    Ok((markets.into_values().collect(), demo_groups))
}
async fn products(State(state): State<AppState>) -> ApiResult {
    distinct(state, "products").await
}
async fn regions(State(state): State<AppState>) -> ApiResult {
    distinct(state, "regions").await
}
async fn fuel_types(State(state): State<AppState>) -> ApiResult {
    distinct(state, "fuel-types").await
}
async fn distinct(state: AppState, field: &'static str) -> ApiResult {
    let now = captured_now(&state)?;
    let owned = state.clone();
    let result = run_admitted(&state, move |connection| {
        Box::pin(async move {
            let sql = order_query_sql(include_str!("../../sql/orderbook/distinct.sql"));
            let path = format!("/api/orderbook/{field}");
            let mut tx = map_transaction(connection, &path).await?;
            let rows = bind_order_query(
                &sql,
                &owned.policy,
                &VisibilityClock::at(now),
                &owned.config,
                &PublicOrderFilters::default(),
            )
            .map_err(|error| query_error(&path, &error))?
            .bind(field)
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| query_error(&path, &error))?;
            tx.commit().await.map_err(|e| query_error(&path, &e))?;
            let mut values: Vec<String> = rows
                .iter()
                .map(|row| text(row, "value"))
                .collect::<Result<_, _>>()?;
            if field == "fuel-types" {
                values.sort();
            }
            Ok(values)
        })
    })
    .await?;
    Ok(axum::Json(result).into_response())
}

/// Keep expensive admission-expression JIT compilation out of bounded reads.
/// The setting ends with this transaction and cannot leak through the pool.
async fn map_transaction<'a>(
    connection: &'a mut sqlx::PgConnection,
    path: &str,
) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, ApiError> {
    let mut tx = connection
        .begin()
        .await
        .map_err(|e| query_error(path, &e))?;
    sqlx::query("SET LOCAL jit = off")
        .execute(&mut *tx)
        .await
        .map_err(|e| query_error(path, &e))?;
    Ok(tx)
}
