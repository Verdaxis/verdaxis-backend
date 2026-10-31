//! N02: the onboarding-attention classifier and its administrator read.
//!
//! Source: `app/routers/admin_attention.py` and
//! `app/services/onboarding_attention.py`. The stage rules are the exact
//! rules the external Telegram timer uses, with an explicit `now`, so the
//! alert channel (Phase 10) and this read can never disagree.
//!
//! No message state, dedup file, Telegram client or timer lives here. The
//! route is an identified operational surface, not product analytics: it
//! returns Python's fields and nothing else.

use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;

use crate::{
    ApiError, AppState,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        run_authenticated, verify_authorization,
    },
    business::error::{authentication, database_failure, refusal},
    runtime::http::client_ip,
};

/// N02's exact path.
pub const ATTENTION_PATH: &str = "/api/admin/onboarding-attention";

/// A first login is overdue this long after the last onboarding gate.
const FIRST_LOGIN_OVERDUE: Duration = Duration::hours(2);
/// An unverified account stalls this long after creation without a token
/// expiry of its own.
const VERIFICATION_STALLED: Duration = Duration::hours(24);
/// A pending organization setup alerts within this window of its expiry.
const ORGANIZATION_SETUP_WARNING: Duration = Duration::minutes(5);
/// Organizations with this provenance never produce operator attention.
const EXCLUDED_PROVENANCE: [&str; 3] = ["DEMO", "TEST", "CANARY"];
/// The monitor canary local part and domain suffix.
const CANARY_LOCAL: &str = "canary+";
const CANARY_DOMAIN_SUFFIX: &str = ".canary.verdaxis.exchange";

/// The read cap of the pinned route inventory: `limit` default 100, 1..=500,
/// `offset` 0..=10000, fetch at most `limit + 1` after filtering.
pub const LIMIT_DEFAULT: i64 = 100;
pub const LIMIT_MINIMUM: i64 = 1;
pub const LIMIT_MAXIMUM: i64 = 500;
pub const OFFSET_MAXIMUM: i64 = 10_000;

/// The action name whose latest timestamp counts as the organization approval
/// gate. It is Python's dotted registry value, a wire contract.
const ORGANIZATION_APPROVED_ACTION: &str = "admin.organization_approved";

/// The five operator stages, in Python's wire spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionStage {
    Rejected,
    OrganizationSetupExpiring,
    VerificationStalled,
    ApprovalRequired,
    FirstLoginOverdue,
}

impl AttentionStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::OrganizationSetupExpiring => "organization_setup_expiring",
            Self::VerificationStalled => "verification_stalled",
            Self::ApprovalRequired => "approval_required",
            Self::FirstLoginOverdue => "first_login_overdue",
        }
    }

    /// The stage's fixed operator copy.
    fn label(self) -> &'static str {
        match self {
            Self::Rejected => "Rejected",
            Self::OrganizationSetupExpiring => "Organization setup expiring",
            Self::VerificationStalled => "Email verification stalled",
            Self::ApprovalRequired => "Approval required",
            Self::FirstLoginOverdue => "First login overdue",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Rejected => "Review the rejection and contact the user if appropriate.",
            Self::OrganizationSetupExpiring => {
                "Contact the user before their organization setup expires."
            }
            Self::VerificationStalled => "Ask the user to request a new verification email.",
            Self::ApprovalRequired => "Review and complete the outstanding onboarding approvals.",
            Self::FirstLoginOverdue => {
                "Contact the user and offer the password-reset flow if needed."
            }
        }
    }
}

/// One onboarding projection, exactly the facts the classifier reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub role: Option<String>,
    pub created_at: DateTime<Utc>,
    pub email_verified: bool,
    pub verification_expires_at: Option<DateTime<Utc>>,
    pub user_status: Option<String>,
    pub organization_name: Option<String>,
    pub organization_status: Option<String>,
    pub organization_provenance: Option<String>,
    pub membership_status: Option<String>,
    pub membership_reviewed_at: Option<DateTime<Utc>>,
    pub user_approved_at: Option<DateTime<Utc>>,
    pub organization_approved_at: Option<DateTime<Utc>>,
    pub last_login: Option<DateTime<Utc>>,
    pub pending_registration_expires_at: Option<DateTime<Utc>>,
}

