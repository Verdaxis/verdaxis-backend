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

/// The most pending trades, and separately the most open orders, one
/// invalidation changes. A larger scope is refused whole, never truncated.
const MAX_INVALIDATION_ROWS: usize = 100;

pub struct InvalidateOrganization {
    pub organization_id: Uuid,
    pub actor_id: Uuid,
    pub now: DateTime<Utc>,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
}

/// One person's market scope: the open orders they own and the pending trades
/// that record them as buyer or seller. Their organization's other rows stay.
pub struct InvalidateUser {
    pub subject_id: Uuid,
    pub actor_id: Uuid,
    pub now: DateTime<Utc>,
    pub policy: Arc<MarketPolicy>,
    pub config: Arc<Config>,
}

/// The rows one invalidation covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    /// Every row of one organization: company rejection.
    Organization(Uuid),
    /// The rows of one person: individual offboarding.
    User(Uuid),
}

impl Scope {
    /// The audit reason for every row this scope changes.
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Organization(_) => "organization_rejected",
            Self::User(_) => "account_offboarded",
        }
    }

    /// The internal audit reference. It never reaches a market event payload
    /// of the user scope, so counterparties do not learn who left.
    pub(super) fn reference(self) -> String {
        match self {
            Self::Organization(id) => format!("organization:{id}"),
            Self::User(id) => format!("user:{id}"),
        }
    }

    /// An open order in this scope is withdrawn; any other changed order is
    /// only restored.
    pub(super) fn owns(self, order: &Order) -> bool {
        match self {
            Self::Organization(id) => order.organization_id == id,
            Self::User(id) => order.owner_user_id == Some(id),
        }
    }
}

/// The inputs both public entry points share.
pub(super) struct Invalidation<'a> {
    pub scope: Scope,
    pub actor_id: Uuid,
    pub now: DateTime<Utc>,
    pub policy: &'a MarketPolicy,
    pub config: &'a Config,
}

#[derive(Debug)]
pub struct InvalidationResult {
    pub withdrawn_orders: Vec<Uuid>,
    pub cancelled_trade_ids: Vec<Uuid>,
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
    pub owner_user_id: Option<Uuid>,
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
    pub expires_at: Option<DateTime<Utc>>,
}

pub(super) struct OrderRestoration {
    pub before: Order,
    pub after: Order,
    pub restored: Decimal,
    pub released: Decimal,
}

#[derive(PartialEq, Eq)]
struct Preview {
    commitments: Vec<Commitment>,
    orders: Vec<Order>,
}

const ORDER_COLUMNS: &str = "id,organization_id,owner_user_id,side::text AS side,product_id,delivery_point_id,availability_window,status::text AS status,quantity_mt,remaining_quantity_mt,version,inventory_item_id,price_per_mt_usd,provenance::text AS provenance,expires_at";

// One person's market scope as predicates on `$1`. The invalidation selects
// its pending trades and open orders with them, and the offboarding preview
// counts with the same predicates ([`count_user_scope`]).
const USER_TRADES: &str = "(buyer_user_id=$1 OR seller_user_id=$1)";
const USER_ORDERS: &str = "owner_user_id=$1";
// The states either scope changes: pending trades and open order remainders.
const PENDING_TRADE: &str = "status='PENDING_CONFIRMATION'";
const OPEN_ORDER: &str = "status IN ('OPEN','PARTIALLY_FILLED')";
// Order states that are only restored, never withdrawn (see `invalidate`).
const TERMINAL_ORDER: &str = "status IN ('CANCELLED','EXPIRED')";

