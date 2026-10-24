//! Invitation writes: the A05 organization/user insert, initial status,
//! referral and audit, inside one canonical organization transaction.

use std::sync::Arc;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::request::{
    CreateInvitation, INVITATION_TTL_DAYS, InvitationAudit, InvitationListQuery,
    OrganizationDomainPolicy, OrganizationSource,
};
use crate::{
    ApiError,
    auth::{CurrentActor, token_bytes},
    business::{
        audit::{AuditFact, append_audit},
        error::refusal,
        status::{AccountStatus, StatusFact, append_status_transition},
        transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
    },
};

/// `audit_actions.ADMIN_USER_INVITED`, copied exactly from Python's registry.
pub const ADMIN_USER_INVITED: &str = "admin.user_invited";

/// The audit mask Python applies to the invited address: the first character
/// of the local part, `***`, and the domain. An address without a domain
/// masks to `***` entirely.
pub fn mask_email(email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    if domain.is_empty() {
        return "***".into();
    }
    if local.is_empty() {
        return format!("***@{domain}");
    }
    let first = local.chars().next().expect("non-empty local part");
    format!("{first}***@{domain}")
}

fn admin_required() -> AttemptError {
    refusal(
        StatusCode::FORBIDDEN,
        "ADMIN_REQUIRED",
        "Administrator access is required.",
    )
    .into()
}

fn email_conflict() -> AttemptError {
    refusal(
        StatusCode::CONFLICT,
        "EMAIL_ALREADY_REGISTERED",
        "The email address is already registered.",
    )
    .into()
}

fn domain_conflict() -> AttemptError {
    refusal(
        StatusCode::CONFLICT,
        "ORGANIZATION_DOMAIN_CONFLICT",
        "An organization already owns this email domain",
    )
    .into()
}

fn invalid_invitation() -> ApiError {
    // The single refusal for every wrong, used or expired secret.
    refusal(
        StatusCode::BAD_REQUEST,
        "INVITATION_INVALID",
        "This invitation is invalid, expired, or has already been accepted.",
    )
}

/// The role/type matrix: `organization_type_matches_role`.
pub fn organization_type_matches_role(role: &str, kind: &str) -> bool {
    match role {
        "SUPPLIER" => matches!(kind, "FUEL_SUPPLIER" | "FUEL_TRADER"),
        "BUYER" => matches!(
            kind,
            "SHIPPING_LINE" | "SHIP_MANAGER" | "FUEL_BUYER" | "CHARTERER"
        ),
        _ => false,
    }
}

/// The eligible-organization filter: APPROVED and REAL or UNKNOWN.
fn eligible_where() -> &'static str {
    "verification_status='APPROVED' AND provenance IN ('REAL','UNKNOWN')"
}

/// One A04 row.
#[derive(Debug, Serialize)]
pub struct InvitationOrganization {
    pub id: Uuid,
    pub name: String,
    pub domain: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub provenance: String,
}

/// A04's response body: exactly Python's `{items}`. The bounded read's
/// truncation marker travels in the `X-Result-Truncated` header, not the body.
#[derive(Debug, Serialize)]
pub struct InvitationOrganizationResponse {
    pub items: Vec<InvitationOrganization>,
}

pub struct OrganizationPage {
    pub items: Vec<InvitationOrganization>,
    pub truncated: bool,
}

/// A05's response. The raw invitation secret exists only inside
/// `acceptance_url`; the database stores its digest.
#[derive(Debug, Serialize)]
pub struct CreatedInvitation {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    pub organization_name: String,
    pub organization_created: bool,
    pub acceptance_url: String,
    pub expires_at: DateTime<Utc>,
    pub reissued: bool,
}

impl std::fmt::Debug for InvitationAudit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Addresses and correlation ids stay out of logs.
        formatter.write_str("InvitationAudit(REDACTED)")
    }
}

/// A05's write command. One organization/user insert, one initial status, one
/// referral, one audit row, one token rotation: either all commit or none.
pub struct CreateInvitationCommand {
    pub invitation: CreateInvitation,
    pub now: DateTime<Utc>,
    pub audit: InvitationAudit,
    pub domains: Arc<dyn OrganizationDomainPolicy>,
    pub frontend_url: Arc<str>,
}

/// A new invitation secret and its digest. The secret is zeroized on drop.
pub struct InvitationSecret {
    raw: Zeroizing<String>,
    digest: String,
    expires_at: DateTime<Utc>,
}

