//! Paginated public orders and the two legacy list shapes.
use super::{
    ports::query_error,
    prices::{fields, integer, uuid},
};
use crate::market::{
    live_benchmarks::LiveBenchmarkCache,
    order_projection::PublicOrder,
    visibility::{
        OrderSurface, PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
    },
    windows::normalize_window,
};
use crate::{ApiError, ApiResult, AppState, Json};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::json;
use sqlx::{Connection, Row};
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/orderbook/bids", get(bids))
        .route("/api/orderbook/asks", get(asks))
        .route("/api/orderbook", get(list).post(create_order_boundary))
        .route("/api/orderbook/with-ci", get(with_ci))
}

/// Python registers a real POST write route on `/api/orderbook`, so an
/// unauthenticated POST reaches the route's OAuth2 dependency and answers 401
/// instead of the generic 405 the other ported GET paths produce. The read
/// service never executes the write; deployment routing keeps writes on
/// Python, and this boundary reproduces only its authentication answer.
async fn create_order_boundary(headers: axum::http::HeaderMap) -> Response {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let scheme = authorization
        .split_once(' ')
        .map_or(authorization, |(scheme, _)| scheme);
    let detail = if !authorization.is_empty() && scheme.eq_ignore_ascii_case("bearer") {
        "Could not validate credentials"
    } else {
        "Not authenticated"
    };
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
        axum::Json(serde_json::json!({"detail": detail})),
    )
        .into_response()
}