/// A reported stage and the instant it has been true since.
#[derive(Debug, Clone, PartialEq)]
pub struct Attention {
    pub stage: AttentionStage,
    pub since: DateTime<Utc>,
}

/// Python's monitor-canary predicate: a `canary+` local part and the canary
/// domain suffix.
pub fn is_monitor_canary_email(email: &str) -> bool {
    let (local, domain) = email
        .split_once('@')
        .map_or((email, ""), |(local, domain)| (local, domain));
    local.starts_with(CANARY_LOCAL) && domain.ends_with(CANARY_DOMAIN_SUFFIX)
}

/// The classifier. `now` is explicit so the read route and the alert timer
/// share one deterministic rule.
pub fn classify(candidate: &Candidate, now: DateTime<Utc>) -> Option<Attention> {
    if candidate.role.as_deref() == Some("ADMIN")
        || is_monitor_canary_email(&candidate.email)
        || candidate
            .organization_provenance
            .as_deref()
            .is_some_and(|provenance| EXCLUDED_PROVENANCE.contains(&provenance))
    {
        return None;
    }

    let statuses = [
        candidate.user_status.as_deref(),
        candidate.organization_status.as_deref(),
        candidate.membership_status.as_deref(),
    ];
    if statuses.contains(&Some("REJECTED")) {
        return Some(Attention {
            stage: AttentionStage::Rejected,
            since: candidate.created_at,
        });
    }

    if let Some(expiry) = candidate.pending_registration_expires_at {
        if expiry <= now + ORGANIZATION_SETUP_WARNING {
            return Some(Attention {
                stage: AttentionStage::OrganizationSetupExpiring,
                since: expiry,
            });
        }
        return None;
    }

    if !candidate.email_verified {
        let expiry = candidate
            .verification_expires_at
            .unwrap_or(candidate.created_at + VERIFICATION_STALLED);
        if expiry <= now {
            return Some(Attention {
                stage: AttentionStage::VerificationStalled,
                since: expiry,
            });
        }
        return None;
    }

    if statuses != [Some("APPROVED"), Some("APPROVED"), Some("APPROVED")] {
        return Some(Attention {
            stage: AttentionStage::ApprovalRequired,
            since: candidate.created_at,
        });
    }

    let approved_at = [
        Some(candidate.created_at),
        candidate.user_approved_at,
        candidate.organization_approved_at,
        candidate.membership_reviewed_at,
    ]
    .into_iter()
    .flatten()
    .max()?;
    if candidate.last_login.is_none() && approved_at <= now - FIRST_LOGIN_OVERDUE {
        return Some(Attention {
            stage: AttentionStage::FirstLoginOverdue,
            since: approved_at,
        });
    }
    None
}

/// The operator display name: both name parts joined on one space, or `None`
/// when neither part has content. Python joins truthy parts, so an empty
/// string part is dropped and only an all-empty pair gives `None`.
pub fn joined_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let joined = [first, last]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// `_elapsed`: the timing line of the Telegram message. A future `since`
/// reports the whole minutes until it; a past one reports minutes, hours or
/// days ago, rounded down, in Python's exact spelling.
fn elapsed_text(since: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - since).num_seconds();
    if seconds < 0 {
        let minutes = (-seconds + 59) / 60;
        return format!("in {}m", minutes.max(1));
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h {}m ago", minutes % 60);
    }
    format!("{}d ago", hours / 24)
}

