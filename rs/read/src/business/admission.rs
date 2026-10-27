use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    audit::{AuditFact, append_audit},
    error::refusal,
    status::{AccountStatus, StatusFact, append_status_transition},
    transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
};
use crate::auth::{CurrentActor, store::revoke_user_authentication};

#[path = "review_queue.rs"]
pub mod review_queue;

/// The route integration supplies the shared public projection and event producer.
/// Neither method may commit or perform provider I/O.
pub trait AdmissionEvents: Send + Sync + 'static {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError>;
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError>;
}

pub struct RejectAccount<E> {
    pub user_id: Uuid,
    pub now: DateTime<Utc>,
    pub reason: String,
    pub events: E,
}

#[derive(Debug, serde::Serialize)]
pub struct RejectionResult {
    pub user_id: Uuid,
    pub cutoff: DateTime<Utc>,
}

pub fn revocation_failure(error: sqlx::Error) -> AttemptError {
    match error {
        sqlx::Error::RowNotFound => missing(),
        other => AttemptError::Database(other),
    }
}

fn missing() -> AttemptError {
    refusal(
        StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
        "The user was not found.",
    )
    .into()
}

impl<E: AdmissionEvents> BusinessCommand for RejectAccount<E> {
    type Output = RejectionResult;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row =
                sqlx::query("SELECT organization_id,role::text AS role FROM users WHERE id=$1")
                    .bind(self.user_id)
                    .fetch_optional(conn)
                    .await?
                    .ok_or_else(missing)?;
            let organization: Option<Uuid> = row.get("organization_id");
            Ok(OrganizationWritePlan::new(
                organization,
                [self.user_id],
                [],
                json!({"organization":organization,"role":row.get::<Option<String>,_>("role")}),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, RejectionResult, AttemptError> {
        // The standalone command has no request; routed rejections supply the
        // optional wire reason and correlation through `AccountRejection`.
        let audit = AuditContext::default();
        Box::pin(async move {
            execute_rejection(
                conn,
                actor,
                self.user_id,
                self.now,
                Some(&self.reason),
                &audit,
                &self.events,
            )
            .await
        })
    }
}

/// Python's `record_audit(..., **request_audit_context(request))` inputs.
/// The client address is always a string; an absent address is `"unknown"`.
#[derive(Debug, Clone, Default)]
pub struct AuditContext {
    pub ip_address: Option<String>,
    pub request_id: Option<String>,
}

async fn execute_rejection<E: AdmissionEvents>(
    conn: &mut PgConnection,
    actor: &CurrentActor,
    user_id: Uuid,
    now: DateTime<Utc>,
    reason: Option<&str>,
    audit: &AuditContext,
    events: &E,
) -> Result<RejectionResult, AttemptError> {
    if actor.role.as_deref() != Some("ADMIN") {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        )
        .into());
    }
    let row = sqlx::query(
        "SELECT status::text AS status,role::text AS role,organization_id FROM users WHERE id=$1",
    )
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(missing)?;
    let role: Option<String> = row.get("role");
    if role.as_deref() == Some("ADMIN") {
        return Err(missing());
    }
    let previous = match row.get::<String, _>("status").as_str() {
        "PENDING" => AccountStatus::Pending,
        "APPROVED" => AccountStatus::Approved,
        "REJECTED" => AccountStatus::Rejected,
        _ => {
            return Err(refusal(
                StatusCode::CONFLICT,
                "ACCOUNT_STATE_INVALID",
                "The account state cannot be changed.",
            )
            .into());
        }
    };
    let organization: Option<Uuid> = row.get("organization_id");
    let before = events.visible(conn, user_id).await?;
    sqlx::query("UPDATE users SET status='REJECTED',pending_approval_email_transition_id=NULL,pending_approval_email_payload=NULL,pending_approval_email_retry_at=NULL WHERE id=$1")
        .bind(user_id).execute(&mut *conn).await?;
    // The ordered command holds the user row; sample the cutoff after any lock wait.
    let cutoff = revoke_user_authentication(conn, user_id, Utc::now())
        .await
        .map_err(revocation_failure)?;
    append_status_transition(
        conn,
        &StatusFact {
            id: Uuid::new_v4(),
            user_id,
            organization_id: organization,
            role,
            from: Some(previous),
            to: AccountStatus::Rejected,
            at: now,
        },
    )
    .await?;
    let after = events.visible(conn, user_id).await?;
    if previous != AccountStatus::Rejected
        && let Some(organization) = organization
    {
        events
            .append(conn, user_id, organization, before || after)
            .await?;
    }
    append_audit(
        conn,
        &AuditFact {
            id: Uuid::new_v4(),
            actor_id: Some(actor.id),
            action: "admin.user_rejected".into(),
            resource_type: "user".into(),
            resource_id: Some(user_id.to_string()),
            changes: Some(json!({
                "status":{"from":previous.as_str(),"to":"REJECTED"},
                "reason":reason
            })),
            ip_address: audit.ip_address.clone(),
            request_id: audit.request_id.clone(),
            at: now,
        },
    )
    .await?;
    Ok(RejectionResult { user_id, cutoff })
}

