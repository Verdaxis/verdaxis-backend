//! Disposable-only watchlist HTTP adapter, owned by the shared runner.
use axum::{Json, Router, routing::get};
use verdaxis_read::auth::VerificationSettings;
use verdaxis_read::crud::watchlists::{WatchlistSettings, WriteControls, router};
use verdaxis_read::{AppState, Config, MarketPolicy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test-only adapter".into());
    }
    let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
    if !bind.ip().is_loopback() {
        return Err("loopback required".into());
    }
    let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
    if token.len() < 32 {
        return Err("identity token required".into());
    }
    let config = Config {
        kyc_execution_gate_enabled: std::env::var("KYC_EXECUTION_GATE_ENABLED").as_deref()
            != Ok("false"),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: std::env::var("PHASE2_DATABASE_URL")?,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        bind,
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let state = AppState::new(
        config.clone(),
        verdaxis_read::connect(&config)
            .await
            .map_err(|_| "runtime connect")?,
        serde_json::from_str::<MarketPolicy>(include_str!(
            "../../../deploy/rust/market-policy.json"
        ))?,
    )?;
    let settings = WatchlistSettings::new(
        VerificationSettings::new(
            std::env::var("JWT_SECRET")?,
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ),
        chrono::DateTime::parse_from_rfc3339(&std::env::var("WATCHLIST_TEST_CUTOFF")?)?
            .with_timezone(&chrono::Utc),
    )
    .with_controls(controls);
    let identity = Router::new().route(
        "/.well-known/verdaxis-disposable-test",
        get(move || {
            let token = token.clone();
            async move { Json(serde_json::json!({"disposable":true,"token":token})) }
        }),
    );
    let routes = router(state, settings).merge(identity);
    axum::serve(
        tokio::net::TcpListener::bind(bind).await?,
        verdaxis_read::runtime::http::middleware(routes),
    )
    .await?;
    Ok(())
}

/// Disposable write controls: the runner supplies the same clock, identifier
/// sequence and summary failpoint to both stacks through authenticated headers.
fn controls(headers: &axum::http::HeaderMap) -> WriteControls {
    let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let trusted =
        text("x-disposable-token") == std::env::var("DISPOSABLE_TEST_TOKEN").ok().as_deref();
    let Some(clock) = text("x-test-watchlist-clock").filter(|_| trusted) else {
        return WriteControls::real();
    };
    let ids = text("x-test-watchlist-ids")
        .unwrap_or_default()
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().expect("controlled identifier"))
        .collect();
    WriteControls::controlled(
        clock.parse().expect("controlled clock"),
        ids,
        text("x-test-watchlist-fail-summary") == Some("true"),
    )
}
