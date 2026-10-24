use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{Extensions, HeaderMap, StatusCode},
    routing::put,
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Row};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    ApiError,
    auth::{
        AccountState, CurrentActor, VerificationSettings,
        limits::{BusinessLimitKeys, SharedLimits},
        validate_account, verify_authorization,
    },
    business::{
        admission::AdmissionEvents,
        audit::{AuditFact, append_audit},
        error::{authentication, database_failure, refusal},
        transaction::{
            AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan, run_ordered_write,
        },
    },
    crud::referrals::invite::MailClient,
};

// The review body contains bounded text only, never document content.
const REVIEW_BODY_BYTES: usize = 16 * 1024;

// Python's audit registry (`app/services/audit_actions.py`). The append-only
// audit table stores these dotted values verbatim and compliance reads filter
// on them, so they are a wire contract, not a label.
const APPROVED_ACTION: &str = "kyc.approved";
const REJECTED_ACTION: &str = "kyc.rejected";

pub enum Decision {
    Approve {
        external_reference: String,
        note: String,
    },
    Reject {
        reason: String,
    },
}

impl Decision {
    pub fn approval(body: Value) -> Result<Self, ApiError> {
        let external_reference = text(&body, "external_evidence_reference", 3, 200)?;
        if external_reference.chars().any(|c| c.is_ascii_control()) {
            return Err(invalid_body());
        }
        Ok(Self::Approve {
            external_reference,
            note: text(&body, "review_note", 10, 2000)?,
        })
    }

    pub fn rejection(body: Value) -> Result<Self, ApiError> {
        Ok(Self::Reject {
            reason: text(&body, "reason", 3, 1000)?,
        })
    }

    pub fn status(&self) -> &'static str {
        match self {
            Self::Approve { .. } => "APPROVED",
            Self::Reject { .. } => "REJECTED",
        }
    }
}

fn text(body: &Value, field: &str, minimum: usize, maximum: usize) -> Result<String, ApiError> {
    let value = body
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(invalid_body)?;
    let normalized = value.trim();
    if value.chars().count() > maximum || normalized.chars().count() < minimum {
        return Err(invalid_body());
    }
    Ok(normalized.to_owned())
}

fn invalid_body() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "KYC_REVIEW_INVALID",
        "The review fields are invalid.",
    )
}

fn require_admin(actor: &CurrentActor) -> Result<(), ApiError> {
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

fn missing() -> ApiError {
    refusal(
        StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
        "The user was not found.",
    )
}

#[derive(Clone)]
pub struct ReviewService {
    pool: PgPool,
    permits: Arc<Semaphore>,
    verifier: Arc<VerificationSettings>,
    limits: SharedLimits,
    events: Arc<dyn AdmissionEvents>,
    mail: MailClient,
    sender: String,
}

impl ReviewService {
    pub fn new(
        pool: PgPool,
        permits: Arc<Semaphore>,
        verifier: Arc<VerificationSettings>,
        limits: SharedLimits,
        events: Arc<dyn AdmissionEvents>,
        mail: MailClient,
        sender: String,
    ) -> Self {
        Self {
            pool,
            permits,
            verifier,
            limits,
            events,
            mail,
            sender,
        }
    }

    /// Component router only. The serving application does not mount it here.
    pub fn router(self) -> Router {
        Router::new()
            .route("/api/kyc/admin/{user_id}/approve", put(approve))
            .route("/api/kyc/admin/{user_id}/reject", put(reject))
            .layer(DefaultBodyLimit::max(REVIEW_BODY_BYTES))
            .with_state(self)
    }

    async fn review(
        self,
        headers: HeaderMap,
        client: String,
        request_id: String,
        target: String,
        raw: Bytes,
        approve: bool,
    ) -> Result<Json<Value>, ApiError> {
        // Authentication and administrator authority precede body/target lookup
        // and quota. The write owner revalidates the account under sorted locks.
        let access = verify_authorization(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            &self.verifier,
            Utc::now(),
        )
        .map_err(authentication)?;
        {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| database_failure(None))?;
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|_| database_failure(None))?;
            let row = sqlx::query("SELECT organization_id,role::text AS role,status::text AS status,email_verified,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1")
                .bind(access.subject).fetch_optional(&mut *conn).await.map_err(|_| database_failure(None))?
                .ok_or_else(|| authentication(crate::auth::AuthError::Credentials))?;
            let password: Option<DateTime<Utc>> = row.get("password_changed_at");
            let revoked: Option<DateTime<Utc>> = row.get("authentication_revoked_at");
            validate_account(
                &access,
                &AccountState {
                    exists: true,
                    status: row.get("status"),
                    must_change_password: row.get("must_change_password"),
                    password_changed_at: password.map(|value| value.timestamp_micros()),
                    authentication_revoked_at: revoked.map(|value| value.timestamp_micros()),
                },
            )
            .map_err(authentication)?;
            require_admin(&CurrentActor {
                id: access.subject,
                organization_id: row.get("organization_id"),
                role: row.get("role"),
                email_verified: row.get("email_verified"),
            })?;
        }
        let template = if approve {
            "/api/kyc/admin/{user_id}/approve"
        } else {
            "/api/kyc/admin/{user_id}/reject"
        };
        // These routes currently declare no route quota. Use the shared exact
        // method/template policy so future integration cannot silently omit one,
        // and pass the real caller identity so no future limit shares a bucket.
        self.limits.check_business(
            "PUT",
            template,
            BusinessLimitKeys {
                client: &client,
                user: Some(&access.subject.to_string()),
                organization: None,
                token: None,
            },
            Utc::now().timestamp().max(0) as u64,
        )?;
        let user_id = Uuid::parse_str(&target).map_err(|_| invalid_body())?;
        let body = serde_json::from_slice(&raw).map_err(|_| invalid_body())?;
        let decision = if approve {
            Decision::approval(body)?
        } else {
            Decision::rejection(body)?
        };
        let command = ReviewCommand {
            user_id,
            decision,
            now: Utc::now(),
            client: Some(client),
            request_id: Some(request_id),
            events: self.events,
        };
        let committed = run_ordered_write(self.pool, self.permits, access, command).await?;
        let payload = notification(&self.sender, &committed);
        // MailClient owns the existing absolute 10-second timeout and does not
        // retry. An uncertain send cannot roll back or replay the decision.
        if !self.mail.send(&payload).await {
            return Err(refusal(
                StatusCode::BAD_GATEWAY,
                "KYC_NOTIFICATION_FAILED",
                "The review was saved, but its notification failed. Refresh the case before any further action.",
            ));
        }
        Ok(Json(committed.response))
    }
}

