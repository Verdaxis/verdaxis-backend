//! Loopback-only release-profile adapter for the Phase 3 coexistence tests.
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use std::sync::Arc;
use verdaxis_read::{AppState, Config, MarketPolicy, auth};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("disposable test environment required".into());
    }
    let bind: std::net::SocketAddr = std::env::var("PHASE3_BIND")?.parse()?;
    if !bind.ip().is_loopback() {
        return Err("numeric loopback required".into());
    }
    let config = Config {
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "p3_20261020_auth_tokens".into(),
        database_url: std::env::var("PHASE3_DATABASE_URL")?,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind,
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        kyc_execution_gate_enabled: false,
    };
    let pools = verdaxis_read::connect(&config)
        .await
        .map_err(|_| "runtime connect")?;
    let pool = pools.read.clone();
    let state = AppState::new(
        config.clone(),
        pools,
        serde_json::from_str::<MarketPolicy>(include_str!(
            "../../../deploy/rust/market-policy.json"
        ))?,
    )?;
    let secret = std::env::var("JWT_SECRET")?;
    let issuer = std::env::var("JWT_ISSUER")?;
    let audience = std::env::var("JWT_AUDIENCE")?;
    let verification = Arc::new(auth::VerificationSettings::new(
        secret.clone(),
        None,
        issuer.clone(),
        audience.clone(),
    ));
    let notifications = verdaxis_read::crud::notifications::routes(
        verdaxis_read::crud::notifications::NotificationAuthority::new(
            auth::VerificationSettings::new(secret.clone(), None, issuer.clone(), audience.clone()),
            auth::limits::SharedLimits::default(),
        ),
    )
    .with_state(state.clone());
    let stream = Router::new()
        .route("/api/auth/stream-token", get(stream_token))
        .with_state((
            state.clone(),
            verification,
            secret.clone(),
            issuer.clone(),
            audience.clone(),
        ));
    let routes = notifications
        .merge(stream)
        .merge(auth::login::router(
            auth::login::LoginState::new(
                pool.clone(),
                "production",
                &secret,
                None,
                &issuer,
                &audience,
                auth::limits::SharedLimits::default(),
            )
            .await,
        ))
        .merge(auth::refresh::router(auth::refresh::RefreshState::new(
            pool.clone(),
            "production",
            &secret,
            None,
            &issuer,
            &audience,
            auth::limits::SharedLimits::default(),
        )))
        .merge(auth::logout::router(auth::logout::LogoutState::new(
            pool.clone(),
            "production",
            &secret,
            None,
            &issuer,
            &audience,
        )));
    axum::serve(tokio::net::TcpListener::bind(bind).await?, routes).await?;
    Ok(())
}

type StreamState = (
    AppState,
    Arc<auth::VerificationSettings>,
    String,
    String,
    String,
);

async fn stream_token(
    State((state, verification, secret, issuer, audience)): State<StreamState>,
    headers: HeaderMap,
) -> Result<Json<auth::stream::StreamTokenResponse>, verdaxis_read::ApiError> {
    let now = chrono::Utc::now();
    let access = auth::verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &verification,
        now,
    )
    .map_err(|_| {
        verdaxis_read::ApiError::from(auth::error::AuthFailure::new(
            auth::error::AuthCode::CredentialsInvalid,
        ))
    })?;
    auth::run_authenticated(
        &state,
        access,
        auth::AuthenticatedRequest::new("GET", "/api/auth/stream-token", false),
        move |transaction| {
            Box::pin(async move {
                auth::stream::StreamTokens::new(
                    auth::TokenIssuer::new(&secret, &issuer, &audience),
                    "test",
                    auth::limits::SharedLimits::default(),
                )
                .issue(transaction.actor(), "loopback", now)
                .map(Json)
            })
        },
    )
    .await
    .map_err(|_| verdaxis_read::ApiError::database())
}
