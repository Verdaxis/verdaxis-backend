//! Task 8 disposable adapter. Never mounted in the serving binary.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use verdaxis_read::{
    AppState, Config, MarketPolicy,
    auth::{VerificationSettings, limits::SharedLimits},
    crud::notifications::{self, NotificationAuthority},
};

async fn response_loss(request: Request, next: Next) -> Response {
    let lose = request
        .headers()
        .get("x-notification-fault")
        .is_some_and(|v| v == "after-commit");
    let response = next.run(request).await;
    if lose && response.status().is_success() {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"detail":"Injected response loss"})),
        )
            .into_response()
    } else {
        response
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test environment required".into());
    }
    let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
    if !bind.ip().is_loopback() {
        return Err("loopback required".into());
    }
    let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
    if token.len() < 32 {
        return Err("disposable token required".into());
    }
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "p0_20261016_union".into(),
        database_url: std::env::var("PHASE2_DATABASE_URL")?,
        db_session_time_zone: "Asia/Singapore".into(),
        prepared_budget: None,
        bind,
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 20,
        configured_max_connections: 100,
    };
    let pools = verdaxis_read::connect(&config)
        .await
        .map_err(|_| "database attestation failed")?;
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )?;
    let limits = SharedLimits::default();
    let authority = NotificationAuthority::new(
        VerificationSettings::new(
            std::env::var("JWT_SECRET")?,
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ),
        limits.clone(),
    );
    let routes = notifications::routes(authority)
        .with_state(state)
        .route(
            "/.well-known/verdaxis-disposable-test",
            get(move || async move { axum::Json(json!({"disposable":true,"token":token})) }),
        )
        .layer(middleware::from_fn(response_loss));
    let router: Router = verdaxis_read::runtime::http::middleware_with_limits(routes, limits);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
