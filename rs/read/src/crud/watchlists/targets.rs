//! W05 and W06: SLICE and PIN target writes.
//!
//! One transaction covers a request, so a PIN and its companion SLICE commit or
//! roll back together. A PIN reads its order through the shared public-order
//! relation only (PD1): an order outside that scope is "not found", and the
//! write never locks or changes an order row.
use super::Target;
use super::http::{WatchlistState, WriteControls, not_found, refusal, run};
use super::validation::{TargetCreate, parse_body, path_uuid, unprocessable, validate_target};
use crate::ApiError;
use crate::market::visibility::{
    PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
};
use crate::market::windows::normalize_window;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

const DUPLICATE_TARGET: &str = "Duplicate watchlist target";
const INSERT_TARGET: &str = "INSERT INTO watchlist_targets(id,watchlist_id,target_type,\
    market_product_code,delivery_point_id,availability_window_code,order_id,\
    snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,\
    snapshot_status,snapshot_side,snapshot_market_product,snapshot_delivery_point_name,\
    snapshot_availability_window,snapshot_counterparty_label,created_at) \
    VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$4,$13,$6,NULL,$14)";
/// The order columns a PIN snapshot copies, read from the shared relation.
const PUBLIC_ORDER: &str = "SELECT market_product,delivery_point_id,delivery_point_name,\
    availability_window,price_per_mt_usd::float8 AS price,quantity_mt::float8 AS quantity,\
    remaining_quantity_mt::float8 AS remaining,status::text AS status,side::text AS side \
    FROM eligible_orders WHERE id = $14";

/// Any integrity violation is the source's one 409; everything else is a database error.
fn conflict_or_database(error: sqlx::Error) -> ApiError {
    let integrity = error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code.starts_with("23"));
    if integrity {
        refusal(StatusCode::CONFLICT, DUPLICATE_TARGET)
    } else {
        ApiError::database()
    }
}

fn database(_: sqlx::Error) -> ApiError {
    ApiError::database()
}

/// The container must exist and belong to the actor.
async fn require_container(
    connection: &mut PgConnection,
    actor: Uuid,
    container: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT id FROM watchlists WHERE id=$1 AND user_id=$2")
        .bind(container)
        .bind(actor)
        .fetch_optional(connection)
        .await
        .map_err(database)?
        .map(|_| ())
        .ok_or_else(|| not_found("Watchlist not found"))
}

fn canonical_window(raw: &str, now: DateTime<Utc>) -> Result<String, ApiError> {
    normalize_window(raw, now.date_naive())
        .map_err(|message| refusal(StatusCode::UNPROCESSABLE_ENTITY, message))
}

/// The slice as a response row: stored columns plus the live point name.
async fn load_target(connection: &mut PgConnection, id: Uuid) -> Result<Target, ApiError> {
    let raw: String = sqlx::query_scalar(
        "SELECT (to_jsonb(t)||jsonb_build_object('delivery_point_name',\
         COALESCE(p.name,t.snapshot_delivery_point_name)))::text \
         FROM watchlist_targets t LEFT JOIN delivery_points p ON p.id=t.delivery_point_id \
         WHERE t.id=$1",
    )
    .bind(id)
    .fetch_one(connection)
    .await
    .map_err(database)?;
    serde_json::from_str(&raw).map_err(|_| ApiError::database())
}

struct SliceKey<'a> {
    product: &'a str,
    point: Uuid,
    window: &'a str,
}

/// Insert a bare SLICE with the snapshot columns Python sets for one.
async fn insert_slice(
    connection: &mut PgConnection,
    container: Uuid,
    key: SliceKey<'_>,
    point_name: Option<&str>,
    controls: &mut WriteControls,
) -> Result<Uuid, sqlx::Error> {
    let id = controls.next_id();
    sqlx::query(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,\
         delivery_point_id,availability_window_code,snapshot_market_product,\
         snapshot_delivery_point_name,snapshot_availability_window,created_at) \
         VALUES($1,$2,'SLICE',$3,$4,$5,$3,$6,$5,$7)",
    )
    .bind(id)
    .bind(container)
    .bind(key.product)
    .bind(key.point)
    .bind(key.window)
    .bind(point_name)
    .bind(controls.now())
    .execute(connection)
    .await?;
    Ok(id)
}

