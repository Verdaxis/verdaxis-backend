//! Trusted monitoring evidence. Rank each evidence class before selecting REAL.
#[path = "curve_keys.rs"]
pub mod curve_keys;
use super::curve_projection::{decimal, timestamp};
use crate::{ApiError, Json};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::{PgConnection, Row, postgres::PgRow};
use std::collections::{HashMap, HashSet};

pub type SliceKey = (String, String, String);

pub fn key(row: &PgRow) -> Result<SliceKey, ApiError> {
    Ok((
        row.try_get("market_product")
            .map_err(|_| ApiError::internal())?,
        row.try_get("delivery_point_id")
            .map_err(|_| ApiError::internal())?,
        row.try_get("availability_window")
            .map_err(|_| ApiError::internal())?,
    ))
}

pub struct Signals {
    pub indications: HashMap<SliceKey, Vec<PgRow>>,
    pub bands: HashMap<SliceKey, Vec<PgRow>>,
    pub stems: HashMap<SliceKey, Vec<PgRow>>,
    pub now: DateTime<Utc>,
}

async fn load_family(
    connection: &mut PgConnection,
    sql: &'static str,
    keys: &Json,
    now: DateTime<Utc>,
) -> Result<HashMap<SliceKey, Vec<PgRow>>, ApiError> {
    // Rank in PostgreSQL so obsolete observations do not cross the wire.
    let sql = sql.replace("/* exact_or_grid_keys */", curve_keys::FILTER);
    let ranked = format!(
        "WITH requested_keys AS (SELECT * FROM jsonb_to_recordset($1::jsonb) \
         AS k(market_product text, delivery_point_id uuid, availability_window text)), candidates AS ({sql}), ranked AS (SELECT candidates.*, \
         row_number() OVER (PARTITION BY market_product, delivery_point_id, \
         availability_window, partition_key, evidence \
         ORDER BY observed_at DESC, created_at DESC, id DESC) AS position \
         FROM candidates) SELECT * FROM ranked WHERE position = 1 \
         ORDER BY observed_at DESC, created_at DESC, id DESC"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(ranked))
        .bind(keys.to_string())
        .bind(now)
        .bind(curve_keys::complete_grid(keys))
        .fetch_all(connection)
        .await
        .map_err(|error| crate::routes::ports::query_error("/api/curves/forward/table", &error))?;
    let mut seen = HashSet::new();
    let mut result: HashMap<SliceKey, Vec<PgRow>> = HashMap::new();
    for row in rows {
        let key = key(&row)?;
        let partition: String = row
            .try_get("partition_key")
            .map_err(|_| ApiError::internal())?;
        let evidence: String = row.try_get("evidence").map_err(|_| ApiError::internal())?;
        if seen.insert((key.clone(), partition, evidence)) {
            result.entry(key).or_default().push(row);
        }
    }
    Ok(result)
}

fn text(row: &PgRow, field: &str) -> Json {
    row.try_get::<Option<String>, _>(field)
        .ok()
        .flatten()
        .map(Json::String)
        .unwrap_or(Json::Null)
}
fn time(row: &PgRow, field: &str) -> Json {
    row.try_get::<Option<DateTime<Utc>>, _>(field)
        .ok()
        .flatten()
        .map(timestamp)
        .unwrap_or(Json::Null)
}
fn evidence(row: &PgRow) -> &str {
    row.get("evidence")
}
fn selected(rows: &[PgRow]) -> Vec<&PgRow> {
    let class = if rows.iter().any(|r| evidence(r) == "REAL") {
        "REAL"
    } else {
        "DEMO"
    };
    rows.iter().filter(|r| evidence(r) == class).collect()
}

pub fn provenance(rows: &[&PgRow], family: &str, now: DateTime<Utc>) -> Json {
    let real = rows.iter().filter(|r| evidence(r) == "REAL").count();
    let demo = rows.iter().filter(|r| evidence(r) == "DEMO").count();
    let unknown = rows.len() - real - demo;
    let (class, source, status) = if real > 0 {
        (
            "REAL",
            if family == "FAIR_PRICE_BAND" {
                "FAIR_PRICE_MODEL"
            } else {
                family
            },
            "REAL_ONLY",
        )
    } else if demo > 0 {
        ("DEMO", "DEMO_SEED", "DEMO_ONLY")
    } else if unknown > 0 {
        ("UNKNOWN", "UNKNOWN", "UNKNOWN")
    } else {
        ("", "NO_DATA", "NOT_APPLICABLE")
    };
    let observed = rows
        .iter()
        .filter(|r| evidence(r) == class)
        .filter_map(|r| r.try_get::<DateTime<Utc>, _>("observed_at").ok())
        .max()
        .map(timestamp);
    json!({"signal_type":family,"signal_source_kind":source,"scope":"DELIVERY_POINT",
        "demo_status":status,"observed_at":observed,"generated_at":timestamp(now),
        "real_count":real,"demo_count":demo,"unknown_count":unknown})
}

