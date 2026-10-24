//! A03: the write-once onboarding survey.
//!
//! The first accepted answer wins and every later submit is a no-op with the
//! same 204, exactly as Python refuses a second write by returning early. The
//! write is guarded by the current account under a row lock, so two concurrent
//! first submits serialize on the row and exactly one value lands.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::error::{authentication, refusal},
    runtime::http::client_ip,
};

const SURVEY_PATH: &str = "/api/auth/survey";

/// `SurveySubmission.referral_source` bound, in Unicode code points.
pub const MAX_REFERRAL_SOURCE_CHARS: usize = 500;

/// The pinned body limit of the frozen inventory.
const BODY_LIMIT_BYTES: usize = 131_072;

/// The closed use-case domain, sorted for the refusal message.
const USE_CASES: [&str; 3] = ["buyer", "financier_other", "supplier"];

pub struct SurveyAuthority {
    verification: VerificationSettings,
    limits: SharedLimits,
}

impl SurveyAuthority {
    pub fn new(verification: VerificationSettings, limits: SharedLimits) -> Self {
        Self {
            verification,
            limits,
        }
    }
}

/// A validated submission. The wire body may carry an `email`; it is not an
/// addressing field, and the authenticated account is always the target.
#[derive(Debug, PartialEq, Eq)]
pub struct SurveySubmission {
    pub use_case: String,
    pub referral_source: Option<String>,
}

pub fn validate_submission(body: Value) -> Result<SurveySubmission, ApiError> {
    let unprocessable = |message: &'static str| {
        refusal(StatusCode::UNPROCESSABLE_ENTITY, "SURVEY_VALIDATION_FAILED", message)
    };
    let Some(object) = body.as_object() else {
        return Err(unprocessable("A survey object is required."));
    };
    let mut use_case: Option<&str> = None;
    let mut referral: Option<&str> = None;
    for (field, value) in object {
        match field.as_str() {
            "use_case" => {
                use_case = value.as_str();
                if use_case.is_none() {
                    return Err(unprocessable("The use case must be a string."));
                }
            }
            "referral_source" => {
                if !value.is_null() {
                    referral = value.as_str();
                    if referral.is_none() {
                        return Err(unprocessable("The referral source must be a string."));
                    }
                }
            }
            // Python's `SurveySubmission` sets `extra="forbid"`; `email` is the
            // one legacy key the shipped client still sends and Pydantic-era
            // logs accepted, so it is ignored rather than refused.
            "email" => {}
            _ => return Err(unprocessable("The field is not part of the survey.")),
        }
    }
    let Some(use_case) = use_case else {
        return Err(unprocessable("The use case is required."));
    };
    if !USE_CASES.contains(&use_case) {
        // The corrected typed refusal; message wording is free.
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "SURVEY_USE_CASE_INVALID",
            "use_case must be one of the declared values.",
        ));
    }
    if let Some(referral) = referral
        && referral.chars().count() > MAX_REFERRAL_SOURCE_CHARS
    {
        return Err(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "SURVEY_REFERRAL_SOURCE_TOO_LONG",
            "The referral source exceeds its bound.",
        ));
    }
    Ok(SurveySubmission {
        use_case: use_case.to_owned(),
        referral_source: referral.map(str::to_owned),
    })
}

/// Store the first answer on the caller's transaction. A row that already has
/// an answer is never rewritten; the function reports whether this call wrote.
pub async fn store_survey(
    connection: &mut sqlx::PgConnection,
    user: Uuid,
    submission: &SurveySubmission,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE users SET onboarding_use_case=$2,onboarding_referral_source=$3 \
         WHERE id=$1 AND onboarding_use_case IS NULL",
    )
    .bind(user)
    .bind(&submission.use_case)
    .bind(&submission.referral_source)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

pub fn routes(authority: SurveyAuthority) -> Router<AppState> {
    Router::new()
        .route(SURVEY_PATH, post(submit))
        .layer(Extension(Arc::new(authority)))
}

async fn submit(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<SurveyAuthority>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    // The pinned body limit is refused before the body is parsed or counted.
    let raw = match axum::body::to_bytes(body, BODY_LIMIT_BYTES).await {
        Ok(raw) => raw,
        Err(_) => {
            return refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BODY_TOO_LARGE",
                "The survey body is too large.",
            )
            .into_response();
        }
    };
    let payload = if raw.is_empty() {
        Value::Null
    } else {
        match crate::auth::request::parse_json_body(&raw) {
            Ok(payload) => payload,
            Err(error) => return error.into_response(),
        }
    };
    // Authentication precedes typed validation, as FastAPI resolves the
    // security dependency before it validates the body model.
    let now = Utc::now();
    let access = match verify_authorization(
        parts.headers.get("authorization").and_then(|value| value.to_str().ok()),
        &authority.verification,
        now,
    ) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let submission = match validate_submission(payload) {
        Ok(submission) => submission,
        Err(error) => return error.into_response(),
    };
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        parts
            .headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let limits = authority.limits.clone();
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("POST", SURVEY_PATH, false),
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor().clone();
                let user = actor.id.to_string();
                let organization = actor.organization_id.map(|id| id.to_string());
                limits.check_business(
                    "POST",
                    SURVEY_PATH,
                    BusinessLimitKeys {
                        client: &client,
                        user: Some(&user),
                        organization: organization.as_deref(),
                        token: None,
                    },
                    now.timestamp().max(0) as u64,
                )?;
                store_survey(transaction.connection(), actor.id, &submission)
                    .await
                    .map_err(|_| ApiError::database())?;
                Ok(())
            })
        },
    )
    .await;
    match result {
        // Repeats answer 204 without a write; that is the write-once contract.
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthenticatedError::Authentication(error)) => authentication(error).into_response(),
        // The write owner maps SQLSTATEs itself; a residual failure is opaque.
        Err(AuthenticatedError::Database(_)) => ApiError::database().into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}
