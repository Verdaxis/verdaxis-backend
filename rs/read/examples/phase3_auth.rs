//! Loopback-only release-profile adapter for the Phase 3 coexistence tests.
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
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
        phase2: Default::default(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        // This adapter does not prepare a release or move the production pin.
        migration_revision: "prc_20261018_cache".into(),
        database_url: std::env::var("PHASE3_DATABASE_URL")?,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind,
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
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
        .merge(
            Router::new()
                .route("/api/auth/me", get(profile))
                .route("/api/test/cleanup", get(cleanup))
                .route("/api/test/issue", post(issue_at))
                .with_state((
                    state.clone(),
                    Arc::new(auth::VerificationSettings::new(
                        secret.clone(),
                        None,
                        issuer.clone(),
                        audience.clone(),
                    )),
                    secret.clone(),
                    issuer.clone(),
                    audience.clone(),
                )),
        )
        .merge(auth::password_change::router(
            auth::password_change::PasswordChangeState::new(
                pool.clone(),
                "test",
                &secret,
                None,
                &issuer,
                &audience,
                auth::limits::SharedLimits::default(),
            ),
        ))
        .merge(auth::login::router(
            auth::login::LoginState::new(
                pool.clone(),
                "test",
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
            "test",
            &secret,
            None,
            &issuer,
            &audience,
            auth::limits::SharedLimits::default(),
        )))
        .merge(auth::logout::router(auth::logout::LogoutState::new(
            pool.clone(),
            "test",
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

async fn profile(
    State((state, verification, _, _, _)): State<StreamState>,
    headers: HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let result = auth::verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &verification,
        chrono::Utc::now(),
    );
    let access = match result {
        Ok(value) => value,
        Err(error) => {
            return (
                axum::http::StatusCode::from_u16(error.status()).unwrap(),
                Json(error.body()),
            )
                .into_response();
        }
    };
    match auth::run_authenticated(
        &state,
        access,
        auth::AuthenticatedRequest::new("GET", "/api/auth/me", false),
        |tx| Box::pin(async move { Ok(serde_json::json!({"id":tx.actor().id})) }),
    )
    .await
    {
        Ok(body) => Json(body).into_response(),
        Err(auth::AuthenticatedError::Authentication(error)) => (
            axum::http::StatusCode::from_u16(error.status()).unwrap(),
            Json(error.body()),
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn cleanup(
    State((_, verification, _, _, _)): State<StreamState>,
    headers: HeaderMap,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let access = match auth::verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &verification,
        chrono::Utc::now(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return (
                axum::http::StatusCode::from_u16(error.status()).unwrap(),
                Json(error.body()),
            )
                .into_response();
        }
    };
    let pool = sqlx::PgPool::connect(&std::env::var("PHASE3_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let row: (
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT status::text,password_changed_at,authentication_revoked_at FROM users WHERE id=$1",
    )
    .bind(access.subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    let account = auth::AccountState {
        exists: true,
        status: row.0.clone(),
        must_change_password: false,
        password_changed_at: row.1.map(|v| v.timestamp_micros()),
        authentication_revoked_at: if row.0 == "REJECTED" {
            None
        } else {
            row.2.map(|v| v.timestamp_micros())
        },
    };
    match auth::cutoff_refusal(
        auth::TokenIssuance {
            issued_at: access.issued_at,
            out_of_range: false,
        },
        &account,
    ) {
        Some(error) => (
            axum::http::StatusCode::from_u16(error.status()).unwrap(),
            Json(error.body()),
        )
            .into_response(),
        None => Json(serde_json::json!({"id":access.subject})).into_response(),
    }
}

async fn issue_at(
    State((_, _, secret, issuer, audience)): State<StreamState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let user = body["user"].as_str().unwrap().parse().unwrap();
    let now = body["issued_at"].as_str().unwrap().parse().unwrap();
    Json(
        serde_json::json!({"access_token":auth::TokenIssuer::new(&secret,&issuer,&audience).access(user,now,None)}),
    )
}
