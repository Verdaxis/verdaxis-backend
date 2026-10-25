//! Task 24 disposable adapter: the six Phase 2 groups in one process.
//!
//! The client journeys need one endpoint that serves the same group routers the
//! serving binary mounts. This example composes them exactly as
//! `runtime::http::phase2_routes` does, then applies the shared middleware. It
//! is never mounted in the serving binary and reads only loopback settings.
use axum::{Router, routing::get};
use serde_json::json;
use std::sync::Arc;
use verdaxis_read::{
    AppState, Config, MarketPolicy,
    auth::{VerificationSettings, limits::SharedLimits},
    crud::{
        alerts, feedback, notifications, preferences, referrals,
        referrals::invite::MailClient,
        watchlists::{self, WatchlistSettings},
    },
};

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
        phase2: Default::default(),
    };
    let pools = verdaxis_read::connect(&config)
        .await
        .map_err(|_| "database attestation failed")?;
    let state = AppState::new(
        config,
        pools,
        // The shared market policy: the watchlist target writes admit product
        // codes only from it, so the journey server loads the deployed policy
        // exactly as the other contract servers do.
        serde_json::from_str::<MarketPolicy>(include_str!(
            "../../../deploy/rust/market-policy.json"
        ))?,
    )?;
    let limits = SharedLimits::default();
    let verification = VerificationSettings::new(
        std::env::var("JWT_SECRET")?,
        None,
        "verdaxis-test-api".into(),
        "verdaxis-test-web".into(),
    );
    let mut referral_settings = referrals::ReferralSettings::new(
        verification.clone(),
        "https://app.journey.invalid".to_owned(),
    );
    referral_settings.limits = limits.clone();
    referral_settings.email_from = "Verdaxis <sender@journey.invalid>".into();
    referral_settings.mail = MailClient::loopback(
        std::env::var("PHASE2_MAIL_URL").unwrap_or_else(|_| "http://127.0.0.1:9/emails".into()),
        "disposable-only".into(),
    )?;

    let routes = Router::new()
        .merge(
            notifications::routes(notifications::NotificationAuthority::new(
                verification.clone(),
                limits.clone(),
            ))
            .with_state(state.clone()),
        )
        .merge(preferences::router(
            state.clone(),
            Arc::new(verification.clone()),
            limits.clone(),
        ))
        .merge(
            feedback::routes(Arc::new(verification.clone()), limits.clone())
                .with_state(state.clone()),
        )
        .merge(watchlists::router(
            state.clone(),
            WatchlistSettings::new(verification.clone(), None),
        ))
        .merge(alerts::router(state.clone(), verification, limits.clone()))
        .merge(referrals::routes(referral_settings).with_state(state.clone()))
        .route(
            "/.well-known/verdaxis-disposable-test",
            get(move || {
                let token = token.clone();
                async move { axum::Json(json!({"disposable": true, "token": token})) }
            }),
        );
    let router: Router = verdaxis_read::runtime::http::middleware_with_limits(routes, limits);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
