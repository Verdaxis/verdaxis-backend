use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    audit::{AuditFact, append_audit},
    error::refusal,
    transaction::AttemptError,
};
use crate::{ApiError, auth::CurrentActor};

pub fn require_admin(actor: &CurrentActor) -> Result<(), ApiError> {
    if actor.role.as_deref() == Some("ADMIN") {
        Ok(())
    } else {
        Err(refusal(
            StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        ))
    }
}

pub fn role_matches_organization(role: Option<&str>, organization_type: &str) -> bool {
    match role {
        Some("BUYER") => matches!(
            organization_type,
            "SHIPPING_LINE" | "SHIP_MANAGER" | "FUEL_BUYER" | "CHARTERER"
        ),
        Some("SUPPLIER") => matches!(organization_type, "FUEL_SUPPLIER" | "FUEL_TRADER"),
        _ => false,
    }
}

/// This is a membership transition, not execution admission. It never evaluates
/// the KYC switch, grants eligibility, or changes retained evidence or grace.
pub async fn invalidate_review(
    conn: &mut PgConnection,
    user_id: Uuid,
    actor_id: Uuid,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<bool, AttemptError> {
    let row = sqlx::query("SELECT kyc_status,kyc_organization_id FROM users WHERE id=$1")
        .bind(user_id)
        .fetch_one(&mut *conn)
        .await?;
    let previous: Option<String> = row.get("kyc_status");
    let organization: Option<Uuid> = row.get("kyc_organization_id");
    // The shared contract's persisted domain is closed, even in advisory mode.
    if !matches!(
        previous.as_deref(),
        Some("PENDING" | "SUBMITTED" | "APPROVED" | "REJECTED")
    ) {
        return Err(refusal(
            StatusCode::CONFLICT,
            "KYC_STATE_INVALID",
            "The retained review state requires operator review.",
        )
        .into());
    }
    if previous.as_deref() == Some("PENDING") && organization.is_none() {
        return Ok(false);
    }
    sqlx::query("UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL,kyc_rejection_reason=NULL,kyc_external_evidence_reference=NULL,kyc_review_note=NULL,kyc_reviewed_by=NULL,kyc_reviewed_at=NULL WHERE id=$1")
        .bind(user_id).execute(&mut *conn).await?;
    append_audit(conn,&AuditFact {
        id: Uuid::new_v4(), actor_id: Some(actor_id), action: "kyc.membership_invalidated".into(),
        resource_type: "kyc".into(), resource_id: Some(user_id.to_string()),
        changes: Some(json!({"kyc_status":{"from":previous,"to":"PENDING"},"kyc_organization_id":{"from":organization,"to":null},"reason":reason})),
        ip_address: None, request_id: None, at: now,
    }).await?;
    Ok(true)
}

/// One admission path for every organization-review component. This read-only
/// phase holds no row locks; the ordered write owner still revalidates authority
/// under canonical locks before mutation. Quota is charged once, not per retry.
pub async fn admit(
    state: &crate::AppState,
    access: crate::auth::VerifiedAccess,
    limits: &crate::auth::limits::SharedLimits,
    client: &str,
    method: &'static str,
    template: &'static str,
) -> Result<crate::auth::VerifiedAccess, ApiError> {
    let access=crate::runtime::admission::run_admitted(state,move |conn|Box::pin(async move {
        let row=sqlx::query("SELECT status::text AS status,must_change_password,password_changed_at,authentication_revoked_at,role::text AS role FROM users WHERE id=$1")
            .bind(access.subject).fetch_optional(conn).await.map_err(|_|super::error::database_failure(None))?
            .ok_or_else(||super::error::authentication(crate::auth::AuthError::Credentials))?;
        let password:Option<DateTime<Utc>>=row.get("password_changed_at");
        let revoked:Option<DateTime<Utc>>=row.get("authentication_revoked_at");
        crate::auth::validate_account(&access,&crate::auth::AccountState {
            exists:true,status:row.get("status"),must_change_password:row.get("must_change_password"),
            password_changed_at:password.map(|v|v.timestamp_micros()),authentication_revoked_at:revoked.map(|v|v.timestamp_micros()),
        }).map_err(super::error::authentication)?;
        if row.get::<Option<String>,_>("role").as_deref()!=Some("ADMIN") {
            return Err(refusal(StatusCode::FORBIDDEN,"ADMIN_REQUIRED","Administrator access is required."));
        }
        Ok(access)
    })).await?;
    limits.check_business(
        method,
        template,
        crate::auth::limits::BusinessLimitKeys {
            client,
            user: None,
            organization: None,
            token: None,
        },
        Utc::now().timestamp().max(0) as u64,
    )?;
    Ok(access)
}
