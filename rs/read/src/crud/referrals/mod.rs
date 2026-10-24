//! Referral routes: R02 statistics, R03 leaderboard and R05 public resolution.
//!
//! Source: `app/routers/referrals.py` and `app/schemas/referral.py`.
//!
//! Every authenticated referral path takes the exclusive current-user row lock
//! Python takes: `_resolve_authenticated_user` uses `FOR UPDATE` for these
//! paths, so two concurrent R01 calls for one account serialize and return the
//! same committed code.
//!
//! Authentication settings and the frontend URL are injected through
//! [`ReferralSettings`] rather than read from [`crate::Config`]. This module
//! therefore compiles, runs and is tested without a new runtime setting; Task
//! 23 owns the projection of `JWT_SECRET`, `JWT_SECRET_PREVIOUS`, `JWT_ISSUER`,
//! `JWT_AUDIENCE` and `FRONTEND_URL`.

mod code;
mod email;
mod email_tables;
pub mod invite;
mod reads;

pub use code::{COLLISION_PROBES, Candidates, referral_code_response};

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::auth::request::RequestError;
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, VerificationSettings, VerifiedAccess,
    run_authenticated, verify_authorization,
};
use crate::{ApiError, AppState};

pub use reads::{
    LeaderboardUser, MyReferralRow, ReferrerCount, ResolveRow, code_matches_python_format,
    display_name, leaderboard_entries, referral_stats, resolve_response,
};

/// Deployment values the referral routes need.
///
/// `frontend_url` is the raw `FRONTEND_URL` value: R01 appends `/invite/<code>`
/// by plain concatenation, so a trailing slash in the configured value is kept.
#[derive(Clone)]
pub struct ReferralSettings {
    pub verification: Arc<VerificationSettings>,
    pub frontend_url: Arc<str>,
    pub mail: invite::MailClient,
    pub email_from: Arc<str>,
    pub limits: crate::auth::limits::SharedLimits,
}

impl ReferralSettings {
    pub fn new(verification: VerificationSettings, frontend_url: impl Into<Arc<str>>) -> Self {
        Self {
            verification: Arc::new(verification),
            frontend_url: frontend_url.into(),
            mail: invite::MailClient::disabled(),
            email_from: "".into(),
            limits: crate::auth::limits::SharedLimits::default(),
        }
    }
}

/// Build the referral routes for the composition root to merge.
///
/// The serving router must also mount R05's path in its public GET inventory
/// and merge this router in `public_routes`, both of which stay with the
/// integration owner.
pub fn routes(settings: ReferralSettings) -> Router<AppState> {
    Router::new()
        .route("/api/referrals/my-code", axum::routing::post(my_code))
        .route("/api/referrals/invite", axum::routing::post(send_invite))
        .route(
            "/api/referrals/my-referrals",
            get(my_referrals).head(refuse_head),
        )
        .route(
            "/api/referrals/leaderboard",
            get(leaderboard).head(refuse_head),
        )
        .route(
            "/api/referrals/resolve/{code}",
            get(resolve_referral_code).head(refuse_head),
        )
        .layer(Extension(settings))
}

async fn refuse_head() -> Response {
    (
        axum::http::StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET")],
    )
        .into_response()
}

/// One referral-route failure, with Python's two response shapes kept apart.
enum ReferralError {
    /// A typed refusal: authentication, validation or a handled route error.
    Handled(RequestError),
    /// Python's outer `ServerErrorMiddleware` response for an unhandled server
    /// failure, which bypasses CORS and request-id decoration.
    Unhandled,
}

impl From<RequestError> for ReferralError {
    fn from(error: RequestError) -> Self {
        Self::Handled(error)
    }
}

impl From<ApiError> for ReferralError {
    fn from(error: ApiError) -> Self {
        Self::Handled(RequestError::Validation(error))
    }
}

impl From<AuthenticatedError> for ReferralError {
    fn from(error: AuthenticatedError) -> Self {
        match error {
            AuthenticatedError::Authentication(error) => {
                Self::Handled(RequestError::Authentication(error))
            }
            AuthenticatedError::Database(error) => Self::Handled(RequestError::Validation(error)),
            // A commit acknowledgement was lost: never reply success and never
            // replay the unit, exactly like Python's unhandled 500.
            AuthenticatedError::AmbiguousCommit => Self::Unhandled,
        }
    }
}

impl IntoResponse for ReferralError {
    fn into_response(self) -> Response {
        match self {
            Self::Handled(error) => error.into_response(),
            Self::Unhandled => crate::runtime::http::unhandled_server_error(),
        }
    }
}

/// Extract and verify the bearer token, exactly once per authenticated request.
fn admitted(
    headers: &HeaderMap,
    settings: &ReferralSettings,
) -> Result<VerifiedAccess, RequestError> {
    verify_authorization(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        &settings.verification,
        chrono::Utc::now(),
    )
    .map_err(RequestError::Authentication)
}

