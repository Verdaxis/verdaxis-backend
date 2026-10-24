use std::sync::Arc;

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    keyring::KeyRing,
    vault::{self, DocumentKind, DocumentMetadata, EncryptedDocument},
};
use crate::{
    ApiError,
    auth::CurrentActor,
    business::{
        audit::{AuditFact, append_audit},
        error::refusal,
        transaction::{AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan},
    },
};

/// Integration supplies one shared quota authority and startup-loaded keys.
/// These handlers are components only; this module mounts no serving route.
#[derive(Clone)]
pub struct EvidenceContext {
    pub settings: Arc<crate::auth::VerificationSettings>,
    pub limits: crate::auth::limits::SharedLimits,
    pub keys: Option<Arc<KeyRing>>,
    pub upload_limits: super::submission::UploadLimits,
}

pub(crate) fn map_authenticated(error: crate::auth::AuthenticatedError) -> ApiError {
    match error {
        crate::auth::AuthenticatedError::Authentication(error) => {
            crate::business::error::authentication(error)
        }
        crate::auth::AuthenticatedError::Database(error) => error,
        crate::auth::AuthenticatedError::AmbiguousCommit => crate::business::error::refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "TRANSACTION_OUTCOME_UNKNOWN",
            "The transaction outcome could not be confirmed.",
        ),
    }
}

pub(crate) fn authorize(
    headers: &axum::http::HeaderMap,
    context: &EvidenceContext,
    now: DateTime<Utc>,
) -> Result<crate::auth::VerifiedAccess, ApiError> {
    if crate::auth::scope::support_present(headers) {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "SUPPORT_CONTEXT_FORBIDDEN",
            "This operation does not accept support authority.",
        ));
    }
    crate::auth::verify_authorization(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        &context.settings,
        now,
    )
    .map_err(crate::business::error::authentication)
}

pub(crate) fn quota(
    context: &EvidenceContext,
    method: &str,
    path: &str,
    client: &str,
    user: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let user = user.to_string();
    context.limits.check_business(
        method,
        path,
        crate::auth::limits::BusinessLimitKeys {
            client,
            user: Some(&user),
            organization: None,
            token: None,
        },
        now.timestamp().max(0) as u64,
    )
}

pub async fn handle_status(
    state: &crate::AppState,
    context: Arc<EvidenceContext>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<KycStatus>, ApiError> {
    let access = authorize(&headers, &context, Utc::now())?;
    crate::auth::run_authenticated(
        state,
        access,
        crate::auth::AuthenticatedRequest::new("GET", "/api/kyc/status", false),
        move |tx| {
            Box::pin(async move {
                let actor = tx.actor().clone();
                status(tx.connection(), &actor)
                    .await
                    .map(axum::Json)
                    .map_err(|_| crate::business::error::database_failure(None))
            })
        },
    )
    .await
    .map_err(map_authenticated)
}

pub async fn handle_list(
    state: &crate::AppState,
    context: Arc<EvidenceContext>,
    headers: axum::http::HeaderMap,
    client: String,
    user: &str,
) -> Result<axum::Json<Vec<DocumentSummary>>, ApiError> {
    let now = Utc::now();
    let access = authorize(&headers, &context, now)?;
    let user = Uuid::parse_str(user).map_err(|_| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_IDENTIFIER",
            "The identifier is invalid.",
        )
    })?;
    crate::auth::run_authenticated(
        state,
        access,
        crate::auth::AuthenticatedRequest::new("GET", "/api/kyc/admin/{user_id}/documents", false),
        move |tx| {
            Box::pin(async move {
                let actor = tx.actor().clone();
                admin(&actor)?;
                quota(
                    &context,
                    "GET",
                    "/api/kyc/admin/{user_id}/documents",
                    &client,
                    actor.id,
                    now,
                )?;
                list(tx.connection(), &actor, user)
                    .await
                    .map(axum::Json)
                    .map_err(|error| match error {
                        AttemptError::Application(error) => error,
                        _ => crate::business::error::database_failure(None),
                    })
            })
        },
    )
    .await
    .map_err(map_authenticated)
}

