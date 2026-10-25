//! Bounded expiry inside the caller's already-locked market transaction.
//!
//! The caller previews the exact slice, includes those parties, orders and
//! inventory rows in its complete lock plan, then revalidates that preview.
//! Effects are facts for the shared effects owner, never a second emitter.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::decimal::{OrderRemainder, StockQuantity};
use super::error::MarketWriteError;
use super::transaction::StockLedger;

pub const MAX_EXPIRY_ORDERS: usize = 100;

#[derive(Clone, Debug)]
pub struct ExpirySlice<'a> {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: &'a str,
    pub side: &'a str,
}

/// Before/after evidence for Task 5, which owns audit, public visibility,
/// benchmarks and outbox. No effect has been appended merely by returning it.
#[derive(Clone, Debug)]
pub struct ExpiryEffect {
    pub order_id: Uuid,
    pub organization_id: Uuid,
    pub inventory_item_id: Option<Uuid>,
    pub previous_status: String,
    pub remaining_quantity_mt: OrderRemainder,
    pub released_quantity_mt: StockQuantity,
    pub previous_version: i32,
    pub version: i32,
    pub expires_at: DateTime<Utc>,
}

/// At most one hundred rows, no new locks, no commit and no timer. Any error
/// requires the caller to roll back its entire command transaction.
pub(super) async fn expire_slice(
    ledger: &mut StockLedger<'_>,
    slice: &ExpirySlice<'_>,
    now: DateTime<Utc>,
) -> Result<Vec<ExpiryEffect>, MarketWriteError> {
    let orders = ledger.expired_orders(slice, now).await?;
    ledger.verify_invariants().await?;
    let mut effects = Vec::with_capacity(orders.len());
    for order in orders {
        let remaining = OrderRemainder::checked(order.remaining)?;
        let released = if order.item.is_some() {
            StockQuantity::checked(remaining.value())?
        } else {
            StockQuantity::checked(rust_decimal::Decimal::ZERO)?
        };
        let previous_version = order.version;
        let version = previous_version
            .checked_add(1)
            .ok_or_else(super::transaction::inconsistent_stock)?;
        ledger.withdraw_order(&order, "EXPIRED").await?;
        ledger.verify_invariants().await?;
        effects.push(ExpiryEffect {
            order_id: order.id,
            organization_id: order.organization,
            inventory_item_id: order.item,
            previous_status: order.status,
            remaining_quantity_mt: remaining,
            released_quantity_mt: released,
            previous_version,
            version,
            expires_at: order.expires_at.ok_or_else(super::transaction::inconsistent_stock)?,
        });
    }
    Ok(effects)
}
