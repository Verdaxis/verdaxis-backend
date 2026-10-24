//! Vessel read components. Runtime composition remains with the integration owner.

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::error::{authentication, database_failure, refusal},
};

const LIST_PATH: &str = "/api/vessels";
const DETAIL_PATH: &str = "/api/vessels/{vessel_id}";

pub struct VesselAuthority {
    verification: VerificationSettings,
    limits: SharedLimits,
}

impl VesselAuthority {
    pub fn new(verification: VerificationSettings, limits: SharedLimits) -> Self {
        Self { verification, limits }
    }
}

/// A response projection contains coordinates, never driver geography values.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct VesselResponse {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub name: String,
    pub imo_number: String,
    pub vessel_type: Option<String>,
    pub flag_state: Option<String>,
    pub dwt: Option<f64>,
    pub cii_rating: Option<String>,
    pub eu_ets_status: Option<String>,
    pub fueleu_status: Option<String>,
    pub current_location: Option<String>,
    pub previous_location: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub prev_lat: Option<f64>,
    pub prev_lng: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT id,organization_id,name,imo_number,vessel_type,flag_state,\
    dwt::double precision AS dwt,cii_rating,eu_ets_status,fueleu_status,\
    NULL::text AS current_location,NULL::text AS previous_location,\
    ST_Y(current_location::geometry) AS lat,ST_X(current_location::geometry) AS lng,\
    ST_Y(previous_location::geometry) AS prev_lat,ST_X(previous_location::geometry) AS prev_lng,updated_at \
    FROM vessels WHERE ($1::boolean OR organization_id=$2::uuid)";

/// Borrow the authenticated caller's connection; no transaction or write is started.
pub async fn select_vessels(
    connection: &mut PgConnection,
    actor: &CurrentActor,
    id: Option<Uuid>,
) -> Result<Vec<VesselResponse>, ApiError> {
    let sql = format!("{SELECT} AND ($3::uuid IS NULL OR id=$3) ORDER BY id");
    sqlx::query_as::<_, VesselResponse>(sqlx::AssertSqlSafe(sql))
        .bind(actor.role.as_deref() == Some("ADMIN"))
        .bind(actor.organization_id)
        .bind(id)
        .fetch_all(connection)
        .await
        .map_err(|_| database_failure(None))
}

pub fn routes(authority: VesselAuthority) -> Router<AppState> {
    Router::new()
        .route(LIST_PATH, get(list))
        .route(DETAIL_PATH, get(detail))
        .layer(Extension(Arc::new(authority)))
}

async fn list(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<VesselAuthority>>,
    request: Request,
) -> Result<Response, ApiError> {
    read(state, authority, request, None).await
}

async fn detail(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<VesselAuthority>>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    read(state, authority, request, Some(id)).await
}

/// Both adapters use token verification, locked current-account admission, quota,
/// then resource validation/lookup in that order. Neither read requires admin.
async fn read(
    state: AppState,
    authority: Arc<VesselAuthority>,
    request: Request,
    raw_id: Option<String>,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let access = verify_authorization(
        request.headers().get("authorization").and_then(|value| value.to_str().ok()),
        &authority.verification,
        now,
    ).map_err(authentication)?;
    let template = if raw_id.is_some() { DETAIL_PATH } else { LIST_PATH };
    let authenticated = AuthenticatedRequest::new("GET", template, false);
    let rows = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            let user = actor.id.to_string();
            let organization = actor.organization_id.map(|id| id.to_string());
            // The frozen inventory declares no endpoint quota for these reads.
            authority.limits.check_business("GET", template, BusinessLimitKeys {
                client: "", user: Some(&user), organization: organization.as_deref(), token: None,
            }, now.timestamp().max(0) as u64)?;
            let id = raw_id.as_deref().map(|raw| Uuid::parse_str(raw).map_err(|_| refusal(
                StatusCode::UNPROCESSABLE_ENTITY, "INVALID_VESSEL_ID", "The vessel identifier must be a UUID.",
            ))).transpose()?;
            let rows = select_vessels(transaction.connection(), &actor, id).await?;
            if id.is_some() && rows.is_empty() {
                return Err(refusal(StatusCode::NOT_FOUND, "VESSEL_NOT_FOUND", "The vessel was not found."));
            }
            Ok(rows)
        })
    }).await.map_err(|error| match error {
        AuthenticatedError::Authentication(error) => authentication(error),
        AuthenticatedError::Database(error) => error,
        AuthenticatedError::AmbiguousCommit => refusal(StatusCode::SERVICE_UNAVAILABLE, "COMMIT_OUTCOME_UNKNOWN", "The request outcome is unknown."),
    })?;
    if template == DETAIL_PATH {
        let row = rows.into_iter().next().ok_or_else(ApiError::internal)?;
        Ok(axum::Json(row).into_response())
    } else {
        Ok(axum::Json(rows).into_response())
    }
}
