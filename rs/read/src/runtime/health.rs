use crate::{AppState, Json};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::sync::atomic::{AtomicU64, Ordering};

pub async fn probe(pool: &sqlx::PgPool, environment: &str, sha: &str) -> (StatusCode, Json) {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool),
    )
    .await;
    match result {
        Ok(Ok(1)) => (
            StatusCode::OK,
            serde_json::json!({"status":"ok","db":"ok","environment":environment,"release_sha":sha}),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            // A failed probe sends no deployment provenance out of the process.
            serde_json::json!({"status":"error","db":"unavailable"}),
        ),
    }
}

/// The database part of readiness, shared by every request of one process.
#[derive(Default)]
pub struct DatabaseReadiness {
    probes_started: AtomicU64,
}

impl DatabaseReadiness {
    pub async fn check(
        &self,
        pool: &sqlx::PgPool,
        environment: &str,
        sha: &str,
    ) -> (StatusCode, Json) {
        self.probes_started.fetch_add(1, Ordering::Relaxed);
        probe(pool, environment, sha).await
    }

    /// How many database probes this process has started.
    pub fn probes_started(&self) -> u64 {
        self.probes_started.load(Ordering::Relaxed)
    }
}

pub async fn ready(State(state): State<AppState>) -> Response {
    let (mut status, mut body) = state
        .database_readiness
        .check(
            &state.pools.health,
            &state.config.environment,
            &state.config.release_sha,
        )
        .await;
    if !crate::streams::runtime::ready(&state) {
        // A stream failure is a 503 like any other: `db` stays truthful and no
        // deployment provenance leaves the process.
        status = StatusCode::SERVICE_UNAVAILABLE;
        body = serde_json::json!({"status":"error","db":body["db"]});
    }
    (status, axum::Json(body)).into_response()
}

pub async fn live() -> axum::Json<Json> {
    axum::Json(serde_json::json!({"status":"ok"}))
}
