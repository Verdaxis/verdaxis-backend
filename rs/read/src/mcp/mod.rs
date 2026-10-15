use crate::AppState;

pub fn mcp_router() -> axum::Router<AppState> {
    axum::Router::new()
}