pub fn page_offset(
    skip: u64,
    limit: u64,
    page: Option<u64>,
    page_size: Option<u64>,
) -> Option<(u64, u64)> {
    let size = page_size.unwrap_or(limit);
    let offset = match page {
        Some(p) => p.checked_sub(1)?.checked_mul(size)?,
        None => skip,
    };
    Some((offset, size))
}
#[derive(Clone, Copy, PartialEq)]
enum Surface {
    Bids,
    Asks,
    List,
    Ci,
}
impl Surface {
    fn path(self) -> &'static str {
        match self {
            Self::Bids => "/api/orderbook/bids",
            Self::Asks => "/api/orderbook/asks",
            Self::List => "/api/orderbook",
            Self::Ci => "/api/orderbook/with-ci",
        }
    }
    fn paginated(self) -> bool {
        matches!(self, Self::Bids | Self::Asks)
    }
}
struct ListQuery {
    product: Option<Uuid>,
    point: Option<Uuid>,
    side: Option<String>,
    fuel: Option<String>,
    market: Option<String>,
    region: Option<String>,
    window: Option<String>,
    off_spec: bool,
    sort: String,
    skip: i64,
    limit: i64,
}
fn validation(errors: Vec<Json>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Json::Array(errors),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}
fn parse(raw: &str, surface: Surface, now: DateTime<Utc>) -> Result<ListQuery, ApiError> {
    let values = fields(raw);
    let mut errors = Vec::new();
    let mut get_uuid = |name| match uuid(raw, name) {
        Ok(v) => v,
        Err(e) => {
            errors.extend(e.detail.as_array().unwrap().clone());
            None
        }
    };
    let product = get_uuid("product_id");
    let point = get_uuid("delivery_point_id");
    let market = surface
        .paginated()
        .then(|| values.get("market_product").cloned())
        .flatten();
    if let Some(value) = &market {
        let data: Json = serde_json::from_str(include_str!("../market/orderbook_query.json"))
            .map_err(|_| ApiError::internal())?;
        let variants = data["market_product_enum"]
            .as_array()
            .ok_or_else(ApiError::internal)?;
        if !variants.iter().any(|v| v.as_str() == Some(value)) {
            let labels: Vec<_> = variants
                .iter()
                .filter_map(Json::as_str)
                .map(|v| format!("'{v}'"))
                .collect();
            let (last, rest) = labels.split_last().ok_or_else(ApiError::internal)?;
            let expected = format!("{} or {last}", rest.join(", "));
            errors.push(json!({"type":"enum","loc":["query","market_product"],"msg":format!("Input should be {expected}"),"input":value,"ctx":{"expected":expected}}));
        }
    }
    let side = match surface {
        Surface::Bids => Some("BID".into()),
        Surface::Asks => Some("ASK".into()),
        _ => values.get("side").cloned(),
    };
    if !surface.paginated()
        && let Some(value) = &side
        && value != "BID"
        && value != "ASK"
    {
        errors.push(json!({"type":"enum","loc":["query","side"],"msg":"Input should be 'BID' or 'ASK'","input":value,"ctx":{"expected":"'BID' or 'ASK'"}}));
    }
    let off_spec = match values
        .get("include_off_spec")
        .map(|v| v.to_lowercase())
        .as_deref()
    {
        None | Some("false" | "0" | "off" | "no" | "f" | "n") => false,
        Some("true" | "1" | "on" | "yes" | "t" | "y") => true,
        _ => {
            errors.push(json!({"type":"bool_parsing","loc":["query","include_off_spec"],"msg":"Input should be a valid boolean, unable to interpret input","input":values["include_off_spec"]}));
            false
        }
    };
    let sort = if surface.paginated() {
        values.get("sort_by").cloned().unwrap_or("newest".into())
    } else {
        "newest".into()
    };
    if !["newest", "price_asc", "price_desc", "quantity_desc"].contains(&sort.as_str()) {
        let expected = "'price_asc', 'price_desc', 'quantity_desc' or 'newest'";
        errors.push(json!({"type":"literal_error","loc":["query","sort_by"],"msg":format!("Input should be {expected}"),"input":sort,"ctx":{"expected":expected}}));
    }
    let mut get_int = |name, min, max, default| {
        // Python has no upper bound on skip/page. Keep their wide values until
        // aliases have selected the effective SQL offset.
        if matches!(name, "skip" | "page")
            && let Some(value) = values.get(name)
            && let Some(parsed) = crate::runtime::http::python_integer(value)
            && parsed >= i128::from(min)
        {
            return parsed;
        }
        match integer(raw, name, min, max, default) {
            Ok(v) => i128::from(v),
            Err(e) => {
                errors.extend(e.detail.as_array().unwrap().clone());
                i128::from(default)
            }
        }
    };
    let skip = get_int("skip", 0, i64::MAX, 0);
    let limit = get_int("limit", 1, 100, if surface.paginated() { 20 } else { 50 });
    let (page, size) = if surface == Surface::List {
        (
            values
                .contains_key("page")
                .then(|| get_int("page", 1, i64::MAX, 1)),
            values
                .contains_key("page_size")
                .then(|| get_int("page_size", 1, 100, 50)),
        )
    } else {
        (None, None)
    };
    if !errors.is_empty() {
        return Err(validation(errors));
    }
    let limit = size.unwrap_or(limit);
    let skip = match page {
        Some(page) => (page - 1)
            .checked_mul(limit)
            .ok_or_else(ApiError::internal)?,
        None => skip,
    };
    let window = if surface == Surface::Ci {
        None
    } else {
        values
            .get("availability_window")
            .map(|v| normalize_window(v, now.date_naive()))
            .transpose()
            .map_err(|message| ApiError {
                status: StatusCode::BAD_REQUEST,
                detail: json!(message),
                retry_after: None,
                kind: crate::ApiErrorKind::Expected,
            })?
    };
    Ok(ListQuery {
        product,
        point,
        side,
        fuel: surface
            .paginated()
            .then(|| {
                values
                    .get("fuel_type")
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
            .flatten(),
        market,
        region: surface
            .paginated()
            .then(|| values.get("region").cloned())
            .flatten(),
        window,
        off_spec,
        sort,
        skip: i64::try_from(skip).map_err(|_| ApiError::internal())?,
        limit: limit as i64,
    })
}
async fn bids(State(s): State<AppState>, RawQuery(q): RawQuery) -> ApiResult {
    serve(s, q, Surface::Bids).await
}
async fn asks(State(s): State<AppState>, RawQuery(q): RawQuery) -> ApiResult {
    serve(s, q, Surface::Asks).await
}
async fn list(State(s): State<AppState>, RawQuery(q): RawQuery) -> ApiResult {
    serve(s, q, Surface::List).await
}
async fn with_ci(State(s): State<AppState>, RawQuery(q): RawQuery) -> ApiResult {
    serve(s, q, Surface::Ci).await
}

async fn serve(state: AppState, raw: Option<String>, surface: Surface) -> ApiResult {
    let clock = crate::runtime::trading_routes::capture(&state)?;
    let now = clock.wall.into();
    let raw = raw.unwrap_or_default();
    let query = parse(&raw, surface, now)?;
    let policy = state.policy.clone();
    let config = state.config.clone();
    let body = crate::runtime::trading_routes::read(&state, surface.path(), raw, "orderbook", clock, None, move |connection| {
        Box::pin(async move { load_list(connection, &policy, &config, query, surface, now).await })
    })
    .await?;
    Ok(axum::Json(body).into_response())
}

async fn load_list(
    connection: &mut sqlx::PgConnection,
    policy: &crate::MarketPolicy,
    config: &crate::Config,
    query: ListQuery,
    surface: Surface,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    let path = surface.path();
    let mut tx = connection
        .begin()
        .await
        .map_err(|e| query_error(path, &e))?;
    // The expanded corpus makes eligibility subplans trigger expensive JIT
    // compilation even for tiny pages. Keep planner policy transaction-local.
    sqlx::query("SET LOCAL jit = off")
        .execute(&mut *tx)
        .await
        .map_err(|e| query_error(path, &e))?;
    let clock = VisibilityClock::at(now);
    let filters = PublicOrderFilters {
        surface: OrderSurface::Orderbook,
        product_id: query.product,
        delivery_point_id: query.point,
        side: query.side.as_deref(),
        fuel_type: query.fuel.as_deref(),
        market_product: query.market.as_deref(),
        region: query.region.as_deref(),
        availability_window: query.window.as_deref(),
        include_off_spec: query.off_spec,
        ..Default::default()
    };
    let count = order_query_sql("SELECT count(*) AS total FROM eligible_orders");
    let mut total = 0i64;
    if surface == Surface::Bids {
        total = bind_order_query(&count, policy, &clock, config, &filters)
            .map_err(|e| query_error(path, &e))?
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| query_error(path, &e))?
            .try_get("total")
            .map_err(|_| ApiError::internal())?;
    }
    let sql = order_query_sql(match surface {
        Surface::Bids => include_str!("../../sql/orderbook/bids.sql"),
        Surface::Asks => include_str!("../../sql/orderbook/asks.sql"),
        Surface::List => include_str!("../../sql/orderbook/list.sql"),
        Surface::Ci => include_str!("../../sql/orderbook/with_ci.sql"),
    });
    let rows = bind_order_query(&sql, policy, &clock, config, &filters)
        .map_err(|e| query_error(path, &e))?
        .bind(&query.sort)
        .bind(query.limit)
        .bind(query.skip)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| query_error(path, &e))?;
    if surface == Surface::Asks {
        if let Some(row) = rows.first() {
            total = row.try_get("total").map_err(|_| ApiError::internal())?;
        } else if query.skip > 0 {
            total = bind_order_query(&count, policy, &clock, config, &filters)
                .map_err(|e| query_error(path, &e))?
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| query_error(path, &e))?
                .try_get("total")
                .map_err(|_| ApiError::internal())?;
        }
    }
    let orders: Vec<_> = rows
        .iter()
        .map(PublicOrder::from_row)
        .collect::<Result<_, _>>()?;
    let keys: Vec<_> = orders
        .iter()
        .filter_map(|o| o.benchmark_key(now).transpose())
        .collect::<Result<_, _>>()?;
    let mut cache = LiveBenchmarkCache::default();
    let benchmarks = cache.load(&mut tx, policy, config, &keys, now).await?;
    // Opposing prices use the default public scope, not the requested off-spec
    // toggle. Restrict to the page's exact slices and provenance in one batch.
    let opposing = if surface.paginated() && !orders.is_empty() {
        let keys: Vec<_> = orders.iter().map(crossing_key).collect();
        let sql = order_query_sql(include_str!("../../sql/orderbook/opposing.sql"));
        let filters = PublicOrderFilters {
            surface: OrderSurface::Orderbook,
            side: Some(if surface == Surface::Bids {
                "ASK"
            } else {
                "BID"
            }),
            ..Default::default()
        };
        bind_order_query(&sql, policy, &clock, config, &filters)
            .map_err(|e| query_error(path, &e))?
            .bind(json!(keys))
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| query_error(path, &e))?
    } else {
        Vec::new()
    };
    let mut items = Vec::new();
    for order in orders {
        let key = order.benchmark_key(now)?;
        let benchmark = key
            .as_ref()
            .and_then(|k| benchmarks.get(k))
            .and_then(Option::as_ref);
        let mut crossed = false;
        for row in &opposing {
            let matches = row
                .try_get::<Json, _>("slice")
                .map_err(|_| ApiError::internal())?
                == crossing_key(&order);
            if matches {
                let price: Decimal = row
                    .try_get("best_price")
                    .map_err(|_| ApiError::internal())?;
                crossed = if surface == Surface::Bids {
                    order.price >= price
                } else {
                    order.price <= price
                };
                break;
            }
        }
        let mut item = order.public(crossed, benchmark)?;
        if surface == Surface::Ci {
            let lng = policy.products.iter().any(|p| {
                Some(p.id.to_string()) == order.private["product_id"].as_str().map(str::to_owned)
                    && p.qualification_profile == "lng_v1"
            });
            item["ci_adjusted_price"] = ci_overlay(&order, lng)?;
        }
        items.push(item);
    }
    tx.commit().await.map_err(|e| query_error(path, &e))?;
    Ok(if surface.paginated() {
        json!({"items":items,"total":total,"skip":query.skip,"limit":query.limit})
    } else {
        json!(items)
    })
}

