use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::auth::{CurrentActor, store::revoke_user_authentication};
use super::{
    audit::{AuditFact, append_audit},
    error::refusal,
    status::{AccountStatus, StatusFact, append_status_transition},
    transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
};

/// The route integration supplies the shared public projection and event producer.
/// Neither method may commit or perform provider I/O.
pub trait AdmissionEvents: Send + Sync + 'static {
    fn visible<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid) -> CommandFuture<'a, bool, AttemptError>;
    fn append<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid, organization: Uuid, public: bool) -> CommandFuture<'a, (), AttemptError>;
}

pub struct RejectAccount<E> {
    pub user_id: Uuid,
    pub now: DateTime<Utc>,
    pub reason: String,
    pub events: E,
}

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
    refusal(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "The user was not found.").into()
}

impl<E: AdmissionEvents> BusinessCommand for RejectAccount<E> {
    type Output = RejectionResult;

    fn preview<'a>(&'a self, conn: &'a mut PgConnection) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row = sqlx::query("SELECT organization_id,role::text AS role FROM users WHERE id=$1")
                .bind(self.user_id).fetch_optional(conn).await?.ok_or_else(missing)?;
            let organization: Option<Uuid> = row.get("organization_id");
            Ok(OrganizationWritePlan::new(organization, [self.user_id], [], json!({"organization":organization,"role":row.get::<Option<String>,_>("role")})))
        })
    }

    fn execute<'a>(&'a self, conn: &'a mut PgConnection, actor: &'a CurrentActor) -> CommandFuture<'a, RejectionResult, AttemptError> {
        Box::pin(async move {
            if actor.role.as_deref() != Some("ADMIN") {
                return Err(refusal(StatusCode::FORBIDDEN, "ADMIN_REQUIRED", "Administrator access is required.").into());
            }
            let row = sqlx::query("SELECT status::text AS status,role::text AS role,organization_id FROM users WHERE id=$1")
                .bind(self.user_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let role: Option<String> = row.get("role");
            if role.as_deref() == Some("ADMIN") { return Err(missing()); }
            let previous = match row.get::<String,_>("status").as_str() {
                "PENDING" => AccountStatus::Pending,
                "APPROVED" => AccountStatus::Approved,
                "REJECTED" => AccountStatus::Rejected,
                _ => return Err(refusal(StatusCode::CONFLICT, "ACCOUNT_STATE_INVALID", "The account state cannot be changed.").into()),
            };
            let organization: Option<Uuid> = row.get("organization_id");
            let before = self.events.visible(conn, self.user_id).await?;
            sqlx::query("UPDATE users SET status='REJECTED',pending_approval_email_transition_id=NULL,pending_approval_email_payload=NULL,pending_approval_email_retry_at=NULL WHERE id=$1")
                .bind(self.user_id).execute(&mut *conn).await?;
            let cutoff = revoke_user_authentication(conn, self.user_id, self.now).await.map_err(revocation_failure)?;
            append_status_transition(conn, &StatusFact {
                id: Uuid::new_v4(), user_id: self.user_id, organization_id: organization,
                role, from: Some(previous), to: AccountStatus::Rejected, at: self.now,
            }).await?;
            let after = self.events.visible(conn, self.user_id).await?;
            if previous != AccountStatus::Rejected {
                if let Some(organization) = organization {
                    self.events.append(conn, self.user_id, organization, before || after).await?;
                }
            }
            append_audit(conn, &AuditFact {
                id: Uuid::new_v4(), actor_id: Some(actor.id), action: "admin_user_rejected".into(),
                resource_type: "user".into(), resource_id: Some(self.user_id.to_string()),
                changes: Some(json!({"status":{"from":previous.as_str(),"to":"REJECTED"},"reason":self.reason})),
                ip_address: None, request_id: None, at: self.now,
            }).await?;
            Ok(RejectionResult { user_id: self.user_id, cutoff })
        })
    }
}