/// R01: allocation commits before a successful response is returned.
async fn my_code(
    State(state): State<AppState>,
    Extension(settings): Extension<ReferralSettings>,
    headers: HeaderMap,
) -> Result<Response, ReferralError> {
    let access = admitted(&headers, &settings)?;
    let request = AuthenticatedRequest::new(
        "POST",
        "/api/referrals/my-code",
        crate::auth::scope::support_present(&headers),
    );
    let body = run_authenticated(&state, access, request, move |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            code::allocate_my_code(
                transaction.connection(),
                &actor,
                &settings.frontend_url,
                &mut Candidates::for_actor(actor.id),
            )
            .await
        })
    })
    .await?;
    Ok(axum::Json(body).into_response())
}

/// R04: code allocation commits before provider delivery. Provider failure must
/// not roll that code back, and an existing recipient never causes allocation.
async fn send_invite(
    State(state): State<AppState>,
    Extension(settings): Extension<ReferralSettings>,
    headers: HeaderMap,
    raw: axum::body::Bytes,
) -> Result<Response, ReferralError> {
    use crate::auth::request::{RequestAdmission, parse_json_body, validate_and_admit};
    let body = parse_json_body(&raw)?;
    let access = admitted(&headers, &settings)?;
    let request = AuthenticatedRequest::new(
        "POST",
        "/api/referrals/invite",
        crate::auth::scope::support_present(&headers),
    );
    let admission = RequestAdmission {
        client: crate::runtime::http::client_ip(
            None,
            headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        ),
        path: "/api/referrals/invite".into(),
        now_seconds: chrono::Utc::now().timestamp() as u64,
        limits: settings.limits.clone(),
    };
    let frontend = settings.frontend_url.clone();
    let sender = settings.email_from.clone();
    let prepared = run_authenticated(&state, access, request, move |transaction| {
        Box::pin(async move {
            let recipient = validate_and_admit(body, &admission, transaction.actor().id, |body| {
                invite::normalize_email(&body)
            })?;
            let actor = transaction.actor().clone();
            let (email, first, last): (String, Option<String>, Option<String>) =
                sqlx::query_as("SELECT email,first_name,last_name FROM users WHERE id=$1")
                    .bind(actor.id)
                    .fetch_one(&mut *transaction.connection())
                    .await
                    .map_err(|_| ApiError::database())?;
            if !actor.email_verified {
                return Err(invite_refusal(
                    403,
                    "Email must be verified to send invites",
                ));
            }
            if email == recipient {
                return Err(invite_refusal(400, "Cannot invite yourself"));
            }
            let existing: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email=$1)")
                    .bind(&recipient)
                    .fetch_one(&mut *transaction.connection())
                    .await
                    .map_err(|_| ApiError::database())?;
            if existing {
                return Ok((recipient, None));
            }
            let response = code::allocate_my_code(
                transaction.connection(),
                &actor,
                &frontend,
                &mut Candidates::for_actor(actor.id),
            )
            .await?;
            let code = response["referral_code"].as_str().expect("code response");
            let payload = invite::render_invite(
                &sender,
                &recipient,
                &display_name(first.as_deref(), last.as_deref()),
                code,
                &frontend,
            );
            Ok((recipient, Some(payload)))
        })
    })
    .await?;
    if let Some(payload) = prepared.1
        && !settings.mail.send(&payload).await
    {
        return Err(invite_refusal(502, "Failed to send invite email").into());
    }
    Ok(
        axum::Json(serde_json::json!({"message":"Invitation processed","email":prepared.0}))
            .into_response(),
    )
}

fn invite_refusal(status: u16, message: &str) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::from_u16(status).expect("static status"),
        detail: serde_json::json!(message),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}

/// R02: GET `/api/referrals/my-referrals`.
async fn my_referrals(
    State(state): State<AppState>,
    Extension(settings): Extension<ReferralSettings>,
    headers: HeaderMap,
) -> Result<Response, ReferralError> {
    let access = admitted(&headers, &settings)?;
    let request = AuthenticatedRequest::new(
        "GET",
        "/api/referrals/my-referrals",
        crate::auth::scope::support_present(&headers),
    );
    let body = run_authenticated(&state, access, request, |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().clone();
            reads::select_my_referrals(transaction.connection(), actor.id).await
        })
    })
    .await?;
    Ok(axum::Json(body).into_response())
}

/// R03: GET `/api/referrals/leaderboard`.
async fn leaderboard(
    State(state): State<AppState>,
    Extension(settings): Extension<ReferralSettings>,
    headers: HeaderMap,
) -> Result<Response, ReferralError> {
    let access = admitted(&headers, &settings)?;
    let request = AuthenticatedRequest::new(
        "GET",
        "/api/referrals/leaderboard",
        crate::auth::scope::support_present(&headers),
    );
    let body = run_authenticated(&state, access, request, |transaction| {
        Box::pin(async move { reads::select_leaderboard(transaction.connection()).await })
    })
    .await?;
    Ok(axum::Json(body).into_response())
}

/// R05: GET `/api/referrals/resolve/{code}`, public.
///
/// No token is read or required here; an invalid bearer changes nothing.
async fn resolve_referral_code(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Response, ReferralError> {
    let body = crate::run_admitted(&state, |connection| {
        Box::pin(async move { reads::select_resolve(connection, &code).await })
    })
    .await?;
    Ok(axum::Json(body).into_response())
}
