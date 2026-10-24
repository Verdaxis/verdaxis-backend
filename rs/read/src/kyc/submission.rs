use std::{fmt, sync::Arc};

use axum::{body::Body, http::StatusCode};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    documents::Submitted,
    keyring::KeyRing,
    vault::{self, DocumentKind, DocumentMetadata},
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

const MULTIPART_OVERHEAD_BYTES: usize = 64 * 1024;
const MAX_PARTS: usize = 4;

#[derive(Clone, Copy)]
pub struct UploadLimits {
    pub file_bytes: usize,
    pub total_bytes: usize,
}
impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            file_bytes: 10 * 1024 * 1024,
            total_bytes: 20 * 1024 * 1024,
        }
    }
}

fn invalid() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "KYC_INVALID_DECLARATION",
        "The evidence declarations are invalid.",
    )
}
fn oversized() -> ApiError {
    refusal(
        StatusCode::PAYLOAD_TOO_LARGE,
        "KYC_BODY_TOO_LARGE",
        "The evidence exceeds the configured limit.",
    )
}
fn malformed() -> ApiError {
    refusal(
        StatusCode::BAD_REQUEST,
        "KYC_INVALID_MULTIPART",
        "The multipart evidence is invalid.",
    )
}
fn unsupported() -> ApiError {
    refusal(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "KYC_DOCUMENT_TYPE_UNSUPPORTED",
        "Evidence must be JPEG, PNG or PDF with the declared signature.",
    )
}

pub struct Declarations {
    company: Option<String>,
    registration: Option<String>,
}
impl fmt::Debug for Declarations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Declarations([REDACTED])")
    }
}

fn identity(value: &str) -> String {
    // Python compares `" ".join(value.split()).casefold()`: split on Unicode
    // whitespace, rejoin with single spaces, then full default (non-Turkic)
    // case folding. ICU4X owns the folding tables; no exceptions are kept here.
    let whitespace_normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    icu_casemap::CaseMapper::new()
        .fold_string(&whitespace_normalized)
        .into_owned()
}

impl Declarations {
    pub fn new(company: Option<String>, registration: Option<String>) -> Result<Self, ApiError> {
        if company.as_ref().is_some_and(|s| s.chars().count() > 200)
            || registration
                .as_ref()
                .is_some_and(|s| s.chars().count() > 100)
        {
            return Err(invalid());
        }
        let company = company.map(|s| s.trim().to_owned());
        let registration = registration.map(|s| s.trim().to_owned());
        if company.as_ref().is_some_and(|s| s.is_empty()) {
            return Err(invalid());
        }
        if let Some(value) = &registration
            && (value.len() < 2
                || !value.as_bytes()[0].is_ascii_alphanumeric()
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b" ./_-".contains(&b)))
        {
            return Err(invalid());
        }
        Ok(Self {
            company,
            registration,
        })
    }
    pub fn validate_organization(&self, name: &str, tax_id: Option<&str>) -> Result<(), ApiError> {
        if self
            .company
            .as_ref()
            .is_some_and(|s| identity(s) != identity(name))
        {
            return Err(invalid());
        }
        if let (Some(registration), Some(tax)) =
            (&self.registration, tax_id.filter(|s| !s.is_empty()))
            && identity(registration) != identity(tax)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub struct UploadedDocument {
    pub(crate) content: Zeroizing<Vec<u8>>,
    pub(crate) mime: String,
}
pub struct Submission {
    documents: [UploadedDocument; 2],
    declarations: Declarations,
}
impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Submission([REDACTED])")
    }
}

