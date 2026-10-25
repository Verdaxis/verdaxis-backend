//! Exact-slice opposing scan proof on the market runner's seeded PostgreSQL.

use super::*;
use sha2::{Digest, Sha256};

const OLD_OPPOSING: &str = r#"
SELECT keys.slice,
       CASE WHEN min(o.side::text) = 'ASK'
            THEN min(o.price_per_mt_usd)
            ELSE max(o.price_per_mt_usd) END AS best_price
FROM eligible_orders AS o
JOIN jsonb_array_elements($14::jsonb) AS keys(slice)
  ON o.product_id = (keys.slice ->> 'product_id')::uuid
 AND o.delivery_point_id IS NOT DISTINCT FROM
     (keys.slice ->> 'delivery_point_id')::uuid
 AND o.availability_window = keys.slice ->> 'availability_window'
 AND o.provenance::text = keys.slice ->> 'provenance'
GROUP BY keys.slice
"#;

#[tokio::test]
async fn opposing_null_point_prefilter_keeps_the_tail_match() {
    let pools = connect(&config()).await.unwrap();
    let mut connection = pools.read.acquire().await.unwrap();
    let key = serde_json::json!({
        "product_id": "00000000-0000-0000-0000-000000000001",
        "delivery_point_id": null,
        "availability_window": "SPOT",
        "provenance": "REAL"
    });
    // A page repeats the same NULL slice. Membership must not duplicate base
    // rows, and the old tail must still group the repeated page key once.
    let keys = serde_json::json!([key, key]);
    let base = include_str!("sql/opposing_null_orders.sql");
    let keyed = include_str!("../../sql/orderbook/opposing_keyed_orders.sql")
        .replace("orderbook_orders", "fixture_orders")
        .replace("$14", "$1");
    for (side, expected) in [("BID", "610.25"), ("ASK", "620.75")] {
        let tail = OLD_OPPOSING.replace("$14", "$1");
        let mut results = Vec::new();
        for scan in ["fixture_orders", keyed.as_str()] {
            let sql = format!(
                "WITH fixture_orders AS ({base}), eligible_orders AS \
                 (SELECT * FROM {scan} AS o WHERE side = $2) \
                 SELECT to_jsonb(x)::text AS row FROM ({tail}) x ORDER BY slice::text"
            );
            let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(keys.to_string())
                .bind(side)
                .fetch_all(&mut *connection)
                .await
                .unwrap();
            results.push(rows);
        }
        assert_eq!(results[0].len(), 1, "old tail must match the NULL slice");
        let row: Value = serde_json::from_str(&results[0][0]).unwrap();
        assert_eq!(row["best_price"].to_string(), expected);
        assert_eq!(results[1].len(), 1, "prefilter dropped the {side} NULL slice");
        assert_eq!(ordered_hash(&results[0]), ordered_hash(&results[1]));
        assert_eq!(results[0], results[1]);
    }
}

