//! Fleet, vessel and scenario compliance reads: `C01`, `C02` and `C05`.
//!
//! The three routes are protected reads of the compliance score calculation
//! core. Each one keeps the accepted order of the authenticated boundary:
//! parse the request syntax under its byte bound, verify the bearer, lock and
//! validate the current account, apply the route policy and typed rules, run
//! the one domain statement, and return the pure projection. Caller
//! authentication is not execution admission, so the routes add no KYC gate
//! and no business write; the shared authenticated helper still commits its
//! read-only transaction.
//!
//! Two corrections are deliberate and covered by tests:
//!
//! * a caller without an organization has an empty fleet and cannot read an
//!   unowned vessel, so a missing organization is never treated as a match;
//! * a scenario echoes the submitted `vessel_id` exactly and names an
//!   inaccessible, missing or non-UUID vessel `Vessel {vessel_id}`, so the
//!   route is not an existence oracle across organizations.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

/// The scenario route template, re-exported for composition and tests.
pub use crate::compliance::http::SCENARIO_PATH;

use crate::auth::limits::SharedLimits;
use crate::auth::scope::support_present;
use crate::auth::{
    AuthenticatedError, CurrentActor, VerificationSettings, run_authenticated, verify_authorization,
};
use crate::compliance::http::{
    CompliancePost, authenticated_request, bounded_body, parse_post_body,
};
use crate::compliance::scoring::{ScoreInput, compliance_score};
use crate::compliance::validation::{
    FuelMix, INVALID_INPUT_CODE, ReportingYear, ValidationError, fuel_mix_from_json,
};
use crate::market::fueleu::{FIRST_REPORTING_YEAR, LAST_REPORTING_YEAR};
use crate::routes::market_data_error;
use crate::runtime::http::client_ip;
use crate::{ApiError, ApiErrorKind, AppState, Json};

/// The fleet route template.
pub const FLEET_PATH: &str = "/api/compliance/fleet";

/// Largest accepted number of vessels in one effective fleet scope.
pub const MAX_FLEET_VESSELS: u64 = 2000;

/// Largest accepted count of Unicode scalar values in a generic vessel label.
pub const MAX_LABEL_SCALARS: usize = 128;

/// Largest accepted UTF-8 length of a generic vessel label.
pub const MAX_LABEL_BYTES: usize = 512;

/// Everything these routes need that `AppState` does not carry: token
/// verification settings, the shared endpoint quota authority and the clock.
pub struct RouteContext {
    pub settings: Arc<VerificationSettings>,
    pub limits: SharedLimits,
    pub clock: Clock,
}

/// The instant the route uses for authentication and the quota window.
///
/// `Wall` is what the composition owner mounts; the tests inject `Fixed`.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum Clock {
    Wall,
    Fixed(DateTime<Utc>),
}

impl Clock {
    fn now(self) -> DateTime<Utc> {
        match self {
            Self::Wall => Utc::now(),
            Self::Fixed(instant) => instant,
        }
    }
}

/// Mount `C01`, `C02` and `C05` with an explicit clock, without mounting the
/// serving router. The integration owner composes the four authenticated
/// compliance routes.
pub fn routes_with_context(context: Arc<RouteContext>) -> Router<AppState> {
    Router::new()
        .route(FLEET_PATH, get(fleet))
        .route(
            "/api/compliance/vessels/{vessel_id}/score",
            get(vessel_score),
        )
        .route(SCENARIO_PATH, post(scenario))
        .layer(Extension(context))
}

/// Whether the actor holds the administrator role.
pub fn is_admin(actor: &CurrentActor) -> bool {
    actor.role.as_deref() == Some("ADMIN")
}

/// Whether the actor may read this vessel.
///
/// An administrator reads every vessel. Any other actor reads the vessels of
/// its own organization only: a missing organization matches nothing, not the
/// vessels that carry no organization.
pub fn vessel_accessible(actor: &CurrentActor, vessel: &VesselRow) -> bool {
    is_admin(actor)
        || actor
            .organization_id
            .is_some_and(|organization| vessel.organization_id == Some(organization))
}

