//! R09–R11: canonical table/slice and the separate older board contract.
#[path = "curves_canonical_views.rs"]
mod views;
use views::{load_board, slice_projection, table_projection};
#[path = "../market/benchmarks.rs"]
pub mod benchmarks;
#[path = "../market/curve_projection.rs"]
pub mod curve_projection;
#[path = "../market/curve_unknown_metadata.rs"]
pub mod curve_unknown_metadata;
#[path = "../market/signals.rs"]
pub mod signals;

use crate::market::{
    visibility::{
        OrderSurface, PublicOrderFilters, TradeEvidenceFilters, VisibilityClock, bind_order_query,
        bind_trade_query, order_query_sql, trade_query_sql,
    },
    windows::{normalize_window, window_display_label, window_sort_key},
};
use crate::{ApiError, AppState, Json, run_admitted};
use axum::{
    Router,
    extract::{RawQuery, State},
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use curve_projection::{Book, decimal, default_windows, money, timestamp};
use serde_json::json;
use signals::SliceKey;
use sqlx::{PgConnection, Row};
use std::collections::{HashMap, HashSet};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/curves/forward/table", get(table))
        .route("/api/curves/forward/slice", get(slice))
        .route("/api/curves/forward/board", get(board))
}
async fn table(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<impl IntoResponse, ApiError> {
    request(state, raw, "table").await
}
async fn slice(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<impl IntoResponse, ApiError> {
    request(state, raw, "slice").await
}
async fn board(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<impl IntoResponse, ApiError> {
    request(state, raw, "board").await
}
async fn request(
    state: AppState,
    raw: Option<String>,
    shape: &str,
) -> Result<axum::response::Response, ApiError> {
    let mut query = json!({"shape":shape});
    for (name, value) in url::form_urlencoded::parse(raw.as_deref().unwrap_or("").as_bytes()) {
        if shape == "table"
            && ["windows", "market_products", "delivery_point_ids"].contains(&name.as_ref())
        {
            if query.get(name.as_ref()).is_none() {
                query[name.as_ref()] = json!([]);
            }
            query[name.as_ref()]
                .as_array_mut()
                .unwrap()
                .push(json!(value));
        } else if (shape == "slice"
            && ["market_product", "delivery_point_id", "availability_window"]
                .contains(&name.as_ref()))
            || (shape == "board"
                && [
                    "focus_market_product",
                    "focus_delivery_point_id",
                    "availability_window",
                ]
                .contains(&name.as_ref()))
        {
            query[name.as_ref()] = json!(value);
        }
    }
    // Route identity is not a caller-controlled query parameter.
    query["shape"] = json!(shape);
    let clock = crate::runtime::trading_routes::capture(&state)?;
    let now = clock.wall.into();
    validate(&state, &query)?;
    if shape == "board"
        && let Some(response) = board_window_error(&query, now)
    {
        return Ok(response);
    }
    let copy = state.clone();
    let key = query.to_string();
    let mut value = crate::runtime::trading_routes::read(&state, "/api/curves/forward", key, "curves", clock, None,
        move |connection| Box::pin(async move { load_curve_on(connection, &copy, &query, now).await })).await?;
    crate::runtime::trading_routes::generated_at(&mut value, now, false);
    Ok(axum::Json(value).into_response())
}
fn board_window_error(query: &Json, now: DateTime<Utc>) -> Option<axum::response::Response> {
    // Unlike table/slice, Python leaves the board's ValueError unhandled.
    normalize_window(
        query["availability_window"].as_str().unwrap_or("SPOT"),
        now.date_naive(),
    )
    .err()
    .map(|_| crate::runtime::http::unhandled_server_error())
}

fn invalid(message: impl Into<String>) -> ApiError {
    super::prices::invalid(message)
}
fn validate(state: &AppState, query: &Json) -> Result<(), ApiError> {
    let shape = query["shape"].as_str().unwrap_or("slice");
    let mut errors = Vec::new();
    let names: &[(&str, &str, bool)] = match shape {
        "table" => &[
            ("market_products", "enum", false),
            ("delivery_point_ids", "uuid", false),
        ],
        "board" => &[
            ("focus_market_product", "enum", false),
            ("focus_delivery_point_id", "uuid", false),
        ],
        _ => &[
            ("market_product", "enum", true),
            ("delivery_point_id", "uuid", true),
            ("availability_window", "string", true),
        ],
    };
    let codes: Vec<_> = benchmarks::policy()["market_product_enum"]
        .as_array()
        .expect("committed enum order")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    let _ = state;
    let expected = codes
        .iter()
        .enumerate()
        .map(|(i, code)| {
            format!(
                "{}'{}'",
                if i == 0 {
                    ""
                } else if i + 1 == codes.len() {
                    " or "
                } else {
                    ", "
                },
                code
            )
        })
        .collect::<String>();
    for &(name, kind, required) in names {
        if query.get(name).is_none() {
            if required {
                errors.push(json!({"type":"missing","loc":["query",name],"msg":"Field required","input":null}));
            }
            continue;
        }
        let values = query[name]
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![query[name].clone()]);
        for (index, value) in values.iter().enumerate() {
            let mut loc = json!(["query", name]);
            if shape == "table" {
                loc.as_array_mut().unwrap().push(json!(index));
            }
            let text = value.as_str().unwrap_or("");
            if kind == "enum" && !codes.contains(&text) {
                errors.push(json!({"type":"enum","loc":loc,"msg":format!("Input should be {expected}"),"input":value,"ctx":{"expected":expected}}));
            }
            if kind == "uuid" && uuid::Uuid::parse_str(text).is_err() {
                let parsed = crate::runtime::http::QueryFields::parse(&format!("{name}={text}"));
                if let Err(error) =
                    parsed.validate(name, crate::runtime::http::QueryKind::Uuid, true)
                {
                    let mut entry = error.detail[0].clone();
                    entry["loc"] = loc;
                    errors.push(entry);
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError {
            status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            detail: json!(errors),
            retry_after: None,
            kind: crate::ApiErrorKind::Expected,
        })
    }
}

pub async fn load_curve(
    state: &AppState,
    query: &Json,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    validate(state, query)?;
    let state_copy = state.clone();
    let query = query.clone();
    run_admitted(state, move |connection| {
        Box::pin(async move {
            load_curve_on(connection, &state_copy, &query, now).await
        })
    })
    .await
}

async fn load_curve_on(connection: &mut PgConnection, state: &AppState, query: &Json, now: DateTime<Utc>) -> Result<Json, ApiError> {
    use sqlx::Connection;
    let mut transaction = connection.begin().await.map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    sqlx::query("SET LOCAL jit = off").execute(&mut *transaction).await.map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    let result = load(&mut transaction, state, query, now).await?;
    transaction.commit().await.map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    Ok(result)
}

#[derive(Clone)]
struct Product {
    id: String,
    code: String,
    name: String,
    profile: String,
    lanes: Vec<String>,
}
#[derive(Clone)]
struct Point {
    id: String,
    name: String,
    region: String,
}
struct Inputs {
    books: HashMap<SliceKey, Book>,
    depth: (Vec<Json>, Vec<Json>),
    trades: HashMap<SliceKey, Vec<Json>>,
    benchmarks: HashMap<SliceKey, Json>,
    signals: signals::Signals,
}
async fn catalog(
    connection: &mut PgConnection,
    state: &AppState,
) -> Result<(Vec<Product>, Vec<Point>), ApiError> {
    let policy = serde_json::to_string(state.policy.as_ref()).map_err(|_| ApiError::internal())?;
    let products = sqlx::query(sqlx::AssertSqlSafe(include_str!(
        "../../sql/catalog_products.sql"
    )))
    .bind(&policy)
    .fetch_all(&mut *connection)
    .await
    .map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    let points = sqlx::query(sqlx::AssertSqlSafe(include_str!(
        "../../sql/catalog_points.sql"
    )))
    .bind(&policy)
    .fetch_all(&mut *connection)
    .await
    .map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    let products = products
        .into_iter()
        .map(|row| {
            let id: String = row.get("id");
            let p = state
                .policy
                .products
                .iter()
                .find(|p| p.id.to_string() == id)
                .expect("joined policy");
            Product {
                id,
                code: p.code.clone(),
                name: p.name.clone(),
                profile: p.qualification_profile.clone(),
                lanes: p.lanes.iter().map(ToString::to_string).collect(),
            }
        })
        .collect();
    let points = points
        .into_iter()
        .map(|r| Point {
            id: r.get("id"),
            name: r.get("name"),
            region: r.get("region"),
        })
        .collect();
    Ok((products, points))
}
fn key(product: &Product, point: &Point, window: &str) -> SliceKey {
    (product.code.clone(), point.id.clone(), window.to_owned())
}
fn keys_json(keys: &[SliceKey]) -> Json {
    json!(
        keys.iter()
            .map(
                |k| json!({"market_product":k.0,"delivery_point_id":k.1,"availability_window":k.2})
            )
            .collect::<Vec<_>>()
    )
}
async fn inputs(
    connection: &mut PgConnection,
    state: &AppState,
    products: &[Product],
    keys: &[SliceKey],
    now: DateTime<Utc>,
    table: bool,
    board_focus: Option<(&Product, &Point, &str)>,
) -> Result<Inputs, ApiError> {
    let mut order_keys: Vec<Json> = keys.iter().map(|k| json!({"product_id":products.iter().find(|p|p.code==k.0).unwrap().id,"delivery_point_id":k.1,"availability_window":k.2})).collect();
    if let Some((p, point, _)) = board_focus {
        order_keys.push(
            json!({"product_id":p.id,"delivery_point_id":point.id,"availability_window":null}),
        );
    }
    let clock = VisibilityClock::at(now);
    let query = order_query_sql(include_str!("../../sql/curves/orders.sql"));
    // A single slice can use the shared scalar binds as well as exact keys;
    // this lets PostgreSQL prune before evaluating the owner-admission checks.
    let single_key = (board_focus.is_none() && keys.len() == 1).then(|| &keys[0]);
    let product_id = single_key.map(|key| {
        uuid::Uuid::parse_str(&products.iter().find(|p| p.code == key.0).unwrap().id).unwrap()
    });
    let point_id = single_key.map(|key| uuid::Uuid::parse_str(&key.1).unwrap());
    let orders = bind_order_query(
        &query,
        &state.policy,
        &clock,
        &state.config,
        &PublicOrderFilters {
            surface: OrderSurface::Candidates,
            product_id,
            delivery_point_id: point_id,
            availability_window: single_key.map(|key| key.2.as_str()),
            ..Default::default()
        },
    )
    .map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?
    .bind(json!(order_keys).to_string())
    .fetch_all(&mut *connection)
    .await
    .map_err(|e| super::ports::query_error("/api/curves/forward/table", &e))?;
    let mut books: HashMap<SliceKey, Book> = HashMap::new();
    for row in orders {
        let product_id: &str = row.get("product_id");
        let product = products.iter().find(|p| p.id == product_id).unwrap();
        let raw: &str = row.get("availability_window");
        let window = normalize_window(raw, now.date_naive()).map_err(invalid)?;
        let book = books
            .entry((product.code.clone(), row.get("delivery_point_id"), window))
            .or_default();
        let real = row.get::<&str, _>("provenance") == "REAL";
        book.merge(
            row.get("side"),
            real,
            curve_projection::BookGroup {
                count: row.get("order_count"),
                volume: decimal(row.get("quantity")),
                best_price: decimal(row.get("price")),
                observed_at: row.get("observed_at"),
                managed_count: row.get("managed_count"),
            },
        );
    }
    let unknown =
        curve_unknown_metadata::load(connection, state, &clock, &json!(order_keys)).await?;
    for row in unknown {
        let product = products.iter().find(|p| row["product_id"] == p.id).unwrap();
        let window = normalize_window(
            row["availability_window"].as_str().unwrap(),
            now.date_naive(),
        )
        .map_err(invalid)?;
        books
            .entry((
                product.code.clone(),
                row["delivery_point_id"].as_str().unwrap().to_owned(),
                window,
            ))
            .or_default()
            .unknown_count += row["unknown_order_count"].as_i64().unwrap();
    }
    let mut all_keys = keys.to_vec();
    if board_focus.is_some() {
        for key in books.keys() {
            if !all_keys.contains(key) {
                all_keys.push(key.clone());
            }
        }
    }
    let encoded = keys_json(&all_keys);
    let query = trade_query_sql(include_str!("../../sql/curves/trades.sql"));
    let rows = if board_focus.is_some() {
        Vec::new()
    } else {
        bind_trade_query(
            &query,
            &state.policy,
            &TradeEvidenceFilters {
                confirmed_since: Some(now - Duration::days(30)),
                ..Default::default()
            },
        )
        .map_err(|e| super::ports::query_error("/api/curves/forward/slice", &e))?
        .bind(encoded.to_string())
        .fetch_all(&mut *connection)
        .await
        .map_err(|e| super::ports::query_error("/api/curves/forward/slice", &e))?
    };
    let mut trades: HashMap<SliceKey, Vec<Json>> = HashMap::new();
    for row in rows {
        let key = signals::key(&row)?;
        let real = row.get::<&str, _>("provenance") == "REAL";
        let entries = trades.entry(key).or_default();
        let status = if real { "REAL_ONLY" } else { "DEMO_ONLY" };
        if entries.first().is_some_and(|v| v["demo_status"] != status) {
            continue;
        }
        if entries.len() < 8 {
            entries.push(json!({"price_per_mt_usd":money(decimal(row.get("price"))),"quantity_mt":money(decimal(row.get("quantity"))),"confirmed_at":timestamp(row.get("confirmed_at")),"source_kind":if real {"CONFIRMED_TRADE"}else{"DEMO_SEED"},"demo_status":status}));
        }
    }
    let benchmarks = benchmarks::load(connection, &encoded).await?;
    let signals = signals::Signals::load(connection, &encoded, now, table).await?;
    let depth = if table {
        (Vec::new(), Vec::new())
    } else {
        let (product_id, point_id, window) = if let Some((product, point, window)) = board_focus {
            (product.id.as_str(), point.id.as_str(), window)
        } else {
            let key = &keys[0];
            (
                products
                    .iter()
                    .find(|p| p.code == key.0)
                    .unwrap()
                    .id
                    .as_str(),
                key.1.as_str(),
                key.2.as_str(),
            )
        };
        load_depth(connection, state, &clock, product_id, point_id, window).await?
    };
    Ok(Inputs {
        books,
        depth,
        trades,
        benchmarks,
        signals,
    })
}

async fn load_depth(
    connection: &mut PgConnection,
    state: &AppState,
    clock: &VisibilityClock,
    product_id: &str,
    point_id: &str,
    window: &str,
) -> Result<(Vec<Json>, Vec<Json>), ApiError> {
    let query = order_query_sql(include_str!("../../sql/curves/depth.sql"));
    let rows = bind_order_query(
        &query,
        &state.policy,
        clock,
        &state.config,
        &PublicOrderFilters {
            surface: OrderSurface::Candidates,
            ..Default::default()
        },
    )
    .map_err(|e| super::ports::query_error("/api/curves/forward/slice", &e))?
    .bind(uuid::Uuid::parse_str(product_id).expect("catalog UUID"))
    .bind(uuid::Uuid::parse_str(point_id).expect("catalog UUID"))
    .bind(window)
    .fetch_all(connection)
    .await
    .map_err(|e| super::ports::query_error("/api/curves/forward/slice", &e))?;
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for row in rows {
        let real = row.get::<&str, _>("provenance") == "REAL";
        let count: i64 = row.get("order_count");
        let value = json!({"price_per_mt_usd":money(decimal(row.get("price"))),"quantity_mt":money(decimal(row.get("quantity"))),"order_count":count,"source_kind":if real{"LIVE_ORDER"}else{"DEMO_SEED"},"demo_status":if real{"REAL_ONLY"}else{"DEMO_ONLY"},"real_order_count":if real{count}else{0},"demo_order_count":if real{0}else{count},"unknown_order_count":0});
        if row.get::<&str, _>("side") == "BID" {
            bids.push(value);
        } else {
            asks.push(value);
        }
    }
    Ok((bids, asks))
}

fn windows(query: &Json, now: DateTime<Utc>) -> Result<Vec<String>, ApiError> {
    let raw = query["windows"]
        .as_array()
        .map(|v| {
            v.iter()
                .map(|s| s.as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_else(|| default_windows(now.date_naive(), false));
    let mut result = Vec::new();
    for value in raw {
        let normalized = strict_window(&value, now)?;
        if !result.contains(&normalized) {
            result.push(normalized);
        }
    }
    let mut keyed = Vec::new();
    for window in result {
        keyed.push((
            window_sort_key(&window, now.date_naive()).map_err(invalid)?,
            window,
        ));
    }
    keyed.sort_by_key(|(key, _)| *key);
    if keyed.len() > 24 {
        return Err(invalid("At most 24 windows may be requested"));
    }
    Ok(keyed.into_iter().map(|(_, w)| w).collect())
}
fn strict_window(value: &str, now: DateTime<Utc>) -> Result<String, ApiError> {
    let normalized = normalize_window(value, now.date_naive()).map_err(invalid)?;
    if value.trim() != normalized
        && !(value.trim().to_uppercase() == "SPOT" && normalized == "SPOT")
    {
        return Err(invalid(
            "availability_window must be canonical: SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL",
        ));
    }
    Ok(normalized)
}
async fn load(
    connection: &mut PgConnection,
    state: &AppState,
    query: &Json,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    let shape = query["shape"].as_str().unwrap_or("slice");
    let (mut products, mut points) = catalog(connection, state).await?;
    if shape == "board" {
        return load_board(connection, state, query, now, products, points).await;
    }
    if shape == "table" {
        if let Some(wanted) = query["market_products"].as_array() {
            products.retain(|p| wanted.contains(&json!(p.code)));
            if wanted
                .iter()
                .any(|v| !products.iter().any(|p| v == &p.code))
            {
                return Err(invalid("a requested market product is not active"));
            }
        }
        if let Some(wanted) = query["delivery_point_ids"].as_array() {
            points.retain(|p| {
                wanted.iter().any(|v| {
                    uuid::Uuid::parse_str(v.as_str().unwrap_or(""))
                        .ok()
                        .map(|id| id.to_string())
                        == Some(p.id.clone())
                })
            });
            if wanted.iter().any(|v| {
                !points.iter().any(|p| {
                    uuid::Uuid::parse_str(v.as_str().unwrap_or(""))
                        .ok()
                        .map(|id| id.to_string())
                        == Some(p.id.clone())
                })
            }) {
                return Err(invalid(
                    "a requested delivery point is not approved or active",
                ));
            }
        }
        let windows = windows(query, now)?;
        let mut keys = Vec::new();
        for product in &products {
            for point in &points {
                if product.lanes.contains(&point.id) {
                    for window in &windows {
                        keys.push(key(product, point, window));
                    }
                }
            }
        }
        let inputs = inputs(connection, state, &products, &keys, now, true, None).await?;
        return table_projection(&products, &points, &windows, &keys, &inputs, now);
    }
    let window = strict_window(query["availability_window"].as_str().unwrap_or(""), now)?;
    let product = products
        .iter()
        .find(|p| query["market_product"] == p.code)
        .ok_or_else(|| invalid("market_product is not active"))?;
    let point_id = uuid::Uuid::parse_str(query["delivery_point_id"].as_str().unwrap_or(""))
        .map_err(|_| invalid("delivery_point_id is not approved or active"))?
        .to_string();
    let point = points
        .iter()
        .find(|p| p.id == point_id)
        .ok_or_else(|| invalid("delivery_point_id is not approved or active"))?;
    if !product.lanes.contains(&point.id) {
        return Err(invalid(
            "market_product is not available at this delivery point",
        ));
    }
    let key = key(product, point, &window);
    let inputs = inputs(
        connection,
        state,
        &products,
        std::slice::from_ref(&key),
        now,
        false,
        None,
    )
    .await?;
    slice_projection(product, point, &key, &inputs, now)
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[tokio::test]
    async fn invalid_board_window_retains_python_unhandled_response() {
        let now = "2026-10-15T12:00:00Z".parse().unwrap();
        let response = board_window_error(&json!({"availability_window":"not-a-window"}), now)
            .expect("Python board normalization failure");
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"Internal Server Error");
    }
}
