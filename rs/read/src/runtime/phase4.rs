//! The one Phase 4 serving composition; background work remains Python-owned.
use super::phase4_adapters::{Domains, Events, FrozenMail, ProfilePolicy, Template};
use crate::{ApiError, AppState, auth, business, kyc, subscriptions};
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
struct Authority {
    app: AppState,
    verification: Arc<auth::VerificationSettings>,
    limits: auth::limits::SharedLimits,
    evidence: Arc<kyc::documents::EvidenceContext>,
}

pub fn routes(app: &AppState, limits: auth::limits::SharedLimits) -> Option<Router> {
    let settings = app.phase2.as_ref()?;
    let verification = Arc::new(settings.verification.clone());
    let keys = kyc::keyring::KeyRing::load_runtime(
        std::env::var_os("CREDENTIALS_DIRECTORY")
            .as_deref()
            .map(std::path::Path::new),
        None,
        &app.config.environment,
    )
    .ok()
    .map(Arc::new);
    let evidence = Arc::new(kyc::documents::EvidenceContext {
        settings: verification.clone(),
        limits: limits.clone(),
        keys,
        upload_limits: Default::default(),
    });
    let authority = Authority {
        app: app.clone(),
        verification: verification.clone(),
        limits: limits.clone(),
        evidence,
    };
    let events = Arc::new(Events(app.clone()));
    let mail = auth::email::MailDispatcher::new(settings.mail.clone())
        .with_rendering(settings.email_from.clone(), settings.frontend_url.clone());
    let mut registration =
        auth::registration::RegistrationSettings::new(app.pools.read.clone(), mail);
    if let Some(raw) = app.config.phase2.auth_domain_datasets.as_deref()
        && let Ok(Some(domains)) =
            serde_json::from_str::<Option<super::phase3::DomainDatasets>>(raw)
    {
        registration = registration.with_domain_lists(domains.public, domains.disposable);
    }
    let fee = |key: &str, default: &str| {
        std::env::var(key)
            .unwrap_or_else(|_| default.into())
            .parse()
            .expect("prepared seller fee")
    };
    let fees = subscriptions::fees::FeeConfig::new(
        fee("SELLER_FEE_PILOT_PER_MT_USD", "2"),
        fee("SELLER_FEE_PROFESSIONAL_PER_MT_USD", "1.50"),
    )
    .expect("validated seller fee configuration");
    let provider = super::phase4_analytics::provider(app);
    let stateful = Router::new()
        .merge(business::profile::routes(
            business::profile::ProfileAuthority::new(
                settings.verification.clone(),
                limits.clone(),
                Arc::new(ProfilePolicy),
                events.clone(),
            ),
        ))
        .merge(business::survey::routes(
            business::survey::SurveyAuthority::new(settings.verification.clone(), limits.clone()),
        ))
        .merge(business::invitations::request::router(
            app.clone(),
            settings.verification.clone(),
            limits.clone(),
            business::invitations::request::InvitationService {
                pool: app.pools.read.clone(),
                domains: Arc::new(Domains(Arc::new(registration))),
                frontend_url: settings.frontend_url.clone().into(),
            },
        ))
        .merge(business::vessels::routes(
            business::vessels::VesselAuthority::new(settings.verification.clone(), limits.clone()),
        ))
        .merge(crate::analytics::usage::routes(
            crate::analytics::usage::UsageAuthority::new(
                settings.verification.clone(),
                limits.clone(),
                Some(Arc::new(provider.clone())),
            ),
        ))
        .with_state(app.clone());
    Some(
        stateful
            .merge(super::phase4_analytics::routes(
                app,
                limits.clone(),
                provider,
            ))
            .merge(business::admission::router(
                app.clone(),
                settings.verification.clone(),
                limits.clone(),
                business::admission::AdmissionService {
                    mail: Arc::new(FrozenMail),
                    events: events.clone(),
                    template: Arc::new(Template {
                        sender: settings.email_from.clone(),
                        frontend: settings.frontend_url.clone(),
                    }),
                },
            ))
            .merge(business::attention::router(
                app.clone(),
                settings.verification.clone(),
                limits.clone(),
            ))
            .merge(business::audit_query::router(
                app.clone(),
                settings.verification.clone(),
                limits.clone(),
            ))
            .merge(subscriptions::routes::router(
                app.clone(),
                verification.clone(),
                fees,
            ))
            .merge(super::http::analytics_routes(
                app.clone(),
                verification.clone(),
                limits.clone(),
            ))
            .merge(
                kyc::review::ReviewService::new(
                    app.pools.read.clone(),
                    app.reads.clone(),
                    verification,
                    limits,
                    events,
                    settings.mail.clone(),
                    settings.email_from.clone(),
                )
                .router(),
            )
            .merge(
                Router::new()
                    .route("/api/kyc/submit", post(submit))
                    .route("/api/kyc/status", get(status))
                    .route("/api/kyc/admin/{user_id}/documents", get(documents))
                    .route("/api/kyc/admin/documents/{document_id}", get(download))
                    .route(
                        "/api/auth/organization/{organization_id}/approve",
                        put(approve_organization),
                    )
                    .route(
                        "/api/auth/organization/{organization_id}/reject",
                        put(reject_organization),
                    )
                    .route("/api/auth/organization-joins", get(joins))
                    .route(
                        "/api/auth/organization-joins/{join_request_id}/approve",
                        put(approve_join),
                    )
                    .route(
                        "/api/auth/organization-joins/{join_request_id}/reject",
                        put(reject_join),
                    )
                    .with_state(authority),
            ),
    )
}
fn client(request: &Request) -> String {
    super::http::client_ip(
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip()),
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    )
}
fn access(authority: &Authority, request: &Request) -> Result<auth::VerifiedAccess, ApiError> {
    auth::verify_authorization(
        request
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok()),
        &authority.verification,
        Utc::now(),
    )
    .map_err(business::error::authentication)
}
async fn submit(
    State(a): State<Authority>,
    request: Request,
) -> Result<impl IntoResponse, ApiError> {
    let client = client(&request);
    let (parts, body) = request.into_parts();
    kyc::submission::handle_submit(&a.app, a.evidence, parts.headers, client, body).await
}
async fn status(
    State(a): State<Authority>,
    request: Request,
) -> Result<impl IntoResponse, ApiError> {
    kyc::documents::handle_status(&a.app, a.evidence, request.headers().clone()).await
}
async fn documents(
    State(a): State<Authority>,
    Path(user): Path<String>,
    request: Request,
) -> Result<impl IntoResponse, ApiError> {
    kyc::documents::handle_list(
        &a.app,
        a.evidence,
        request.headers().clone(),
        client(&request),
        &user,
    )
    .await
}
async fn download(
    State(a): State<Authority>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    kyc::documents::handle_download(
        &a.app,
        a.evidence,
        request.headers().clone(),
        client(&request),
        &id,
    )
    .await
}
fn identifier(raw: &str) -> Result<uuid::Uuid, ApiError> {
    raw.parse().map_err(|_| {
        business::error::refusal(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_IDENTIFIER",
            "The identifier is invalid.",
        )
    })
}
async fn review_organization(
    a: Authority,
    id: String,
    request: Request,
    approve: bool,
) -> Result<Json<Value>, ApiError> {
    let access = access(&a, &request)?;
    let client = client(&request);
    let id = identifier(&id)?;
    let result = business::organizations::review(
        &a.app,
        access,
        &a.limits,
        &client,
        business::organizations::ReviewOrganization {
            organization_id: id,
            approve,
            reason: None,
            now: Utc::now(),
            policy: a.app.policy.clone(),
            config: a.app.config.clone(),
        },
    )
    .await?;
    Ok(Json(
        serde_json::to_value(result).map_err(|_| ApiError::internal())?,
    ))
}
async fn approve_organization(
    State(a): State<Authority>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    review_organization(a, id, request, true).await
}
async fn reject_organization(
    State(a): State<Authority>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    review_organization(a, id, request, false).await
}
async fn joins(State(a): State<Authority>, request: Request) -> Result<Response, ApiError> {
    let access = access(&a, &request)?;
    let client = client(&request);
    let fields: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let integer = |key: &str, default: i64| {
        fields
            .get(key)
            .map(|v| {
                v.parse::<i64>().map_err(|_| {
                    business::error::refusal(
                        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                        "INVALID_QUERY",
                        "The query is invalid.",
                    )
                })
            })
            .unwrap_or(Ok(default))
    };
    let query = business::joins::JoinListQuery::parse(
        fields.get("status").map(String::as_str),
        integer("limit", 100)?,
        integer("offset", 0)?,
    )?;
    let page = business::joins::read(&a.app, access, a.limits, client, query).await?;
    let truncated = page.truncated;
    Ok((
        [(
            "x-result-truncated",
            if truncated { "true" } else { "false" },
        )],
        Json(page.items),
    )
        .into_response())
}
async fn review_join(
    a: Authority,
    id: String,
    request: Request,
    approve: bool,
) -> Result<Json<Value>, ApiError> {
    let access = access(&a, &request)?;
    let client = client(&request);
    let id = identifier(&id)?;
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| ApiError::internal())?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
        business::error::refusal(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_BODY",
            "The body is invalid.",
        )
    })?;
    let note = business::joins::ReviewNote::parse(
        body.get("review_note")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    let result = business::joins::review(
        &a.app,
        access,
        &a.limits,
        &client,
        business::joins::ReviewJoin {
            join_request_id: id,
            approve,
            note,
            now: Utc::now(),
        },
    )
    .await?;
    Ok(Json(
        serde_json::to_value(result).map_err(|_| ApiError::internal())?,
    ))
}
async fn approve_join(
    State(a): State<Authority>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    review_join(a, id, request, true).await
}
async fn reject_join(
    State(a): State<Authority>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    review_join(a, id, request, false).await
}
