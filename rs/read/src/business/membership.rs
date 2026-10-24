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
