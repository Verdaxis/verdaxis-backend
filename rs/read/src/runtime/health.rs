use crate::{AppState, Json};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// How long one database probe answers for every readiness request. Load
/// balancers and monitors together poll several times a second, and the
/// health pool has one connection, so probing per request made checks queue
/// and time out. One second keeps the database load to about one `SELECT 1`
/// per second per process, while an outage still shows as 503 at most about
/// one second after it starts.
const DATABASE_PROBE_REUSE: Duration = Duration::from_secs(1);

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
///
/// At most one database probe runs at a time. Requests that arrive while it
/// runs wait for it and share its answer, and any answer, good or bad, is
/// reused for `DATABASE_PROBE_REUSE`.
#[derive(Default)]
pub struct DatabaseReadiness {
    latest: tokio::sync::Mutex<Option<ProbeAnswer>>,
    probes_started: AtomicU64,
}

struct ProbeAnswer {
    finished_at: Instant,
    status: StatusCode,
    body: Json,
}

impl DatabaseReadiness {
    pub async fn check(
        &self,
        pool: &sqlx::PgPool,
        environment: &str,
        sha: &str,
    ) -> (StatusCode, Json) {
        // The lock is held for the whole probe: that is what keeps the probe
        // single. Waiting requests then find the answer it stored. If the
        // request running the probe is cancelled, the lock is released with
        // no answer stored and the next request probes instead.
        let mut latest = self.latest.lock().await;
        if let Some(answer) = latest.as_ref()
            && answer.finished_at.elapsed() < DATABASE_PROBE_REUSE
        {
            return (answer.status, answer.body.clone());
        }
        self.probes_started.fetch_add(1, Ordering::Relaxed);
        let (status, body) = probe(pool, environment, sha).await;
        *latest = Some(ProbeAnswer {
            finished_at: Instant::now(),
            status,
            body: body.clone(),
        });
        (status, body)
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