async fn approve(
    State(service): State<ReviewService>,
    Path(target): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    extensions: Extensions,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    owned_review(service, peer, extensions, target, headers, raw, true).await
}

async fn reject(
    State(service): State<ReviewService>,
    Path(target): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    extensions: Extensions,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Json<Value>, ApiError> {
    owned_review(service, peer, extensions, target, headers, raw, false).await
}

#[allow(clippy::too_many_arguments)]
async fn owned_review(
    service: ReviewService,
    peer: SocketAddr,
    extensions: Extensions,
    target: String,
    headers: HeaderMap,
    raw: Bytes,
    approve: bool,
) -> Result<Json<Value>, ApiError> {
    // The same address rule the serving middleware applies, so the audit row
    // carries the value Python's request context records. `client_ip` trusts a
    // forwarded hop only from a loopback peer, exactly as `app/rate_limit.py`.
    let client = crate::runtime::http::client_ip(
        Some(peer.ip()),
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    // Reuse the correlation id the middleware already minted for this request
    // (it owns the log line and the response header). Deriving a second id here
    // would split one request across two audit identities; the header fallback
    // only serves the component router, which has no middleware.
    let request_id = extensions.get::<String>().cloned().unwrap_or_else(|| {
        crate::runtime::logging::request_id(
            headers
                .get("x-request-id")
                .and_then(|value| value.to_str().ok()),
        )
    });
    // Retain ownership through caller cancellation, including the single send.
    tokio::spawn(service.review(headers, client, request_id, target, raw, approve))
        .await
        .map_err(|_| database_failure(None))?
}

pub struct ReviewCommand {
    pub user_id: Uuid,
    pub decision: Decision,
    pub now: DateTime<Utc>,
    pub client: Option<String>,
    pub request_id: Option<String>,
    pub events: Arc<dyn AdmissionEvents>,
}

pub struct CommittedReview {
    response: Value,
    email: String,
    name: String,
    approved: bool,
    reason: Option<String>,
}

impl BusinessCommand for ReviewCommand {
    type Output = CommittedReview;

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        Box::pin(async move {
            let row =
                sqlx::query("SELECT organization_id,kyc_organization_id FROM users WHERE id=$1")
                    .bind(self.user_id)
                    .fetch_optional(conn)
                    .await?
                    .ok_or_else(missing)?;
            let organization: Option<Uuid> = row.get("organization_id");
            Ok(OrganizationWritePlan::new(
                organization,
                [self.user_id],
                [],
                json!({"organization":organization,"kyc_organization":row.get::<Option<Uuid>,_>("kyc_organization_id")}),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, CommittedReview, AttemptError> {
        Box::pin(async move {
            require_admin(actor)?;
            let row = sqlx::query("SELECT organization_id,kyc_organization_id,kyc_status,email,first_name,status::text AS account_status FROM users WHERE id=$1")
                .bind(self.user_id).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
            let organization: Option<Uuid> = row.get("organization_id");
            let binding: Option<Uuid> = row.get("kyc_organization_id");
            let organization = organization
                .filter(|id| Some(*id) == binding)
                .ok_or_else(|| {
                    refusal(
                        StatusCode::CONFLICT,
                        "KYC_ORGANIZATION_MISMATCH",
                        "KYC evidence must match the current organization.",
                    )
                })?;
            let previous: Option<String> = row.get("kyc_status");
            // Python's approve path requires SUBMITTED; its reject path has no
            // status precondition (PD4-T11b-1). Both still require the current
            // organization binding above.
            if matches!(self.decision, Decision::Approve { .. })
                && previous.as_deref() != Some("SUBMITTED")
            {
                return Err(refusal(
                    StatusCode::CONFLICT,
                    "KYC_REVIEW_NOT_SUBMITTED",
                    "The current case cannot receive this review decision.",
                )
                .into());
            }
            let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM kyc_documents WHERE user_id=$1 AND organization_id=$2)")
                .bind(self.user_id).bind(organization).fetch_one(&mut *conn).await?;
            let before = self.events.visible(conn, self.user_id).await?;
            let (reference, note, reason) = match &self.decision {
                Decision::Approve {
                    external_reference,
                    note,
                } => (Some(external_reference.as_str()), note.as_str(), None),
                Decision::Reject { reason } => (None, reason.as_str(), Some(reason.as_str())),
            };
            // Rejection is a user KYC decision, not tenant withdrawal. No market
            // row, participant, reservation, session or evidence row is changed.
            sqlx::query("UPDATE users SET kyc_status=$2,kyc_rejection_reason=$3,kyc_external_evidence_reference=CASE WHEN $2='APPROVED' THEN $4 ELSE kyc_external_evidence_reference END,kyc_review_note=$5,kyc_reviewed_by=$6,kyc_reviewed_at=$7 WHERE id=$1")
                .bind(self.user_id).bind(self.decision.status()).bind(reason).bind(reference).bind(note).bind(actor.id).bind(self.now).execute(&mut *conn).await?;
            let after = self.events.visible(conn, self.user_id).await?;
            // Python enqueues the admission event only on an actual change, so a
            // repeated decision keeps its audit row and notification without a
            // second event.
            if previous.as_deref() != Some(self.decision.status()) {
                self.events
                    .append(conn, self.user_id, organization, before || after)
                    .await?;
            }
            let approved = matches!(self.decision, Decision::Approve { .. });
            let mut changes = json!({"kyc_status":{"from":previous,"to":self.decision.status()},"raw_documents_retained_in_platform":retained});
            if approved {
                changes["external_evidence_reference"] = json!(reference);
                changes["review_note"] = json!(note);
            } else {
                changes["reason"] = json!(reason);
            }
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.id),
                    action: if approved {
                        APPROVED_ACTION
                    } else {
                        REJECTED_ACTION
                    }
                    .into(),
                    resource_type: "kyc".into(),
                    resource_id: Some(self.user_id.to_string()),
                    changes: Some(changes),
                    ip_address: self.client.clone(),
                    request_id: self.request_id.clone(),
                    at: self.now,
                },
            )
            .await?;
            let response = if approved {
                json!({"user_id":self.user_id,"kyc_status":"APPROVED","account_status":row.get::<String,_>("account_status"),"message":"KYC approved."})
            } else {
                json!({"user_id":self.user_id,"kyc_status":"REJECTED","rejection_reason":reason,"message":"KYC rejected."})
            };
            Ok(CommittedReview {
                response,
                email: row.get("email"),
                name: row
                    .get::<Option<String>, _>("first_name")
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "there".into()),
                approved,
                reason: reason.map(str::to_owned),
            })
        })
    }
}

fn html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn notification(sender: &str, review: &CommittedReview) -> Value {
    let subject = if review.approved {
        "Your KYC review is approved"
    } else {
        "Your KYC review needs attention"
    };
    let message = if review.approved {
        "Your KYC review has been approved.".to_owned()
    } else {
        format!(
            "Your KYC review was not approved. {}",
            review.reason.as_deref().unwrap_or("")
        )
    };
    json!({"from":sender,"to":[review.email],"subject":subject,"html":format!("<p>Hello {},</p><p>{}</p>",html(&review.name),html(&message))})
}
