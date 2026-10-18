//! Price quarantine metadata shares the formal snapshot core with economics.
use super::TradeQuery;

/// The returned relation exposes identity and observation time, never economics.
/// Bind with `bind_trade_query`; consumer parameters begin at $7.
pub fn price_quarantine_query(tail: &'static str) -> TradeQuery {
    TradeQuery(format!(
        "WITH quarantine_metadata AS ({} {}) {}",
        include_str!("../../sql/formal_trade_snapshots.sql"),
        include_str!("../../sql/price_quarantine.sql"),
        tail
    ))
}
