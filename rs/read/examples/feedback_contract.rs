//! Disposable Task 10 adapter. Never part of the deployed binary.
#[cfg(feature = "contract-tests")]
mod adapter {
    use axum::{Router, extract::Request, response::IntoResponse, routing::get};
    use chrono::{DateTime, Utc};
    use std::sync::Arc;
    use tower::ServiceExt;
    use uuid::Uuid;
    use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
    use verdaxis_read::crud::feedback::{Clock, routes_with_clock};
    use verdaxis_read::{AppState, Config};

    pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(std::env::var("ENVIRONMENT")?, "test");
        let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
        assert!(bind.ip().is_loopback());
        let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
        assert!(token.len() >= 32);
        let mut config = Config {
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "kycg_20261014_staging".into(),
            database_url: std::env::var("PHASE2_DATABASE_URL")?,
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind,
            read_admission_slots: 16,
            pool_max: 17,
            python_budget: 52,
            configured_max_connections: 100,
            compliance_eur_usd_rate: None,
            fleet_artifact_path: "data/fleet_demand.json".into(),
            kyc_execution_gate_enabled: true,
            phase2: Default::default(),
        };
        // Runner cell names do not end in _test. This test-only adapter connects
        // to the runner-assigned app role; production configuration is unchanged.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect(&config.database_url)
            .await?;
        let health = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&config.database_url)
            .await?;
        let pools = verdaxis_read::runtime::db::Pools { read: pool, health };
        let policy = serde_json::from_slice(&std::fs::read("deploy/rust/market-policy.json")?)?;
        config.environment = "development".into();
        let state = AppState::new(config, pools, policy)?;
        let settings = Arc::new(VerificationSettings::new(
            std::env::var("JWT_SECRET")?,
            None,
            "verdaxis".into(),
            "verdaxis-api".into(),
        ));
        let app =
            Router::new()
                .route(
                    "/.well-known/verdaxis-disposable-test",
                    get(move || {
                        let token = token.clone();
                        async move {
                            axum::Json(serde_json::json!({"disposable": true, "token": token}))
                        }
                    }),
                )
                .fallback(move |request: Request| {
                    let state = state.clone();
                    let settings = settings.clone();
                    async move {
                        // Deployment keys these quotas by verified user, and the
                        // scenario replays many requests as one user. Giving each
                        // request its own store keeps the scenario a route oracle;
                        // quota itself is covered by the quota contract and the
                        // PostgreSQL route tests.
                        let limits = SharedLimits::default();
                        let instant: DateTime<Utc> = request.headers()["x-feedback-clock"]
                            .to_str()
                            .unwrap()
                            .parse()
                            .unwrap();
                        let id: Uuid = request.headers()["x-feedback-id"]
                            .to_str()
                            .unwrap()
                            .parse()
                            .unwrap();
                        let lost = request.headers().contains_key("x-feedback-response-loss");
                        let request_id = request.headers()["x-request-id"].clone();
                        let router = routes_with_clock(
                            settings,
                            limits.clone(),
                            Clock::Contract { instant, id },
                        )
                        .with_state(state);
                        let app = verdaxis_read::runtime::http::middleware_with_limits(
                            router,
                            limits,
                            &std::env::var("ENVIRONMENT").unwrap_or_default(),
                        );
                        let response = app.oneshot(request).await.unwrap();
                        if lost && response.status().is_success() {
                            (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                [("x-request-id", request_id)],
                                axum::Json(
                                    serde_json::json!({"detail":"response lost after commit"}),
                                ),
                            )
                                .into_response()
                        } else {
                            response
                        }
                    }
                });
        let listener = tokio::net::TcpListener::bind(bind).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok(())
    }
}
#[cfg(feature = "contract-tests")]
#[tokio::main]
async fn main() {
    adapter::serve().await.unwrap();
}
#[cfg(not(feature = "contract-tests"))]
fn main() {
    panic!("feedback adapter requires contract-tests");
}
