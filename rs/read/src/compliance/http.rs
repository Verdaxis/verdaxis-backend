//! Ordered request preparation for the authenticated compliance reads.
//!
//! A protected route keeps the accepted Phase 2 order: bound the raw body and
//! parse one JSON document, verify the bearer and lock the current-account row,
//! check the current account and the route's support policy, and only then
//! apply the route's typed rules. This module holds the parts of that order
//! that belong to the four authenticated compliance routes, so a semantic
//! refusal cannot come before authentication and a Market Support request
//! cannot reach a domain statement.
//!
//! The transaction stays in `auth::transaction`. A route bounds and
//! parses the body with [`parse_post_body`] (or [`bounded_body`]), verifies the
//! access token, and calls [`PreparedPost::admit`] inside the admitted closure
//! of `run_authenticated`, after the helper has checked the current account.
//! A body that never passed [`parse_post_body`] cannot be validated, so size
//! and syntax are the only checks that may precede authentication.

use axum::{
    Json,
    body::{Body, Bytes},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};

use crate::{
    ApiError, ApiErrorKind,
    auth::{
        AuthenticatedRequest,
        limits::SharedLimits,
        request::{RequestAdmission, validate_and_admit},
    },
};

/// The byte bound of both compliance POST bodies. It is applied while the body
/// is read, before JSON is allocated.
pub const POST_BODY_MAX_BYTES: usize = 65_536;

pub use crate::compliance::validation::{
    MAX_FUEL_KEY_BYTES as FUEL_MIX_KEY_MAX_BYTES, MAX_MIX_ENTRIES as FUEL_MIX_MAX_ENTRIES,
};

/// The scenario route template.
pub const SCENARIO_PATH: &str = "/api/compliance/scenario";

/// The pricing-overlay route template.
pub const OVERLAY_PATH: &str = "/api/compliance/pricing-overlay";

/// A body refusal that happens before any typed rule can run.
///
/// Both variants answer with the Phase 5 input envelope (`INVALID_INPUT`, HTTP
/// 422); the two are distinct so tests and logs can name which bound refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplianceBodyError {
    /// The raw body is larger than [`POST_BODY_MAX_BYTES`].
    TooLarge,
    /// The raw body is not exactly one JSON document.
    InvalidSyntax,
}

impl ComplianceBodyError {
    pub fn status(self) -> StatusCode {
        StatusCode::UNPROCESSABLE_ENTITY
    }

    pub fn code(self) -> &'static str {
        "INVALID_INPUT"
    }

    pub fn message(self) -> String {
        match self {
            Self::TooLarge => {
                format!("Request body is larger than {POST_BODY_MAX_BYTES} bytes.")
            }
            Self::InvalidSyntax => "Request body is not one JSON document.".into(),
        }
    }
}

impl IntoResponse for ComplianceBodyError {
    fn into_response(self) -> Response {
        let detail = json!({"code": self.code(), "message": self.message()});
        (self.status(), Json(json!({"detail": detail}))).into_response()
    }
}

/// The admission inputs of one authenticated compliance POST.
///
/// The fields are private so a route cannot validate a body under a different
/// path, clock or support policy than the one it authenticated.
pub struct CompliancePost {
    admission: RequestAdmission,
    support_present: bool,
}

impl CompliancePost {
    pub fn new(
        path: impl Into<String>,
        client: String,
        now_seconds: u64,
        limits: SharedLimits,
        support_present: bool,
    ) -> Self {
        Self {
            admission: RequestAdmission {
                client,
                path: path.into(),
                now_seconds,
                limits,
            },
            support_present,
        }
    }

    /// The route path this admission was prepared for.
    pub fn path(&self) -> &str {
        &self.admission.path
    }

    /// Whether the request carried a Market Support context.
    pub fn support_present(&self) -> bool {
        self.support_present
    }
}

/// A body that passed the byte bound and the JSON syntax check.
pub struct PreparedPost {
    body: Value,
}

impl PreparedPost {
    /// Finish the order inside the admitted closure of `run_authenticated`:
    /// support-route authorization, then the typed rules, then endpoint quota.
    ///
    /// The closure has already validated the current account, so a refusal here
    /// is a route policy result, not an authentication result.
    pub fn admit<T>(
        self,
        post: &CompliancePost,
        validate: impl FnOnce(Value) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        if post.support_present {
            return Err(support_context_denied());
        }
        validate_and_admit(self.body, &post.admission, validate)
    }
}

/// Check the POST byte bound, then parse exactly one JSON document.
///
/// This is the only step that may run before authentication. It never echoes
/// the submitted text.
pub fn parse_post_body(raw: &[u8]) -> Result<PreparedPost, ComplianceBodyError> {
    if raw.len() > POST_BODY_MAX_BYTES {
        return Err(ComplianceBodyError::TooLarge);
    }
    let body = serde_json::from_slice(raw).map_err(|_| ComplianceBodyError::InvalidSyntax)?;
    Ok(PreparedPost { body })
}

/// Read a request body under the compliance POST bound.
///
/// The extra byte tells an oversized body apart from one that exactly fills the
/// bound, so the rest of an oversized body is never buffered.
pub async fn bounded_body(body: Body) -> Result<Bytes, ComplianceBodyError> {
    let bytes = axum::body::to_bytes(body, POST_BODY_MAX_BYTES + 1)
        .await
        .map_err(|_| ComplianceBodyError::TooLarge)?;
    if bytes.len() > POST_BODY_MAX_BYTES {
        return Err(ComplianceBodyError::TooLarge);
    }
    Ok(bytes)
}

/// The current-account request of a compliance route.
///
/// Every compliance route takes the account row in update mode. The shared lock
/// is reserved for the audited read templates in Phase 2, and both compliance
/// POSTs are protected reads whose transaction the helper still commits.
pub fn authenticated_request(
    method: &str,
    route_template: &'static str,
    support_present: bool,
) -> AuthenticatedRequest {
    AuthenticatedRequest::new(method, route_template, support_present)
}

/// Check the scenario fuel-mix container before any known-fuel or coefficient
/// rule, and return the entries for the numeric validator.
///
/// The container bound is separate from the fuel table: a body with many
/// entries is refused by the container rule without naming a fuel, and an
/// accepted container is still subject to the known-fuel rule.
pub fn fuel_mix_container(body: &Value) -> Result<&Map<String, Value>, ApiError> {
    let mix = body
        .get("fuel_mix")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_input("fuel_mix must be an object of fuel names to fractions."))?;
    if mix.len() > FUEL_MIX_MAX_ENTRIES {
        return Err(invalid_input(format!(
            "fuel_mix must have at most {FUEL_MIX_MAX_ENTRIES} entries."
        )));
    }
    if mix.keys().any(|key| key.len() > FUEL_MIX_KEY_MAX_BYTES) {
        return Err(invalid_input(format!(
            "Each fuel_mix key must be at most {FUEL_MIX_KEY_MAX_BYTES} UTF-8 bytes."
        )));
    }
    Ok(mix)
}

fn support_context_denied() -> ApiError {
    ApiError {
        status: StatusCode::FORBIDDEN,
        detail: json!({
            "code": "MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
            "message": "This operation is not available in Market Support mode",
        }),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn invalid_input(message: impl Into<String>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!({"code": "INVALID_INPUT", "message": message.into()}),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}