/// The display name of the alert message. Python strips each part and keeps
/// only a part that still has content, then falls back to the email; this is
/// deliberately stricter than the response's `name`, which only tests
/// truthiness.
fn display_name(candidate: &Candidate) -> String {
    let joined = [
        candidate.first_name.as_deref(),
        candidate.last_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    if joined.is_empty() {
        candidate.email.clone()
    } else {
        joined
    }
}

/// The alert message the external timer sends. The route never builds it; it
/// is the operator copy of the reported stage with no credential, token,
/// hash or address fact in it.
pub fn format_message(candidate: &Candidate, attention: &Attention, now: DateTime<Utc>) -> String {
    format!(
        "Verdaxis onboarding needs attention\n\
         {} · {}\n\
         Organization: {}\n\
         Stage: {}\n\
         Timing: {}\n\
         Action: {}",
        display_name(candidate),
        candidate.email,
        candidate
            .organization_name
            .as_deref()
            .unwrap_or("Not created"),
        attention.stage.label(),
        elapsed_text(attention.since, now),
        attention.stage.action(),
    )
}

/// The validated N02 query: `limit` default 100 (1..=500), `offset` 0..=10000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionQuery {
    limit: i64,
    offset: i64,
}

impl AttentionQuery {
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Result<Self, ApiError> {
        Self::from_query(
            &pairs
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&"),
        )
    }

    /// Parse and bound the query the way FastAPI validates the typed
    /// parameters, with the route's explicit offset cap.
    pub fn from_query(query: &str) -> Result<Self, ApiError> {
        let fields = crate::runtime::http::QueryFields::parse(query);
        let mut errors: Vec<Value> = Vec::new();
        let limit = Self::integer(
            &fields,
            "limit",
            LIMIT_MINIMUM,
            LIMIT_MAXIMUM,
            LIMIT_DEFAULT,
            &mut errors,
        );
        let offset = Self::integer(&fields, "offset", 0, OFFSET_MAXIMUM, 0, &mut errors);
        if !errors.is_empty() {
            return Err(validation(errors));
        }
        Ok(Self { limit, offset })
    }

    fn integer(
        fields: &crate::runtime::http::QueryFields,
        name: &str,
        minimum: i64,
        maximum: i64,
        default: i64,
        errors: &mut Vec<Value>,
    ) -> i64 {
        match fields.validate(
            name,
            crate::runtime::http::QueryKind::Integer { minimum, maximum },
            false,
        ) {
            Ok(Some(crate::runtime::http::QueryValue::Integer(value))) => value,
            Ok(_) => default,
            Err(error) => {
                errors.extend(error.detail.as_array().cloned().unwrap_or_default());
                default
            }
        }
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }
}

fn validation(entries: Vec<Value>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Value::Array(entries),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}

/// One projected attention row.
#[derive(sqlx::FromRow)]
struct AttentionRow {
    email: String,
    name: Option<String>,
    role: Option<String>,
    stage: String,
    since: DateTime<Utc>,
    organization_name: Option<String>,
    last_login: Option<DateTime<Utc>>,
}

impl AttentionRow {
    /// `AttentionItem`, in the model's field order.
    fn response(&self) -> Value {
        json!({
            "email": self.email,
            "name": self.name,
            "role": self.role,
            "stage": self.stage,
            "since": crate::routes::ports::pydantic_datetime(self.since),
            "organization_name": self.organization_name,
            "last_login": self.last_login.map(crate::routes::ports::pydantic_datetime),
        })
    }
}

