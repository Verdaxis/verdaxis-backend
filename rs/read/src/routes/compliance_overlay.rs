//! Pricing-overlay route: one authenticated batch read of the marketplace ASK map.
//!
//! The client contract is Python's `POST /api/compliance/pricing-overlay`
//! (`app/routers/compliance_api.py:198-263`): every requested id is a key in
//! `overlays`, and only rows visible in the public marketplace book carry a
//! value. Bid, hidden, expired, off-policy and unpriced rows answer `null`
//! indistinguishably, so the map is not an existence oracle.
//!
//! The HTTP order is the accepted Phase 2/5 one: bound the raw body and parse
//! one JSON document first, verify the bearer and lock the current account,
//! refuse a Market Support context, and only then apply the typed rules. The
//! one domain statement supplies the ASK map and the exact scoped fleet count
//! together, so both come from one snapshot. The transaction, its commit and
//! its cancellation owner belong to `auth::transaction`; this route adds no
//! business write, ledger row or outbox event.
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;

use crate::auth::{
    AuthenticatedError, CurrentActor, VerificationSettings, limits::SharedLimits, request::RequestError,
    run_authenticated, verify_authorization,
};
use crate::compliance::http::{
    CompliancePost, OVERLAY_PATH, authenticated_request, bounded_body, parse_post_body,
};
use crate::compliance::pricing::{
    AlcoholFamily, LifecycleOverlayCore, ListingOverlay, OverlayAssumptions, PricingError,
};
use crate::market::visibility::{
    PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql,
};
use crate::routes::market_data_error;
use crate::{ApiError, ApiErrorKind, AppState, Config, MarketPolicy};
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

/// Bearer verification and route quota for the pricing overlay.
///
/// The integration owner supplies one instance through the serving router, as
/// the other authenticated Phase 2/5 groups do. Nothing here loads or writes an
/// account; the transaction helper owns the current-account lock.
#[derive(Clone)]
pub struct OverlayAuthority {
    settings: Arc<VerificationSettings>,
    limits: SharedLimits,
}

impl OverlayAuthority {
    pub fn new(settings: VerificationSettings, limits: SharedLimits) -> Self {
        Self {
            settings: Arc::new(settings),
            limits,
        }
    }

    pub fn settings(&self) -> &VerificationSettings {
        &self.settings
    }

    pub fn limits(&self) -> SharedLimits {
        self.limits.clone()
    }
}

/// The pricing-overlay route table. The integration owner merges this into the
/// serving router; this module mounts nothing on its own.
pub fn routes(authority: OverlayAuthority) -> Router<AppState> {
    Router::new()
        .route(OVERLAY_PATH, post(overlay))
        .layer(Extension(Arc::new(authority)))
}

async fn overlay(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<OverlayAuthority>>,
    peer: Option<Extension<ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // Size and syntax may fail before authentication; nothing else may.
    let raw = match bounded_body(body).await {
        Ok(raw) => raw,
        Err(error) => return error.into_response(),
    };
    let prepared = match parse_post_body(&raw) {
        Ok(prepared) => prepared,
        Err(error) => return error.into_response(),
    };
    let access = match verify_authorization(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        authority.settings(),
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(error) => return RequestError::Authentication(error).into_response(),
    };
    let support_present = crate::auth::scope::support_present(&headers);
    let client = crate::runtime::http::client_ip(
        peer.map(|Extension(ConnectInfo(address))| address.ip()),
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let admission = CompliancePost::new(
        OVERLAY_PATH,
        client,
        Utc::now().timestamp().max(0) as u64,
        authority.limits(),
        support_present,
    );
    let policy = state.policy.clone();
    let config = state.config.clone();
    let clock = VisibilityClock::at(Utc::now());
    let result = run_authenticated(
        &state,
        access,
        authenticated_request("POST", OVERLAY_PATH, support_present),
        move |transaction| {
            Box::pin(async move {
                // Support refusal, then the typed rules, then the domain
                // statement: the closure has already validated the account.
                let request = prepared.admit(&admission, |value| {
                    OverlayRequest::validate(value).map_err(|_| invalid_input())
                })?;
                let actor = transaction.actor().clone();
                select_overlay(
                    transaction.connection(),
                    &policy,
                    &clock,
                    &config,
                    &actor,
                    &request,
                )
                .await
                .map_err(overlay_failure)
            })
        },
    )
    .await;
    match result {
        Ok(response) => Json(response).into_response(),
        Err(AuthenticatedError::Authentication(error)) => {
            RequestError::Authentication(error).into_response()
        }
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

/// The input refusal keeps the Phase 5 envelope and never echoes the body.
fn invalid_input() -> ApiError {
    market_data_error::invalid_input(
        "order_ids must contain at most 100 UUIDs; year must be from 2025 to 9999.",
    )
}

fn overlay_failure(error: OverlayError) -> ApiError {
    match error {
        OverlayError::InvalidInput => invalid_input(),
        OverlayError::Database(source) => {
            market_data_error::database_failure(OVERLAY_PATH, &source)
        }
        OverlayError::Projection | OverlayError::Pricing(_) => read_failed(),
    }
}

/// A refusal that is not the client's input and not a retryable database
/// condition. Wording is free; no error text, SQL or path is included.
fn read_failed() -> ApiError {
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: serde_json::json!({
            "code": "READ_FAILED",
            "message": "Compliance pricing data could not be read.",
        }),
        retry_after: None,
        kind: ApiErrorKind::Unexpected,
    }
}
