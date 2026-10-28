//! Persisted benchmark reads and source-attested legacy board fallback data.
use super::{
    curve_projection::{decimal, money, timestamp},
    signals::{SliceKey, key},
};
use crate::{ApiError, Json};
use chrono::{DateTime, Datelike, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use std::collections::HashMap;

pub fn policy() -> &'static Json {
    static POLICY: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("curve_policy.json")).expect("committed curve policy")
    });
    &POLICY
}

pub async fn load(
    connection: &mut PgConnection,
    keys: &Json,
) -> Result<HashMap<SliceKey, Json>, ApiError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(include_str!(
        "../../sql/curves/benchmarks.sql"
    )))
    .bind(keys.to_string())
    .fetch_all(connection)
    .await
    .map_err(|error| crate::routes::ports::query_error("/api/curves/forward/board", &error))?;
    rows.into_iter()
        .map(|row| {
            let key = key(&row)?;
            Ok((
                key,
                json!({"price":row.get::<String,_>("price"),"source":row.get::<String,_>("source"),
            "observed_at":row.get::<Option<DateTime<Utc>>,_>("observed_at").map(timestamp)}),
            ))
        })
        .collect()
}

pub fn board_quote(stored: Option<&Json>, key: &SliceKey, now: DateTime<Utc>) -> Json {
    if let Some(stored) = stored {
        return stored.clone();
    }
    let base = policy()["seed_quotes"]
        .as_array()
        .expect("seed quote data")
        .iter()
        .find(|row| row["market_product"] == key.0 && row["delivery_point_id"] == key.1);
    let Some(base) = base else {
        return Json::Null;
    };
    let adjustment = if key.2 == "SPOT" {
        0
    } else {
        let start = crate::market::windows::window_start_date(&key.2, now.date_naive()).ok();
        start
            .map(|date| {
                ((date.year() - now.year()) * 12 + date.month() as i32 - now.month() as i32).max(0)
            })
            .unwrap_or(0)
    };
    let rate = if key.2.ends_with("-CAL") {
        "5.00"
    } else {
        "7.50"
    };
    json!({"price":money(decimal(base["base_price"].as_str().expect("base price"))+decimal(rate)*rust_decimal::Decimal::from(adjustment)),
        "source":"seed_matrix","observed_at":null})
}