/// Frozen payload fields are the CF4_01 mail producer boundary.
#[derive(Clone, serde::Serialize)]
pub struct FrozenApprovalPayload {
    pub from: String,
    pub to: [String; 1],
    pub subject: String,
    pub html: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// Match the CF4_01 producer: never print an address or message content.
impl std::fmt::Debug for FrozenApprovalPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FrozenApprovalPayload(REDACTED)")
    }
}

pub struct ApprovalMailMarker {
    pub user_id: Uuid,
    pub transition_id: Uuid,
    pub payload: FrozenApprovalPayload,
    pub retry_at: DateTime<Utc>,
}

/// A borrowed producer adapter. Implementations call the CF4_01 producer and
/// retain raw SQL errors for the outer transaction's retry classifier.
pub trait ApprovalMail: Send + Sync + 'static {
    fn deliver_after_commit<'a>(
        &'a self,
        pool: &'a sqlx::PgPool,
        user: Uuid,
        transition: Uuid,
    ) -> CommandFuture<'a, (), ()> {
        Box::pin(async move {
            super::approval_mail::deliver_account_approval_mail(
                pool,
                user,
                transition,
                Utc::now,
                |_, _| async { false },
            )
            .await
            .map(|_| ())
            .map_err(|_| ())
        })
    }
    fn freeze_approval_mail<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        marker: &'a ApprovalMailMarker,
    ) -> CommandFuture<'a, (), AttemptError>;
}

pub struct ApproveAccount<M, E> {
    pub user_id: Uuid,
    pub now: DateTime<Utc>,
    pub payload: FrozenApprovalPayload,
    pub audit: AuditContext,
    pub mail: M,
    pub events: E,
}

#[derive(Debug)]
pub struct ApprovalResult {
    pub user_id: Uuid,
    pub transition_id: Option<Uuid>,
}

fn account_status(value: &str) -> Result<AccountStatus, AttemptError> {
    match value {
        "PENDING" => Ok(AccountStatus::Pending),
        "APPROVED" => Ok(AccountStatus::Approved),
        "REJECTED" => Ok(AccountStatus::Rejected),
        _ => Err(refusal(
            StatusCode::CONFLICT,
            "ACCOUNT_STATE_INVALID",
            "The account state cannot be changed.",
        )
        .into()),
    }
}

async fn account_plan(
    conn: &mut PgConnection,
    user: Uuid,
) -> Result<OrganizationWritePlan, AttemptError> {
    let row = sqlx::query("SELECT organization_id,role::text AS role FROM users WHERE id=$1")
        .bind(user)
        .fetch_optional(conn)
        .await?
        .ok_or_else(missing)?;
    let organization: Option<Uuid> = row.get("organization_id");
    Ok(OrganizationWritePlan::new(
        organization,
        [user],
        [],
        json!({"organization":organization,"role":row.get::<Option<String>,_>("role")}),
    ))
}

