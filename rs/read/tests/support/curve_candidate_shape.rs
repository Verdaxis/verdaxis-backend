//! Historical keyed candidate shape, retained only for equivalence checks.

use verdaxis_read::market::visibility::public_order_sql;

pub fn curve_candidate_sql(tail: &str) -> String {
    let relation = public_order_sql().replace("FROM orderbook_orders AS o", "FROM curve_orders AS o");
    format!(
        "WITH requested_curve_slices AS MATERIALIZED ({}) , \
         curve_orders AS NOT MATERIALIZED ({}) , eligible_orders AS NOT MATERIALIZED ({}) {}",
        include_str!("sql/requested_curve_slices.sql"),
        include_str!("sql/curve_orders.sql"),
        relation,
        tail
    )
}