/// Load one bounded attention page in a set-based projection.
///
/// The inner candidate set is exactly Python's `load_candidates`: every
/// non-admin user with its organization, current-membership join row, latest
/// `APPROVED` status transition and latest organization-approval audit row,
/// then every unused pending registration. The stage classification runs in
/// the same statement, one stage per candidate in Python's first-match order,
/// and the exclusion predicate (admin, monitor canary, synthetic provenance)
/// is the lateral join condition, so an excluded candidate produces no stage
/// at all. Filtering and classification therefore both happen before the
/// ordered limit, and the page never depends on loading every user row.
pub async fn load_page(
    connection: &mut PgConnection,
    query: AttentionQuery,
    now: DateTime<Utc>,
) -> Result<(Vec<Value>, bool), ApiError> {
    // `u.role::text <> 'ADMIN'` keeps Python's filter exactly: the comparison
    // is NULL for a NULL role, so that row is dropped too. Pending
    // registrations are classified separately and keep their own rule.
    let fetch = query.limit + 1;
    const SQL: &str = r#"
WITH candidates AS (
    SELECT 'user:' || u.id AS candidate_key,
           u.email,
           nullif(concat_ws(' ', nullif(u.first_name, ''), nullif(u.last_name, '')), '') AS name,
           u.role::text AS role,
           u.created_at,
           u.email_verified,
           u.email_verification_token_expires_at,
           u.status::text AS user_status,
           o.name AS organization_name,
           o.verification_status AS organization_status,
           o.provenance AS organization_provenance,
           m.status::text AS membership_status,
           m.reviewed_at AS membership_reviewed_at,
           (SELECT max(t.effective_at)
              FROM user_status_transitions t
             WHERE t.user_id = u.id AND t.to_status = 'APPROVED') AS user_approved_at,
           (SELECT max(l.timestamp)
              FROM audit_logs l
             WHERE l.action = $3 AND l.resource_id = o.id::text) AS organization_approved_at,
           u.last_login,
           NULL::timestamptz AS pending_registration_expires_at,
           (COALESCE(o.provenance IN ('DEMO', 'TEST', 'CANARY'), false)
            OR (split_part(u.email, '@', 1) LIKE 'canary+%'
                AND split_part(u.email, '@', 2) LIKE '%.canary.verdaxis.exchange')) AS excluded
      FROM users u
      LEFT JOIN organizations o ON o.id = u.organization_id
      LEFT JOIN organization_join_requests m
             ON m.user_id = u.id AND m.organization_id = u.organization_id
     WHERE u.role::text <> 'ADMIN'
    UNION ALL
    SELECT 'pending:' || p.id,
           p.email,
           nullif(concat_ws(' ', nullif(p.first_name, ''), nullif(p.last_name, '')), ''),
           p.role::text,
           p.created_at,
           false,
           NULL::timestamptz,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL,
           NULL::timestamptz,
           NULL::timestamptz,
           NULL,
           p.expires_at,
           (p.role IS NOT DISTINCT FROM 'ADMIN'
            OR (split_part(p.email, '@', 1) LIKE 'canary+%'
                AND split_part(p.email, '@', 2) LIKE '%.canary.verdaxis.exchange'))
      FROM pending_registrations p
     WHERE p.used_at IS NULL
),
classified AS (
    SELECT c.email, c.name, c.role, c.organization_name, c.last_login,
           s.stage, s.since, c.candidate_key
      FROM candidates c
      LEFT JOIN LATERAL (
          SELECT stage, since, priority
            FROM (
              SELECT 'rejected' AS stage, c.created_at AS since, 1 AS priority
               WHERE c.user_status = 'REJECTED'
                  OR c.organization_status = 'REJECTED'
                  OR c.membership_status = 'REJECTED'
              UNION ALL
              SELECT 'organization_setup_expiring', c.pending_registration_expires_at, 2
               WHERE c.pending_registration_expires_at IS NOT NULL
                 AND c.pending_registration_expires_at <= $1 + interval '5 minutes'
              UNION ALL
              SELECT 'verification_stalled',
                     COALESCE(c.email_verification_token_expires_at,
                              c.created_at + interval '24 hours'),
                     3
               WHERE c.pending_registration_expires_at IS NULL
                 AND NOT c.email_verified
                 AND COALESCE(c.email_verification_token_expires_at,
                              c.created_at + interval '24 hours') <= $1
              UNION ALL
              SELECT 'approval_required', c.created_at, 4
               WHERE c.pending_registration_expires_at IS NULL
                 AND c.email_verified
                 AND NOT (COALESCE(c.user_status, '') = 'APPROVED'
                          AND COALESCE(c.organization_status, '') = 'APPROVED'
                          AND COALESCE(c.membership_status, '') = 'APPROVED')
              UNION ALL
              SELECT 'first_login_overdue',
                     GREATEST(c.created_at,
                              COALESCE(c.user_approved_at, c.created_at),
                              COALESCE(c.organization_approved_at, c.created_at),
                              COALESCE(c.membership_reviewed_at, c.created_at)),
                     5
               WHERE c.pending_registration_expires_at IS NULL
                 AND c.email_verified
                 AND COALESCE(c.user_status, '') = 'APPROVED'
                 AND COALESCE(c.organization_status, '') = 'APPROVED'
                 AND COALESCE(c.membership_status, '') = 'APPROVED'
                 AND c.last_login IS NULL
                 AND GREATEST(c.created_at,
                              COALESCE(c.user_approved_at, c.created_at),
                              COALESCE(c.organization_approved_at, c.created_at),
                              COALESCE(c.membership_reviewed_at, c.created_at))
                     <= $1 - interval '2 hours'
            ) attention
           ORDER BY priority
           LIMIT 1
      ) s ON NOT c.excluded
)
SELECT email, name, role, stage, since, organization_name, last_login
  FROM classified
 WHERE stage IS NOT NULL
 ORDER BY since, candidate_key
 LIMIT $2
OFFSET $4
"#;
    let rows: Vec<AttentionRow> = sqlx::query_as::<_, AttentionRow>(SQL)
        .bind(now)
        .bind(fetch)
        .bind(ORGANIZATION_APPROVED_ACTION)
        .bind(query.offset)
        .fetch_all(connection)
        .await
        .map_err(|_| database_failure(None))?;
    let truncated = rows.len() as i64 > query.limit;
    let items = rows
        .into_iter()
        .take(query.limit as usize)
        .map(|row| row.response())
        .collect();
    Ok((items, truncated))
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
}