impl<M: ApprovalMail, E: AdmissionEvents> BusinessCommand for ApproveAccount<M, E> {
    type Output = ApprovalResult;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(account_plan(conn, self.user_id))
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            require_admin(actor)?;
            let row = sqlx::query("SELECT status::text AS status,role::text AS role,organization_id,email_verified FROM users WHERE id=$1")
                .bind(self.user_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let role: Option<String> = row.get("role");
            if role.as_deref() == Some("ADMIN") {
                return Err(refusal(
                    StatusCode::FORBIDDEN,
                    "ACCOUNT_NOT_MANAGEABLE",
                    "Administrator accounts cannot be managed here.",
                )
                .into());
            }
            if !row.get::<bool, _>("email_verified") {
                return Err(refusal(
                    StatusCode::FORBIDDEN,
                    "EMAIL_VERIFICATION_REQUIRED",
                    "The account email must be verified.",
                )
                .into());
            }
            let previous = account_status(&row.get::<String, _>("status"))?;
            let organization: Option<Uuid> = row.get("organization_id");
            let before = self.events.visible(conn, self.user_id).await?;
            sqlx::query("UPDATE users SET status='APPROVED' WHERE id=$1")
                .bind(self.user_id)
                .execute(&mut *conn)
                .await?;
            let transition_id = append_status_transition(
                conn,
                &StatusFact {
                    id: Uuid::new_v4(),
                    user_id: self.user_id,
                    organization_id: organization,
                    role,
                    from: Some(previous),
                    to: AccountStatus::Approved,
                    at: self.now,
                },
            )
            .await?;
            if let Some(transition_id) = transition_id {
                self.mail
                    .freeze_approval_mail(
                        conn,
                        &ApprovalMailMarker {
                            user_id: self.user_id,
                            transition_id,
                            payload: self.payload.clone(),
                            retry_at: self.now,
                        },
                    )
                    .await?;
            }
            let after = self.events.visible(conn, self.user_id).await?;
            if transition_id.is_some()
                && let Some(organization) = organization
            {
                self.events
                    .append(conn, self.user_id, organization, before || after)
                    .await?;
            }
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: "admin.user_approved".into(),
                    resource_type: "user".into(),
                    resource_id: Some(self.user_id.to_string()),
                    changes: Some(json!({"status":{"from":previous.as_str(),"to":"APPROVED"}})),
                    ip_address: self.audit.ip_address.clone(),
                    request_id: self.audit.request_id.clone(),
                    at: self.now,
                },
            )
            .await?;
            Ok(ApprovalResult {
                user_id: self.user_id,
                transition_id,
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionRoute {
    Auth,
    Analytics,
}

pub struct AccountRejection<E> {
    pub command: RejectAccount<E>,
    pub route: RejectionRoute,
    /// The client-supplied decision reason; `None` when the body omitted it,
    /// which Python records as JSON null. The standalone command's own `reason`
    /// cannot represent "omitted", so this field is authoritative here.
    pub reason: Option<String>,
    pub audit: AuditContext,
}

impl<E: AdmissionEvents> BusinessCommand for AccountRejection<E> {
    type Output = RejectionResult;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        self.command.preview(conn)
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            require_admin(actor)?;
            if self.route == RejectionRoute::Analytics {
                let row = sqlx::query(
                    "SELECT role::text AS role,status::text AS status FROM users WHERE id=$1",
                )
                .bind(self.command.user_id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(missing)?;
                if row.get::<Option<String>, _>("role").as_deref() == Some("ADMIN") {
                    return Err(refusal(
                        StatusCode::FORBIDDEN,
                        "ACCOUNT_NOT_MANAGEABLE",
                        "Administrator accounts cannot be managed here.",
                    )
                    .into());
                }
                if row.get::<String, _>("status") == "REJECTED" {
                    return Err(refusal(
                        StatusCode::CONFLICT,
                        "ACCOUNT_ALREADY_REJECTED",
                        "The account is already rejected.",
                    )
                    .into());
                }
            }
            execute_rejection(
                conn,
                actor,
                self.command.user_id,
                self.command.now,
                self.reason.as_deref(),
                &self.audit,
                &self.command.events,
            )
            .await
        })
    }
}

pub fn require_admin(actor: &CurrentActor) -> Result<(), crate::ApiError> {
    if actor.role.as_deref() != Some("ADMIN") {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum AdmissionRoute {
    Approve,
    Reject,
    Queue,
    Detail,
    AnalyticsReject,
}
impl AdmissionRoute {
    pub const ALL: [Self; 5] = [
        Self::Approve,
        Self::Reject,
        Self::Queue,
        Self::Detail,
        Self::AnalyticsReject,
    ];
    pub fn method(self) -> &'static str {
        match self {
            Self::Queue | Self::Detail => "GET",
            _ => "PUT",
        }
    }
    pub fn template(self) -> &'static str {
        match self {
            Self::Approve => "/api/auth/approve/{user_id}",
            Self::Reject => "/api/auth/reject/{user_id}",
            Self::Queue => "/api/auth/admin/review-queue",
            Self::Detail => "/api/auth/admin/review-queue/{user_id}",
            Self::AnalyticsReject => "/api/admin/analytics/users/{user_id}/reject",
        }
    }
}

