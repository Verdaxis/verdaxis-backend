//! Bounded match planning and application inside the ordered caller transaction.
//! Trade completion and route admission are supplied by the command owner.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::locks::{LockClass, Locked, OperationLockPlan, SliceKey};
use super::{MarketErrorCode, MarketWriteError, OrderQuantity, OrderRemainder, Price};

pub const MAX_CROSSING_ORDERS_PER_MATCH: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}
impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }
    fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}
impl OrderStatus {
    fn live(self) -> bool {
        matches!(self, Self::Open | Self::PartiallyFilled)
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub side: Side,
    pub provenance: String,
    pub organization_approved: bool,
    pub price_per_mt_usd: Decimal,
    pub quantity_mt: Decimal,
    pub remaining_quantity_mt: Decimal,
    pub acceptance_ordinal: i64,
    pub version: i32,
    pub status: OrderStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub inventory_item_id: Option<Uuid>,
}
impl OrderRow {
    fn slice(&self) -> SliceKey {
        SliceKey {
            product_id: self.product_id,
            delivery_point_id: self.delivery_point_id,
            availability_window: self.availability_window.clone(),
        }
    }
    fn validate(&self) -> Result<(), MarketWriteError> {
        OrderQuantity::checked(self.quantity_mt).map_err(|_| inconsistent())?;
        OrderRemainder::checked(self.remaining_quantity_mt).map_err(|_| inconsistent())?;
        Price::checked(self.price_per_mt_usd).map_err(|_| inconsistent())?;
        if self.remaining_quantity_mt > self.quantity_mt
            || self.version < 1
            || self.acceptance_ordinal < 1
            || super::normalize_slice_window(&self.availability_window).is_err()
        {
            return Err(inconsistent());
        }
        Ok(())
    }
    fn live_at(&self, now: DateTime<Utc>) -> bool {
        self.status.live()
            && self.remaining_quantity_mt > Decimal::ZERO
            && self.expires_at.is_none_or(|expiry| expiry > now)
    }
}

/// The fixed transaction cap and the command's single clock sample.
#[derive(Clone, Copy, Debug)]
pub struct MatchBound {
    now: DateTime<Utc>,
}
impl MatchBound {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }
}

#[derive(Clone, Debug)]
pub struct Fill {
    pub resting_before: OrderRow,
    pub resting_after: OrderRow,
    pub aggressor_after: OrderRow,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
}
#[derive(Clone, Debug)]
pub struct MatchPlan {
    aggressor_before: OrderRow,
    aggressor_after: OrderRow,
    fills: Vec<Fill>,
    now: DateTime<Utc>,
}
impl MatchPlan {
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }
    pub fn aggressor_after(&self) -> &OrderRow {
        &self.aggressor_after
    }
}
fn inconsistent() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryInconsistent)
}
fn changed() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::MarketSliceChanged)
}
fn eligible(row: &OrderRow, aggressor: &OrderRow, now: DateTime<Utc>) -> bool {
    row.id != aggressor.id
        && row.side == aggressor.side.opposite()
        && row.product_id == aggressor.product_id
        && row.delivery_point_id == aggressor.delivery_point_id
        && row.availability_window == aggressor.availability_window
        && row.organization_id != aggressor.organization_id
        && row.provenance == aggressor.provenance
        && row.organization_approved
        && row.live_at(now)
        && match aggressor.side {
            Side::Bid => row.price_per_mt_usd <= aggressor.price_per_mt_usd,
            Side::Ask => row.price_per_mt_usd >= aggressor.price_per_mt_usd,
        }
}
fn priority(left: &OrderRow, right: &OrderRow, side: Side) -> std::cmp::Ordering {
    let price = match side {
        Side::Bid => left.price_per_mt_usd.cmp(&right.price_per_mt_usd),
        Side::Ask => right.price_per_mt_usd.cmp(&left.price_per_mt_usd),
    };
    price
        .then(left.acceptance_ordinal.cmp(&right.acceptance_ordinal))
        .then(left.id.cmp(&right.id))
}
fn decrement(row: &mut OrderRow, quantity: Decimal) -> Result<(), MarketWriteError> {
    row.remaining_quantity_mt = row
        .remaining_quantity_mt
        .checked_sub(quantity)
        .ok_or_else(inconsistent)?;
    OrderRemainder::checked(row.remaining_quantity_mt).map_err(|_| inconsistent())?;
    row.version = row.version.checked_add(1).ok_or_else(inconsistent)?;
    row.status = if row.remaining_quantity_mt == Decimal::ZERO {
        OrderStatus::Filled
    } else {
        OrderStatus::PartiallyFilled
    };
    Ok(())
}

