//! E5/E6 registration component. Serving composition owns shared authorities.
use super::email::{AuthMail, MailDispatcher};
use super::email_verification::{auth_json, client_address, database_error};
use crate::auth::{
    error::{AuthCode, AuthFailure},
    limits::SharedLimits,
    password,
    token_bytes::{generate_token, token_digest},
};
use crate::{ApiError, ApiErrorKind};
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use std::{collections::HashSet, net::SocketAddr, sync::Arc};
use uuid::Uuid;

#[derive(Clone)]
pub struct RegistrationSettings {
    pub pool: PgPool,
    pub mail: MailDispatcher,
    pub limits: SharedLimits,
    excluded_domains: Option<Arc<HashSet<String>>>,
}
impl RegistrationSettings {
    pub fn new(pool: PgPool, mail: MailDispatcher) -> Self {
        Self {
            pool,
            mail,
            limits: SharedLimits::default(),
            excluded_domains: None,
        }
    }
    /// Composition supplies both release-owned domain lists. Missing data disables matching.
    pub fn with_domain_lists(
        self,
        public: impl IntoIterator<Item = String>,
        disposable: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            excluded_domains: Some(Arc::new(
                public
                    .into_iter()
                    .chain(disposable)
                    .map(|s| s.to_lowercase())
                    .collect(),
            )),
            ..self
        }
    }
    pub fn organization_domain(&self, email: &str) -> Option<String> {
        let excluded = self.excluded_domains.as_ref()?;
        let domain = email.rsplit_once('@')?.1.trim_matches('.');
        if !domain.contains('.') {
            return None;
        }
        let labels: Vec<_> = domain.split('.').collect();
        if (0..labels.len() - 1).any(|i| excluded.contains(&labels[i..].join("."))) {
            return None;
        }
        Some(domain.to_string())
    }
}
#[derive(Deserialize)]
struct Registration {
    email: String,
    password: String,
    role: String,
    first_name: Option<String>,
    last_name: Option<String>,
    referral_code: Option<String>,
}
#[derive(Deserialize)]
struct Organization {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    tax_id: Option<String>,
    country_code: Option<String>,
}
#[derive(Deserialize)]
struct WithOrganization {
    registration_token: String,
    organization: Organization,
}
fn validation() -> ApiError {
    AuthFailure::new(AuthCode::ValidationFailed).into()
}
fn failure(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError {
        status,
        detail: json!({"code":code,"message":message}),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}
fn invalid_token() -> ApiError {
    failure(
        StatusCode::BAD_REQUEST,
        "REGISTRATION_TOKEN_INVALID",
        "Registration token is invalid or expired",
    )
}
fn conflict() -> ApiError {
    failure(
        StatusCode::CONFLICT,
        "REGISTRATION_CONFLICT",
        "Registration changed; restart registration",
    )
}
fn allowed_kind(kind: &str) -> bool {
    matches!(
        kind,
        "SHIPPING_LINE" | "SHIP_MANAGER" | "FUEL_BUYER" | "CHARTERER" | "FUEL_SUPPLIER"
    )
}
fn matches_role(role: &str, kind: &str) -> bool {
    allowed_kind(kind)
        && match role {
            "BUYER" => kind != "FUEL_SUPPLIER",
            "SUPPLIER" => kind == "FUEL_SUPPLIER",
            _ => false,
        }
}
fn admit(
    settings: &RegistrationSettings,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: &HeaderMap,
    path: &str,
) -> Result<(), ApiError> {
    let client = client_address(peer, headers);
    let now = Utc::now().timestamp() as u64;
    settings
        .limits
        .preauth_check(&client, path, now)
        .map_err(|r| ApiError::from(r.auth_failure()))?;
    settings.limits.check_endpoint(&client, path, now)
}
pub fn router(settings: RegistrationSettings) -> Router {
    Router::new()
        .route("/api/auth/register", post(register_route))
        .route("/api/auth/register/", post(register_route))
        .route("/api/auth/register-with-org", post(with_org_route))
        .route("/api/auth/register-with-org/", post(with_org_route))
        .with_state(settings)
}
async fn register_route(
    State(settings): State<RegistrationSettings>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut input: Registration = auth_json(&headers, body).await?;
    input.email = super::email_verification::email_address(&json!({"email":input.email}))?;
    password::validate_bytes(&input.password).map_err(|_| validation())?;
    if !matches!(input.role.as_str(), "BUYER" | "SUPPLIER")
        || input.referral_code.as_ref().is_some_and(|s| s.len() > 32)
    {
        return Err(validation());
    }
    admit(&settings, peer, &headers, "/api/auth/register")?;
    let hash = password::hash(&input.password)
        .await
        .map_err(|_| validation())?;
    for attempt in 0..3 {
        match register_attempt(&settings, &input, &hash).await {
            Ok(mail) => {
                settings.mail.dispatch(mail);
                return Ok((StatusCode::ACCEPTED, Json(json!({"status":"check_email"}))));
            }
            Err(error)
                if attempt < 2
                    && (crate::auth::lock::WriteConflict::is_write_conflict(&error)
                        || error
                            .as_database_error()
                            .is_some_and(|e| e.is_unique_violation())) =>
            {
                continue;
            }
            Err(error) => return Err(database_error(error)),
        }
    }
    unreachable!("bounded registration attempts")
}
async fn register_attempt(
    settings: &RegistrationSettings,
    input: &Registration,
    hash: &str,
) -> Result<AuthMail, sqlx::Error> {
    let mut tx = settings.pool.begin().await?;
    let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email=$1 FOR UPDATE")
        .bind(&input.email)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_some() {
        tx.commit().await?;
        return Ok(AuthMail::AccountExists {
            to: input.email.clone(),
        });
    }
    let token = generate_token();
    let now = Utc::now();
    let domain = settings.organization_domain(&input.email);
    let org: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id,type::text FROM organizations WHERE domain=$1")
            .bind(domain)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((org_id, kind)) = org.filter(|(_, kind)| matches_role(&input.role, kind)) {
        let id = Uuid::new_v4();
        insert_user(&mut tx, id, input, hash, &token).await?;
        join_and_attribute(&mut tx, id, org_id, input).await?;
        let _ = kind;
    } else {
        sqlx::query(
            "UPDATE pending_registrations SET used_at=$2 WHERE email=$1 AND used_at IS NULL",
        )
        .bind(&input.email)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO pending_registrations(id,token_hash,email,password_hash,first_name,last_name,role,referral_code,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(Uuid::new_v4()).bind(token_digest(&token)).bind(&input.email).bind(hash).bind(&input.first_name).bind(&input.last_name).bind(&input.role).bind(&input.referral_code).bind(now+Duration::minutes(30)).bind(now).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(AuthMail::Verification {
        to: input.email.clone(),
        name: input.first_name.clone().unwrap_or_else(|| "there".into()),
        token,
        pending: None,
    })
}
async fn insert_user(
    conn: &mut PgConnection,
    id: Uuid,
    input: &Registration,
    hash: &str,
    token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO users(id,email,password_hash,first_name,last_name,role,status,email_verified,must_change_password,email_verification_token_hash,email_verification_token_expires_at) VALUES($1,$2,$3,$4,$5,$6,'PENDING',false,false,$7,$8)")
        .bind(id).bind(&input.email).bind(hash).bind(&input.first_name).bind(&input.last_name).bind(&input.role).bind(token_digest(token)).bind(Utc::now()+Duration::hours(24)).execute(conn).await?;
    Ok(())
}
async fn join_and_attribute(
    conn: &mut PgConnection,
    id: Uuid,
    org: Uuid,
    input: &Registration,
) -> Result<(), sqlx::Error> {
    let join = Uuid::new_v4();
    sqlx::query("INSERT INTO organization_join_requests(id,user_id,organization_id,status,created_at) VALUES($1,$2,$3,'PENDING',$4)").bind(join).bind(id).bind(org).bind(Utc::now()).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO user_status_transitions(id,user_id,role,to_status,effective_at,provenance) VALUES($1,$2,$3,'PENDING',$4,'workflow')").bind(Uuid::new_v4()).bind(id).bind(&input.role).bind(Utc::now()).execute(&mut *conn).await?;
    if let Some(code) = &input.referral_code {
        let referrer: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM users WHERE referral_code=$1 FOR UPDATE")
                .bind(code)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some(referrer) = referrer.filter(|referrer| *referrer != id) {
            sqlx::query("UPDATE users SET referred_by_id=$2 WHERE id=$1")
                .bind(id)
                .bind(referrer)
                .execute(&mut *conn)
                .await?;
            sqlx::query("INSERT INTO referrals(id,referrer_id,referred_user_id,referral_code_used,status,created_at) VALUES($1,$2,$3,$4,'SIGNED_UP',$5)").bind(Uuid::new_v4()).bind(referrer).bind(id).bind(code).bind(Utc::now()).execute(&mut *conn).await?;
        }
    }
    for (action, kind, resource, changes) in [
        (
            "USER_REGISTERED",
            "user",
            id,
            json!({"email":input.email,"role":input.role,"organization_id":null,"via":"registration"}),
        ),
        (
            "ORGANIZATION_JOIN_REQUESTED",
            "organization_join_request",
            join,
            json!({"organization_id":org}),
        ),
    ] {
        sqlx::query("INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,changes) VALUES($1,$2,$3,$4,$5,$6)").bind(Uuid::new_v4()).bind(id).bind(action).bind(kind).bind(resource.to_string()).bind(changes).execute(&mut *conn).await?;
    }
    Ok(())
}
#[derive(sqlx::FromRow)]
struct Pending {
    id: Uuid,
    email: String,
    password_hash: String,
    first_name: Option<String>,
    last_name: Option<String>,
    role: Option<String>,
    referral_code: Option<String>,
}
async fn with_org_route(
    State(settings): State<RegistrationSettings>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<Value>, ApiError> {
    let input: WithOrganization = auth_json(&headers, body).await?;
    if !allowed_kind(&input.organization.kind)
        || input.organization.name.trim().is_empty()
        || input
            .organization
            .country_code
            .as_ref()
            .is_some_and(|s| s.len() != 2)
    {
        return Err(validation());
    }
    admit(&settings, peer, &headers, "/api/auth/register-with-org")?;
    if input.registration_token.is_empty() || input.registration_token.len() > 1024 {
        return Err(invalid_token());
    }
    for attempt in 0..3 {
        match with_org_attempt(&settings, &input).await {
            Ok(result) => return result.map(Json),
            Err(error)
                if attempt < 2 && crate::auth::lock::WriteConflict::is_write_conflict(&error) =>
            {
                continue;
            }
            Err(error)
                if error
                    .as_database_error()
                    .is_some_and(|e| e.is_unique_violation()) =>
            {
                return Err(conflict());
            }
            Err(error) => return Err(database_error(error)),
        }
    }
    unreachable!("bounded registration attempts")
}
async fn with_org_attempt(
    settings: &RegistrationSettings,
    input: &WithOrganization,
) -> Result<Result<Value, ApiError>, sqlx::Error> {
    let mut tx = settings.pool.begin().await?;
    let pending:Option<Pending>=sqlx::query_as("SELECT id,email,password_hash,first_name,last_name,role,referral_code FROM pending_registrations WHERE token_hash=$1 AND used_at IS NULL AND expires_at>$2 FOR UPDATE").bind(token_digest(&input.registration_token)).bind(Utc::now()).fetch_optional(&mut *tx).await?;
    let Some(pending) = pending else {
        return Ok(Err(invalid_token()));
    };
    let role = pending.role.unwrap_or_else(|| "BUYER".into());
    if !matches_role(&role, &input.organization.kind) {
        return Ok(Err(validation()));
    }
    let domain = settings.organization_domain(&pending.email);
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organizations WHERE domain=$1)")
            .bind(&domain)
            .fetch_one(&mut *tx)
            .await?;
    if exists {
        return Ok(Err(conflict()));
    }
    let org = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,name,type,domain,tax_id,country_code,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(org).bind(&input.organization.name).bind(&input.organization.kind).bind(domain).bind(&input.organization.tax_id).bind(&input.organization.country_code).bind(Utc::now()).execute(&mut *tx).await?;
    let registration = Registration {
        email: pending.email,
        password: String::new(),
        role,
        first_name: pending.first_name,
        last_name: pending.last_name,
        referral_code: pending.referral_code,
    };
    let verification = generate_token();
    insert_user(
        &mut tx,
        id,
        &registration,
        &pending.password_hash,
        &verification,
    )
    .await?;
    join_and_attribute(&mut tx, id, org, &registration).await?;
    sqlx::query("UPDATE pending_registrations SET used_at=$2 WHERE id=$1")
        .bind(pending.id)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    settings.mail.dispatch(AuthMail::Verification {
        to: registration.email.clone(),
        name: registration
            .first_name
            .clone()
            .unwrap_or_else(|| "there".into()),
        token: verification,
        pending: None,
    });
    Ok(Ok(
        json!({"id":id,"email":registration.email,"first_name":registration.first_name,"last_name":registration.last_name,"role":registration.role,"status":"PENDING","organization_id":null,"referral_code":null,"must_change_password":false,"kyc_status":"PENDING","execution_eligibility":{"eligible":false,"reason":"KYC_REQUIRED","grace_expires_at":null}}),
    ))
}