impl InvitationSecret {
    /// The fragment link: `{frontend}/accept-invite#token=<secret>`.
    pub fn acceptance_url(&self, frontend_url: &str) -> String {
        format!(
            "{}/accept-invite#token={}",
            frontend_url.trim_end_matches('/'),
            self.raw.as_str()
        )
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// The stored form. Exposed for composition and tests; it is not a secret.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

impl std::fmt::Debug for InvitationSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InvitationSecret(REDACTED)")
    }
}

/// Draw a fresh invitation secret. Only this function ever holds the raw value.
pub fn new_invitation_secret(now: DateTime<Utc>) -> InvitationSecret {
    let raw = Zeroizing::new(token_bytes::generate_token());
    let digest = token_bytes::token_digest(&raw);
    InvitationSecret {
        expires_at: now + chrono::Duration::days(INVITATION_TTL_DAYS),
        raw,
        digest,
    }
}

/// The create-or-rotate state the preview pins and execute re-reads.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct InviteeFingerprint {
    invited: bool,
    user: Uuid,
    organization: Option<Uuid>,
    role: Option<String>,
}

impl BusinessCommand for CreateInvitationCommand {
    type Output = CreatedInvitation;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id,organization_id,role::text AS role FROM users WHERE email=$1",
            )
            .bind(&self.invitation.email)
            .fetch_optional(&mut *conn)
            .await?;
            let mut users: Vec<Uuid> = Vec::new();
            let (invited, fingerprint) = match row {
                Some(row) => {
                    let id: Uuid = row.get("id");
                    users.push(id);
                    (
                        true,
                        InviteeFingerprint {
                            invited: true,
                            user: id,
                            organization: row.get("organization_id"),
                            role: row.get("role"),
                        },
                    )
                }
                None => (
                    false,
                    InviteeFingerprint {
                        invited: false,
                        user: Uuid::nil(),
                        organization: None,
                        role: None,
                    },
                ),
            };
            let mut organizations: Vec<Uuid> = Vec::new();
            let organization = match &self.invitation.organization {
                OrganizationSource::Existing(id) => {
                    organizations.push(*id);
                    Some(*id)
                }
                OrganizationSource::New(_) => {
                    if let Some(domain) = self.domains.organization_domain(&self.invitation.email) {
                        let existing: Option<Uuid> =
                            sqlx::query_scalar("SELECT id FROM organizations WHERE domain=$1")
                                .bind(&domain)
                                .fetch_optional(&mut *conn)
                                .await?;
                        if existing.is_some() {
                            return Err(domain_conflict());
                        }
                    }
                    None
                }
            };
            let plan = OrganizationWritePlan::new(
                organizations,
                users,
                [],
                json!({"email": self.invitation.email, "invited": invited,
                       "invitee": fingerprint, "organization": organization,
                       "organization_created": self.invitation.organization.is_new()}),
            );
            Ok(plan)
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, CreatedInvitation, AttemptError> {
        Box::pin(async move {
            if actor.role.as_deref() != Some("ADMIN") {
                return Err(admin_required());
            }
            // The locked re-read the source performs: the administrator row and
            // the invitee by normalized email, both already held by the plan.
            let administrator: Option<Option<String>> =
                sqlx::query_scalar("SELECT referral_code FROM users WHERE id=$1")
                    .bind(actor.id)
                    .fetch_optional(&mut *conn)
                    .await?;
            let Some(mut referral_code) = administrator else {
                return Err(admin_required());
            };
            let invitee = sqlx::query(
                "SELECT id,organization_id,role::text AS role,status::text AS status,email_verified,must_change_password FROM users WHERE email=$1",
            )
            .bind(&self.invitation.email)
            .fetch_optional(&mut *conn)
            .await?;
            let reissued = invitee.is_some();
            let invitation = &self.invitation;
            let role = invitation.role.as_str();
            let (
                organization_id,
                organization_name,
                organization_created,
                provenance,
                kind,
                country_code,
            ) = match &invitation.organization {
                OrganizationSource::Existing(id) => {
                    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
                        "SELECT id,name,type,provenance,country_code,({}) AS eligible FROM organizations WHERE id=$1",
                        eligible_where()
                    )))
                    .bind(id)
                        .fetch_optional(&mut *conn)
                        .await?
                        .ok_or_else(missing_organization)?;
                    if !row.try_get::<bool, _>("eligible").unwrap_or(false) {
                        return Err(missing_organization());
                    }
                    let kind: String = row.get("type");
                    if !organization_type_matches_role(role, &kind) {
                        return Err(type_mismatch());
                    }
                    (
                        row.get("id"),
                        row.get::<String, _>("name"),
                        false,
                        row.get::<String, _>("provenance"),
                        kind,
                        row.get::<Option<String>, _>("country_code"),
                    )
                }
                OrganizationSource::New(organization) => {
                    if invitee.is_some() {
                        return Err(email_conflict());
                    }
                    if !organization_type_matches_role(role, &organization.kind) {
                        return Err(type_mismatch());
                    }
                    let domain = self.domains.organization_domain(&invitation.email);
                    if let Some(domain) = &domain {
                        let existing: Option<Uuid> =
                            sqlx::query_scalar("SELECT id FROM organizations WHERE domain=$1")
                                .bind(domain)
                                .fetch_optional(&mut *conn)
                                .await?;
                        if existing.is_some() {
                            return Err(domain_conflict());
                        }
                    }
                    let id = Uuid::new_v4();
                    sqlx::query(
                            "INSERT INTO organizations (id,name,domain,type,tax_id,country_code,verification_status,provenance,created_at) \
                             VALUES($1,$2,$3,$4,$5,$6,'APPROVED','REAL',$7)",
                        )
                        .bind(id)
                        .bind(&organization.name)
                        .bind(domain)
                        .bind(&organization.kind)
                        .bind(&organization.tax_id)
                        .bind(&organization.country_code)
                        .bind(self.now)
                        .execute(&mut *conn)
                        .await
                        .map_err(integrity_domain_conflict)?;
                    let name = organization.name.clone();
                    let kind = organization.kind.clone();
                    let country = Some(organization.country_code.clone());
                    (id, name, true, "REAL".into(), kind, country)
                }
            };
            let invitee = match invitee {
                Some(row) => {
                    let organization_id_row: Option<Uuid> = row.get("organization_id");
                    let role_row: Option<String> = row.get("role");
                    let eligible = row.get::<String, _>("status") == "APPROVED"
                        && !row.get::<bool, _>("email_verified")
                        && row.get::<bool, _>("must_change_password")
                        && organization_id_row == Some(organization_id)
                        && role_row.as_deref() == Some(role)
                        && matches!(role_row.as_deref(), Some("BUYER") | Some("SUPPLIER"));
                    if !eligible {
                        return Err(email_conflict());
                    }
                    row.get("id")
                }
                None => {
                    if referral_code.as_deref().is_none_or(str::is_empty) {
                        // A bounded code allocation, as Python's ten probes.
                        let mut allocated = None;
                        for candidate in crate::crud::referrals::Candidates::new().take(10) {
                            let taken: Option<i32> =
                                sqlx::query_scalar("SELECT 1 FROM users WHERE referral_code=$1")
                                    .bind(&candidate)
                                    .fetch_optional(&mut *conn)
                                    .await?;
                            if taken.is_none() {
                                sqlx::query("UPDATE users SET referral_code=$1 WHERE id=$2")
                                    .bind(&candidate)
                                    .bind(actor.id)
                                    .execute(&mut *conn)
                                    .await?;
                                allocated = Some(candidate);
                                break;
                            }
                        }
                        let Some(code) = allocated else {
                            return Err(unavailable());
                        };
                        referral_code = Some(code);
                    }
                    let id = Uuid::new_v4();
                    let placeholder = token_bytes::generate_token();
                    let password_hash = crate::auth::password::hash(&placeholder)
                        .await
                        .map_err(|_| unavailable())?;
                    sqlx::query(
                        "INSERT INTO users (id,email,password_hash,first_name,last_name,role,status,organization_id,email_verified,must_change_password,referred_by_id,created_at) \
                         VALUES($1,$2,$3,$4,$5,$6,'APPROVED',$7,false,true,$8,$9)",
                    )
                    .bind(id)
                    .bind(&invitation.email)
                    .bind(&password_hash)
                    .bind(&invitation.first_name)
                    .bind(&invitation.last_name)
                    .bind(role)
                    .bind(organization_id)
                    .bind(actor.id)
                    .bind(self.now)
                    .execute(&mut *conn)
                    .await
                    .map_err(integrity_conflict)?;
                    append_status_transition(
                        conn,
                        &StatusFact {
                            id: Uuid::new_v4(),
                            user_id: id,
                            organization_id: Some(organization_id),
                            role: Some(role.to_owned()),
                            from: None,
                            to: AccountStatus::Approved,
                            at: self.now,
                        },
                    )
                    .await?;
                    if let Some(code) = referral_code.as_deref() {
                        sqlx::query(
                            "INSERT INTO referrals (id,referrer_id,referred_user_id,referral_code_used,status,created_at) \
                             VALUES($1,$2,$3,$4,'SIGNED_UP',$5)",
                        )
                        .bind(Uuid::new_v4())
                        .bind(actor.id)
                        .bind(id)
                        .bind(code)
                        .bind(self.now)
                        .execute(&mut *conn)
                        .await?;
                    }
                    id
                }
            };
            let user_id = invitee;
            let secret = new_invitation_secret(self.now);
            sqlx::query(
                "UPDATE users SET password_reset_token_hash=$1,password_reset_expires=$2 WHERE id=$3",
            )
            .bind(&secret.digest)
            .bind(secret.expires_at)
            .bind(user_id)
            .execute(&mut *conn)
            .await?;
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: ADMIN_USER_INVITED.into(),
                    resource_type: "user".into(),
                    resource_id: Some(user_id.to_string()),
                    changes: Some(json!({
                        "email": mask_email(&invitation.email),
                        "organization_id": organization_id.to_string(),
                        "role": role,
                        "reissued": reissued,
                        "organization_created": organization_created,
                        "organization_type": kind,
                        "organization_country_code": country_code,
                        "organization_provenance": provenance,
                    })),
                    ip_address: self.audit.ip_address.clone(),
                    request_id: self.audit.request_id.clone(),
                    at: self.now,
                },
            )
            .await?;
            Ok(CreatedInvitation {
                organization_name,
                organization_created,
                user_id,
                email: invitation.email.clone(),
                role: role.to_owned(),
                acceptance_url: secret.acceptance_url(&self.frontend_url),
                expires_at: secret.expires_at,
                reissued,
            })
        })
    }
}