async fn preview(conn: &mut PgConnection, scope: Scope) -> Result<Preview, InvalidationError> {
    // One row past the bound proves overflow without reading the whole scope.
    let (trades_in_scope, orders_in_scope, subject) = match scope {
        Scope::Organization(id) => ("(buyer_id=$1 OR seller_id=$1)", "organization_id=$1", id),
        Scope::User(id) => (USER_TRADES, USER_ORDERS, id),
    };
    let commitments = sqlx::query_as::<_, Commitment>(sqlx::AssertSqlSafe(format!(
        "SELECT id,buyer_id,seller_id,bid_order_id,ask_order_id,product_id,delivery_point_id,availability_window,status::text AS status,quantity_mt FROM trades WHERE {trades_in_scope} AND {PENDING_TRADE} ORDER BY id LIMIT {}",
        MAX_INVALIDATION_ROWS + 1
    )))
    .bind(subject)
    .fetch_all(&mut *conn)
    .await?;
    let mut orders = sqlx::query_as::<_, Order>(sqlx::AssertSqlSafe(format!(
        "SELECT {ORDER_COLUMNS} FROM orderbook_orders WHERE {orders_in_scope} AND {OPEN_ORDER} ORDER BY id LIMIT {}",
        MAX_INVALIDATION_ROWS + 1
    )))
    .bind(subject)
    .fetch_all(&mut *conn)
    .await?;
    if commitments.len() > MAX_INVALIDATION_ROWS || orders.len() > MAX_INVALIDATION_ROWS {
        return Err(refusal(
            "MARKET_INVALIDATION_LIMIT",
            "Market invalidation exceeds the transaction bound.",
        ));
    }
    let linked_ids: BTreeSet<_> = commitments
        .iter()
        .flat_map(|trade| [trade.bid_order_id, trade.ask_order_id])
        .flatten()
        .collect();
    if !linked_ids.is_empty() {
        let linked = sqlx::query_as::<_, Order>(sqlx::AssertSqlSafe(format!(
            "SELECT {ORDER_COLUMNS} FROM orderbook_orders WHERE id=ANY($1) ORDER BY id"
        )))
        .bind(linked_ids.iter().copied().collect::<Vec<_>>())
        .fetch_all(&mut *conn)
        .await?;
        if linked.len() != linked_ids.len() {
            drift(conn).await?;
        }
        orders.extend(linked);
        orders.sort_by_key(|order| order.id);
        orders.dedup_by_key(|order| order.id);
    }
    Ok(Preview {
        commitments,
        orders,
    })
}

/// Advisory counts of one person's market scope. Each count stops one row
/// past the bound, so a larger scope is reported without reading all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserScopeCounts {
    /// Open order remainders the person owns. The bound applies to them.
    pub open_orders: i64,
    /// Every order [`invalidate_user`] withdraws: the open orders above and
    /// the person's other non-terminal orders linked to the pending trades
    /// (for example a fully taken ASK whose quantity is in a pending trade).
    /// It stops after three times the bound.
    pub withdrawn_orders: i64,
    /// Pending trades that record the person: [`invalidate_user`] cancels them.
    pub pending_trades: i64,
    /// Those pending trades under an active freeze: each one makes
    /// [`invalidate_user`] refuse the whole scope.
    pub frozen_pending_trades: i64,
}

impl UserScopeCounts {
    /// True when [`invalidate_user`] would refuse the scope as over its bound.
    pub fn exceeds_bound(&self) -> bool {
        let bound = MAX_INVALIDATION_ROWS as i64;
        self.open_orders > bound || self.pending_trades > bound
    }
}

/// Count the rows [`invalidate_user`] would change, with the same predicates
/// and without any lock. The answer is advisory: rows can change before the
/// invalidation locks and reads them again.
pub async fn count_user_scope(
    conn: &mut PgConnection,
    subject_id: Uuid,
) -> Result<UserScopeCounts, sqlx::Error> {
    let past_bound = MAX_INVALIDATION_ROWS as i64 + 1;
    // Open orders plus the orders of both pending-trade sides; the scope
    // withdraws at most all three collections.
    let withdrawn_cap = 3 * past_bound;
    let (open_orders, withdrawn_orders, pending_trades, frozen_pending_trades) =
        sqlx::query_as::<_, (i64, i64, i64, i64)>(sqlx::AssertSqlSafe(format!(
            "SELECT \
             (SELECT count(*) FROM (SELECT 1 FROM orderbook_orders WHERE {USER_ORDERS} AND {OPEN_ORDER} LIMIT $2) AS rows),\
             (SELECT count(*) FROM (SELECT 1 FROM orderbook_orders WHERE {USER_ORDERS} AND NOT {TERMINAL_ORDER} \
              AND ({OPEN_ORDER} OR id IN (\
               SELECT bid_order_id FROM trades WHERE {USER_TRADES} AND {PENDING_TRADE} \
               UNION SELECT ask_order_id FROM trades WHERE {USER_TRADES} AND {PENDING_TRADE})) \
              LIMIT $3) AS rows),\
             (SELECT count(*) FROM (SELECT 1 FROM trades WHERE {USER_TRADES} AND {PENDING_TRADE} LIMIT $2) AS rows),\
             (SELECT count(*) FROM (SELECT 1 FROM trades t WHERE {USER_TRADES} AND {PENDING_TRADE} \
              AND EXISTS(SELECT 1 FROM trade_freezes f WHERE f.trade_id=t.id AND f.status='ACTIVE') LIMIT $2) AS rows)"
        )))
        .bind(subject_id)
        .bind(past_bound)
        .bind(withdrawn_cap)
        .fetch_one(&mut *conn)
        .await?;
    Ok(UserScopeCounts {
        open_orders,
        withdrawn_orders,
        pending_trades,
        frozen_pending_trades,
    })
}

