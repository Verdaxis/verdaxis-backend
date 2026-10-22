//! Test-only real referral HTTP adapter. Never used by the deployed binary.
use axum::{Router, routing::get};
use verdaxis_read::{
    auth::VerificationSettings,
    crud::referrals::{ReferralSettings, invite::MailClient},
};

async fn response_loss(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let lose = request
        .headers()
        .get("x-referral-test-response-loss")
        .is_some_and(|v| v == "true");
    let request_id = request.headers().get("x-request-id").cloned();
    let response = next.run(request).await;
    if lose && response.status() == axum::http::StatusCode::OK {
        let mut lost = (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"detail":"disposable response lost after commit"})),
        )
            .into_response();
        if let Some(value) = request_id {
            lost.headers_mut().insert("x-request-id", value);
        }
        return lost;
    }
    response
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test only".into());
    }
    tracing_subscriber::fmt().with_env_filter("info").init();
    let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
    if token.len() < 32 {
        return Err("disposable identity required".into());
    }
    let config = verdaxis_read::Config {
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "p0_20261016_union".into(),
        database_url: std::env::var("PHASE2_DATABASE_URL")?,
        db_session_time_zone: "Asia/Singapore".into(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        bind: std::env::var("PHASE2_BIND")?.parse()?,
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        kyc_execution_gate_enabled: true,
    };
    let pools = verdaxis_read::connect(&config)
        .await
        .map_err(|_| "disposable pool refused")?;
    let state = verdaxis_read::AppState::new(
        config.clone(),
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: config.release_sha.clone(),
            products: vec![],
            points: vec![],
        },
    )?;
    let mut settings = ReferralSettings::new(
        VerificationSettings::new(
            std::env::var("JWT_SECRET")?,
            None,
            "verdaxis".into(),
            "verdaxis-api".into(),
        ),
        std::env::var("FRONTEND_URL")?,
    );
    settings.email_from = std::env::var("EMAIL_FROM")?.into();
    settings.mail = if std::env::var("REFERRAL_TEST_MAIL_KEY_ABSENT").as_deref() == Ok("true") {
        MailClient::disabled()
    } else {
        MailClient::loopback(std::env::var("PHASE2_MAIL_URL")?, "disposable-only".into())?
    };
    let routes = Router::new()
        .merge(verdaxis_read::crud::referrals::routes(settings))
        .route(
            "/.well-known/verdaxis-disposable-test",
            get(move || {
                let token = token.clone();
                async move { axum::Json(serde_json::json!({"disposable":true,"token":token})) }
            }),
        );
    let app = verdaxis_read::runtime::http::router_with(routes, state)
        .layer(axum::middleware::from_fn(response_loss));
    axum::serve(tokio::net::TcpListener::bind(config.bind).await?, app).await?;
    Ok(())
}