async fn create_slice(
    connection: &mut PgConnection,
    state: &WatchlistState,
    container: Uuid,
    (product, point, window): (String, Uuid, String),
    controls: &mut WriteControls,
) -> Result<Uuid, ApiError> {
    let name: String = sqlx::query_scalar("SELECT name FROM delivery_points WHERE id=$1")
        .bind(point)
        .fetch_optional(&mut *connection)
        .await
        .map_err(database)?
        .ok_or_else(|| not_found("Delivery point not found"))?;
    // The source checks the point, then the window, then the product code.
    let window = canonical_window(&window, controls.now())?;
    if !state
        .app
        .policy
        .products
        .iter()
        .any(|known| known.code == product)
    {
        return Err(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid market_product_code",
        ));
    }
    let key = SliceKey {
        product: &product,
        point,
        window: &window,
    };
    insert_slice(connection, container, key, Some(name.as_str()), controls)
        .await
        .map_err(conflict_or_database)
}

struct PinnedOrder {
    product: String,
    point: Uuid,
    point_name: Option<String>,
    window: String,
    price: f64,
    quantity: f64,
    remaining: f64,
    status: String,
    side: String,
}

/// The order through the shared public-order relation, or none (PD1).
async fn load_public_order(
    connection: &mut PgConnection,
    state: &WatchlistState,
    now: DateTime<Utc>,
    order: Uuid,
) -> Result<Option<PinnedOrder>, ApiError> {
    let query = order_query_sql(PUBLIC_ORDER);
    let row = bind_order_query(
        &query,
        &state.app.policy,
        &VisibilityClock::at(now),
        &state.app.config,
        &PublicOrderFilters::default(),
    )
    .map_err(database)?
    .bind(order)
    .fetch_optional(connection)
    .await
    .map_err(database)?;
    let Some(row) = row else { return Ok(None) };
    let product: Option<String> = row.try_get("market_product").map_err(database)?;
    // A public order always maps to a slice; the source still refuses one that does not.
    let product = product.ok_or_else(|| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Pinned order does not map to a canonical slice",
        )
    })?;
    Ok(Some(PinnedOrder {
        product,
        point: row.try_get("delivery_point_id").map_err(database)?,
        point_name: row.try_get("delivery_point_name").map_err(database)?,
        window: row.try_get("availability_window").map_err(database)?,
        price: row.try_get("price").map_err(database)?,
        quantity: row.try_get("quantity").map_err(database)?,
        remaining: row.try_get("remaining").map_err(database)?,
        status: row.try_get("status").map_err(database)?,
        side: row.try_get("side").map_err(database)?,
    }))
}

/// Pin a public order, adding its companion slice first when the container has none.
async fn create_pin(
    connection: &mut PgConnection,
    state: &WatchlistState,
    container: Uuid,
    order_id: Uuid,
    controls: &mut WriteControls,
) -> Result<Uuid, ApiError> {
    let order = load_public_order(connection, state, controls.now(), order_id)
        .await?
        .ok_or_else(|| not_found("Order not found"))?;
    let window = canonical_window(&order.window, controls.now())?;
    let existing = sqlx::query(
        "SELECT id FROM watchlist_targets WHERE watchlist_id=$1 AND target_type='SLICE' \
         AND market_product_code=$2 AND delivery_point_id=$3 AND availability_window_code=$4 LIMIT 1",
    )
    .bind(container)
    .bind(&order.product)
    .bind(order.point)
    .bind(&window)
    .fetch_optional(&mut *connection)
    .await
    .map_err(database)?;
    if existing.is_none() {
        let key = SliceKey {
            product: &order.product,
            point: order.point,
            window: &window,
        };
        insert_slice(
            connection,
            container,
            key,
            order.point_name.as_deref(),
            controls,
        )
        .await
        .map_err(conflict_or_database)?;
    }
    let id = controls.next_id();
    sqlx::query(INSERT_TARGET)
        .bind(id)
        .bind(container)
        .bind("PIN")
        .bind(&order.product)
        .bind(order.point)
        .bind(&window)
        .bind(order_id)
        .bind(order.price)
        .bind(order.quantity)
        .bind(order.remaining)
        .bind(&order.status)
        .bind(&order.side)
        .bind(order.point_name.as_deref())
        .bind(controls.now())
        .execute(connection)
        .await
        .map_err(conflict_or_database)?;
    Ok(id)
}

