//! Administrator invitations: typed request input, the component router and
//! the three route adapters.
//!
//! Source: `app/routers/auth_simple.py:1219-1470` (A04 list, A05 create or
//! rotate, A06 public resolve). The invitation secret is a credential: only its
//! SHA-256 digest reaches the database, the raw value exists only inside the
//! returned fragment link, and no refusal distinguishes a wrong, used or
//! expired secret.
//!
//! Lifecycle and writes live in [`super::store`]; the read-only resolve
//! decision lives in [`super::resolve`].

use std::sync::Arc;

use axum::{
    Router,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{resolve, store};
use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, CurrentActor, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::{
        error::{authentication, refusal},
        transaction::run_organization_write,
    },
    crud::referrals::invite,
    market::terms::ISO_3166_1_ALPHA_2,
    runtime::http::client_ip,
};

/// A04: eligible organizations for an administrator invitation.
pub const LIST_PATH: &str = "/api/auth/admin/invitations/organizations";
/// A05: create a new invitation or rotate an unclaimed one.
pub const CREATE_PATH: &str = "/api/auth/admin/invitations";
/// A06: read-only resolution of an invitation secret.
pub const RESOLVE_PATH: &str = "/api/auth/invitations/resolve";

/// The invitation lifetime: seven days from creation or rotation.
pub const INVITATION_TTL_DAYS: i64 = 7;

/// The route body cap shared by the three auth routes.
const AUTH_BODY_LIMIT_BYTES: usize = 131_072;

/// The closed organization domain for a new invitation organization.
const ORGANIZATION_TYPES: [&str; 11] = [
    "SHIPPING_LINE",
    "SHIP_MANAGER",
    "FUEL_BUYER",
    "FUEL_SUPPLIER",
    "BUNKER_BROKER",
    "PORT_AUTHORITY",
    "FUEL_TRADER",
    "CHARTERER",
    "FINANCIER",
    "INSURER",
    "INDUSTRY_ASSOC",
];

fn invalid() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_FAILED",
        "The invitation input is invalid.",
    )
}

fn admin_required() -> ApiError {
    refusal(
        StatusCode::FORBIDDEN,
        "ADMIN_REQUIRED",
        "Administrator access is required.",
    )
}

fn authenticated(error: AuthenticatedError) -> ApiError {
    match error {
        AuthenticatedError::Authentication(error) => authentication(error),
        AuthenticatedError::Database(error) => error,
        AuthenticatedError::AmbiguousCommit => ApiError::database(),
    }
}

fn require_admin(actor: &CurrentActor) -> Result<(), ApiError> {
    if actor.role.as_deref() == Some("ADMIN") {
        Ok(())
    } else {
        Err(admin_required())
    }
}

/// The release-owned email-domain policy Phase 3 registration uses.
///
/// Composition supplies both domain lists; missing data disables matching, so
/// every address has no organization domain. The policy never performs DNS or
/// network work.
pub trait OrganizationDomainPolicy: Send + Sync + 'static {
    fn organization_domain(&self, email: &str) -> Option<String>;
}

/// Deployment values the invitation routes need. No mail provider: an
/// administrator delivers the fragment link, as the source does.
#[derive(Clone)]
pub struct InvitationService {
    /// Read pool for the unauthenticated resolve route.
    pub pool: sqlx::PgPool,
    pub domains: Arc<dyn OrganizationDomainPolicy>,
    pub frontend_url: Arc<str>,
}

/// One invitation request, normalized and type-checked before any target read.
#[derive(Debug, Clone)]
pub struct CreateInvitation {
    pub email: String,
    pub first_name: String,
    pub last_name: Option<String>,
    pub role: InvitationRole,
    pub organization: OrganizationSource,
}

/// The only roles an invitation may grant. ADMIN is never invitation-eligible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvitationRole {
    Buyer,
    Supplier,
}

impl InvitationRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Supplier => "SUPPLIER",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "BUYER" => Some(Self::Buyer),
            "SUPPLIER" => Some(Self::Supplier),
            _ => None,
        }
    }
}

/// A new organization created by the invitation. It is explicitly REAL; the
/// runtime never updates provenance.
#[derive(Debug, Clone)]
pub struct NewOrganization {
    pub name: String,
    pub kind: String,
    /// Assigned ISO 3166-1 alpha-2, already upper case.
    pub country_code: String,
    pub tax_id: Option<String>,
}

/// Exactly one organization source per request.
#[derive(Debug, Clone)]
pub enum OrganizationSource {
    Existing(Uuid),
    New(NewOrganization),
}

