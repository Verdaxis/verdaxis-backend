//! Disposable-only generated values and SQL interleaving seam. Never in release builds.
use crate::ApiError;
use axum::{
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Barrier, Mutex};
use uuid::Uuid;

tokio::task_local! { pub(crate) static CONTROLS: HeaderMap; }

pub(crate) fn enabled(headers: &HeaderMap) -> bool {
    std::env::var("ENVIRONMENT").as_deref() == Ok("test")
        && std::env::var("DISPOSABLE_TEST_TOKEN").ok().as_deref()
            == headers
                .get("x-disposable-token")
                .and_then(|v| v.to_str().ok())
}
pub(crate) fn capture() -> HeaderMap {
    CONTROLS.try_with(Clone::clone).unwrap_or_default()
}
fn value(name: &str) -> Option<String> {
    CONTROLS
        .try_with(|h| h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned))
        .ok()
        .flatten()
}
pub(crate) fn id() -> Option<Uuid> {
    value("x-alert-id").and_then(|v| v.parse().ok())
}
pub(crate) async fn scope(
    headers: HeaderMap,
    work: impl std::future::Future<Output = Response>,
) -> Response {
    let after = headers.get("x-alert-fault").and_then(|v| v.to_str().ok()) == Some("after-commit");
    let response = CONTROLS.scope(headers, work).await;
    if after && response.status() == 201 {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"detail":"response unavailable after commit"})),
        )
            .into_response();
    }
    response
}
pub(crate) async fn before_cap_lock() {
    if let Some(group) = value("x-alert-barrier") {
        static BARRIERS: OnceLock<Mutex<HashMap<String, Arc<Barrier>>>> = OnceLock::new();
        let barrier = BARRIERS
            .get_or_init(Default::default)
            .lock()
            .await
            .entry(group)
            .or_insert_with(|| Arc::new(Barrier::new(2)))
            .clone();
        tokio::time::timeout(std::time::Duration::from_secs(10), barrier.wait())
            .await
            .expect("both authenticated users must reach the organization cap lock");
    }
}
pub(crate) async fn after_insert(
    connection: &mut sqlx::PgConnection,
    id: Uuid,
) -> Result<(), ApiError> {
    if let Some(clock) = value("x-alert-clock") {
        sqlx::query("UPDATE price_alerts SET created_at=$1::text::timestamptz WHERE id=$2")
            .bind(clock)
            .bind(id)
            .execute(connection)
            .await
            .map_err(|_| ApiError::database())?;
    }
    if value("x-alert-fault").as_deref() == Some("before-commit") {
        return Err(ApiError::database());
    }
    Ok(())
}

/// Build only the real alert router with the ordinary pool/auth owner.
pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    use crate::{AppState, Config, MarketPolicy};
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test only".into());
    }
    let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err("numeric loopback required".into());
    }
    let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
    if token.len() < 32 {
        return Err("disposable identity required".into());
    }
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: crate::compiled_sha().into(),
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
        phase2: Default::default(),
    };
    // The shared runner names its clones phase2_route_N_{python,rust,mixed},
    // not *_test, so the deployed config connector correctly refuses them.
    // This disposable adapter owns only its assigned app-role pools; it does
    // not relax Config or introduce a deployed environment projection.
    let options: sqlx::postgres::PgConnectOptions = config.database_url.parse()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
        .connect_with(options.clone())
        .await?;
    use sqlx::Row;
    let row = sqlx::query("SELECT current_user,rolsuper,pg_get_userbyid(datdba)=current_user AS owner FROM pg_roles CROSS JOIN pg_database WHERE rolname=current_user AND datname=current_database()")
        .fetch_one(&pool).await?;
    if row.get::<String, _>("current_user") != "verdaxis_phase2_app_test"
        || row.get::<bool, _>("rolsuper")
        || row.get::<bool, _>("owner")
    {
        return Err("assigned nonowner app role required".into());
    }
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let pools = crate::runtime::db::Pools { read: pool, health };
    let policy: MarketPolicy =
        serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json"))?;
    // The contract harness serves one group and does not project Phase 2, so
    // the default options leave `phase2` as `None`.
    let state = AppState::new(config, pools, policy)?;
    let app = super::router(
        state,
        crate::auth::VerificationSettings::new(
            std::env::var("JWT_SECRET")?,
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ),
        crate::auth::limits::SharedLimits::default(),
    )
    .route(
        "/.well-known/verdaxis-disposable-test",
        axum::routing::get(move || async move {
            axum::Json(serde_json::json!({"disposable":true,"token":token}))
        }),
    );
    axum::serve(
        tokio::net::TcpListener::bind(bind).await?,
        crate::runtime::http::middleware(app, &std::env::var("ENVIRONMENT").unwrap_or_default()),
    )
    .await?;
    Ok(())
}
