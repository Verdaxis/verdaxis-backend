//! `GET /`: the API banner.
//!
//! The handler reads no database, identity or configuration, so it answers
//! while every pool is down. The body is the two fixed values the Python
//! application returns, with `message` first.

use axum::Json;
use serde::Serialize;

pub const MESSAGE: &str = "Verdaxis API is running";
pub const VERSION: &str = "1.0.0";

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RootBody {
    pub message: &'static str,
    pub version: &'static str,
}

pub async fn root() -> Json<RootBody> {
    Json(RootBody {
        message: MESSAGE,
        version: VERSION,
    })
}