fn crossing_key(order: &PublicOrder) -> Json {
    json!({
        "product_id": order.private["product_id"],
        "delivery_point_id": order.private["delivery_point_id"],
        "availability_window": order.private["availability_window"],
        "provenance": order.provenance,
    })
}

fn ci_overlay(order: &PublicOrder, lng: bool) -> Result<Json, ApiError> {
    if lng {
        return Ok(Json::Null);
    }
    let parse = |name| {
        order.private[name]
            .as_str()
            .map(str::parse::<Decimal>)
            .transpose()
            .map_err(|_| ApiError::internal())
    };
    let (Some(ci), Some(energy)) = (
        parse("carbon_intensity_gco2_mj")?,
        parse("energy_density_mj_kg")?,
    ) else {
        return Ok(Json::Null);
    };
    if energy <= Decimal::ZERO {
        return Ok(Json::Null);
    }
    // FuelEU lifecycle reference, gCO2e/MJ; no inferred financial benefit.
    let reference = Decimal::new(9116, 2);
    let mut reduction = ((reference - ci) / reference * Decimal::from(100))
        .max(Decimal::ZERO)
        .round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero);
    reduction.rescale(1);
    let mut effective = order
        .price
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    effective.rescale(2);
    Ok(
        json!({"financial_benefit_status":"UNPRICED","base_price_per_mt":order.private["price_per_mt_usd"],"carbon_intensity_gco2_mj":order.private["carbon_intensity_gco2_mj"],"fueleu_ghg_intensity":"91.16","compliance_cost_per_mt":"0.00","effective_price_per_mt":effective.to_string(),"ghg_reduction_pct":reduction.to_string()}),
    )
}