/// Set both timeouts for the rest of the transaction, or until set again.
async fn set_timeouts(
    conn: &mut PgConnection,
    lock: &str,
    statement: &str,
) -> Result<(), InvalidationError> {
    sqlx::query(
        "SELECT set_config('lock_timeout',$1,true),set_config('statement_timeout',$2,true)",
    )
    .bind(lock)
    .bind(statement)
    .execute(&mut *conn)
    .await?;
    Ok(())
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
    invalidate(
        conn,
        &Invalidation {
            scope: Scope::Organization(command.organization_id),
            actor_id: command.actor_id,
            now: command.now,
            policy: &command.policy,
            config: &command.config,
        },
    )
    .await
}

/// Withdraw the subject's open order remainders and cancel the pending trades
/// that record the subject, releasing their reservations once.
///
/// The caller holds the canonical organization and user prefix, the subject's
/// row included. The subject row is only confirmed here with `NOWAIT`: a
/// caller that did not lock it never waits out of the sorted user order, and
/// a row another session holds refuses at once with `55P03`. Market locks
/// follow in the shared order. A row that appears after discovery, a scope
/// over the bound, or a frozen pending trade in the scope refuses the whole
/// transaction. This subcommand never
/// completes or retries a transaction.
///
/// While it runs, market locks wait at most 500 ms (`MARKET_LOCK_TIMEOUT`) and
/// statements at most 5 s. A slice or row held longer refuses with `55P03`.
/// On success the caller's own `lock_timeout` and `statement_timeout` are in
/// force again; on an error the caller rolls back.
pub async fn invalidate_user(
    conn: &mut PgConnection,
    command: &InvalidateUser,
) -> Result<InvalidationResult, InvalidationError> {
    // The subject's own market commands take this row first, so holding it
    // keeps the scope from gaining the subject's new orders.
    let subject: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
            .bind(command.subject_id)
            .fetch_optional(&mut *conn)
            .await?;
    if subject.is_none() {
        return Err(refusal(
            "USER_NOT_FOUND",
            "The market scope names an unknown user.",
        ));
    }
    invalidate(
        conn,
        &Invalidation {
            scope: Scope::User(command.subject_id),
            actor_id: command.actor_id,
            now: command.now,
            policy: &command.policy,
            config: &command.config,
        },
    )
    .await
}