/// The generic label rule: at most 128 Unicode scalar values and 512 UTF-8
/// bytes, with no control characters.
pub fn validate_label(label: &str) -> Result<(), ApiError> {
    if label.chars().count() > MAX_LABEL_SCALARS || label.len() > MAX_LABEL_BYTES {
        return Err(invalid_input(format!(
            "vessel_id must be at most {MAX_LABEL_SCALARS} characters and {MAX_LABEL_BYTES} UTF-8 bytes."
        )));
    }
    if label.chars().any(char::is_control) {
        return Err(invalid_input(
            "vessel_id must not contain control characters.".to_owned(),
        ));
    }
    Ok(())
}

/// The typed resource identity of `C02`.
///
/// A vessel path carries a UUID. Parsing it belongs after authentication, so a
/// refused identifier is never an unauthenticated semantic refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VesselPath {
    id: Uuid,
}

impl VesselPath {
    pub fn parse(raw: &str) -> Result<Self, ApiError> {
        Uuid::parse_str(raw)
            .map(|id| Self { id })
            .map_err(|_| invalid_input("vessel_id must be a UUID.".to_owned()))
    }

    pub fn id(self) -> Uuid {
        self.id
    }
}

/// One vessel row as the fleet and vessel statements select it.
#[derive(Debug, Clone)]
pub struct VesselRow {
    pub id: Uuid,
    pub name: String,
    pub cii_rating: Option<String>,
    pub organization_id: Option<Uuid>,
}

/// A validated what-if scenario (`C05`).
#[derive(Debug)]
pub struct ScenarioRequest {
    vessel_id: String,
    mix: FuelMix,
    year: ReportingYear,
}

impl ScenarioRequest {
    /// Apply the route's typed rules in their fixed order: the vessel label,
    /// then the reporting year, then the fuel mix. The mix container bound is
    /// part of the mix rule and precedes the known-fuel rule.
    pub fn validate(value: Value) -> Result<Self, ApiError> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid_input("Request body must be a JSON object.".to_owned()))?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "vessel_id" | "fuel_mix" | "year"))
        {
            return Err(invalid_input(
                "Request body has an unexpected field.".to_owned(),
            ));
        }
        let vessel_id = object
            .get("vessel_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_input("vessel_id must be a string.".to_owned()))?;
        validate_label(vessel_id)?;
        let year = match object.get("year") {
            None => ReportingYear::default(),
            Some(value) => value
                .as_i64()
                .and_then(|year| ReportingYear::new(year).ok())
                .ok_or_else(year_out_of_range)?,
        };
        let mix = object
            .get("fuel_mix")
            .ok_or_else(|| invalid_input("fuel_mix is required.".to_owned()))?;
        let mix = fuel_mix_from_json(mix).map_err(validation_failure)?;
        Ok(Self {
            vessel_id: vessel_id.to_owned(),
            mix,
            year,
        })
    }

    pub fn vessel_id(&self) -> &str {
        &self.vessel_id
    }
}

/// The vessel label and CII rating a scenario reports.
///
/// A vessel the actor cannot read keeps the generic identity: the submitted
/// text and the neutral name, with no stored metadata. A non-UUID label is
/// generic by construction because it has no lookup.
pub fn scenario_identity(
    request: &ScenarioRequest,
    vessel: Option<&VesselRow>,
    actor: &CurrentActor,
) -> (String, Option<String>) {
    match vessel.filter(|vessel| vessel_accessible(actor, vessel)) {
        Some(vessel) => (vessel.name.clone(), vessel.cii_rating.clone()),
        None => (format!("Vessel {}", request.vessel_id), None),
    }
}

/// The complete scenario assessment. Pure after the one optional vessel read.
pub fn score_scenario(
    request: &ScenarioRequest,
    vessel: Option<&VesselRow>,
    actor: &CurrentActor,
) -> Result<Json, ApiError> {
    let (vessel_name, cii_rating) = scenario_identity(request, vessel, actor);
    let score = compliance_score(ScoreInput {
        vessel_id: request.vessel_id(),
        vessel_name: &vessel_name,
        fuel_mix: &request.mix,
        cii_rating: cii_rating.as_deref(),
        total_co2_tonnes: None,
        year: request.year,
        total_energy_mj: None,
    })
    .map_err(score_failure)?;
    Ok(score.to_json())
}

