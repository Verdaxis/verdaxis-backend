//! Shared offline composition of the watchlist router: a lazy pool that never
//! connects, so a request must be refused before any database work.
use axum::Router;
use chrono::{TimeZone, Utc};
use verdaxis_read::crud::watchlists::{WatchlistSettings, router};

pub fn app() -> Router {
    let url = "postgres://unused:unused@127.0.0.1:54321/unused_test";
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: Default::default(),
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect_lazy(url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(url)
            .unwrap(),
    };
    let policy = verdaxis_read::MarketPolicy {
        source_sha: verdaxis_read::compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    let state = verdaxis_read::AppState::new(config, pools, policy).unwrap();
    let settings = WatchlistSettings::new(
        verdaxis_read::auth::VerificationSettings::new(
            "synthetic".into(),
            None,
            "issuer".into(),
            "audience".into(),
        ),
        Some(Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap()),
    );
    router(state, settings)
}
