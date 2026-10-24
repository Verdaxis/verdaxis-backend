//! Disposable-only Task 9 server. Never reads release settings or real keys.
#[cfg(not(feature = "contract-tests"))]
fn main() {
    panic!("preferences_server requires contract-tests");
}

#[cfg(feature = "contract-tests")]
mod disposable {

    use axum::{Json, Router, http::HeaderMap, routing::get};
    use serde_json::json;
    use std::sync::Arc;
    use verdaxis_read::{AppState, Config, MarketPolicy};

    #[tokio::main]
    pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
            return Err("test only".into());
        }
        let bind: std::net::SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
        if !bind.ip().is_loopback() || bind.port() == 0 {
            return Err("loopback ephemeral bind required".into());
        }
        let token = std::env::var("DISPOSABLE_TEST_TOKEN")?;
        if token.len() < 32 {
            return Err("identity token required".into());
        }
        let config = Config {
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "p0_20261016_union".into(),
            database_url: std::env::var("PHASE2_DATABASE_URL")?,
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind,
            read_admission_slots: 16,
            pool_max: 17,
            python_budget: 52,
            configured_max_connections: 100,
            kyc_execution_gate_enabled: true,
        };
        let pools = verdaxis_read::connect(&config)
            .await
            .map_err(|_| "database connection failed")?;
        let state = AppState::new(
            config.clone(),
            pools,
            MarketPolicy {
                source_sha: config.release_sha,
                products: vec![],
                points: vec![],
            },
        )?;
        let verification = Arc::new(verdaxis_read::auth::VerificationSettings::new(
            "test-secret-key-for-testing-minimum-32-chars".into(),
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ));
        let expected = token.clone();
        let app = verdaxis_read::crud::preferences::contract_router(
            state,
            verification,
            verdaxis_read::auth::limits::SharedLimits::default(),
            controls,
        )
        .merge(Router::new().route(
            "/.well-known/verdaxis-disposable-test",
            get(move || async move { Json(json!({"disposable":true,"token":expected})) }),
        ));
        let app = app.layer(axum::middleware::from_fn(response_loss));
        let app = verdaxis_read::runtime::http::middleware(
            app,
            &std::env::var("ENVIRONMENT").unwrap_or_default(),
        );
        axum::serve(
            tokio::net::TcpListener::bind(bind).await?,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok(())
    }

    async fn response_loss(
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let lost = request
            .headers()
            .get("x-test-preference-response-loss")
            .is_some();
        let response = next.run(request).await;
        if lost {
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"detail":"Disposable response lost after commit"})),
            )
                .into_response()
        } else {
            response
        }
    }

    fn controls(headers: &HeaderMap) -> (chrono::DateTime<chrono::Utc>, uuid::Uuid) {
        assert_eq!(
            headers["x-disposable-token"].to_str().unwrap(),
            std::env::var("DISPOSABLE_TEST_TOKEN").unwrap()
        );
        (
            headers["x-test-preference-clock"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
            headers["x-test-preference-id"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap(),
        )
    }
}

#[cfg(feature = "contract-tests")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    disposable::run()
}