/// Multer owns grammar and part headers. Every body byte is counted independently
/// of Content-Length; no filename is retained and no parser error is exposed.
pub async fn parse_multipart(
    body: Body,
    content_type: &str,
    limits: UploadLimits,
) -> Result<Submission, ApiError> {
    if limits.file_bytes == 0
        || limits.total_bytes == 0
        || limits.file_bytes > i32::MAX as usize
        || limits.total_bytes > i32::MAX as usize
    {
        return Err(oversized());
    }
    let boundary = multer::parse_boundary(content_type).map_err(|_| malformed())?;
    if boundary.is_empty() || boundary.len() > 70 {
        return Err(malformed());
    }
    let max_body = limits
        .total_bytes
        .checked_add(MULTIPART_OVERHEAD_BYTES)
        .ok_or_else(oversized)?;
    let mut received = 0usize;
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(|_| std::io::Error::other("invalid evidence stream"))?;
        received = received
            .checked_add(chunk.len())
            .ok_or_else(|| std::io::Error::other("evidence limit"))?;
        if received > max_body {
            return Err(std::io::Error::other("evidence limit"));
        }
        Ok(chunk)
    });
    let constraints = multer::Constraints::new()
        .allowed_fields(vec![
            "passport",
            "company_doc",
            "declared_company_name",
            "registration_number",
        ])
        .size_limit(
            multer::SizeLimit::new()
                .whole_stream(max_body as u64)
                .per_field(limits.file_bytes.max(1024) as u64),
        );
    futures_util::pin_mut!(stream);
    let mut multipart = multer::Multipart::with_constraints(&mut stream, boundary, constraints);
    let mut documents: [Option<UploadedDocument>; 2] = [None, None];
    let mut company = None;
    let mut registration = None;
    let mut count = 0;
    let mut total = 0usize;
    while let Some(mut field) = multipart.next_field().await.map_err(parser_error)? {
        count += 1;
        if count > MAX_PARTS {
            return Err(malformed());
        }
        let name = field.name().ok_or_else(malformed)?.to_owned();
        let index = match name.as_str() {
            "passport" => Some(0),
            "company_doc" => Some(1),
            _ => None,
        };
        let mime = if index.is_some() {
            let supplied = field
                .content_type()
                .map(|m| m.essence_str())
                .ok_or_else(unsupported)?;
            match supplied {
                "application/pdf" | "application/x-pdf" => "application/pdf",
                "image/jpeg" | "image/jpg" => "image/jpeg",
                "image/png" => "image/png",
                _ => return Err(unsupported()),
            }
            .to_owned()
        } else {
            String::new()
        };
        let bound = if index.is_some() {
            limits.file_bytes
        } else {
            1024
        };
        let mut content = Zeroizing::new(Vec::new());
        while let Some(chunk) = field.chunk().await.map_err(parser_error)? {
            if content
                .len()
                .checked_add(chunk.len())
                .is_none_or(|n| n > bound)
            {
                return Err(if index.is_some() {
                    oversized()
                } else {
                    invalid()
                });
            }
            content.extend_from_slice(&chunk);
        }
        if let Some(index) = index {
            if documents[index].is_some() {
                return Err(malformed());
            }
            total = total.checked_add(content.len()).ok_or_else(oversized)?;
            if total > limits.total_bytes {
                return Err(oversized());
            }
            if content.is_empty() {
                return Err(refusal(
                    StatusCode::BAD_REQUEST,
                    "KYC_DOCUMENT_EMPTY",
                    "Evidence cannot be empty.",
                ));
            }
            let matches = match mime.as_str() {
                "application/pdf" => content.starts_with(b"%PDF-"),
                "image/jpeg" => content.starts_with(&[0xff, 0xd8, 0xff]),
                "image/png" => content.starts_with(b"\x89PNG\r\n\x1a\n"),
                _ => false,
            };
            if !matches {
                return Err(unsupported());
            }
            documents[index] = Some(UploadedDocument { content, mime });
        } else {
            let value = std::str::from_utf8(&content)
                .map_err(|_| invalid())?
                .to_owned();
            let slot = if name == "declared_company_name" {
                &mut company
            } else {
                &mut registration
            };
            if slot.replace(value).is_some() {
                return Err(malformed());
            }
        }
    }
    // Consume any epilogue, so the total body cap covers all actual bytes.
    drop(multipart);
    while let Some(chunk) = stream.next().await {
        chunk.map_err(|_| oversized())?;
    }
    let [identity, company_document] = documents;
    Ok(Submission {
        documents: [
            identity.ok_or_else(invalid)?,
            company_document.ok_or_else(invalid)?,
        ],
        declarations: Declarations::new(company, registration)?,
    })
}