/// The planner never mutates input and never returns a partially accepted prefix.
pub fn plan_match(
    candidates: &[OrderRow],
    aggressor: &OrderRow,
    bound: MatchBound,
) -> Result<MatchPlan, MarketWriteError> {
    aggressor.validate()?;
    let mut plan = MatchPlan {
        aggressor_before: aggressor.clone(),
        aggressor_after: aggressor.clone(),
        fills: Vec::new(),
        now: bound.now,
    };
    if !aggressor.live_at(bound.now) || !aggressor.organization_approved {
        return Ok(plan);
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for row in candidates {
        row.validate()?;
        if !seen.insert(row.id) {
            return Err(inconsistent());
        }
        if eligible(row, aggressor, bound.now) {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| priority(a, b, aggressor.side));
    let exceeded = rows.len() > MAX_CROSSING_ORDERS_PER_MATCH;
    for row in rows.into_iter().take(MAX_CROSSING_ORDERS_PER_MATCH) {
        if plan.aggressor_after.remaining_quantity_mt == Decimal::ZERO {
            break;
        }
        let quantity = row
            .remaining_quantity_mt
            .min(plan.aggressor_after.remaining_quantity_mt);
        let mut after = row.clone();
        decrement(&mut after, quantity)?;
        decrement(&mut plan.aggressor_after, quantity)?;
        plan.fills.push(Fill {
            resting_before: row.clone(),
            resting_after: after,
            aggressor_after: plan.aggressor_after.clone(),
            quantity_mt: quantity,
            price_per_mt_usd: row.price_per_mt_usd,
        });
    }
    if exceeded && plan.aggressor_after.remaining_quantity_mt > Decimal::ZERO {
        return Err(MarketWriteError::new(MarketErrorCode::MatchFanoutExceeded));
    }
    Ok(plan)
}

#[derive(Clone, Debug)]
pub struct MatchedTrade {
    pub id: Uuid,
    pub bid_order_id: Uuid,
    pub ask_order_id: Uuid,
    pub buyer_id: Uuid,
    pub seller_id: Uuid,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
    pub confirmed_at: DateTime<Utc>,
}
#[derive(Clone, Debug)]
pub struct ChangedOrder {
    pub before: OrderRow,
    pub after: OrderRow,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedSlice {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub side: String,
}
#[derive(Clone, Debug)]
pub struct MatchOutcome {
    pub trades: Vec<MatchedTrade>,
    pub changed_orders: Vec<ChangedOrder>,
    pub changed_slices: BTreeSet<ChangedSlice>,
}

/// Borrows the sealed caller transaction; no connection or commit escapes.
#[allow(dead_code)]
pub(super) struct MatchingPort<'a> {
    conn: &'a mut PgConnection,
    plan: &'a OperationLockPlan,
    locked: &'a Locked,
}
#[allow(dead_code)]
impl<'a> MatchingPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        plan: &'a OperationLockPlan,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, plan, locked })
    }
    pub(super) async fn plan_match(
        &mut self,
        aggressor: &OrderRow,
        bound: MatchBound,
    ) -> Result<MatchPlan, MarketWriteError> {
        self.locked.require_economic_phase()?;
        self.locked.require_effect_slice(&aggressor.slice())?;
        // Candidate locks belong before inventory and terminal target locks.
        if self
            .plan
            .trace()
            .iter()
            .any(|(class, _)| matches!(class, LockClass::Inventory | LockClass::Pin))
        {
            return Err(changed());
        }
        let candidates = select_candidates(self.conn, aggressor, bound, None).await?;
        for row in candidates.iter().chain([aggressor]) {
            if !self
                .locked
                .contains(LockClass::Organization, row.organization_id)
                || row
                    .owner_user_id
                    .is_some_and(|id| !self.locked.contains(LockClass::User, id))
            {
                return Err(changed());
            }
        }
        let current = load_order(self.conn, aggressor.id).await?;
        if current != *aggressor {
            return Err(changed());
        }
        let ids: Vec<_> = candidates
            .iter()
            .take(MAX_CROSSING_ORDERS_PER_MATCH)
            .map(|row| row.id)
            .collect();
        let mut rows = select_candidates(self.conn, aggressor, bound, Some(&ids)).await?;
        sqlx::query("SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE")
            .bind(aggressor.id)
            .fetch_one(&mut *self.conn)
            .await?;
        if load_order(self.conn, aggressor.id).await? != *aggressor {
            return Err(changed());
        }
        // The look-ahead row decides only the bound and is never written or locked.
        if candidates.len() > MAX_CROSSING_ORDERS_PER_MATCH {
            rows.push(candidates[MAX_CROSSING_ORDERS_PER_MATCH].clone());
        }
        plan_match(&rows, aggressor, bound)
    }
    pub(super) async fn apply_plan(
        &mut self,
        plan: &MatchPlan,
    ) -> Result<MatchOutcome, MarketWriteError> {
        self.locked.require_economic_phase()?;
        apply_plan(self.conn, plan).await
    }
    pub(super) async fn match_order(
        &mut self,
        aggressor: &OrderRow,
        bound: MatchBound,
    ) -> Result<MatchOutcome, MarketWriteError> {
        let plan = self.plan_match(aggressor, bound).await?;
        self.apply_plan(&plan).await
    }
}

