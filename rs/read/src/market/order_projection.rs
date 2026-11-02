//! Shared public order DTO; execution qualification remains separate.
use super::{
    live_benchmarks::{LiveBenchmark, LiveBenchmarkKey},
    terms::{canonical_terms_from_row, digest_terms, public_response_terms},
    windows::normalize_window,
};
use crate::routes::ports::pydantic_datetime;
use crate::{ApiError, Json};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::{Row, postgres::PgRow};

pub(crate) struct PublicOrder {
    pub(crate) private: Json,
    pub(crate) labels: Json,
    pub(crate) price: Decimal,
    pub(crate) side: String,
    pub(crate) provenance: String,
}

impl PublicOrder {
    pub(crate) fn from_row(row: &PgRow) -> Result<Self, ApiError> {
        let get = |name| {
            row.try_get::<String, _>(name)
                .map_err(|_| ApiError::internal())
        };
        let expires: Option<DateTime<Utc>> = row
            .try_get("expires_at")
            .map_err(|_| ApiError::internal())?;
        let created: DateTime<Utc> = row
            .try_get("created_at")
            .map_err(|_| ApiError::internal())?;
        Ok(Self {
            private: row
                .try_get("private_digest_input")
                .map_err(|_| ApiError::internal())?,
            labels: json!({
                "product_name": get("product_name")?, "market_product": get("market_product")?,
                "fuel_type": get("product_fuel_type")?, "fuel_grade": get("product_fuel_grade")?,
                "delivery_point_name": get("delivery_point_name")?, "region": get("delivery_point_region")?,
                "tier_label": get("tier_label")?, "expires_at": expires.map(pydantic_datetime),
                "created_at": pydantic_datetime(created),
            }),
            price: row
                .try_get("price_per_mt_usd")
                .map_err(|_| ApiError::internal())?,
            side: get("side")?,
            provenance: get("provenance")?,
        })
    }

    pub(crate) fn benchmark_key(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<LiveBenchmarkKey>, ApiError> {
        let product = self.labels["market_product"]
            .as_str()
            .filter(|value| !value.is_empty());
        let point = self.private["delivery_point_id"].as_str();
        let window = self.private["availability_window"]
            .as_str()
            .filter(|value| !value.is_empty());
        let (Some(product), Some(point), Some(window)) = (product, point, window) else {
            return Ok(None);
        };
        Ok(Some(LiveBenchmarkKey {
            side: self.side.clone(),
            product: product.into(),
            point: point.parse().map_err(|_| ApiError::internal())?,
            window: normalize_window(window, now.date_naive()).map_err(|_| ApiError::internal())?,
        }))
    }

    pub(crate) fn public(
        &self,
        crossed: bool,
        benchmark: Option<&LiveBenchmark>,
    ) -> Result<Json, ApiError> {
        self.public_with_terms(crossed, benchmark, None)
    }

    /// A caller may supply already validated public terms without changing the
    /// private row used for the reviewed-terms digest.
    pub(crate) fn public_with_terms(
        &self,
        crossed: bool,
        benchmark: Option<&LiveBenchmark>,
        normalized: Option<&Json>,
    ) -> Result<Json, ApiError> {
        let digest = canonical_terms_from_row(&self.private)
            .and_then(|terms| digest_terms(&terms))
            .map_err(|_| ApiError::internal())?;
        let mut out = serde_json::Map::new();
        // Explicit DTO allowlist: never copy an internal row or digest input.
        for name in [
            "certification_declared",
            "certification_scheme",
            "specification_standard",
            "msds_available",
            "carbon_intensity_gco2_mj",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec",
            "off_spec_notes",
            "id",
        ] {
            out.insert(name.into(), self.private[name].clone());
        }
        out.insert("terms_digest".into(), json!(digest));
        out.insert("version".into(), self.private["version"].clone());
        for (name, profile, schema) in [
            ("fame_terms", "fame_v1", "fame"),
            ("lng_terms", "lng_v1", "lng"),
        ] {
            let terms = normalized
                .filter(|terms| terms["side"] == self.side && !self.private[name].is_null())
                .unwrap_or(&self.private[name]);
            let public = public_response_terms(profile, terms).map_err(|_| ApiError::internal())?;
            let schema = format!("{schema}_{}", self.side.to_lowercase());
            out.insert(name.into(), ordered_public_terms(&public, &schema)?);
        }
        for name in [
            "side",
            "product_id",
            "product_name",
            "market_product",
            "fuel_type",
            "fuel_grade",
            "delivery_point_id",
            "delivery_point_name",
            "region",
            "port_id",
            "quantity_mt",
            "remaining_quantity_mt",
            "price_per_mt_usd",
            "availability_window",
            "certifications",
            "is_verdaxis_verified",
            "tier_label",
            "status",
            "expires_at",
            "created_at",
        ] {
            out.insert(
                name.into(),
                self.labels.get(name).unwrap_or(&self.private[name]).clone(),
            );
        }
        let premium = benchmark.map(|value| {
            let mut premium = self.price - value.price;
            premium.rescale(2);
            premium.to_string()
        });
        let real = self.provenance == "REAL";
        for (name, value) in [
            (
                "benchmark_price_per_mt_usd",
                json!(benchmark.map(|value| value.price.to_string())),
            ),
            ("premium_discount_per_mt_usd", json!(premium)),
            (
                "benchmark_source",
                json!(benchmark.map(|_| format!("live_slice_{}_vwap", self.side.to_lowercase()))),
            ),
            ("is_crossed", json!(crossed)),
            ("is_demo_listing", json!(!real)),
            (
                "source_kind",
                json!(if real { "LIVE_ORDER" } else { "DEMO_SEED" }),
            ),
            ("scope", json!("DELIVERY_POINT")),
            (
                "demo_status",
                json!(if real { "REAL_ONLY" } else { "DEMO_ONLY" }),
            ),
            ("unknown_count", json!(0)),
        ] {
            out.insert(name.into(), value);
        }
        Ok(Json::Object(out))
    }
}

/// The shared helper validates and redacts. The response model then writes
/// schema order (including nested quality results), not JSONB storage order.
fn ordered_public_terms(value: &Json, schema: &str) -> Result<Json, ApiError> {
    if value.is_null() {
        return Ok(Json::Null);
    }
    let data: Json = serde_json::from_str(include_str!("orderbook_query.json"))
        .map_err(|_| ApiError::internal())?;
    let fields = data["public_terms_fields"][schema]
        .as_array()
        .ok_or_else(ApiError::internal)?;
    let mut result = serde_json::Map::new();
    for name in fields {
        let name = name.as_str().ok_or_else(ApiError::internal)?;
        let child = match name {
            "quality_evidence" | "sustainability_evidence" => {
                ordered_public_terms(&value[name], name)?
            }
            "results" => Json::Array(
                value[name]
                    .as_array()
                    .ok_or_else(ApiError::internal)?
                    .iter()
                    .map(|item| ordered_public_terms(item, "quality_result"))
                    .collect::<Result<_, _>>()?,
            ),
            _ => value[name].clone(),
        };
        result.insert(name.into(), child);
    }
    Ok(Json::Object(result))
}