/// The fleet summary of a complete accepted row set.
///
/// Counts and the average come from the same rows, so a refused or partial
/// fleet is never reported with a false total.
pub fn fleet_summary(rows: &[VesselRow]) -> Result<Json, ApiError> {
    let mut vessels = Vec::with_capacity(rows.len());
    let (mut green, mut amber, mut red) = (0_u64, 0_u64, 0_u64);
    let mut total = 0_u64;
    for row in rows {
        let (score, overall, light) = score_vessel(row)?;
        total += u64::from(overall);
        match light {
            "GREEN" => green += 1,
            "AMBER" => amber += 1,
            _ => red += 1,
        }
        vessels.push(score);
    }
    let average_score = if rows.is_empty() {
        0.0
    } else {
        average(total, rows.len())
    };
    Ok(json!({
        "total_vessels": rows.len(),
        "green_count": green,
        "amber_count": amber,
        "red_count": red,
        "average_score": average_score,
        "vessels": vessels,
    }))
}

fn average(total: u64, count: usize) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    use rust_decimal::{Decimal, RoundingStrategy};
    (Decimal::from(total) / Decimal::from(count as u64))
        .round_dp_with_strategy(1, RoundingStrategy::MidpointNearestEven)
        .to_f64()
        .unwrap_or(0.0)
}

/// One vessel assessment from its stored fields. A fleet row carries no
/// declared emissions, so its ETS contribution stays unpriced.
fn score_vessel(row: &VesselRow) -> Result<(Json, u8, &'static str), ApiError> {
    let mix = FuelMix::default_vlsfo();
    let score = compliance_score(ScoreInput {
        vessel_id: &row.id.to_string(),
        vessel_name: &row.name,
        fuel_mix: &mix,
        cii_rating: row.cii_rating.as_deref(),
        total_co2_tonnes: None,
        year: ReportingYear::default(),
        total_energy_mj: None,
    })
    .map_err(score_failure)?;
    Ok((
        score.to_json(),
        score.overall_score,
        score.traffic_light.as_str(),
    ))
}

async fn select_fleet(
    connection: &mut PgConnection,
    actor: &CurrentActor,
) -> Result<Vec<VesselRow>, ApiError> {
    let rows = sqlx::query(include_str!("../../sql/compliance_fleet.sql"))
        .bind(is_admin(actor))
        .bind(actor.organization_id)
        .fetch_all(connection)
        .await
        .map_err(|error| market_data_error::database_failure("compliance fleet read", &error))?;
    map_rows(rows)
}

async fn select_vessel(
    connection: &mut PgConnection,
    id: Uuid,
) -> Result<Option<VesselRow>, ApiError> {
    let row = sqlx::query(include_str!("../../sql/compliance_vessel.sql"))
        .bind(id)
        .fetch_optional(connection)
        .await
        .map_err(|error| market_data_error::database_failure("compliance vessel read", &error))?;
    let rows = map_rows(row.into_iter().collect())?;
    Ok(rows.into_iter().next())
}

fn map_rows(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<VesselRow>, ApiError> {
    use sqlx::Row;
    rows.into_iter()
        .map(|row| {
            Ok(VesselRow {
                id: row.try_get("id").map_err(row_failure)?,
                name: row.try_get("name").map_err(row_failure)?,
                cii_rating: row.try_get("cii_rating").map_err(row_failure)?,
                organization_id: row.try_get("organization_id").map_err(row_failure)?,
            })
        })
        .collect()
}

async fn fleet(
    State(state): State<AppState>,
    Extension(context): Extension<Arc<RouteContext>>,
    request: Request,
) -> Response {
    let now = context.clock.now();
    let (parts, _body) = request.into_parts();
    let access = match verify_authorization(bearer(&parts.headers), &context.settings, now) {
        Ok(access) => access,
        Err(error) => return auth_error_response(error),
    };
    let authenticated = authenticated_request("GET", FLEET_PATH, support_present(&parts.headers));
    let result = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            let rows = select_fleet(transaction.connection(), &actor).await?;
            market_data_error::within_limit(rows.len() as u64, MAX_FLEET_VESSELS)?;
            fleet_summary(&rows)
        })
    })
    .await;
    authenticated_response(result)
}