pub fn admit_admin(
    actor: &CurrentActor,
    limits: &crate::auth::limits::SharedLimits,
    route: AdmissionRoute,
    client: &str,
    token: &str,
    now: u64,
) -> Result<(), crate::ApiError> {
    require_admin(actor)?;
    let user = actor.id.to_string();
    limits.check_business(
        route.method(),
        route.template(),
        crate::auth::limits::BusinessLimitKeys {
            client,
            user: Some(&user),
            organization: None,
            token: Some(token),
        },
        now,
    )
}

/// The template builder is pure; retries never contact a mail provider.
pub trait ApprovalTemplate: Send + Sync + 'static {
    fn payload(&self, email: &str, first_name: Option<&str>) -> FrozenApprovalPayload;
}

pub struct AdmissionService<M, E, T> {
    pub mail: std::sync::Arc<M>,
    pub events: std::sync::Arc<E>,
    pub template: std::sync::Arc<T>,
}
impl<M, E, T> Clone for AdmissionService<M, E, T> {
    fn clone(&self) -> Self {
        Self {
            mail: self.mail.clone(),
            events: self.events.clone(),
            template: self.template.clone(),
        }
    }
}
impl<M: ApprovalMail> ApprovalMail for std::sync::Arc<M> {
    fn deliver_after_commit<'a>(
        &'a self,
        pool: &'a sqlx::PgPool,
        user: Uuid,
        transition: Uuid,
    ) -> CommandFuture<'a, (), ()> {
        self.as_ref().deliver_after_commit(pool, user, transition)
    }
    fn freeze_approval_mail<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        marker: &'a ApprovalMailMarker,
    ) -> CommandFuture<'a, (), AttemptError> {
        self.as_ref().freeze_approval_mail(conn, marker)
    }
}
impl<E: AdmissionEvents> AdmissionEvents for std::sync::Arc<E> {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        self.as_ref().visible(conn, user)
    }
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        self.as_ref().append(conn, user, organization, public)
    }
}

struct RouteState<M, E, T> {
    app: crate::AppState,
    verification: std::sync::Arc<crate::auth::VerificationSettings>,
    limits: crate::auth::limits::SharedLimits,
    service: AdmissionService<M, E, T>,
}
impl<M, E, T> Clone for RouteState<M, E, T> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            verification: self.verification.clone(),
            limits: self.limits.clone(),
            service: self.service.clone(),
        }
    }
}

/// Component router only. The serving binary does not mount these routes.
pub fn router<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    app: crate::AppState,
    verification: crate::auth::VerificationSettings,
    limits: crate::auth::limits::SharedLimits,
    service: AdmissionService<M, E, T>,
) -> axum::Router {
    use axum::{
        Router,
        routing::{get, put},
    };
    Router::new()
        .route("/api/auth/approve/{user_id}", put(approve_route::<M, E, T>))
        .route("/api/auth/reject/{user_id}", put(reject_route::<M, E, T>))
        .route(
            "/api/admin/analytics/users/{user_id}/reject",
            put(analytics_route::<M, E, T>),
        )
        .route("/api/auth/admin/review-queue", get(queue_route::<M, E, T>))
        .route(
            "/api/auth/admin/review-queue/{user_id}",
            get(detail_route::<M, E, T>),
        )
        .with_state(RouteState {
            app,
            verification: std::sync::Arc::new(verification),
            limits,
            service,
        })
}

async fn approve_route<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    axum::extract::State(state): axum::extract::State<RouteState<M, E, T>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    dispatch(state, request, AdmissionRoute::Approve).await
}
async fn reject_route<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    axum::extract::State(state): axum::extract::State<RouteState<M, E, T>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    dispatch(state, request, AdmissionRoute::Reject).await
}
async fn analytics_route<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    axum::extract::State(state): axum::extract::State<RouteState<M, E, T>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    dispatch(state, request, AdmissionRoute::AnalyticsReject).await
}
async fn queue_route<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    axum::extract::State(state): axum::extract::State<RouteState<M, E, T>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    dispatch(state, request, AdmissionRoute::Queue).await
}
async fn detail_route<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    axum::extract::State(state): axum::extract::State<RouteState<M, E, T>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    dispatch(state, request, AdmissionRoute::Detail).await
}

