use crate::{AppState, Json};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

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
            serde_json::json!({"status":"error","db":"unavailable"}),
        ),
    }
}

pub async fn ready(State(state): State<AppState>) -> Response {
    if !crate::streams::runtime::ready(&state) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "status":"error", "streams":"unavailable"
            })),
        )
            .into_response();
    }
    let (status, body) = probe(
        &state.pools.health,
        &state.config.environment,
        &state.config.release_sha,
    )
    .await;
    (status, axum::Json(body)).into_response()
}

pub async fn live() -> axum::Json<Json> {
    axum::Json(serde_json::json!({"status":"ok"}))
}