const ORDER_COLUMNS: &str = "o.id,o.organization_id,o.owner_user_id,o.product_id,o.delivery_point_id,o.availability_window,o.side::text AS side,o.provenance::text AS provenance,(g.verification_status::text='APPROVED') AS organization_approved,o.price_per_mt_usd,o.quantity_mt,o.remaining_quantity_mt,o.acceptance_ordinal,o.version,o.status::text AS status,o.expires_at,o.created_at,o.inventory_item_id";
fn decode(row: sqlx::postgres::PgRow) -> Result<OrderRow, MarketWriteError> {
    let side: String = row.try_get("side")?;
    let status: String = row.try_get("status")?;
    Ok(OrderRow {
        id: row.try_get("id")?,
        organization_id: row.try_get("organization_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        product_id: row.try_get("product_id")?,
        delivery_point_id: row.try_get("delivery_point_id")?,
        availability_window: row.try_get("availability_window")?,
        side: match side.as_str() {
            "BID" => Side::Bid,
            "ASK" => Side::Ask,
            _ => return Err(inconsistent()),
        },
        provenance: row.try_get("provenance")?,
        organization_approved: row.try_get("organization_approved")?,
        price_per_mt_usd: row.try_get("price_per_mt_usd")?,
        quantity_mt: row.try_get("quantity_mt")?,
        remaining_quantity_mt: row.try_get("remaining_quantity_mt")?,
        acceptance_ordinal: row.try_get("acceptance_ordinal")?,
        version: row.try_get("version")?,
        status: match status.as_str() {
            "OPEN" => OrderStatus::Open,
            "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
            "FILLED" => OrderStatus::Filled,
            "CANCELLED" => OrderStatus::Cancelled,
            "EXPIRED" => OrderStatus::Expired,
            _ => return Err(inconsistent()),
        },
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        inventory_item_id: row.try_get("inventory_item_id")?,
    })
}
async fn load_order(conn: &mut PgConnection, id: Uuid) -> Result<OrderRow, MarketWriteError> {
    let sql = format!(
        "SELECT {ORDER_COLUMNS} FROM orderbook_orders o JOIN organizations g ON g.id=o.organization_id WHERE o.id=$1"
    );
    decode(
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(conn)
            .await?
            .ok_or_else(changed)?,
    )
}
async fn select_candidates(
    conn: &mut PgConnection,
    aggressor: &OrderRow,
    bound: MatchBound,
    ids: Option<&[Uuid]>,
) -> Result<Vec<OrderRow>, MarketWriteError> {
    let comparison = if aggressor.side == Side::Bid {
        "<="
    } else {
        ">="
    };
    let direction = if aggressor.side == Side::Bid {
        "ASC"
    } else {
        "DESC"
    };
    let lock = if ids.is_some() {
        " FOR UPDATE OF o"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {ORDER_COLUMNS} FROM orderbook_orders o JOIN organizations g ON g.id=o.organization_id WHERE o.product_id=$1 AND o.delivery_point_id IS NOT DISTINCT FROM $2 AND o.availability_window=$3 AND o.side::text=$4 AND o.organization_id<>$5 AND o.provenance::text=$6 AND g.verification_status::text='APPROVED' AND o.status::text IN ('OPEN','PARTIALLY_FILLED') AND o.remaining_quantity_mt>0 AND (o.expires_at IS NULL OR o.expires_at>$7) AND o.price_per_mt_usd {comparison} $8 AND ($9::uuid[] IS NULL OR o.id=ANY($9)) ORDER BY o.price_per_mt_usd {direction},o.acceptance_ordinal,o.id LIMIT 101{lock}"
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(aggressor.product_id)
        .bind(aggressor.delivery_point_id)
        .bind(&aggressor.availability_window)
        .bind(aggressor.side.opposite().as_str())
        .bind(aggressor.organization_id)
        .bind(&aggressor.provenance)
        .bind(bound.now)
        .bind(aggressor.price_per_mt_usd)
        .bind(ids)
        .fetch_all(conn)
        .await?
        .into_iter()
        .map(decode)
        .collect()
}

async fn update_order(
    conn: &mut PgConnection,
    before: &OrderRow,
    after: &OrderRow,
) -> Result<(), MarketWriteError> {
    let result = sqlx::query("UPDATE orderbook_orders SET remaining_quantity_mt=$2,status=$3,version=$4 WHERE id=$1 AND version=$5")
        .bind(after.id).bind(after.remaining_quantity_mt).bind(after.status.as_str()).bind(after.version).bind(before.version).execute(conn).await?;
    if result.rows_affected() != 1 {
        return Err(changed());
    }
    Ok(())
}
fn catalog_products() -> Result<serde_json::Value, MarketWriteError> {
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json"))
            .map_err(|_| inconsistent())?;
    Ok(policy["products"].clone())
}

async fn apply_plan(
    conn: &mut PgConnection,
    plan: &MatchPlan,
) -> Result<MatchOutcome, MarketWriteError> {
    let mut outcome = MatchOutcome {
        trades: Vec::new(),
        changed_orders: Vec::new(),
        changed_slices: BTreeSet::new(),
    };
    let mut aggressor = plan.aggressor_before.clone();
    for fill in &plan.fills {
        let resting = &fill.resting_before;
        let (buyer, seller) = if aggressor.side == Side::Bid {
            (&aggressor, resting)
        } else {
            (resting, &aggressor)
        };
        let trade = MatchedTrade {
            id: Uuid::new_v4(),
            bid_order_id: buyer.id,
            ask_order_id: seller.id,
            buyer_id: buyer.organization_id,
            seller_id: seller.organization_id,
            quantity_mt: fill.quantity_mt,
            price_per_mt_usd: fill.price_per_mt_usd,
            confirmed_at: plan.now,
        };
        // Canonical labels are required by the database at insert. Fee and typed
        // term completion is composed before these trades reach a serving route.
        let inserted = sqlx::query("INSERT INTO trades (id,bid_order_id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,confirmed_at,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,commission_rate_pct,is_anonymous) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,'CONFIRMED',$14,o.product_id,p.name,p.fuel_type,p.fuel_grade,catalog.code,o.delivery_point_id,dp.name,dp.region,o.availability_window,1,0,true FROM orderbook_orders o JOIN products p ON p.id=o.product_id JOIN jsonb_to_recordset($16::jsonb) AS catalog(id uuid,code text) ON catalog.id=p.id JOIN delivery_points dp ON dp.id=o.delivery_point_id WHERE o.id=$15")
            .bind(trade.id).bind(trade.bid_order_id).bind(trade.ask_order_id).bind(trade.buyer_id).bind(trade.seller_id).bind(buyer.owner_user_id).bind(seller.owner_user_id).bind(aggressor.organization_id).bind(&buyer.provenance).bind(&seller.provenance).bind(if aggressor.side==Side::Bid { "BUYER" } else { "SELLER" }).bind(trade.quantity_mt).bind(trade.price_per_mt_usd).bind(plan.now).bind(aggressor.id).bind(catalog_products()?).execute(&mut *conn).await?;
        if inserted.rows_affected() != 1 {
            return Err(inconsistent());
        }

        update_order(conn, resting, &fill.resting_after).await?;
        update_order(conn, &aggressor, &fill.aggressor_after).await?;
        aggressor = fill.aggressor_after.clone();
        outcome.changed_orders.push(ChangedOrder {
            before: resting.clone(),
            after: fill.resting_after.clone(),
        });
        outcome.trades.push(trade);
    }
    if !plan.fills.is_empty() {
        outcome.changed_orders.push(ChangedOrder {
            before: plan.aggressor_before.clone(),
            after: aggressor,
        });
    }
    for change in &outcome.changed_orders {
        let row = &change.after;
        outcome.changed_slices.insert(ChangedSlice {
            product_id: row.product_id,
            delivery_point_id: row.delivery_point_id,
            availability_window: row.availability_window.clone(),
            side: row.side.as_str().to_owned(),
        });
    }
    Ok(outcome)
}

#[cfg(feature = "contract-tests")]
pub mod contract {
    use super::*;
    #[derive(Debug)]
    pub struct PlannedMatch {
        pub plan: MatchPlan,
    }
    pub async fn preview(
        conn: &mut PgConnection,
        aggressor: Uuid,
        bound: MatchBound,
    ) -> Result<Vec<OrderRow>, MarketWriteError> {
        let aggressor = load_order(conn, aggressor).await?;
        select_candidates(conn, &aggressor, bound, None).await
    }
    pub async fn lock_and_plan(
        conn: &mut PgConnection,
        aggressor: Uuid,
        bound: MatchBound,
    ) -> Result<PlannedMatch, MarketWriteError> {
        use super::super::locks::{MarketOperation, PartyLockPreview};
        let row = load_order(conn, aggressor).await?;
        let candidates = select_candidates(conn, &row, bound, None).await?;
        let parties = PartyLockPreview::new(
            candidates.iter().chain([&row]).map(|r| r.organization_id),
            candidates
                .iter()
                .chain([&row])
                .filter_map(|r| r.owner_user_id),
            row.owner_user_id.ok_or_else(changed)?,
            serde_json::Value::Null,
        );
        let mut locks = OperationLockPlan::new(MarketOperation::Amend);
        locks.parties(&parties)?.slices([row.slice()])?;
        let _held = locks.acquire(conn).await?;
        let plan = MatchingPort::new(conn, &locks, &_held)?
            .plan_match(&row, bound)
            .await?;
        Ok(PlannedMatch { plan })
    }

    pub async fn apply(
        conn: &mut PgConnection,
        planned: PlannedMatch,
    ) -> Result<MatchOutcome, MarketWriteError> {
        apply_plan(conn, &planned.plan).await
    }
}