fn missing_organization() -> AttemptError {
    refusal(
        StatusCode::NOT_FOUND,
        "ORGANIZATION_NOT_FOUND",
        "Eligible organization not found",
    )
    .into()
}

fn type_mismatch() -> AttemptError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "ORGANIZATION_TYPE_MISMATCH",
        "Organization type does not match the selected account role",
    )
    .into()
}

fn unavailable() -> AttemptError {
    refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        "INVITATION_UNAVAILABLE",
        "Invitation service is temporarily unavailable",
    )
    .into()
}

/// Python maps the unique-violation `IntegrityError` on the shared email and
/// domain columns to a 409 conflict; every other database failure propagates.
fn integrity_conflict(error: sqlx::Error) -> AttemptError {
    match error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
    {
        Some("23505") => email_conflict(),
        _ => AttemptError::Database(error),
    }
}

/// The organization insert's own unique violation is the email-domain
/// conflict, exactly as Python maps the `flush()` failure; the pre-check cannot
/// see a concurrent creator.
fn integrity_domain_conflict(error: sqlx::Error) -> AttemptError {
    match error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
    {
        Some("23505") => domain_conflict(),
        _ => AttemptError::Database(error),
    }
}

/// The A04 list, applied entirely inside PostgreSQL: filter, order, limit+1.
pub async fn list_organizations(
    conn: &mut PgConnection,
    query: &InvitationListQuery,
) -> Result<OrganizationPage, crate::ApiError> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT id,name,domain,type,provenance FROM organizations WHERE {} ORDER BY name,id LIMIT $1 OFFSET $2",
        eligible_where()
    )))
    .bind(query.limit() + 1)
    .bind(query.offset())
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| crate::ApiError::database())?;
    let truncated = rows.len() > query.limit() as usize;
    let items = rows
        .into_iter()
        .take(query.limit() as usize)
        .map(|row| InvitationOrganization {
            id: row.get("id"),
            name: row.get("name"),
            domain: row.get("domain"),
            kind: row.get("type"),
            provenance: row.get("provenance"),
        })
        .collect();
    Ok(OrganizationPage { items, truncated })
}

impl OrganizationSource {
    fn is_new(&self) -> bool {
        matches!(self, Self::New(_))
    }
}

/// The shared invalid-invitation refusal for resolve.
pub fn invitation_invalid() -> ApiError {
    invalid_invitation()
}

/// The digest lookup form every invitation read uses. Exposed for resolve.
pub fn digest_of(token: &str) -> String {
    token_bytes::token_digest(token)
}
