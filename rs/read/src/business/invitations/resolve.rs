//! A06: read-only invitation resolution.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{request::ResolveToken, store};
use crate::ApiError;

/// A06's response body.
#[derive(Debug, Serialize)]
pub struct ResolvedInvitation {
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub role: String,
    pub organization_name: String,
    pub invited_by_name: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// Resolve an invitation secret without consuming it.
///
/// The lookup is one equality on the digest with a strict expiry bound, exactly
/// as the source; a wrong, used or expired secret produces the same single
/// refusal, and no response or log names the secret or its digest.
pub async fn resolve(
    conn: &mut PgConnection,
    token: &ResolveToken,
    now: DateTime<Utc>,
) -> Result<ResolvedInvitation, ApiError> {
    let digest = store::digest_of(token.as_str());
    let row = sqlx::query(
        "SELECT u.id,u.email,u.first_name,u.last_name,u.role::text AS role,u.status::text AS status, \
         u.email_verified,u.must_change_password,u.organization_id,u.referred_by_id, \
         u.password_reset_expires \
         FROM users u WHERE u.password_reset_token_hash=$1 AND u.password_reset_expires > $2",
    )
    .bind(&digest)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| ApiError::database())?;
    let Some(row) = row else {
        return Err(store::invitation_invalid());
    };
    let organization_id: Option<Uuid> = row.get("organization_id");
    if row.get::<String, _>("status") != "APPROVED"
        || row.get::<bool, _>("email_verified")
        || !row.get::<bool, _>("must_change_password")
        || organization_id.is_none()
        || !matches!(row.get::<Option<String>, _>("role").as_deref(), Some("BUYER" | "SUPPLIER"))
    {
        return Err(store::invitation_invalid());
    }
    let organization_id = organization_id.expect("checked above");
    let organization = sqlx::query(
        "SELECT name,type,provenance,verification_status FROM organizations WHERE id=$1",
    )
    .bind(organization_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| ApiError::database())?;
    let Some(organization) = organization else {
        return Err(store::invitation_invalid());
    };
    if organization.get::<String, _>("verification_status") != "APPROVED"
        || !matches!(
            organization.get::<String, _>("provenance").as_str(),
            "REAL" | "UNKNOWN"
        )
    {
        return Err(store::invitation_invalid());
    }
    let role = row.get::<Option<String>, _>("role").expect("checked above");
    if !store::organization_type_matches_role(&role, &organization.get::<String, _>("type")) {
        return Err(store::invitation_invalid());
    }
    let invited_by_name = match row.get::<Option<Uuid>, _>("referred_by_id") {
        Some(inviter) => {
            let inviter = sqlx::query("SELECT first_name,last_name FROM users WHERE id=$1")
                .bind(inviter)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|_| ApiError::database())?;
            inviter.and_then(|inviter| {
                let first: Option<String> = inviter.get("first_name");
                let last: Option<String> = inviter.get("last_name");
                let name = [first, last]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                (!name.is_empty()).then_some(name)
            })
        }
        None => None,
    };
    Ok(ResolvedInvitation {
        email: row.get("email"),
        first_name: row.get("first_name"),
        last_name: row.get("last_name"),
        role,
        organization_name: organization.get("name"),
        invited_by_name,
        expires_at: row.get("password_reset_expires"),
    })
}

/// The single refusal shape, for tests that pin the no-oracle contract.
pub fn invalid_refusal_shape() -> (StatusCode, &'static str) {
    (StatusCode::BAD_REQUEST, "INVITATION_INVALID")
}