impl Signals {
    pub async fn load(
        connection: &mut PgConnection,
        keys: &Json,
        now: DateTime<Utc>,
        table: bool,
    ) -> Result<Self, ApiError> {
        let indications = load_family(
            connection,
            include_str!("../../sql/curves/indications.sql"),
            keys,
            now,
        )
        .await?;
        let bands = load_family(
            connection,
            include_str!("../../sql/curves/bands.sql"),
            keys,
            now,
        )
        .await?;
        let stems = if table {
            HashMap::new()
        } else {
            load_family(
                connection,
                include_str!("../../sql/curves/stems.sql"),
                keys,
                now,
            )
            .await?
        };
        Ok(Self {
            indications,
            bands,
            stems,
            now,
        })
    }
    pub fn indication_summary(&self, key: &SliceKey) -> Json {
        let rows = self.indications.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let chosen = selected(rows);
        let mut bid = Json::Null;
        let mut ask = Json::Null;
        let mut mid = Json::Null;
        let mut quantity = Decimal::ZERO;
        let mut has_quantity = false;
        for row in &chosen {
            match row.get::<&str, _>("side") {
                "BID" => bid = text(row, "price"),
                "ASK" => ask = text(row, "price"),
                _ => mid = text(row, "price"),
            }
            if let Some(value) = text(row, "quantity").as_str() {
                quantity += decimal(value);
                has_quantity = true;
            }
        }
        json!({"provenance":provenance(&rows.iter().collect::<Vec<_>>(), "MARKET_INDICATION", self.now),
            "latest_bid_price_per_mt_usd":bid,"latest_ask_price_per_mt_usd":ask,"latest_mid_price_per_mt_usd":mid,
            "total_quantity_mt":has_quantity.then(|| quantity.to_string()),"indication_count":chosen.len()})
    }
    pub fn band(&self, key: &SliceKey) -> Json {
        let Some(rows) = self.bands.get(key) else {
            return Json::Null;
        };
        let chosen = selected(rows);
        let value = |field| chosen.first().map(|r| text(r, field)).unwrap_or(Json::Null);
        json!({"low_price_per_mt_usd":value("low"),"mid_price_per_mt_usd":value("mid"),
            "high_price_per_mt_usd":value("high"),"provenance":provenance(&rows.iter().collect::<Vec<_>>(), "FAIR_PRICE_BAND", self.now)})
    }
    pub fn stem_summary(&self, key: &SliceKey) -> Json {
        let rows = self.stems.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let mut available = Decimal::ZERO;
        let mut tentative = Decimal::ZERO;
        let mut count = 0;
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        for row in selected(rows) {
            let quantity = decimal(row.get("quantity"));
            match row.get::<&str, _>("status") {
                "AVAILABLE" => available += quantity,
                "TENTATIVE" => tentative += quantity,
                _ => continue,
            }
            count += 1;
            if let Ok(Some(value)) = row.try_get::<Option<DateTime<Utc>>, _>("stem_start") {
                starts.push(value);
            }
            if let Ok(Some(value)) = row.try_get::<Option<DateTime<Utc>>, _>("stem_end") {
                ends.push(value);
            }
        }
        json!({"provenance":provenance(&rows.iter().collect::<Vec<_>>(), "PHYSICAL_STEM", self.now),
            "available_quantity_mt":(available > Decimal::ZERO).then(|| available.to_string()),
            "tentative_quantity_mt":(tentative > Decimal::ZERO).then(|| tentative.to_string()),"stem_count":count,
            "earliest_stem_start":starts.into_iter().min().map(timestamp),"latest_stem_end":ends.into_iter().max().map(timestamp)})
    }
    pub fn focus_indications(&self, key: &SliceKey) -> Vec<Json> {
        selected(self.indications.get(key).map(Vec::as_slice).unwrap_or(&[])).into_iter().take(10).map(|row|
            json!({"side":text(row,"side"),"price_per_mt_usd":text(row,"price"),"quantity_mt":text(row,"quantity"),
                "provenance":provenance(&[row],"MARKET_INDICATION",self.now)})).collect()
    }
    pub fn focus_stems(&self, key: &SliceKey, limit: usize) -> Vec<Json> {
        selected(self.stems.get(key).map(Vec::as_slice).unwrap_or(&[])).into_iter().take(limit).map(|row|
            json!({"quantity_mt":text(row,"quantity"),"status":text(row,"status"),"stem_start":time(row,"stem_start"),"stem_end":time(row,"stem_end"),
                "provenance":provenance(&[row],"PHYSICAL_STEM",self.now)})).collect()
    }
}
