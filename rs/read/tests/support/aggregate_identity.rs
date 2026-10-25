//! SQL-B equivalence pin. The old and proposed tails are frozen independently
//! of the shipped tail, so this proof also runs before the implementation.
use super::*;
use sha2::{Digest, Sha256};

const OLD: &str = include_str!("../data/aggregate_labels_before.sql");
const CANDIDATE: &str = include_str!("../data/aggregate_identity_candidate.sql");
const SHIPPED: &str = include_str!("../../sql/orderbook/aggregate.sql");

fn typed_rows(sql: &str) -> String {
    format!(
        "SELECT jsonb_build_object('value', to_jsonb(x), \
         'types', jsonb_build_array(pg_typeof(x.min_price)::text, \
         pg_typeof(x.max_price)::text, pg_typeof(x.total_quantity)::text, \
         pg_typeof(x.order_count)::text, pg_typeof(x.product_total_order_count)::text))::text \
         AS row FROM ({sql}) x"
    )
}

fn ordered_hash(rows: &[String]) -> String {
    let mut digest = Sha256::new();
    for row in rows {
        digest.update((row.len() as u64).to_be_bytes());
        digest.update(row.as_bytes());
    }
    digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn equivalent(old: &[String], new: &[String], case: &str) {
    assert_eq!(old.len(), new.len(), "row count: {case}");
    assert_eq!(ordered_hash(old), ordered_hash(new), "ordered hash: {case}");
    assert_eq!(old, new, "ordered complete typed rows: {case}");
}

pub(super) async fn aggregate_identity_equivalence(
    connection: &mut PgConnection,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
) {
    let mut broad_counts = Vec::new();
    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for (name, filters) in [
            ("all", PublicOrderFilters::default()),
            ("bid", PublicOrderFilters { side: Some("BID"), ..Default::default() }),
            ("ask", PublicOrderFilters { side: Some("ASK"), ..Default::default() }),
            ("empty", PublicOrderFilters { product_id: Some(Uuid::nil()), ..Default::default() }),
        ] {
            for limit in [1000, 1, 0] {
                let mut results = Vec::new();
                for tail in [OLD, CANDIDATE, SHIPPED] {
                    let sql = order_query_sql_projected(tail, MAP_PROJECTION);
                    results.push(rows(connection, &typed_rows(sql.sql()), policy, clock,
                        &config, &filters, &[Extra::Int(limit)]).await);
                }
                let case = format!("{name} gate={gate} limit={limit}");
                equivalent(&results[0], &results[1], &case);
                equivalent(&results[0], &results[2], &case);
                if name == "empty" || limit == 0 {
                    assert!(results[0].is_empty(), "empty result: {case}");
                } else {
                    assert!(!results[0].is_empty(), "populated result: {case}");
                }
                if name == "all" && limit == 1000 {
                    let values: Vec<Value> = results[0].iter()
                        .map(|row| serde_json::from_str(row).unwrap()).collect();
                    assert!(values.iter().any(|row| row["value"]["order_count"].as_i64().unwrap() > 1),
                        "eligible duplicate keys must contribute more than once");
                    broad_counts.push(values.iter().map(|row| row["value"]["order_count"].as_i64().unwrap()).sum::<i64>());
                    for row in values {
                        assert_eq!(row["types"], serde_json::json!(["numeric", "numeric", "numeric", "bigint", "bigint"]));
                    }
                }
            }
        }
    }
    assert!(broad_counts[1] > broad_counts[0], "the fixture must exercise strict and advisory admission");
    let raw: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe("SELECT count(*) FROM orderbook_orders"))
        .fetch_one(&mut *connection).await.unwrap();
    assert!(raw > broad_counts[1], "ineligible fixture rows must be excluded before grouping");
    synthetic_labels(connection).await;
}

async fn synthetic_labels(connection: &mut PgConnection) {
    // Catalog labels are dependent on identities. Duplicate display names,
    // NULL regions, non-ASCII labels and exact-scale numeric values remain so.
    // Distinct windows break all output ties, retaining the production ORDER BY.
    let prefix = r#"
WITH products(id, name, fuel_type) AS (VALUES
    ('00000000-0000-0000-0000-000000000001'::uuid, 'same', 'fuel')),
delivery_points(id, name, region) AS (VALUES
    ('00000000-0000-0000-0000-000000000011'::uuid, 'Ångström', NULL::text),
    ('00000000-0000-0000-0000-000000000012'::uuid, 'Alpha', 'region'),
    ('00000000-0000-0000-0000-000000000013'::uuid, 'Ångström', 'region')),
input(point, window_code, side, provenance, price, quantity, created_at) AS (VALUES
    (11, 'SPOT', 'ASK', 'REAL', 100.00::numeric, 10.000::numeric, '2026-10-01Z'::timestamptz),
    (11, 'SPOT', 'ASK', 'REAL', 120.00::numeric, 5.000::numeric, '2026-10-02Z'::timestamptz),
    (11, 'SPOT', 'BID', 'REAL', 90.00::numeric, 1.000::numeric, '2026-10-01Z'::timestamptz),
    (11, 'SPOT', 'ASK', 'DEMO', 80.00::numeric, 2.000::numeric, '2026-10-01Z'::timestamptz),
    (12, '2026-11', 'ASK', 'REAL', 0.01::numeric, 0.001::numeric, '2026-10-01Z'::timestamptz),
    (13, '2026-12', 'ASK', 'REAL', 9999999999999999.99::numeric, 0.001::numeric, '2026-10-01Z'::timestamptz)),
eligible_orders AS (
    SELECT p.id AS product_id, p.name AS product_name,
           ($1::jsonb -> 'products' -> 0 ->> 'code') AS market_product,
           p.fuel_type AS product_fuel_type, dp.id AS delivery_point_id,
           dp.name AS delivery_point_name, dp.region AS delivery_point_region,
           i.window_code AS availability_window, i.side, i.provenance,
           i.price AS price_per_mt_usd, i.quantity AS remaining_quantity_mt, i.created_at
    FROM input i CROSS JOIN products p JOIN delivery_points dp
      ON right(dp.id::text, 2)::int = i.point
)
"#;
    let policy = serde_json::json!({"products": [{"id": "00000000-0000-0000-0000-000000000001", "code": "Test"}]}).to_string();
    for limit in [1000_i64, 1, 0] {
        let mut results = Vec::new();
        for tail in [OLD, CANDIDATE, SHIPPED] {
            let sql = typed_rows(&format!("{prefix}{}", tail.replace("$14", "$2")));
            let result = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(&policy).bind(limit)
                .fetch_all(&mut *connection).await.unwrap().into_iter()
                .map(|row| row.get::<String, _>("row")).collect::<Vec<_>>();
            results.push(result);
        }
        equivalent(&results[0], &results[1], "collation, duplicate labels and numeric types");
        equivalent(&results[0], &results[2], "shipped synthetic labels");
        if limit == 1000 {
            assert_eq!(results[0].len(), 5);
            let values: Vec<Value> = results[0].iter().map(|row| serde_json::from_str(row).unwrap()).collect();
            let duplicate = values.iter().find(|row| row["value"]["order_count"] == 2).unwrap();
            assert_eq!(duplicate["value"]["total_quantity_text"], "15.000");
            assert_eq!(duplicate["value"]["min_price_text"], "100.00");
            assert_eq!(duplicate["value"]["max_price_text"], "120.00");
            assert!(duplicate["value"]["delivery_point_region"].is_null());
            assert_eq!(duplicate["value"]["product_total_order_count"], 5);
        }
    }
}
