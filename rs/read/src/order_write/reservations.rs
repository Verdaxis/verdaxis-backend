use std::collections::BTreeMap;

use rust_decimal::Decimal;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::invalidation::{InvalidationError, refusal};

pub(super) async fn release_uncommitted(
    conn: &mut PgConnection,
    releases: &BTreeMap<Uuid, Decimal>,
) -> Result<(), InvalidationError> {
    // Inventory is last in the market row lock order. Keep settlement stock reserved.
    for (id, quantity) in releases {
        let row = sqlx::query(
            "SELECT current_stock_mt,reserved_stock_mt FROM inventory_items WHERE id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| {
            refusal(
                "INVENTORY_RESERVATION_INVALID",
                "The inventory reservation is missing.",
            )
        })?;
        let current: Decimal = row.try_get("current_stock_mt")?;
        let reserved: Decimal = row.try_get("reserved_stock_mt")?;
        let remaining = reserved
            .checked_sub(*quantity)
            .filter(|v| *v >= Decimal::ZERO)
            .ok_or_else(|| {
                refusal(
                    "INVENTORY_RESERVATION_INVALID",
                    "Reserved inventory is smaller than the uncommitted release.",
                )
            })?;
        let available = current.checked_add(*quantity).ok_or_else(|| {
            refusal(
                "INVENTORY_RESERVATION_INVALID",
                "Inventory quantity is out of range.",
            )
        })?;
        sqlx::query(
            "UPDATE inventory_items SET current_stock_mt=$2,reserved_stock_mt=$3 WHERE id=$1",
        )
        .bind(id)
        .bind(available)
        .bind(remaining)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
