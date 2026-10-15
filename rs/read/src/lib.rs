pub mod runtime;
pub use runtime::config::{check_revision, connection_budget, validate_budget, validate_config};
pub use runtime::db::connect;
pub use runtime::health::{live, ready};

impl ApiError {
    pub fn internal() -> Self {
        Self {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            detail: serde_json::json!("Database operation failed."),
            retry_after: None,
        }
    }
}

pub type Json = serde_json::Value;
pub type ApiResult = Result<axum::response::Response, ApiError>;

#[derive(Clone)]
pub struct AppState {
    pub config: std::sync::Arc<Config>,
    pub pool: sqlx::PgPool,
    pub policy: std::sync::Arc<MarketPolicy>,
    pub reads: std::sync::Arc<tokio::sync::Semaphore>,
}

pub struct ApiError {
    pub status: axum::http::StatusCode,
    pub detail: Json,
    pub retry_after: Option<u32>,
}

#[derive(Clone, serde::Deserialize)]
pub struct Config {
    pub environment: String,
    pub release_sha: String,
    pub migration_revision: String,
    pub database_url: String,
    pub bind: std::net::SocketAddr,
    pub pool_max: u32,
    pub python_budget: u32,
    pub configured_max_connections: u32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MarketPolicy {
    pub source_sha: String,
    pub products: Vec<ProductPolicy>,
    pub points: Vec<PointPolicy>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductPolicy {
    pub id: uuid::Uuid,
    pub code: String,
    pub name: String,
    pub fuel_type: String,
    pub fuel_grade: String,
    pub execution_mode: String,
    pub lanes: Vec<uuid::Uuid>,
    pub qualification_profile: String,
    pub display_order: u32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PointPolicy {
    pub id: uuid::Uuid,
    pub name: String,
    pub region: String,
    pub timezone: String,
    pub display_order: u32,
}

pub fn compiled_sha() -> &'static str {
    env!("VERDAXIS_BUILD_SHA")
}

pub fn verify_release(value: &str) -> Result<(), &'static str> {
    if value == compiled_sha() {
        Ok(())
    } else {
        Err("release identity mismatch")
    }
}