fn parser_error(error: multer::Error) -> ApiError {
    match error {
        multer::Error::FieldSizeExceeded { .. }
        | multer::Error::StreamSizeExceeded { .. }
        | multer::Error::StreamReadFailed(_) => oversized(),
        _ => malformed(),
    }
}

pub struct SubmitCommand {
    user: Uuid,
    organization: Uuid,
    upload: Submission,
    keys: Arc<KeyRing>,
    now: DateTime<Utc>,
    admission: Option<(
        axum::http::HeaderMap,
        Arc<super::documents::EvidenceContext>,
    )>,
}
impl SubmitCommand {
    pub fn new(
        user: Uuid,
        organization: Uuid,
        upload: Submission,
        keys: Arc<KeyRing>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            user,
            organization,
            upload,
            keys,
            now,
            admission: None,
        }
    }
    fn with_admission(
        mut self,
        headers: axum::http::HeaderMap,
        context: Arc<super::documents::EvidenceContext>,
    ) -> Self {
        self.admission = Some((headers, context));
        self
    }
}
impl BusinessCommand for SubmitCommand {
    type Output = Submitted;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let current: Option<Uuid> =
                sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                    .bind(self.user)
                    .fetch_optional(conn)
                    .await?
                    .ok_or_else(|| {
                        refusal(
                            StatusCode::UNAUTHORIZED,
                            "INVALID_CREDENTIALS",
                            "The current account is unavailable.",
                        )
                    })?;
            if current.is_none() {
                return Err(refusal(
                    StatusCode::FORBIDDEN,
                    "KYC_ORGANIZATION_REQUIRED",
                    "Organization membership is required.",
                )
                .into());
            }
            if current != Some(self.organization) {
                return Err(refusal(
                    StatusCode::CONFLICT,
                    "KYC_MEMBERSHIP_CHANGED",
                    "Organization membership changed; submit again.",
                )
                .into());
            }
            Ok(OrganizationWritePlan::new(
                [self.organization],
                [self.user],
                [],
                json!({"user":self.user,"organization":current}),
            ))
        })
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Submitted, AttemptError> {
        Box::pin(async move {
            // Execute starts only after canonical locks and account/cutoff checks.
            // Reuse the shared signature verifier, but never the upload clock.
            if let Some((headers, context)) = &self.admission {
                let access = super::documents::authorize(headers, context, Utc::now())?;
                if access.subject != actor.id {
                    return Err(super::documents::invalid_credentials().into());
                }
            }
            if actor.id != self.user || actor.organization_id != Some(self.organization) {
                return Err(refusal(
                    StatusCode::CONFLICT,
                    "KYC_MEMBERSHIP_CHANGED",
                    "Organization membership changed; submit again.",
                )
                .into());
            }
            let row = sqlx::query("SELECT name,tax_id FROM organizations WHERE id=$1")
                .bind(self.organization)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| {
                    refusal(
                        StatusCode::CONFLICT,
                        "KYC_MEMBERSHIP_CHANGED",
                        "Organization membership changed; submit again.",
                    )
                })?;
            self.upload
                .declarations
                .validate_organization(row.get("name"), row.get("tax_id"))?;
            let previous: String = sqlx::query_scalar("SELECT kyc_status FROM users WHERE id=$1")
                .bind(self.user)
                .fetch_one(&mut *conn)
                .await?;
            if !matches!(
                previous.as_str(),
                "PENDING" | "SUBMITTED" | "APPROVED" | "REJECTED"
            ) {
                return Err(refusal(
                    StatusCode::FORBIDDEN,
                    "KYC_STATUS_INVALID",
                    "The current evidence state is unavailable.",
                )
                .into());
            }
            let submission_id = Uuid::new_v4();
            let mut ids = Vec::new();
            for (document, kind) in self
                .upload
                .documents
                .iter()
                .zip([DocumentKind::Identity, DocumentKind::CompanyRegistration])
            {
                let encrypted = vault::encrypt(
                    &self.keys,
                    DocumentMetadata {
                        id: Uuid::new_v4(),
                        submission_id,
                        user_id: self.user,
                        organization_id: self.organization,
                        kind,
                        content_type: document.mime.clone(),
                    },
                    &document.content,
                )
                .map_err(|_| storage_unavailable())?;
                ids.push(encrypted.metadata.id);
                super::documents::insert(conn, &encrypted).await?;
            }
            sqlx::query("UPDATE users SET kyc_status='SUBMITTED',kyc_organization_id=$2,kyc_rejection_reason=NULL,kyc_external_evidence_reference=NULL,kyc_review_note=NULL,kyc_reviewed_by=NULL,kyc_reviewed_at=NULL WHERE id=$1").bind(self.user).bind(self.organization).execute(&mut *conn).await?;
            append_audit(conn,&AuditFact { id:Uuid::new_v4(),actor_id:Some(actor.id),action:"kyc.submitted".into(),resource_type:"kyc".into(),resource_id:Some(self.user.to_string()),changes:Some(json!({"kyc_status":{"from":previous,"to":"SUBMITTED"},"advisory":{"provider":"none","status":"disabled"},"submission_id":submission_id,"document_ids":ids,"raw_documents_retained_in_platform":true,"declared_company_name_provided":self.upload.declarations.company.is_some(),"registration_number_provided":self.upload.declarations.registration.is_some()})),ip_address:None,request_id:None,at:self.now }).await?;
            Ok(Submitted {
                kyc_status: "SUBMITTED".into(),
                message: "KYC submitted for administrator review.".into(),
            })
        })
    }
}
pub fn storage_unavailable() -> ApiError {
    refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        "KYC_DOCUMENT_STORAGE_UNAVAILABLE",
        "Evidence storage is unavailable.",
    )
}