impl CreateInvitation {
    pub fn parse(body: &Value) -> Result<Self, ApiError> {
        let object = body.as_object().ok_or_else(invalid)?;
        let email_value = object.get("email").ok_or_else(invalid)?;
        let email = invite::normalize_email(&json!({"email": email_value.clone()}))
            .map_err(|_| invalid())?
            .trim()
            .to_lowercase();
        let first_name = required_text(object, "first_name", 1, 100)?;
        let last_name = optional_text(object, "last_name", 100)?;
        let role = object
            .get("role")
            .and_then(Value::as_str)
            .and_then(InvitationRole::parse)
            .ok_or_else(invalid)?;
        let organization_id = match object.get("organization_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(Uuid::parse_str(value).map_err(|_| invalid())?),
            Some(_) => return Err(invalid()),
        };
        let new_organization = match object.get("new_organization") {
            None | Some(Value::Null) => None,
            Some(Value::Object(value)) => Some(NewOrganization::parse(value)?),
            Some(_) => return Err(invalid()),
        };
        let organization = match (organization_id, new_organization) {
            (Some(id), None) => OrganizationSource::Existing(id),
            (None, Some(organization)) => OrganizationSource::New(organization),
            _ => return Err(invalid()),
        };
        Ok(Self {
            email,
            first_name,
            last_name,
            role,
            organization,
        })
    }
}

impl NewOrganization {
    fn parse(object: &serde_json::Map<String, Value>) -> Result<Self, ApiError> {
        let name = required_text(object, "name", 1, 200)?;
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| ORGANIZATION_TYPES.contains(value))
            .ok_or_else(invalid)?
            .to_owned();
        let raw = object
            .get("country_code")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let bytes = raw.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
            return Err(invalid());
        }
        let country_code = raw.to_ascii_uppercase();
        if !ISO_3166_1_ALPHA_2.contains(&country_code.as_str()) {
            return Err(invalid());
        }
        let tax_id = optional_tax_id(object)?;
        Ok(Self {
            name,
            kind,
            country_code,
            tax_id,
        })
    }
}

/// Python's `Field(min_length, max_length)` applies to the raw value; the
/// normalizer then strips and refuses a blank result.
fn required_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
    minimum: usize,
    maximum: usize,
) -> Result<String, ApiError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let length = value.chars().count();
    if length < minimum || length > maximum {
        return Err(invalid());
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

/// An optional name field: absent or null is absent, a present string is
/// stripped and must not become blank.
fn optional_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
    maximum: usize,
) -> Result<Option<String>, ApiError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.chars().count() > maximum {
                return Err(invalid());
            }
            let value = value.trim();
            if value.is_empty() {
                return Err(invalid());
            }
            Ok(Some(value.to_owned()))
        }
        Some(_) => Err(invalid()),
    }
}

/// `tax_id` alone normalizes a blank string to null.
fn optional_tax_id(object: &serde_json::Map<String, Value>) -> Result<Option<String>, ApiError> {
    match object.get("tax_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.chars().count() > 100 {
                return Err(invalid());
            }
            let value = value.trim();
            Ok((!value.is_empty()).then(|| value.to_owned()))
        }
        Some(_) => Err(invalid()),
    }
}

/// The bounded administrative read of eligible organizations.
#[derive(Debug, Clone, Copy)]
pub struct InvitationListQuery {
    limit: i64,
    offset: i64,
}

impl InvitationListQuery {
    pub fn parse(limit: Option<i64>, offset: Option<i64>) -> Result<Self, ApiError> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        if !(1..=500).contains(&limit) || !(0..=10_000).contains(&offset) {
            return Err(invalid());
        }
        Ok(Self { limit, offset })
    }

    /// Query-string parsing: unknown parameters are ignored and a repeat takes
    /// the last value, as FastAPI's `Query` binding does.
    pub fn parse_query(raw: &str) -> Result<Self, ApiError> {
        let mut limit = None;
        let mut offset = None;
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            let parsed = value.parse::<i64>().map_err(|_| invalid())?;
            match key.as_ref() {
                "limit" => limit = Some(parsed),
                "offset" => offset = Some(parsed),
                _ => {}
            }
        }
        Self::parse(limit, offset)
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }
}

/// A resolve secret. It is never logged, formatted or cloned.
pub struct ResolveToken(Zeroizing<String>);

impl std::fmt::Debug for ResolveToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolveToken(REDACTED)")
    }
}

