//! `GET /`: the API banner.
//!
//! Placeholder body until the handler is mounted; the tests in
//! `tests/phase10_root.rs` fail against it.

use axum::Json;
use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct RootBody {
    pub message: &'static str,
    pub version: &'static str,
}

pub async fn root() -> Json<RootBody> {
    Json(RootBody {
        message: "",
        version: "",
    })
}