async fn vessel_score(
    State(state): State<AppState>,
    Extension(context): Extension<Arc<RouteContext>>,
    Path(vessel_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let now = context.clock.now();
    let access = match verify_authorization(bearer(&headers), &context.settings, now) {
        Ok(access) => access,
        Err(error) => return auth_error_response(error),
    };
    let authenticated = authenticated_request(
        "GET",
        "/api/compliance/vessels/{vessel_id}/score",
        support_present(&headers),
    );
    let result = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            let path = VesselPath::parse(&vessel_id)?;
            // The reviewed client contract keeps a missing resource at 404 and
            // another organization's resource at 403, with no vessel metadata
            // in either body. The scenario route is the read that must not
            // distinguish these cases, and it does not.
            let vessel = select_vessel(transaction.connection(), path.id())
                .await?
                .ok_or_else(not_found)?;
            if !vessel_accessible(&actor, &vessel) {
                return Err(forbidden());
            }
            let (score, _, _) = score_vessel(&vessel)?;
            Ok(score)
        })
    })
    .await;
    authenticated_response(result)
}

async fn scenario(
    State(state): State<AppState>,
    Extension(context): Extension<Arc<RouteContext>>,
    request: Request,
) -> Response {
    let now = context.clock.now();
    let (parts, body) = request.into_parts();
    let client = client_ip(
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|peer| peer.0.ip()),
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    // Only the byte bound and JSON syntax may refuse before authentication.
    let prepared = match bounded_body(body).await {
        Ok(raw) => match parse_post_body(&raw) {
            Ok(prepared) => prepared,
            Err(error) => return error.into_response(),
        },
        Err(error) => return error.into_response(),
    };
    let access = match verify_authorization(bearer(&parts.headers), &context.settings, now) {
        Ok(access) => access,
        Err(error) => return auth_error_response(error),
    };
    let support = support_present(&parts.headers);
    let post = CompliancePost::new(
        SCENARIO_PATH,
        client,
        now.timestamp().max(0) as u64,
        context.limits.clone(),
        support,
    );
    let authenticated = authenticated_request("POST", SCENARIO_PATH, support);
    let result = run_authenticated(&state, access, authenticated, move |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            let scenario: ScenarioRequest = prepared.admit(&post, ScenarioRequest::validate)?;
            let vessel = match Uuid::parse_str(scenario.vessel_id()) {
                Ok(id) => select_vessel(transaction.connection(), id).await?,
                Err(_) => None,
            };
            score_scenario(&scenario, vessel.as_ref(), &actor)
        })
    })
    .await;
    authenticated_response(result)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

fn authenticated_response(result: Result<Json, AuthenticatedError>) -> Response {
    match result {
        Ok(body) => axum::Json(body).into_response(),
        Err(AuthenticatedError::Authentication(error)) => auth_error_response(error),
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

fn auth_error_response(error: crate::auth::AuthError) -> Response {
    let mut response = (
        StatusCode::from_u16(error.status()).expect("auth status"),
        axum::Json(error.body()),
    )
        .into_response();
    if let Some(value) = error.www_authenticate() {
        response.headers_mut().insert(
            "www-authenticate",
            value.parse().expect("static auth header"),
        );
    }
    response
}

fn validation_failure(error: ValidationError) -> ApiError {
    invalid_input(error.message)
}

fn year_out_of_range() -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!({
            "code": INVALID_INPUT_CODE,
            "message": format!(
                "year must be a whole year between {FIRST_REPORTING_YEAR} and {LAST_REPORTING_YEAR}."
            ),
        }),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn not_found() -> ApiError {
    error(
        StatusCode::NOT_FOUND,
        "VESSEL_NOT_FOUND",
        "Vessel not found.",
    )
}

fn forbidden() -> ApiError {
    error(
        StatusCode::FORBIDDEN,
        "VESSEL_FORBIDDEN",
        "Not authorized to view this vessel.",
    )
}

/// A stored value the scoring core cannot represent is a read failure, never a
/// client input error.
fn score_failure(error: crate::compliance::scoring::ScoreError) -> ApiError {
    let _ = error;
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        detail: json!({"code": "READ_FAILED", "message": "Compliance score could not be calculated."}),
        retry_after: None,
        kind: ApiErrorKind::Database,
    }
}

fn row_failure(error: sqlx::Error) -> ApiError {
    market_data_error::database_failure("compliance vessel projection", &error)
}

fn invalid_input(message: String) -> ApiError {
    market_data_error::invalid_input(message)
}

fn error(status: StatusCode, code: &'static str, message: &str) -> ApiError {
    ApiError {
        status,
        detail: json!({"code": code, "message": message}),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}
