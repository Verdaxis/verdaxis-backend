use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{
    benchmarks::{BenchmarkRebuildKey, normalize_keys, rebuild_live_slice_benchmarks_for_keys},
    events,
    locks::{MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT, SliceKey, ordered_slice_lock_keys},
    reservations::release_uncommitted,
};
use crate::{ApiError, ApiErrorKind, Config, MarketPolicy, business::transaction::AttemptError};

pub struct InvalidateOrganization {
    pub organization_id: Uuid,
    pub actor_id: Uuid,
    pub now: DateTime<Utc>,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
}

#[derive(Debug)]
pub struct InvalidationResult {
    pub withdrawn_orders: Vec<Uuid>,
    pub preserved_trade_ids: Vec<Uuid>,
    pub released_uncommitted_quantity: Decimal,
    pub outbox_ids: Vec<Uuid>,
}

#[derive(Debug)]
pub enum InvalidationError {
    Database(sqlx::Error),
    Application(ApiError),
}
impl From<sqlx::Error> for InvalidationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<InvalidationError> for AttemptError {
    fn from(error: InvalidationError) -> Self {
        match error {
            InvalidationError::Database(e) => Self::Database(e),
            InvalidationError::Application(e) => Self::Application(e),
        }
    }
}

pub(super) fn refusal(code: &str, message: &str) -> InvalidationError {
    InvalidationError::Application(ApiError {
        kind: ApiErrorKind::Expected,
        status: axum::http::StatusCode::CONFLICT,
        detail: serde_json::json!({"code":code,"message":message}),
        retry_after: None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct Commitment {
    id: Uuid,
    buyer_id: Uuid,
    seller_id: Uuid,
    bid_order_id: Option<Uuid>,
    ask_order_id: Option<Uuid>,
    product_id: Option<Uuid>,
    delivery_point_id: Option<Uuid>,
    availability_window: Option<String>,
    status: String,
    quantity_mt: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(super) struct Order {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub side: String,
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub status: String,
    pub quantity_mt: Decimal,
    pub remaining_quantity_mt: Decimal,
    pub version: i32,
    pub inventory_item_id: Option<Uuid>,
    pub price_per_mt_usd: Decimal,
    pub provenance: String,
}

#[derive(PartialEq, Eq)]
struct Preview {
    commitments: Vec<Commitment>,
    orders: Vec<Order>,
}

async fn preview(
    conn: &mut PgConnection,
    organization: Uuid,
) -> Result<Preview, InvalidationError> {
    let commitments = sqlx::query_as::<_,Commitment>("SELECT id,buyer_id,seller_id,bid_order_id,ask_order_id,product_id,delivery_point_id,availability_window,status::text AS status,quantity_mt FROM trades WHERE (buyer_id=$1 OR seller_id=$1) AND status IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED') ORDER BY id LIMIT 101")
        .bind(organization).fetch_all(&mut *conn).await?;
    let orders = sqlx::query_as::<_,Order>("SELECT id,organization_id,side::text AS side,product_id,delivery_point_id,availability_window,status::text AS status,quantity_mt,remaining_quantity_mt,version,inventory_item_id,price_per_mt_usd,provenance::text AS provenance FROM orderbook_orders WHERE organization_id=$1 AND status IN ('OPEN','PARTIALLY_FILLED') ORDER BY id LIMIT 101")
        .bind(organization).fetch_all(&mut *conn).await?;
    if commitments.len() > 100 || orders.len() > 100 {
        return Err(refusal(
            "MARKET_INVALIDATION_LIMIT",
            "Market invalidation exceeds the transaction bound.",
        ));
    }
    Ok(Preview {
        commitments,
        orders,
    })
}

async fn drift(conn: &mut PgConnection) -> Result<(), InvalidationError> {
    // Use a real SQLSTATE so the existing owner retries the entire transaction.
    sqlx::raw_sql(
        "DO $$ BEGIN RAISE EXCEPTION 'Market preview changed' USING ERRCODE='40001'; END $$",
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Caller holds canonical organization/user locks. This subcommand never completes a transaction.
pub async fn invalidate_organization(
    conn: &mut PgConnection,
    command: &InvalidateOrganization,
) -> Result<InvalidationResult, InvalidationError> {
    let before = preview(conn, command.organization_id).await?;
    let mut slices: Vec<_> = before
        .orders
        .iter()
        .map(|o| SliceKey {
            product_id: o.product_id,
            delivery_point_id: o.delivery_point_id,
            availability_window: o.availability_window.clone(),
        })
        .collect();
    for trade in &before.commitments {
        if let (Some(product_id), Some(window)) =
            (trade.product_id, trade.availability_window.as_ref())
        {
            slices.push(SliceKey {
                product_id,
                delivery_point_id: trade.delivery_point_id,
                availability_window: window.clone(),
            });
        }
    }
    let keys: Vec<_> = before
        .orders
        .iter()
        .map(|o| BenchmarkRebuildKey {
            side: o.side.clone(),
            market_product: command
                .policy
                .products
                .iter()
                .find(|p| p.id == o.product_id)
                .map(|p| p.code.clone()),
            delivery_point_id: o.delivery_point_id,
            availability_window: Some(o.availability_window.clone()),
        })
        .collect();
    for order in &before.orders {
        if !command
            .policy
            .products
            .iter()
            .any(|p| p.id == order.product_id)
        {
            return Err(refusal(
                "BENCHMARK_KEY_INVALID",
                "The order product is not in the supplied market policy.",
            ));
        }
        if order.remaining_quantity_mt < Decimal::ZERO
            || order.remaining_quantity_mt > order.quantity_mt
        {
            return Err(refusal(
                "ORDER_QUANTITY_INVALID",
                "Order remainder is outside its immutable capacity.",
            ));
        }
    }
    normalize_keys(&command.policy, &keys)?;
    let lock_keys = ordered_slice_lock_keys(&slices).map_err(|_| {
        refusal(
            "MARKET_SLICE_INVALID",
            "A market slice has an invalid availability window.",
        )
    })?;
    sqlx::query(
        "SELECT set_config('lock_timeout',$1,true),set_config('statement_timeout',$2,true)",
    )
    .bind(MARKET_LOCK_TIMEOUT)
    .bind(MARKET_STATEMENT_TIMEOUT)
    .execute(&mut *conn)
    .await?;
    for key in lock_keys {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    if preview(conn, command.organization_id).await? != before {
        drift(conn).await?;
    }
    let trade_ids: Vec<_> = before.commitments.iter().map(|t| t.id).collect();
    let locked: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM trades WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(&trade_ids)
            .fetch_all(&mut *conn)
            .await?;
    if locked != trade_ids {
        drift(conn).await?;
    }
    let order_ids: Vec<_> = before.orders.iter().map(|o| o.id).collect();
    let locked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM orderbook_orders WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&order_ids)
    .fetch_all(&mut *conn)
    .await?;
    if locked != order_ids || preview(conn, command.organization_id).await? != before {
        drift(conn).await?;
    }
    let mut releases = BTreeMap::<Uuid, Decimal>::new();
    let mut total = Decimal::ZERO;
    for order in &before.orders {
        if let Some(item) = order.inventory_item_id
            && order.remaining_quantity_mt > Decimal::ZERO
        {
            let value = releases.entry(item).or_default();
            *value = value
                .checked_add(order.remaining_quantity_mt)
                .ok_or_else(|| {
                    refusal(
                        "INVENTORY_RESERVATION_INVALID",
                        "Reservation sum is out of range.",
                    )
                })?;
            total = total
                .checked_add(order.remaining_quantity_mt)
                .ok_or_else(|| {
                    refusal(
                        "INVENTORY_RESERVATION_INVALID",
                        "Reservation sum is out of range.",
                    )
                })?;
        }
    }
    release_uncommitted(conn, &releases).await?;
    let mut participants = BTreeSet::from([command.organization_id]);
    for trade in &before.commitments {
        participants.extend([trade.buyer_id, trade.seller_id]);
    }
    for order in &before.orders {
        sqlx::query("UPDATE orderbook_orders SET status='CANCELLED',version=version+1,updated_at=$2 WHERE id=$1")
            .bind(order.id).bind(command.now).execute(&mut *conn).await?;
    }
    rebuild_live_slice_benchmarks_for_keys(
        conn,
        &command.policy,
        &command.config,
        &keys,
        command.now,
    )
    .await?;
    events::withdrawal_effects(conn, command, &before.orders).await?;
    let outbox_id = events::organization_effects(
        conn,
        command,
        &order_ids,
        &trade_ids,
        participants.into_iter().collect(),
    )
    .await?;
    Ok(InvalidationResult {
        withdrawn_orders: order_ids,
        preserved_trade_ids: trade_ids,
        released_uncommitted_quantity: total,
        outbox_ids: vec![outbox_id],
    })
}