async fn dispatch<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    state: RouteState<M, E, T>,
    request: axum::extract::Request,
    route: AdmissionRoute,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let now = Utc::now();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let access =
        match crate::auth::verify_authorization(authorization.as_deref(), &state.verification, now)
        {
            Ok(value) => value,
            Err(error) => return super::error::authentication(error).into_response(),
        };
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = crate::runtime::http::client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    // Account and administrator authority precede quota and all target reads.
    let limits = state.limits.clone();
    let token = authorization.clone().unwrap_or_default();
    let quota_client = client.clone();
    let admission = crate::auth::run_authenticated(
        &state.app,
        access,
        crate::auth::AuthenticatedRequest::new(route.method(), route.template(), false),
        move |tx| {
            Box::pin(async move {
                admit_admin(
                    tx.actor(),
                    &limits,
                    route,
                    &quota_client,
                    &token,
                    now.timestamp().max(0) as u64,
                )
            })
        },
    )
    .await;
    if let Err(error) = admission {
        return match error {
            crate::auth::AuthenticatedError::Authentication(error) => {
                super::error::authentication(error).into_response()
            }
            crate::auth::AuthenticatedError::Database(error) => error.into_response(),
            crate::auth::AuthenticatedError::AmbiguousCommit => {
                crate::ApiError::database().into_response()
            }
        };
    }
    // Python's `request_audit_context`: the middleware correlation id when the
    // request carries one, else the supplied header; the client address always.
    let audit = AuditContext {
        ip_address: Some(client),
        request_id: request.extensions().get::<String>().cloned().or_else(|| {
            request
                .headers()
                .get("x-request-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        }),
    };
    let result = dispatch_admitted(
        &state,
        request,
        route,
        Admitted {
            authorization: authorization.as_deref(),
            path,
            query,
            audit,
            now,
        },
    )
    .await;
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}

/// The parts of one admitted request that the route bodies read.
struct Admitted<'a> {
    authorization: Option<&'a str>,
    path: String,
    query: String,
    audit: AuditContext,
    now: DateTime<Utc>,
}

async fn dispatch_admitted<M: ApprovalMail, E: AdmissionEvents, T: ApprovalTemplate>(
    state: &RouteState<M, E, T>,
    request: axum::extract::Request,
    route: AdmissionRoute,
    admitted: Admitted<'_>,
) -> Result<serde_json::Value, crate::ApiError> {
    let Admitted {
        authorization,
        path,
        query,
        audit,
        now,
    } = admitted;
    let user = if matches!(route, AdmissionRoute::Queue) {
        None
    } else {
        let index = if matches!(route, AdmissionRoute::AnalyticsReject) {
            path.split('/').nth_back(1)
        } else {
            path.rsplit('/').next()
        };
        Some(
            index
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "VALIDATION_FAILED",
                        "The user ID must be a UUID.",
                    )
                })?,
        )
    };
    if matches!(route, AdmissionRoute::Queue | AdmissionRoute::Detail) {
        let access = crate::auth::verify_authorization(authorization, &state.verification, now)
            .map_err(super::error::authentication)?;
        let result = crate::auth::run_authenticated(
            &state.app,
            access,
            crate::auth::AuthenticatedRequest::new(route.method(), route.template(), false),
            move |tx| {
                Box::pin(async move {
                    require_admin(tx.actor())?;
                    if let Some(user) = user {
                        review_queue::detail(tx.connection(), user).await
                    } else {
                        let mut limit = 50;
                        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                            if key == "limit" {
                                limit = value.parse().map_err(|_| {
                                    refusal(
                                        StatusCode::UNPROCESSABLE_ENTITY,
                                        "VALIDATION_FAILED",
                                        "The review limit must be an integer.",
                                    )
                                })?;
                            }
                        }
                        review_queue::list(tx.connection(), review_queue::ReviewQuery::new(limit)?)
                            .await
                    }
                })
            },
        )
        .await;
        return result.map_err(|error| match error {
            crate::auth::AuthenticatedError::Authentication(error) => {
                super::error::authentication(error)
            }
            crate::auth::AuthenticatedError::Database(error) => error,
            crate::auth::AuthenticatedError::AmbiguousCommit => crate::ApiError::database(),
        });
    }
    let user = user.expect("entity route");
    let access = crate::auth::verify_authorization(authorization, &state.verification, now)
        .map_err(super::error::authentication)?;
    if matches!(route, AdmissionRoute::Approve) {
        let mut conn = state
            .app
            .pools
            .read
            .acquire()
            .await
            .map_err(|_| crate::ApiError::database())?;
        let row = sqlx::query("SELECT email,first_name FROM users WHERE id=$1")
            .bind(user)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| crate::ApiError::database())?
            .ok_or_else(|| {
                refusal(
                    StatusCode::NOT_FOUND,
                    "USER_NOT_FOUND",
                    "The user was not found.",
                )
            })?;
        let payload = state
            .service
            .template
            .payload(row.get("email"), row.get::<Option<&str>, _>("first_name"));
        drop(conn);
        let committed = super::transaction::run_organization_write(
            &state.app,
            access,
            ApproveAccount {
                user_id: user,
                now,
                payload,
                audit: audit.clone(),
                mail: state.service.mail.clone(),
                events: state.service.events.clone(),
            },
        )
        .await?;
        if let Some(transition) = committed.transition_id {
            if state
                .service
                .mail
                .deliver_after_commit(&state.app.pools.read, user, transition)
                .await
                .is_err()
            {
                tracing::warn!("account approval mail attempt deferred");
            }
        }
        return user_response(&state.app, user, false).await;
    }
    let reason = if matches!(route, AdmissionRoute::AnalyticsReject) {
        Some("admin_analytics_user_rejected".to_owned())
    } else {
        // A08's pinned `body_limit_bytes`, not a smaller route-local cap.
        let body = axum::body::to_bytes(request.into_body(), 131_072)
            .await
            .map_err(|_| {
                refusal(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "BODY_TOO_LARGE",
                    "The decision body is too large.",
                )
            })?;
        #[derive(serde::Deserialize)]
        struct Decision {
            reason: Option<String>,
        }
        let decision: Decision = serde_json::from_slice(&body).map_err(|_| {
            refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                "A decision object is required.",
            )
        })?;
        // Python's AdminDecisionBody.reason is optional; an omitted reason is
        // stored as JSON null, never the empty string.
        decision.reason
    };
    let analytics = matches!(route, AdmissionRoute::AnalyticsReject);
    super::transaction::run_organization_write(
        &state.app,
        access,
        AccountRejection {
            command: RejectAccount {
                user_id: user,
                now,
                reason: reason.clone().unwrap_or_default(),
                events: state.service.events.clone(),
            },
            route: if analytics {
                RejectionRoute::Analytics
            } else {
                RejectionRoute::Auth
            },
            reason,
            audit,
        },
    )
    .await?;
    if analytics {
        user_response(&state.app, user, true).await
    } else {
        Ok(json!({"user_id":user,"status":"REJECTED"}))
    }
}

