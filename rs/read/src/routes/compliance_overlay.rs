//! Pricing-overlay domain component. HTTP composition supplies the locked actor.
use crate::auth::CurrentActor;
use crate::compliance::pricing::{
    AlcoholFamily, LifecycleOverlayCore, ListingOverlay, OverlayAssumptions, PricingError,
};
use crate::market::visibility::{
    PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
};
use crate::{Config, MarketPolicy};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const MAX_ORDER_IDS: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayInput {
    order_ids: Vec<Uuid>,
    #[serde(default = "default_year")]
    year: i32,
}

fn default_year() -> i32 {
    2026
}

/// Construct only after current-account and support authorization. Body syntax
/// and the streaming byte cap belong to the authenticated HTTP adapter.
#[derive(Debug)]
pub struct OverlayRequest {
    order_ids: Vec<Uuid>,
    year: i32,
}

impl OverlayRequest {
    pub fn validate(value: serde_json::Value) -> Result<Self, OverlayError> {
        let input: OverlayInput =
            serde_json::from_value(value).map_err(|_| OverlayError::InvalidInput)?;
        if input.order_ids.len() > MAX_ORDER_IDS || !(2025..=9999).contains(&input.year) {
            return Err(OverlayError::InvalidInput);
        }
        let order_ids = input.order_ids.into_iter().collect::<BTreeSet<_>>();
        Ok(Self {
            order_ids: order_ids.into_iter().collect(),
            year: input.year,
        })
    }

    pub fn order_ids(&self) -> &[Uuid] {
        &self.order_ids
    }

    pub fn year(&self) -> i32 {
        self.year
    }
}

#[derive(Debug)]
pub enum OverlayError {
    InvalidInput,
    Database(sqlx::Error),
    Projection,
    Pricing(PricingError),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => formatter.write_str("Invalid overlay input"),
            Self::Database(error) => {
                let _ = error;
                formatter.write_str("Overlay data is unavailable")
            }
            Self::Projection => formatter.write_str("Overlay data is unavailable"),
            Self::Pricing(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for OverlayError {}

#[derive(Serialize)]
pub struct OverlayResponse {
    pub overlays: BTreeMap<Uuid, Option<ListingOverlay>>,
    pub assumptions: OverlayAssumptions,
}

pub struct OverlayRow {
    pub requested_id: Option<Uuid>,
    pub fuel_type: Option<String>,
    pub ci: Option<Decimal>,
    pub lcv: Option<Decimal>,
    pub fleet_vessel_count: i64,
}

/// Catalog fuel types name physical families. Product labels/codes are not
/// parsed, and unrelated families do not acquire an alcohol energy default.
pub fn alcohol_family(fuel_type: &str) -> Option<AlcoholFamily> {
    match fuel_type {
        "Methanol" => Some(AlcoholFamily::Methanol),
        "Ethanol" => Some(AlcoholFamily::Ethanol),
        _ => None,
    }
}

pub fn project(
    request: &OverlayRequest,
    rows: Vec<OverlayRow>,
) -> Result<OverlayResponse, OverlayError> {
    let count = rows
        .first()
        .ok_or(OverlayError::Projection)?
        .fleet_vessel_count;
    let fleet_vessel_count = u64::try_from(count).map_err(|_| OverlayError::Projection)?;
    let mut overlays: BTreeMap<_, _> = request.order_ids.iter().map(|id| (*id, None)).collect();
    for row in rows {
        if row.fleet_vessel_count != count {
            return Err(OverlayError::Projection);
        }
        let Some(id) = row.requested_id else {
            if !overlays.is_empty() {
                return Err(OverlayError::Projection);
            }
            continue;
        };
        let target = overlays.get_mut(&id).ok_or(OverlayError::Projection)?;
        // An unmatched ASK projection has no catalog family or listing values.
        if let Some(fuel_type) = row.fuel_type {
            *target = LifecycleOverlayCore::compute(alcohol_family(&fuel_type), row.ci, row.lcv)
                .map_err(OverlayError::Pricing)?;
        }
    }
    Ok(OverlayResponse {
        overlays,
        assumptions: LifecycleOverlayCore::assumptions(request.year, fleet_vessel_count, None)
            .map_err(OverlayError::Pricing)?,
    })
}

/// One domain statement supplies both the ASK map and exact scoped fleet count,
/// even for an empty request. Call inside the authenticated transaction, after
/// its actor lock and scoped timeouts; this function does not own that boundary.
pub async fn select_overlay(
    connection: &mut sqlx::PgConnection,
    policy: &MarketPolicy,
    clock: &VisibilityClock,
    config: &Config,
    actor: &CurrentActor,
    request: &OverlayRequest,
) -> Result<OverlayResponse, OverlayError> {
    let sql = order_query_sql(include_str!("../../sql/compliance_overlay.sql"));
    let filters = PublicOrderFilters {
        side: Some("ASK"),
        ..Default::default()
    };
    let rows = bind_order_query(&sql, policy, clock, config, &filters)
        .map_err(OverlayError::Database)?
        .bind(request.order_ids())
        .bind(actor.organization_id)
        .bind(actor.role.as_deref() == Some("ADMIN"))
        .fetch_all(connection)
        .await
        .map_err(OverlayError::Database)?;
    let rows = rows
        .into_iter()
        .map(|row| {
            Ok(OverlayRow {
                requested_id: row.try_get("requested_id")?,
                fuel_type: row.try_get("fuel_type")?,
                ci: row.try_get("ci")?,
                lcv: row.try_get("lcv")?,
                fleet_vessel_count: row.try_get("fleet_vessel_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(OverlayError::Database)?;
    project(request, rows)
}
