//! Referral activation shared by automatic and manual trade confirmation.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use super::MarketWriteError;

/// Call after the confirmed trade row is visible on the caller transaction.
/// Current organization membership determines the referrals to advance.
/// Repeated confirmations leave already active referrals and their clock intact.
pub(super) async fn on_trade_confirmed(
    conn: &mut PgConnection,
    trade: Uuid,
    buyer: Uuid,
    seller: Uuid,
    at: DateTime<Utc>,
) -> Result<(), MarketWriteError> {
    sqlx::query("UPDATE referrals SET status='ACTIVE',activated_at=$3 WHERE status='VERIFIED' AND referred_user_id IN (SELECT id FROM users WHERE organization_id=ANY($1::uuid[])) AND EXISTS (SELECT 1 FROM trades WHERE id=$2 AND status::text='CONFIRMED' AND buyer_id=$4 AND seller_id=$5)")
        .bind([buyer,seller].as_slice())
        .bind(trade)
        .bind(at)
        .bind(buyer)
        .bind(seller)
        .execute(conn)
        .await?;
    Ok(())
}