async fn user_response(
    state: &crate::AppState,
    user: Uuid,
    analytics: bool,
) -> Result<serde_json::Value, crate::ApiError> {
    let sql = if analytics {
        "SELECT jsonb_build_object('id',u.id,'email',u.email,'first_name',u.first_name,'last_name',u.last_name,'role',COALESCE(u.role::text,''),'status',COALESCE(u.status::text,''),'created_at',u.created_at,'organization_id',u.organization_id,'org_name',o.name,'org_type',o.type,'org_provenance',o.provenance,'email_verified',u.email_verified,'must_change_password',u.must_change_password,'last_login',u.last_login,'org_has_orders',EXISTS(SELECT 1 FROM orderbook_orders x WHERE x.organization_id=u.organization_id)) FROM users u LEFT JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1"
    } else {
        // Python's A07 emits the `UserResponse` schema default for
        // `execution_eligibility`; it does not compute eligibility here.
        "SELECT jsonb_build_object('id',id,'email',email,'first_name',first_name,'last_name',last_name,'role',role,'status',status,'organization_id',organization_id,'referral_code',referral_code,'must_change_password',must_change_password,'kyc_status',kyc_status,'execution_eligibility',jsonb_build_object('eligible',false,'reason','KYC_REQUIRED','grace_expires_at',NULL)) FROM users WHERE id=$1"
    };
    sqlx::query_scalar(sql)
        .bind(user)
        .fetch_one(&state.pools.read)
        .await
        .map_err(|_| crate::ApiError::database())
}