pub async fn handle_submit(
    state: &crate::AppState,
    context: Arc<super::documents::EvidenceContext>,
    headers: axum::http::HeaderMap,
    client: String,
    body: Body,
) -> Result<axum::Json<Submitted>, ApiError> {
    let now = Utc::now();
    let access = super::documents::authorize(&headers, &context, now)?;
    let user = access.subject;
    let admission = context.clone();
    // This preview transaction ends before upload parsing. It carries no locks
    // into the org-first command; final authority is checked again by its owner.
    let organization = crate::auth::run_authenticated(
        state,
        access,
        crate::auth::AuthenticatedRequest::new("POST", "/api/kyc/submit", false),
        move |tx| {
            Box::pin(async move {
                let organization = tx.actor().organization_id.ok_or_else(|| {
                    refusal(
                        StatusCode::FORBIDDEN,
                        "KYC_ORGANIZATION_REQUIRED",
                        "Organization membership is required.",
                    )
                })?;
                super::documents::quota(&admission, "POST", "/api/kyc/submit", &client, user, now)?;
                Ok(organization)
            })
        },
    )
    .await
    .map_err(super::documents::map_authenticated)?;
    let keys = context.keys.clone().ok_or_else(storage_unavailable)?;
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(malformed)?;
    let upload = parse_multipart(body, content_type, context.upload_limits).await?;
    let access = super::documents::authorize(&headers, &context, Utc::now())?;
    crate::business::transaction::run_organization_write(
        state,
        access,
        SubmitCommand::new(user, organization, upload, keys, now).with_admission(headers, context),
    )
    .await
    .map(axum::Json)
}