/// Component router only. The serving binary does not mount these routes.
pub fn router(app: AppState, verification: VerificationSettings, limits: SharedLimits) -> Router {
    Router::new()
        .route(ATTENTION_PATH, get(attention_route))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            limits,
        })
}

/// N02's adapter. Admission order is fixed: verify the token, revalidate the
/// current account under its row lock, require ADMIN, consume the route quota,
/// and only then parse the query and read candidates.
async fn attention_route(State(state): State<RouteState>, request: Request) -> Response {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access = match verify_authorization(authorization.as_deref(), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return authentication(error).into_response(),
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let raw_query = request.uri().query().unwrap_or_default().to_owned();
    let limits = state.limits.clone();
    let token = authorization.clone().unwrap_or_default();
    let quota_client = client.clone();
    let result = run_authenticated(
        &state.app,
        access,
        AuthenticatedRequest::new("GET", ATTENTION_PATH, false),
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor();
                if actor.role.as_deref() != Some("ADMIN") {
                    return Err(refusal(
                        StatusCode::FORBIDDEN,
                        "ADMIN_REQUIRED",
                        "Administrator access is required.",
                    ));
                }
                let user = actor.id.to_string();
                limits.check_business(
                    "GET",
                    ATTENTION_PATH,
                    BusinessLimitKeys {
                        client: &quota_client,
                        user: Some(&user),
                        organization: None,
                        token: Some(&token),
                    },
                    now.timestamp().max(0) as u64,
                )?;
                let query = AttentionQuery::from_query(&raw_query)?;
                let (items, truncated) = load_page(transaction.connection(), query, now).await?;
                Ok(json!({
                    "items": items,
                    "generated_at": crate::routes::ports::pydantic_datetime(now),
                    "truncated": truncated,
                }))
            })
        },
    )
    .await;
    let body = match result {
        Ok(value) => value,
        Err(AuthenticatedError::Authentication(error)) => {
            return authentication(error).into_response();
        }
        Err(AuthenticatedError::Database(error)) => return error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => return ApiError::database().into_response(),
    };
    let truncated = body["truncated"].as_bool().unwrap_or(false);
    let mut response = axum::Json(body).into_response();
    response.headers_mut().insert(
        "x-result-truncated",
        if truncated {
            HeaderValue::from_static("true")
        } else {
            HeaderValue::from_static("false")
        },
    );
    response
}
