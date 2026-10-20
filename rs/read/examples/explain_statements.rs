//! Print the exact composed statement for each Task 9/10 section-8 item.
//!
//! Decision 110: this pass chooses changes by plan work, not by timing. The
//! disposable `scripts/rust_contract/explain_routes.py` runs `EXPLAIN
//! (ANALYZE, BUFFERS)` on these bytes with the same binds, so the plan it
//! reports is the statement the route runs — no second SQL composer.
//!
//! Diagnostic only; no gate.

use serde_json::{json, Value};
use verdaxis_read::market::visibility::{
    order_query_sql, order_query_sql_projected, trade_query_sql, COUNT_PROJECTION, MAP_PROJECTION,
};
use verdaxis_read::routes::curves_canonical::curve_unknown_metadata;

fn main() {
    let mut statements = serde_json::Map::new();
    let mut add = |name: &str, profile: &str, sql: String| {
        statements.insert(name.to_owned(), json!({"profile": profile, "sql": sql}));
    };

    // Task 9 — curves, reference export and the MCP market slice.
    add(
        "R09.orders",
        "curve_keys",
        order_query_sql(include_str!("../sql/curves/orders.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R10.orders",
        "slice",
        order_query_sql(include_str!("../sql/curves/orders.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R10.depth",
        "depth",
        order_query_sql(include_str!("../sql/curves/depth.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R11.orders",
        "board",
        order_query_sql(include_str!("../sql/curves/orders.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R11.depth",
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
    add(
        "R14.reference",
        "reference",
        trade_query_sql(include_str!("../sql/prices/reference.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "MCP.orders",
        "mcp_slice",
        order_query_sql(
            ", requested_slice_keys AS MATERIALIZED (SELECT product_id, delivery_point_id, \
             availability_window FROM jsonb_to_recordset($14::jsonb) AS k(product_id uuid, \
             delivery_point_id uuid, availability_window text)), slice_orders AS (SELECT \
             product_id, delivery_point_id, availability_window, price_per_mt_usd::text AS price, \
             provenance::text AS provenance, carbon_intensity_gco2_mj::text AS ci, \
             energy_density_mj_kg::text AS energy FROM eligible_orders WHERE (product_id::text || \
             E'\\x01' || delivery_point_id::text || E'\\x01' || availability_window) = ANY \
             (ARRAY(SELECT k.product_id::text || E'\\x01' || k.delivery_point_id::text || \
             E'\\x01' || k.availability_window FROM requested_slice_keys AS k)) ORDER BY \
             price_per_mt_usd, acceptance_ordinal) SELECT * FROM slice_orders",
        )
        .sql()
        .to_owned(),
    );

    // Task 10 — orderbook aggregate, counts, distinct labels, map and listings.
    add(
        "R19.aggregate",
        "aggregate",
        order_query_sql_projected(
            include_str!("../sql/orderbook/aggregate.sql"),
            MAP_PROJECTION,
        )
        .sql()
        .to_owned(),
    );
    add(
        "R20.counts",
        "none",
        order_query_sql_projected(
            include_str!("../sql/orderbook/counts.sql"),
            COUNT_PROJECTION,
        )
        .sql()
        .to_owned(),
    );
    add(
        "R21.aggregate",
        "aggregate",
        order_query_sql_projected(
            include_str!("../sql/orderbook/aggregate.sql"),
            MAP_PROJECTION,
        )
        .sql()
        .to_owned(),
    );
    add(
        "R21.recent",
        "none",
        order_query_sql_projected(
            include_str!("../sql/orderbook/recent_asks.sql"),
            MAP_PROJECTION,
        )
        .sql()
        .to_owned(),
    );
    add(
        "R24.distinct",
        "regions",
        order_query_sql(include_str!("../sql/orderbook/distinct.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R25.distinct",
        "fuel-types",
        order_query_sql(include_str!("../sql/orderbook/distinct.sql"))
            .sql()
            .to_owned(),
    );
    add(
        "R27.listings",
        "listings",
        order_query_sql(include_str!("../sql/orderbook/listings.sql"))
            .sql()
            .to_owned(),
    );

    let document = Value::Object(statements);
    println!("{}", serde_json::to_string(&document).unwrap());
}
