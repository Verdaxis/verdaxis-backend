#[path = "../market/curve_projection.rs"]
pub mod curve_projection;

use crate::AppState;

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new()
}