fn ordered_hash(rows: &[String]) -> String {
    let mut hash = Sha256::new();
    for row in rows {
        hash.update((row.len() as u64).to_be_bytes());
        hash.update(row.as_bytes());
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn opposing_rows(
    connection: &mut PgConnection,
    sql: &str,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
    side: &str,
    keys: &Value,
) -> Vec<String> {
    // GROUP BY has no promised order. PostgreSQL, not Rust, applies the active
    // database collation to a deterministic wrapper for the equivalence hash.
    let sql = format!(
        "SELECT (to_jsonb(x) || jsonb_build_object('price_type', pg_typeof(x.best_price)::text, \
         'price_text', x.best_price::text))::text AS row FROM ({}) x ORDER BY x.slice::text",
        sql.trim_end().trim_end_matches(';')
    );
    bind_order_sql(
        &sql,
        policy,
        clock,
        config,
        &PublicOrderFilters {
            side: Some(side),
            ..PublicOrderFilters::default()
        },
    )
    .unwrap()
    .bind(keys.to_string())
    .fetch_all(connection)
    .await
    .expect("opposing query must run under the read-only role")
    .iter()
    .map(|row| row.get("row"))
    .collect()
}

#[tokio::test]
async fn opposing_exact_slices_keep_prices_and_ordered_hash() {
    let config = config();
    let pools = connect(&config).await.unwrap();
    let policy = policy();
    let clock = clock();
    let mut connection = pools.read.acquire().await.unwrap();
    // Use every raw fixture slice, not just admitted slices. Rejected owners,
    // expired orders, wrong lanes and mixed provenance must be pruned by the
    // unchanged admission relation, not by test key selection.
    let keys: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(
        "SELECT jsonb_agg(DISTINCT jsonb_build_object(
            'product_id', product_id, 'delivery_point_id', delivery_point_id,
            'availability_window', availability_window, 'provenance', provenance))
         FROM orderbook_orders"
            .to_owned(),
    ))
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let keys = keys.as_array().unwrap();
    assert!(keys.len() > 5, "the seeded slice matrix must be populated");
    let old = order_query_sql(OLD_OPPOSING);
    // The historical IN trial is pinned only for the full eligible relation:
    // that relation's inner point join and lane check exclude NULL points.
    let trial = old.sql().replace(
        "FROM orderbook_orders AS o",
        &format!(
            "FROM {} AS o",
            include_str!("sql/opposing_keyed_orders.sql").trim()
        ),
    );
    assert_ne!(old.sql(), trial, "the trial must restrict the base scan");
    let shipped = verdaxis_read::market::visibility::opposing_order_query_sql();
    let mut variants = vec![Value::Array(keys.clone()), serde_json::json!([])];
    let mut duplicate_and_null = keys.clone();
    duplicate_and_null.extend(keys.iter().cloned());
    for key in keys {
        let mut null = key.clone();
        null["delivery_point_id"] = Value::Null;
        duplicate_and_null.push(null);
        // Extra JSON members remain part of the tail's group key. They also
        // exercise collation without changing the exact slice membership.
        for label in ["A", "a", "é", "Z"] {
            let mut labelled = key.clone();
            labelled["label"] = Value::String(label.into());
            duplicate_and_null.push(labelled);
        }
    }
    variants.push(Value::Array(duplicate_and_null));
    let mut missing = keys[0].clone();
    missing["availability_window"] = Value::String("2099-12".into());
    variants.push(serde_json::json!([missing]));

    for gate in [true, false] {
        let mut config = config.clone();
        config.kyc_execution_gate_enabled = gate;
        for side in ["BID", "ASK"] {
            for (case, keys) in variants.iter().enumerate() {
                let before = opposing_rows(
                    &mut connection,
                    old.sql(),
                    &policy,
                    &clock,
                    &config,
                    side,
                    keys,
                )
                .await;
                for candidate in [&trial, shipped.sql()] {
                    let after = opposing_rows(
                        &mut connection,
                        candidate,
                        &policy,
                        &clock,
                        &config,
                        side,
                        keys,
                    )
                    .await;
                    assert_eq!(before.len(), after.len(), "count gate={gate} {side} {case}");
                    assert_eq!(ordered_hash(&before), ordered_hash(&after));
                    assert_eq!(before, after, "ordered rows gate={gate} {side} {case}");
                }
                if case == 0 {
                    assert!(!before.is_empty(), "both opposing sides must have prices");
                }
                if case == 1 || case == 3 {
                    assert!(before.is_empty(), "empty and missing slices return no rows");
                }
            }
        }
    }
    // SQLx's decimal decoder and PostgreSQL must still see NUMERIC, not float
    // or text. Fetch type metadata even when a particular slice has no rows.
    let rows = bind_order_sql(
        old.sql(),
        &policy,
        &clock,
        &config,
        &PublicOrderFilters::default(),
    )
    .unwrap()
    .bind(Value::Array(keys.clone()).to_string())
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    use sqlx::{Column, TypeInfo};
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(row.column("best_price").type_info().name(), "NUMERIC");
        let price: rust_decimal::Decimal = row.get("best_price");
        assert_eq!(price, rust_decimal::Decimal::new(60000, 2));
    }
}
