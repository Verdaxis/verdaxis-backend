//! Print the exact composed statements for each Task 9/10 section-8 item.
//!
//! Decision 110: this pass chooses changes by plan work, not by timing. The
//! disposable `scripts/rust_contract/explain_routes.py` runs `EXPLAIN
//! (ANALYZE, BUFFERS)` on these bytes with the same binds, so the plan it
//! reports is the statement the route runs — no second SQL composer.
//!
//! Every changed item prints its `before` and `after` shape. `before` is the
//! constructor the branch base used, frozen here so both plans run on one
//! database with one set of statistics; `after` is what the route runs now.
//!
//! Diagnostic only; no gate.

use serde_json::{Value, json};
use verdaxis_read::market::visibility::{
    COUNT_PROJECTION, DISTINCT_PROJECTION, MAP_PROJECTION, order_query_sql,
    order_query_sql_projected,
};
use verdaxis_read::routes::curves_canonical::curve_unknown_metadata;

const MCP_TAIL: &str = ", slice_orders AS (SELECT product_id, delivery_point_id, \
     availability_window, price_per_mt_usd::text AS price, provenance::text AS provenance, \
     carbon_intensity_gco2_mj::text AS ci, energy_density_mj_kg::text AS energy \
     FROM eligible_orders ORDER BY price_per_mt_usd, acceptance_ordinal) \
     SELECT * FROM slice_orders";

const MCP_RESOLVE_BEFORE: &str = ", requested_slice_keys AS MATERIALIZED (SELECT product_id, delivery_point_id, availability_window FROM jsonb_to_recordset($14::jsonb) AS k(product_id uuid, delivery_point_id uuid, availability_window text)), slice_orders AS (SELECT product_id, delivery_point_id, availability_window, price_per_mt_usd::text AS price, provenance::text AS provenance, carbon_intensity_gco2_mj::text AS ci, energy_density_mj_kg::text AS energy FROM eligible_orders WHERE (product_id::text || E'\\x01' || delivery_point_id::text || E'\\x01' || availability_window) = ANY (ARRAY(SELECT k.product_id::text || E'\\x01' || k.delivery_point_id::text || E'\\x01' || k.availability_window FROM requested_slice_keys AS k)) ORDER BY price_per_mt_usd, acceptance_ordinal) SELECT * FROM slice_orders";

const MCP_SLICE: &str = "SELECT side::text AS side, price_per_mt_usd::text AS price, remaining_quantity_mt::text AS quantity, provenance::text AS provenance FROM eligible_orders";

#[path = "../tests/support/curve_candidate_shape.rs"]
mod curve_candidate_shape;
use curve_candidate_shape::curve_candidate_sql;

const AGGREGATE_PROJECTION: &str = "o.product_id, o.delivery_point_id, o.side, o.provenance, \
     o.availability_window, o.price_per_mt_usd, o.remaining_quantity_mt, o.created_at,";

fn main() {
    let mut statements = serde_json::Map::new();
    let mut add = |name: &str, profile: &str, sql: String| {
        statements.insert(name.to_owned(), json!({"profile": profile, "sql": sql}));
    };

    let orders = include_str!("../sql/curves/orders.sql");
    // R09/R10/R11: compare the parent with the rejected keyed candidate.
    for (item, profile) in [
        ("R09.orders", "curve_keys"),
        ("R10.orders", "slice"),
        ("R11.orders", "board"),
        ("MCP.orders", "mcp_slice"),
    ] {
        add(
            &format!("{item}.before"),
            if item == "MCP.orders" {
                "mcp_slice_before"
            } else {
                profile
            },
            order_query_sql(if item == "MCP.orders" {
                MCP_TAIL
            } else {
                orders
            })
            .sql()
            .to_owned(),
        );
        add(
            &format!("{item}.after"),
            profile,
            curve_candidate_sql(if item == "MCP.orders" {
                MCP_TAIL
            } else {
                orders
            }),
        );
    }
    add(
        "MCP.resolve.before",
        "mcp_resolve",
        order_query_sql(MCP_RESOLVE_BEFORE).sql().to_owned(),
    );
    add(
        "MCP.resolve.after",
        "mcp_resolve",
        curve_candidate_sql(MCP_TAIL),
    );
    add(
        "MCP.slice.before",
        "mcp_slice_before",
        order_query_sql(MCP_SLICE).sql().to_owned(),
    );
    add(
        "MCP.slice.after",
        "mcp_slice",
        curve_candidate_sql(MCP_SLICE),
    );
    // The slice's depth read keeps the scalar binds; it is not keyed by $14.
    add(
        "R10.depth",
        "depth",
        order_query_sql(include_str!("../sql/curves/depth.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R09.unknown",
        "unknown_keys",
        curve_unknown_metadata::statement_sql(),
    );

    // Task 10 — orderbook aggregate, counts, distinct labels, map and listings.
    let aggregate = include_str!("../sql/orderbook/aggregate.sql");
    add(
        "R19.aggregate.before",
        "aggregate",
        order_query_sql_projected(aggregate, MAP_PROJECTION)
            .sql()
            .to_owned(),
    );
    add(
        "R19.aggregate.after",
        "aggregate",
        order_query_sql_projected(aggregate, AGGREGATE_PROJECTION)
            .sql()
            .to_owned(),
    );
    let counts = include_str!("../sql/orderbook/counts.sql");
    add(
        "R20.counts.before",
        "none",
        order_query_sql_projected(counts, COUNT_PROJECTION)
            .sql()
            .to_owned(),
    );
    add(
        "R20.counts.after",
        "none",
        order_query_sql_projected(counts, COUNT_PROJECTION)
            .sql()
            .to_owned(),
    );
    let distinct = include_str!("../sql/orderbook/distinct.sql");
    for (item, profile) in [("R24.distinct", "regions"), ("R25.distinct", "fuel-types")] {
        add(
            &format!("{item}.before"),
            profile,
            order_query_sql(distinct).sql().to_owned(),
        );
        add(
            &format!("{item}.after"),
            profile,
            order_query_sql_projected(distinct, DISTINCT_PROJECTION)
                .sql()
                .to_owned(),
        );
    }

    println!(
        "{}",
        serde_json::to_string(&Value::Object(statements)).unwrap()
    );
}