async fn invalidate(
    conn: &mut PgConnection,
    command: &Invalidation<'_>,
) -> Result<InvalidationResult, InvalidationError> {
    let before = preview(conn, command.scope).await?;
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
    normalize_keys(command.policy, &keys)?;
    let lock_keys = ordered_slice_lock_keys(&slices).map_err(|_| {
        refusal(
            "MARKET_SLICE_INVALID",
            "A market slice has an invalid availability window.",
        )
    })?;
    // The market bounds apply to this subcommand only; the caller's own
    // values are set again before a successful return.
    let caller_timeouts: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'),current_setting('statement_timeout')",
    )
    .fetch_one(&mut *conn)
    .await?;
    set_timeouts(conn, MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT).await?;
    for key in lock_keys {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    if preview(conn, command.scope).await? != before {
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
    // The database refuses any status change of a frozen trade. Freeze
    // administration locks the trade row first, so with the trades locked
    // the answer holds until commit: refuse the whole scope before any write.
    let frozen: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trade_freezes WHERE trade_id=ANY($1) AND status='ACTIVE')",
    )
    .bind(&trade_ids)
    .fetch_one(&mut *conn)
    .await?;
    if frozen {
        return Err(refusal(
            "MARKET_INVALIDATION_TRADE_FROZEN",
            "A pending trade in this market scope is frozen; clear the freeze first.",
        ));
    }
    let order_ids: Vec<_> = before.orders.iter().map(|o| o.id).collect();
    let locked: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM orderbook_orders WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&order_ids)
    .fetch_all(&mut *conn)
    .await?;
    if locked != order_ids || preview(conn, command.scope).await? != before {
        drift(conn).await?;
    }
    let mut restored = BTreeMap::<Uuid, Decimal>::new();
    let mut participants = BTreeSet::new();
    if let Scope::Organization(id) = command.scope {
        participants.insert(id);
    }
    for trade in &before.commitments {
        participants.extend([trade.buyer_id, trade.seller_id]);
        for order in [trade.bid_order_id, trade.ask_order_id]
            .into_iter()
            .flatten()
        {
            let quantity = restored.entry(order).or_default();
            *quantity = quantity.checked_add(trade.quantity_mt).ok_or_else(|| {
                refusal("ORDER_QUANTITY_INVALID", "Restoration sum is out of range.")
            })?;
        }
    }
    let mut releases = BTreeMap::<Uuid, Decimal>::new();
    let mut total = Decimal::ZERO;
    let mut changes = Vec::new();
    let mut withdrawn = Vec::new();
    for order in &before.orders {
        participants.insert(order.organization_id);
        let restored = restored.get(&order.id).copied().unwrap_or_default();
        let mut after = order.clone();
        after.remaining_quantity_mt = after
            .remaining_quantity_mt
            .checked_add(restored)
            .filter(|quantity| *quantity <= order.quantity_mt)
            .ok_or_else(|| {
                refusal(
                    "ORDER_QUANTITY_INVALID",
                    "Restored quantity exceeds the order capacity.",
                )
            })?;
        let terminal = matches!(order.status.as_str(), "CANCELLED" | "EXPIRED");
        let owned = command.scope.owns(order);
        let released = if terminal {
            restored
        } else if owned {
            after.status = "CANCELLED".into();
            withdrawn.push(order.id);
            after.remaining_quantity_mt
        } else if order.expires_at.is_some_and(|expiry| expiry <= command.now) {
            after.status = "EXPIRED".into();
            after.remaining_quantity_mt
        } else {
            if restored > Decimal::ZERO {
                after.status = if after.remaining_quantity_mt == order.quantity_mt {
                    "OPEN".into()
                } else {
                    "PARTIALLY_FILLED".into()
                };
            }
            Decimal::ZERO
        };
        if let Some(item) = order.inventory_item_id
            && released > Decimal::ZERO
        {
            let quantity = releases.entry(item).or_default();
            *quantity = quantity.checked_add(released).ok_or_else(|| {
                refusal(
                    "INVENTORY_RESERVATION_INVALID",
                    "Reservation sum is out of range.",
                )
            })?;
            total = total.checked_add(released).ok_or_else(|| {
                refusal(
                    "INVENTORY_RESERVATION_INVALID",
                    "Reservation sum is out of range.",
                )
            })?;
        }
        changes.push(OrderRestoration {
            before: order.clone(),
            after,
            restored,
            released,
        });
    }
    release_uncommitted(conn, &releases).await?;
    sqlx::query("UPDATE trades SET status='CANCELLED' WHERE id=ANY($1)")
        .bind(&trade_ids)
        .execute(&mut *conn)
        .await?;
    for change in &changes {
        if change.restored != Decimal::ZERO || change.before.status != change.after.status {
            sqlx::query("UPDATE orderbook_orders SET status=$2,remaining_quantity_mt=$3,version=version+1,updated_at=$4 WHERE id=$1")
                .bind(change.after.id).bind(&change.after.status).bind(change.after.remaining_quantity_mt)
                .bind(command.now).execute(&mut *conn).await?;
        }
    }
    rebuild_live_slice_benchmarks_for_keys(
        conn,
        command.policy,
        command.config,
        &keys,
        command.now,
    )
    .await?;
    events::restoration_effects(conn, command, &changes, &trade_ids).await?;
    let outbox_ids = events::scope_effects(
        conn,
        command,
        &order_ids,
        &trade_ids,
        participants.into_iter().collect(),
    )
    .await?;
    set_timeouts(conn, &caller_timeouts.0, &caller_timeouts.1).await?;
    Ok(InvalidationResult {
        withdrawn_orders: withdrawn,
        cancelled_trade_ids: trade_ids,
        released_uncommitted_quantity: total,
        outbox_ids,
    })
}
