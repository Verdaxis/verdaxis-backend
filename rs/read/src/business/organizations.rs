use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    audit::{AuditFact, append_audit},
    error::refusal,
    membership::{invalidate_review, require_admin},
    transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
};
use crate::{
    auth::CurrentActor,
    order_write::{InvalidateOrganization, invalidate_organization},
};

pub struct ReviewOrganization {
    pub organization_id: Uuid,
    pub approve: bool,
    pub reason: Option<String>,
    pub audit: super::admission::AuditContext,
    pub now: DateTime<Utc>,
    pub policy: Arc<crate::MarketPolicy>,
    pub config: Arc<crate::Config>,
}

#[derive(Debug, Serialize)]
pub struct OrganizationDecision {
    pub organization_id: Uuid,
    pub verification_status: String,
}

fn missing() -> AttemptError {
    refusal(
        StatusCode::NOT_FOUND,
        "ORGANIZATION_NOT_FOUND",
        "The organization was not found.",
    )
    .into()
}

/// Component entry point. Routing supplies verified credentials and the shared
/// quota identity; the ordered owner performs current-account revalidation.
pub async fn review(
    state: &crate::AppState,
    access: crate::auth::VerifiedAccess,
    limits: &crate::auth::limits::SharedLimits,
    client: &str,
    mut command: ReviewOrganization,
) -> Result<OrganizationDecision, crate::ApiError> {
    // Capture the request runtime once; retries retain these exact snapshots.
    command.policy = Arc::clone(&state.policy);
    command.config = Arc::clone(&state.config);
    let template = if command.approve {
        "/api/auth/organization/{organization_id}/approve"
    } else {
        "/api/auth/organization/{organization_id}/reject"
    };
    let access = super::membership::admit(state, access, limits, client, "PUT", template).await?;
    super::transaction::run_organization_write(state, access, command).await
}

impl BusinessCommand for ReviewOrganization {
    type Output = OrganizationDecision;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row=sqlx::query("SELECT verification_status,provenance::text AS provenance FROM organizations WHERE id=$1")
                .bind(self.organization_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let members: Vec<Uuid> =
                sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                    .bind(self.organization_id)
                    .fetch_all(&mut *conn)
                    .await?;
            Ok(OrganizationWritePlan::new(
                [self.organization_id],
                members.iter().copied(),
                [],
                json!({"members":members,"status":row.get::<String,_>("verification_status"),"provenance":row.get::<String,_>("provenance")}),
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
            let row=sqlx::query("SELECT verification_status,provenance::text AS provenance FROM organizations WHERE id=$1")
                .bind(self.organization_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let previous: String = row.get("verification_status");
            let provenance: String = row.get("provenance");
            let status = if self.approve { "APPROVED" } else { "REJECTED" };
            let changed =
                sqlx::query("UPDATE organizations SET verification_status=$2 WHERE id=$1")
                    .bind(self.organization_id)
                    .bind(status)
                    .execute(&mut *conn)
                    .await;
            if let Err(error) = changed {
                if error
                    .as_database_error()
                    .and_then(|error| error.code())
                    .as_deref()
                    == Some("VD001")
                {
                    return Err(refusal(
                        StatusCode::CONFLICT,
                        "ORGANIZATION_APPROVAL_CONFLICT",
                        "Close unresolved orders before approving this organization.",
                    )
                    .into());
                }
                return Err(error.into());
            }
            let mut changes = json!({"verification_status":{"from":previous,"to":status}});
            if self.approve {
                let current: String =
                    sqlx::query_scalar("SELECT provenance::text FROM organizations WHERE id=$1")
                        .bind(self.organization_id)
                        .fetch_one(&mut *conn)
                        .await?;
                if current != provenance {
                    changes["provenance"] = json!({"from":provenance,"to":current});
                }
                if previous != status || current != provenance {
                    // Invalidation is conservative: no identity or economics enter the
                    // public signal. Existing projection caches must refresh on approval.
                    sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload) VALUES($1,'market_admission_changed','organization',$2,$3,$4)")
                        .bind(Uuid::new_v4()).bind(self.organization_id.to_string()).bind(json!([self.organization_id]))
                        .bind(json!({"schema_version":1,"organization_id":self.organization_id,"_public_market_invalidation":true}))
                        .execute(&mut *conn).await?;
                }
            } else {
                let members: Vec<Uuid> =
                    sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 ORDER BY id")
                        .bind(self.organization_id)
                        .fetch_all(&mut *conn)
                        .await?;
                for member in members {
                    invalidate_review(conn, member, actor.id, "organization_rejected", self.now)
                        .await?;
                    sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
                        .bind(member)
                        .execute(&mut *conn)
                        .await?;
                }
                let result = invalidate_organization(
                    conn,
                    &InvalidateOrganization {
                        organization_id: self.organization_id,
                        actor_id: actor.id,
                        now: self.now,
                        policy: Arc::clone(&self.policy),
                        config: Arc::clone(&self.config),
                    },
                )
                .await?;
                changes["market_invalidation_events"] = json!(result.outbox_ids.len());
                changes["reason"] = json!(self.reason);
            }
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: if self.approve {
                        "admin.organization_approved"
                    } else {
                        "admin.organization_rejected"
                    }
                    .into(),
                    resource_type: "organization".into(),
                    resource_id: Some(self.organization_id.to_string()),
                    changes: Some(changes),
                    ip_address: self.audit.ip_address.clone(),
                    request_id: self.audit.request_id.clone(),
                    at: self.now,
                },
            )
            .await?;
            Ok(OrganizationDecision {
                organization_id: self.organization_id,
                verification_status: status.into(),
            })
        })
    }
}
