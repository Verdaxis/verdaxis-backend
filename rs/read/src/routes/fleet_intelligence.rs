//! Public fleet observation read, independent of database readiness.

#[path = "../market/fleet_snapshot.rs"]
pub mod fleet_snapshot;

use crate::{ApiError, AppState};
use axum::{Json, Router, extract::State, routing::get};

pub const PATH: &str = "/api/fleet-intelligence";

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(fleet))
}

async fn fleet(
    State(state): State<AppState>,
) -> Result<Json<fleet_snapshot::FleetSnapshot>, ApiError> {
    fleet_snapshot::read_snapshot(&state.config.fleet_artifact_path, state.reads.clone())
        .await
        .map(Json)
}
