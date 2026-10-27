//! Production credential composition shares the ingress quota and release authority.
use std::sync::Arc;
use axum::{Router, Json, extract::{State, Request, ConnectInfo}, response::{Response, IntoResponse}, routing::get};
use crate::{AppState, ApiError, auth};

#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainDatasets {
    pub public: Vec<String>,
    pub disposable: Vec<String>,
}

#[derive(Clone)]
pub struct Phase3Settings {
    domains: Option<DomainDatasets>,
    monitor_token: Option<String>,
}
impl Phase3Settings {
    pub fn from_options(options: &crate::Phase2Options) -> Result<Self, String> {
        let domains = options.auth_domain_datasets.as_deref().map(|raw| {
            serde_json::from_str::<Option<DomainDatasets>>(raw)
                .map_err(|_| "AUTH_DOMAIN_DATASETS must be null or the release domain lists".to_owned())
        }).transpose()?.flatten();
        if let Some(domains) = &domains {
            if domains.public.is_empty() || domains.disposable.is_empty() {
                return Err("AUTH_DOMAIN_DATASETS requires both release domain lists".into());
            }
        }
        Ok(Self { domains, monitor_token: options.monitor_token.clone() })
    }
}

pub fn routes(state: &AppState, limits: auth::limits::SharedLimits) -> Option<Router> {
    let settings = state.phase2.as_ref()?;
    let options = &state.config.phase2;
    let secret = options.jwt_secret.as_deref()?;
    let previous = options.jwt_secret_previous.as_deref().filter(|value| !value.trim().is_empty());
    let issuer = options.jwt_issuer.as_deref()?;
    let audience = options.jwt_audience.as_deref()?;
    let environment = state.config.environment.as_str();
    let pool = state.pools.read.clone();
    let mail = auth::email::MailDispatcher::new(settings.mail.clone())
        .with_rendering(settings.email_from.clone(), settings.frontend_url.clone());
    let mut email = auth::email_verification::EmailSettings::new(pool.clone(), mail.clone());
    email.limits = limits.clone();
    let mut registration = auth::registration::RegistrationSettings::new(pool.clone(), mail)
        .with_environment(environment.into());
    registration.limits = limits.clone();
    if let Some(domains) = &state.phase3.domains {
        registration = registration.with_domain_lists(domains.public.clone(), domains.disposable.clone());
    }
    if let Some(token) = &state.phase3.monitor_token {
        email = email.with_monitor_token(token.clone());
        registration = registration.with_monitor_token(token.clone());
    }
    let stream = StreamState {
        app: state.clone(), verification: Arc::new(settings.verification.clone()),
        tokens: Arc::new(auth::stream::StreamTokens::new(auth::TokenIssuer::new(secret, issuer, audience), environment, limits.clone())),
    };
    Some(Router::new()
        .merge(auth::login::router(auth::login::LoginState::from_settings(pool.clone(), environment, secret, previous, issuer, audience, limits.clone())))
        .merge(auth::refresh::router(auth::refresh::RefreshState::new(pool.clone(), environment, secret, previous, issuer, audience, limits.clone())))
        .merge(auth::logout::router(auth::logout::LogoutState::new(pool.clone(), environment, secret, previous, issuer, audience)))
        .merge(auth::password_change::router(auth::password_change::PasswordChangeState::new(pool.clone(), environment, secret, previous, issuer, audience, limits.clone())))
        .merge(auth::invitation_accept::router(auth::invitation_accept::InvitationAcceptState::new(pool, environment, secret, issuer, audience, &settings.frontend_url, limits)))
        .merge(auth::email_verification::router(email.clone()))
        .merge(auth::password_reset::router(email))
        .merge(auth::registration::router(registration))
        .merge(Router::new().route("/api/auth/stream-token", get(stream_token))
            .route("/api/auth/stream-token/", get(stream_token)).with_state(stream)))
}

#[derive(Clone)]
struct StreamState {
    app: AppState,
    verification: Arc<auth::VerificationSettings>,
    tokens: Arc<auth::stream::StreamTokens>,
}
async fn stream_token(State(state): State<StreamState>, request: Request) -> Response {
    let now = chrono::Utc::now();
    let access = match auth::verify_authorization(request.headers().get("authorization").and_then(|value| value.to_str().ok()), &state.verification, now) {
        Ok(access) => access,
        Err(error) => return (axum::http::StatusCode::from_u16(error.status()).unwrap(), Json(error.body())).into_response(),
    };
    let peer = request.extensions().get::<ConnectInfo<std::net::SocketAddr>>().map(|peer| peer.0.ip());
    let client = super::http::client_ip(peer, request.headers().get("x-forwarded-for").and_then(|value| value.to_str().ok()));
    let tokens = state.tokens;
    match auth::run_authenticated(&state.app, access, auth::AuthenticatedRequest::new("GET", "/api/auth/stream-token", false), move |transaction| {
        Box::pin(async move { tokens.issue(transaction.actor(), &client, now).map(Json) })
    }).await {
        Ok(body) => body.into_response(),
        Err(auth::AuthenticatedError::Authentication(error)) => (axum::http::StatusCode::from_u16(error.status()).unwrap(), Json(error.body())).into_response(),
        Err(auth::AuthenticatedError::Database(error)) => error.into_response(),
        Err(auth::AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}