pub async fn handle_download(
    state: &crate::AppState,
    context: Arc<EvidenceContext>,
    headers: axum::http::HeaderMap,
    client: String,
    id: &str,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let access = authorize(&headers, &context, now)?;
    let user = access.subject;
    let id = Uuid::parse_str(id).map_err(|_| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_IDENTIFIER",
            "The identifier is invalid.",
        )
    })?;
    let check_context = context.clone();
    crate::auth::run_authenticated(
        state,
        access,
        crate::auth::AuthenticatedRequest::new(
            "GET",
            "/api/kyc/admin/documents/{document_id}",
            false,
        ),
        move |tx| {
            Box::pin(async move {
                admin(tx.actor())?;
                quota(
                    &check_context,
                    "GET",
                    "/api/kyc/admin/documents/{document_id}",
                    &client,
                    user,
                    now,
                )
            })
        },
    )
    .await
    .map_err(map_authenticated)?;
    let access = authorize(&headers, &context, now)?;
    crate::business::transaction::run_organization_write(
        state,
        access,
        DownloadCommand {
            id,
            keys: context.keys.clone(),
            now,
        },
    )
    .await
    .map(Download::into_response)
}

#[derive(Serialize)]
pub struct Submitted {
    pub kyc_status: String,
    pub message: String,
}
#[derive(Serialize)]
pub struct KycStatus {
    pub email_verified: bool,
    pub kyc_status: String,
    pub kyc_organization_id: Option<Uuid>,
    pub kyc_rejection_reason: Option<String>,
}
#[derive(Serialize)]
pub struct DocumentSummary {
    pub id: Uuid,
    pub submission_id: Uuid,
    pub kind: String,
    pub content_type: String,
    pub size_bytes: i32,
    pub organization_id: Uuid,
    pub created_at: DateTime<Utc>,
}

fn admin(actor: &CurrentActor) -> Result<(), ApiError> {
    if actor.role.as_deref() != Some("ADMIN") {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "ADMIN_REQUIRED",
            "Administrator access is required.",
        ));
    }
    Ok(())
}
fn missing() -> ApiError {
    refusal(
        StatusCode::NOT_FOUND,
        "KYC_DOCUMENT_NOT_FOUND",
        "The requested evidence is unavailable.",
    )
}
fn decryption_failure() -> ApiError {
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        "KYC_DOCUMENT_DECRYPTION_FAILED",
        "The retained evidence cannot be decrypted.",
    )
}

pub async fn status(
    conn: &mut PgConnection,
    actor: &CurrentActor,
) -> Result<KycStatus, sqlx::Error> {
    let row=sqlx::query("SELECT email_verified,kyc_status,kyc_organization_id,kyc_rejection_reason FROM users WHERE id=$1").bind(actor.id).fetch_one(conn).await?;
    Ok(KycStatus {
        email_verified: row.get("email_verified"),
        kyc_status: row.get("kyc_status"),
        kyc_organization_id: row.get("kyc_organization_id"),
        kyc_rejection_reason: row.get("kyc_rejection_reason"),
    })
}

pub async fn list(
    conn: &mut PgConnection,
    actor: &CurrentActor,
    user: Uuid,
) -> Result<Vec<DocumentSummary>, AttemptError> {
    admin(actor)?;
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(user)
        .fetch_one(&mut *conn)
        .await?
    {
        return Err(refusal(
            StatusCode::NOT_FOUND,
            "USER_NOT_FOUND",
            "The requested account is unavailable.",
        )
        .into());
    }
    let rows=sqlx::query("SELECT id,submission_id,kind,content_type,size_bytes,organization_id,created_at FROM kyc_documents WHERE user_id=$1 ORDER BY created_at DESC,id DESC LIMIT 50").bind(user).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|row| DocumentSummary {
            id: row.get("id"),
            submission_id: row.get("submission_id"),
            kind: row.get("kind"),
            content_type: row.get("content_type"),
            size_bytes: row.get("size_bytes"),
            organization_id: row.get("organization_id"),
            created_at: row.get("created_at"),
        })
        .collect())
}