/// W05: add a SLICE, or a PIN with its companion SLICE.
pub(super) async fn create(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: Bytes,
) -> Response {
    // JSON syntax is refused before authentication, as FastAPI reads the body first.
    let body = match parse_body(&body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let mut controls = state.settings.controls_for(&headers);
    let worker = state.clone();
    let result = run(
        &state,
        &headers,
        "POST",
        "/api/watchlists/{watchlist_id}/targets",
        move |transaction| {
            Box::pin(async move {
                // Path errors precede body errors in one FastAPI validation list.
                let mut errors = Vec::new();
                let container = path_uuid("watchlist_id", &raw)
                    .map_err(|error| errors.push(error))
                    .ok();
                let target = validate_target(&body)
                    .map_err(|mut found| errors.append(&mut found))
                    .ok();
                let (Some(container), Some(target)) = (container, target) else {
                    return Err(unprocessable(errors));
                };
                let actor = transaction.actor().id;
                require_container(transaction.connection(), actor, container).await?;
                let created = match target {
                    TargetCreate::Slice {
                        market_product_code,
                        delivery_point_id,
                        availability_window_code,
                    } => {
                        create_slice(
                            transaction.connection(),
                            &worker,
                            container,
                            (
                                market_product_code,
                                delivery_point_id,
                                availability_window_code,
                            ),
                            &mut controls,
                        )
                        .await?
                    }
                    TargetCreate::Pin { order_id } => {
                        create_pin(
                            transaction.connection(),
                            &worker,
                            container,
                            order_id,
                            &mut controls,
                        )
                        .await?
                    }
                };
                load_target(transaction.connection(), created).await
            })
        },
    )
    .await;
    match result {
        Ok(target) => (StatusCode::CREATED, Json(target)).into_response(),
        Err(response) => response,
    }
}

/// W06: remove a target and its events. A SLICE also removes the PINs under its
/// exact key, with their events.
pub(super) async fn delete(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path((raw_container, raw_target)): Path<(String, String)>,
) -> Response {
    let result = run(
        &state,
        &headers,
        "DELETE",
        "/api/watchlists/{watchlist_id}/targets/{target_id}",
        move |transaction| {
            Box::pin(async move {
                let mut errors = Vec::new();
                let container = path_uuid("watchlist_id", &raw_container)
                    .map_err(|error| errors.push(error))
                    .ok();
                let target = path_uuid("target_id", &raw_target)
                    .map_err(|error| errors.push(error))
                    .ok();
                let (Some(container), Some(target)) = (container, target) else {
                    return Err(unprocessable(errors));
                };
                let actor = transaction.actor().id;
                require_container(transaction.connection(), actor, container).await?;
                let row = sqlx::query(
                    "SELECT target_type::text AS kind,market_product_code,delivery_point_id,\
                     availability_window_code FROM watchlist_targets WHERE id=$1 AND watchlist_id=$2",
                )
                .bind(target)
                .bind(container)
                .fetch_optional(&mut *transaction.connection())
                .await
                .map_err(database)?
                .ok_or_else(|| not_found("Target not found"))?;
                let mut doomed = vec![target];
                let kind: String = row.try_get("kind").map_err(database)?;
                if kind == "SLICE" {
                    let pins: Vec<Uuid> = sqlx::query_scalar(
                        "SELECT id FROM watchlist_targets WHERE watchlist_id=$1 AND target_type='PIN' \
                         AND market_product_code=$2 AND delivery_point_id=$3 \
                         AND availability_window_code=$4",
                    )
                    .bind(container)
                    .bind(row.try_get::<Option<String>, _>("market_product_code").map_err(database)?)
                    .bind(row.try_get::<Option<Uuid>, _>("delivery_point_id").map_err(database)?)
                    .bind(row.try_get::<Option<String>, _>("availability_window_code").map_err(database)?)
                    .fetch_all(&mut *transaction.connection())
                    .await
                    .map_err(database)?;
                    doomed.extend(pins);
                }
                for statement in [
                    "DELETE FROM watchlist_events WHERE watchlist_target_id = ANY($1)",
                    "DELETE FROM watchlist_targets WHERE id = ANY($1)",
                ] {
                    sqlx::query(statement)
                        .bind(&doomed)
                        .execute(&mut *transaction.connection())
                        .await
                        .map_err(database)?;
                }
                Ok(())
            })
        },
    )
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => response,
    }
}