impl ResolveToken {
    pub fn parse(body: &Value) -> Result<Self, ApiError> {
        let value = body
            .as_object()
            .and_then(|object| object.get("token"))
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let length = value.chars().count();
        if !(32..=128).contains(&length) {
            return Err(invalid());
        }
        Ok(Self(Zeroizing::new(value.to_owned())))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Python's `request_audit_context`: the middleware correlation id when the
/// request carries one, else the supplied header, plus the client address.
#[derive(Clone, Default)]
pub struct InvitationAudit {
    pub ip_address: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
    service: InvitationService,
}

/// Component router only. The serving binary does not mount these routes.
///
/// The group carries its own settings in an extension and returns the shared
/// `Router<AppState>` state type, so the composition root merges it with the
/// existing `router_with` and needs no second router constructor.
pub fn router(
    app: AppState,
    verification: VerificationSettings,
    limits: SharedLimits,
    service: InvitationService,
) -> Router<AppState> {
    Router::new()
        .route(LIST_PATH, get(list_route))
        .route(CREATE_PATH, post(create_route))
        .route(RESOLVE_PATH, post(resolve_route))
        .layer(axum::Extension(RouteState {
            app,
            verification: Arc::new(verification),
            limits,
            service,
        }))
}

fn authorization(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn client_of(request: &Request) -> String {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    )
}

fn request_id_of(request: &Request) -> Option<String> {
    request.extensions().get::<String>().cloned().or_else(|| {
        request
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    })
}

async fn read_body(request: Request) -> Result<Value, Response> {
    let body = axum::body::to_bytes(request.into_body(), AUTH_BODY_LIMIT_BYTES)
        .await
        .map_err(|_| {
            refusal(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BODY_TOO_LARGE",
                "The invitation body is too large.",
            )
            .into_response()
        })?;
    crate::auth::request::parse_json_body(&body).map_err(|error| error.into_response())
}

/// A04: eligible organizations, ordered by name and id, with a bounded read.
async fn list_route(
    axum::Extension(state): axum::Extension<RouteState>,
    request: Request,
) -> Response {
    let now = Utc::now();
    let authorization = authorization(&request);
    let access = match verify_authorization(authorization.as_deref(), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let client = client_of(&request);
    let raw_query = request.uri().query().unwrap_or("").to_owned();
    let quota_now = now.timestamp().max(0) as u64;
    let limits = state.limits.clone();
    let quota_token = authorization.clone();
    let result = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", LIST_PATH, false),
        move |transaction| {
            Box::pin(async move {
                require_admin(transaction.actor())?;
                let query = InvitationListQuery::parse_query(&raw_query)?;
                let user = transaction.actor().id.to_string();
                limits.check_business(
                    "GET",
                    LIST_PATH,
                    BusinessLimitKeys {
                        client: &client,
                        user: Some(&user),
                        organization: None,
                        token: quota_token.as_deref(),
                    },
                    quota_now,
                )?;
                store::list_organizations(transaction.connection(), &query).await
            })
        },
    )
    .await;
    let page = match result {
        Ok(page) => page,
        Err(error) => return authenticated(error).into_response(),
    };
    let mut response = axum::Json(store::InvitationOrganizationResponse {
        items: page.items,
        truncated: page.truncated,
    })
    .into_response();
    response.headers_mut().insert(
        "x-result-truncated",
        if page.truncated {
            axum::http::HeaderValue::from_static("true")
        } else {
            axum::http::HeaderValue::from_static("false")
        },
    );
    response
}

/// A05: create a new invitation or rotate an unclaimed one.
async fn create_route(
    axum::Extension(state): axum::Extension<RouteState>,
    request: Request,
) -> Response {
    let now = Utc::now();
    let authorization = authorization(&request);
    let client = client_of(&request);
    let request_id = request_id_of(&request);
    let body = match read_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let access = match verify_authorization(authorization.as_deref(), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let quota_now = now.timestamp().max(0) as u64;
    let limits = state.limits.clone();
    let quota_client = client.clone();
    let quota_token = authorization.clone().unwrap_or_default();
    let admitted = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("POST", CREATE_PATH, false),
        move |transaction| {
            Box::pin(async move {
                require_admin(transaction.actor())?;
                let invitation = CreateInvitation::parse(&body)?;
                let user = transaction.actor().id.to_string();
                limits.check_business(
                    "POST",
                    CREATE_PATH,
                    BusinessLimitKeys {
                        client: &quota_client,
                        user: Some(&user),
                        organization: None,
                        token: Some(quota_token.as_str()),
                    },
                    quota_now,
                )?;
                Ok(invitation)
            })
        },
    )
    .await;
    let invitation = match admitted {
        Ok(invitation) => invitation,
        Err(error) => return authenticated(error).into_response(),
    };
    // The write runs in its own canonical transaction and revalidates the
    // administrator under the lock; the read above is not authority.
    let access = match verify_authorization(authorization.as_deref(), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let command = store::CreateInvitationCommand {
        invitation,
        now,
        audit: InvitationAudit {
            ip_address: Some(client),
            request_id,
        },
        domains: state.service.domains.clone(),
        frontend_url: state.service.frontend_url.clone(),
    };
    match run_organization_write(&state.app, access, command).await {
        Ok(created) => {
            let status = if created.reissued {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (status, axum::Json(created)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// A06: resolve an invitation secret without consuming it.
async fn resolve_route(
    axum::Extension(state): axum::Extension<RouteState>,
    request: Request,
) -> Response {
    let now = Utc::now();
    let client = client_of(&request);
    let body = match read_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let token = match ResolveToken::parse(&body) {
        Ok(token) => token,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = state.limits.check_business(
        "POST",
        RESOLVE_PATH,
        BusinessLimitKeys {
            client: &client,
            user: None,
            organization: None,
            token: None,
        },
        now.timestamp().max(0) as u64,
    ) {
        return error.into_response();
    }
    let mut connection = match state.service.pool.acquire().await {
        Ok(connection) => connection,
        Err(_) => return ApiError::database().into_response(),
    };
    match resolve::resolve(&mut connection, &token, now).await {
        Ok(invitation) => axum::Json(invitation).into_response(),
        Err(error) => error.into_response(),
    }
}