pub(crate) async fn insert(
    conn: &mut PgConnection,
    row: &EncryptedDocument,
) -> Result<(), sqlx::Error> {
    let m = &row.metadata;
    sqlx::query("INSERT INTO kyc_documents(id,submission_id,user_id,organization_id,kind,content_type,size_bytes,key_id,wrapped_key,key_nonce,data_nonce,ciphertext) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(m.id).bind(m.submission_id).bind(m.user_id).bind(m.organization_id).bind(m.kind.as_str()).bind(&m.content_type).bind(row.size_bytes).bind(&row.key_id).bind(&row.wrapped_key).bind(&row.key_nonce).bind(&row.data_nonce).bind(&row.ciphertext).execute(conn).await?;
    Ok(())
}

/// Only a successful transaction owner may return this value to a caller.
/// Plaintext is private and remains zeroizing until moved into the response.
pub struct Download {
    content: Zeroizing<Vec<u8>>,
    mime: String,
    disposition: String,
}
impl Download {
    pub fn into_response(mut self) -> Response {
        let bytes = std::mem::take(&mut *self.content);
        let mut response = Body::from(bytes).into_response();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            self.mime.parse().expect("closed document MIME"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            self.disposition.parse().expect("generated attachment name"),
        );
        headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        headers.insert(
            "x-content-type-options",
            header::HeaderValue::from_static("nosniff"),
        );
        response
    }
}

pub struct DownloadCommand {
    id: Uuid,
    keys: Option<Arc<KeyRing>>,
    now: DateTime<Utc>,
}
impl DownloadCommand {
    pub fn new(id: Uuid, keys: Arc<KeyRing>, now: DateTime<Utc>) -> Self {
        Self {
            id,
            keys: Some(keys),
            now,
        }
    }
    pub fn without_keys(id: Uuid, now: DateTime<Utc>) -> Self {
        Self {
            id,
            keys: None,
            now,
        }
    }
}
impl BusinessCommand for DownloadCommand {
    type Output = Download;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row = sqlx::query("SELECT user_id,organization_id FROM kyc_documents WHERE id=$1")
                .bind(self.id)
                .fetch_optional(conn)
                .await?
                .ok_or_else(missing)?;
            let user: Uuid = row.get("user_id");
            let org: Uuid = row.get("organization_id");
            Ok(OrganizationWritePlan::new(
                [org],
                [user],
                [],
                json!({"document":self.id,"user":user,"organization":org}),
            ))
        })
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Download, AttemptError> {
        Box::pin(async move {
            admin(actor)?;
            let row=sqlx::query("SELECT id,submission_id,user_id,organization_id,kind,content_type,size_bytes,key_id,wrapped_key,key_nonce,data_nonce,ciphertext FROM kyc_documents WHERE id=$1").bind(self.id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let encrypted = EncryptedDocument {
                metadata: DocumentMetadata {
                    id: row.get("id"),
                    submission_id: row.get("submission_id"),
                    user_id: row.get("user_id"),
                    organization_id: row.get("organization_id"),
                    kind: DocumentKind::from_storage(row.get("kind"))
                        .map_err(|_| decryption_failure())?,
                    content_type: row.get("content_type"),
                },
                size_bytes: row.get("size_bytes"),
                key_id: row.get("key_id"),
                wrapped_key: row.get("wrapped_key"),
                key_nonce: row.get("key_nonce"),
                data_nonce: row.get("data_nonce"),
                ciphertext: row.get("ciphertext"),
            };
            let content = vault::decrypt(
                self.keys.as_deref().ok_or_else(decryption_failure)?,
                &encrypted,
            )
            .map_err(|_| decryption_failure())?;
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: "kyc.document_viewed".into(),
                    resource_type: "kyc_document".into(),
                    resource_id: Some(self.id.to_string()),
                    changes: Some(
                        json!({"document_id":self.id,"user_id":encrypted.metadata.user_id}),
                    ),
                    ip_address: None,
                    request_id: None,
                    at: self.now,
                },
            )
            .await?;
            let extension = match encrypted.metadata.content_type.as_str() {
                "application/pdf" => "pdf",
                "image/jpeg" => "jpg",
                "image/png" => "png",
                _ => return Err(decryption_failure().into()),
            };
            Ok(Download {
                content,
                mime: encrypted.metadata.content_type,
                disposition: format!(
                    "attachment; filename=\"kyc-{}-{}.{extension}\"",
                    encrypted.metadata.kind.as_str(),
                    self.id
                ),
            })
        })
    }
}
