use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    audit::{AuditFact, append_audit},
    error::refusal,
    membership::{invalidate_review, require_admin, role_matches_organization},
    transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
};
use crate::{ApiError, auth::CurrentActor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JoinStatus {
    Pending,
    Approved,
    Rejected,
}
impl JoinStatus {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "APPROVED" => Ok(Self::Approved),
            "REJECTED" => Ok(Self::Rejected),
            _ => Err(invalid()),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }
}
fn invalid() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_FAILED",
        "The review input is invalid.",
    )
}
fn conflict(code: &'static str, message: &'static str) -> AttemptError {
    refusal(StatusCode::CONFLICT, code, message).into()
}
fn missing() -> AttemptError {
    refusal(
        StatusCode::NOT_FOUND,
        "ORGANIZATION_JOIN_NOT_FOUND",
        "The organization join request was not found.",
    )
    .into()
}

#[derive(Debug)]
pub struct ReviewNote(String);
impl ReviewNote {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        let value = value.trim();
        if !(3..=1000).contains(&value.chars().count()) {
            return Err(invalid());
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub struct JoinListQuery {
    status: Option<JoinStatus>,
    limit: i64,
    offset: i64,
}
impl JoinListQuery {
    pub fn parse(status: Option<&str>, limit: i64, offset: i64) -> Result<Self, ApiError> {
        if !(1..=500).contains(&limit) || !(0..=10000).contains(&offset) {
            return Err(invalid());
        }
        Ok(Self {
            status: status.map(JoinStatus::parse).transpose()?,
            limit,
            offset,
        })
    }
}
#[derive(Debug, Serialize)]
pub struct JoinItem {
    pub id: Uuid,
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub status: JoinStatus,
    pub reviewed_by: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub review_note: Option<String>,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug)]
pub struct JoinPage {
    pub items: Vec<JoinItem>,
    pub truncated: bool,
}

/// The read adapter keeps the array response and sends truncated in
/// X-Result-Truncated. Filtering and the limit+1 bound remain in PostgreSQL.
pub async fn list(
    conn: &mut PgConnection,
    actor: &CurrentActor,
    query: &JoinListQuery,
) -> Result<JoinPage, ApiError> {
    require_admin(actor)?;
    let rows=sqlx::query("SELECT id,user_id,organization_id,status::text AS status,reviewed_by,reviewed_at,review_note,created_at FROM organization_join_requests WHERE ($1::text IS NULL OR status::text=$1) ORDER BY created_at,id LIMIT $2 OFFSET $3")
        .bind(query.status.map(JoinStatus::as_str)).bind(query.limit+1).bind(query.offset).fetch_all(conn).await
        .map_err(|error|super::error::database_failure(error.as_database_error().and_then(|error|error.code()).as_deref()))?;
    let truncated = rows.len() > query.limit as usize;
    let items = rows
        .into_iter()
        .take(query.limit as usize)
        .map(|row| {
            Ok(JoinItem {
                id: row.get("id"),
                user_id: row.get("user_id"),
                organization_id: row.get("organization_id"),
                status: JoinStatus::parse(&row.get::<String, _>("status")).map_err(|_| {
                    refusal(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "JOIN_STATE_INVALID",
                        "The stored request state is invalid.",
                    )
                })?,
                reviewed_by: row.get("reviewed_by"),
                reviewed_at: row.get("reviewed_at"),
                review_note: row.get("review_note"),
                created_at: row.get("created_at"),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(JoinPage { items, truncated })
}

/// Component adapters only; no route is mounted here.
pub async fn review(
    state: &crate::AppState,
    access: crate::auth::VerifiedAccess,
    limits: &crate::auth::limits::SharedLimits,
    client: &str,
    command: ReviewJoin,
) -> Result<JoinDecision, ApiError> {
    let template = if command.approve {
        "/api/auth/organization-joins/{join_request_id}/approve"
    } else {
        "/api/auth/organization-joins/{join_request_id}/reject"
    };
    super::transaction::run_organization_write(
        state,
        access,
        LimitedReview {
            command,
            limits: limits.clone(),
            client: client.into(),
            template,
        },
    )
    .await
}

pub async fn read(
    state: &crate::AppState,
    access: crate::auth::VerifiedAccess,
    limits: crate::auth::limits::SharedLimits,
    client: String,
    query: JoinListQuery,
) -> Result<JoinPage, ApiError> {
    use crate::auth::{AuthenticatedError, AuthenticatedRequest, run_authenticated};
    run_authenticated(
        state,
        access,
        AuthenticatedRequest::new("GET", "/api/auth/organization-joins", false),
        move |tx| {
            Box::pin(async move {
                require_admin(tx.actor())?;
                limits.check_business(
                    "GET",
                    "/api/auth/organization-joins",
                    crate::auth::limits::BusinessLimitKeys {
                        client: &client,
                        user: None,
                        organization: None,
                        token: None,
                    },
                    Utc::now().timestamp().max(0) as u64,
                )?;
                let actor = tx.actor().clone();
                list(tx.connection(), &actor, &query).await
            })
        },
    )
    .await
    .map_err(|error| match error {
        AuthenticatedError::Authentication(error) => super::error::authentication(error),
        AuthenticatedError::Database(error) => error,
        AuthenticatedError::AmbiguousCommit => super::error::database_failure(None),
    })
}

struct LimitedReview {
    command: ReviewJoin,
    limits: crate::auth::limits::SharedLimits,
    client: String,
    template: &'static str,
}
impl BusinessCommand for LimitedReview {
    type Output = JoinDecision;
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
            self.limits.check_business(
                "PUT",
                self.template,
                crate::auth::limits::BusinessLimitKeys {
                    client: &self.client,
                    user: None,
                    organization: None,
                    token: None,
                },
                Utc::now().timestamp().max(0) as u64,
            )?;
            self.command.execute(conn, actor).await
        })
    }
}

pub struct ReviewJoin {
    pub join_request_id: Uuid,
    pub approve: bool,
    pub note: ReviewNote,
    pub now: DateTime<Utc>,
}
#[derive(Debug, Serialize)]
pub struct JoinDecision {
    pub id: Uuid,
    pub status: JoinStatus,
}
impl BusinessCommand for ReviewJoin {
    type Output = JoinDecision;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row=sqlx::query("SELECT j.user_id,j.organization_id,j.status::text AS join_status,u.organization_id AS current_organization FROM organization_join_requests j LEFT JOIN users u ON u.id=j.user_id WHERE j.id=$1")
                .bind(self.join_request_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let user: Uuid = row.get("user_id");
            let organization: Uuid = row.get("organization_id");
            let current: Option<Uuid> = row.get("current_organization");
            Ok(OrganizationWritePlan::new(
                [Some(organization), current].into_iter().flatten(),
                [user],
                [self.join_request_id],
                json!({"user":user,"organization":organization,"current":current,"status":row.get::<String,_>("join_status")}),
            ))
        })
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError> {
        Box::pin(async move {
            require_admin(actor)?;
            let row=sqlx::query("SELECT user_id,organization_id,status::text AS status FROM organization_join_requests WHERE id=$1")
                .bind(self.join_request_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            if row.get::<String, _>("status") != "PENDING" {
                return Err(conflict(
                    "ORGANIZATION_JOIN_ALREADY_REVIEWED",
                    "The organization join request was already reviewed.",
                ));
            }
            let user: Uuid = row.get("user_id");
            let organization: Uuid = row.get("organization_id");
            let mut membership_changed = false;
            if self.approve {
                let row=sqlx::query("SELECT u.status::text AS status,u.email_verified,u.role::text AS role,u.organization_id,o.type::text AS organization_type,o.verification_status FROM users u CROSS JOIN organizations o WHERE u.id=$1 AND o.id=$2")
                    .bind(user).bind(organization).fetch_optional(&mut *conn).await?
                    .ok_or_else(||conflict("ORGANIZATION_JOIN_TARGET_UNAVAILABLE","The membership target is not available."))?;
                if row.get::<String, _>("status") != "APPROVED"
                    || !row.get::<bool, _>("email_verified")
                {
                    return Err(conflict(
                        "ORGANIZATION_JOIN_USER_INELIGIBLE",
                        "Account approval and verified email are required.",
                    ));
                }
                if row.get::<String, _>("verification_status") != "APPROVED" {
                    return Err(conflict(
                        "ORGANIZATION_JOIN_ORG_INELIGIBLE",
                        "Organization approval is required.",
                    ));
                }
                let role: Option<String> = row.get("role");
                if !role_matches_organization(
                    role.as_deref(),
                    &row.get::<String, _>("organization_type"),
                ) {
                    return Err(conflict(
                        "ORGANIZATION_JOIN_ROLE_MISMATCH",
                        "The account role does not match the organization.",
                    ));
                }
                let current: Option<Uuid> = row.get("organization_id");
                if current.is_some_and(|id| id != organization) {
                    return Err(conflict(
                        "ORGANIZATION_JOIN_MEMBERSHIP_CONFLICT",
                        "The user belongs to another organization.",
                    ));
                }
                membership_changed = current != Some(organization);
                if membership_changed {
                    invalidate_review(
                        conn,
                        user,
                        actor.id,
                        "organization_membership_changed",
                        self.now,
                    )
                    .await?;
                    sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
                        .bind(user)
                        .bind(organization)
                        .execute(&mut *conn)
                        .await?;
                }
            }
            let status = if self.approve {
                JoinStatus::Approved
            } else {
                JoinStatus::Rejected
            };
            sqlx::query("UPDATE organization_join_requests SET status=CASE WHEN $2 THEN 'APPROVED' ELSE 'REJECTED' END,reviewed_by=$3,reviewed_at=$4,review_note=$5 WHERE id=$1")
                .bind(self.join_request_id).bind(self.approve).bind(actor.id).bind(self.now).bind(self.note.as_str()).execute(&mut *conn).await?;
            let mut changes = json!({"user_id":user,"organization_id":organization,"status":{"from":"PENDING","to":status},"review_note":self.note.as_str()});
            if self.approve {
                changes["membership_changed"] = json!(membership_changed);
            }
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: if self.approve {
                        "organization.join_approved"
                    } else {
                        "organization.join_rejected"
                    }
                    .into(),
                    resource_type: "organization_join_request".into(),
                    resource_id: Some(self.join_request_id.to_string()),
                    changes: Some(changes),
                    ip_address: None,
                    request_id: None,
                    at: self.now,
                },
            )
            .await?;
            Ok(JoinDecision {
                id: self.join_request_id,
                status,
            })
        })
    }
}
