//! Legacy curve scope: current REAL orders, without active catalog admission.
//! It deliberately does not apply typed benchmark qualification.
use super::visibility::OrderQuery;

/// Use the common order binder; the tail reads `eligible_orders`.
/// Only product and delivery point filters apply to this named relation.
pub fn legacy_curve_query(tail: &'static str) -> OrderQuery {
    let relation = crate::market::visibility::expand_shared_markers(
        &include_str!("../../sql/legacy_curve_orders.sql").replace(
            "/* current_order_rule */",
            include_str!("../../sql/current_order_rule.sql"),
        ),
        "$13",
        "$2",
    );
    OrderQuery(format!("WITH eligible_orders AS ({relation}) {tail}"))
}
