pub mod mcp;
pub mod routes;
pub mod runtime;
pub use runtime::admission::{ADMISSION_SLOTS, run_admitted, selected_read, try_read};
pub use runtime::config::{check_revision, connection_budget, validate_budget, validate_config};
pub use runtime::db::connect;
pub use runtime::error::{busy, database_error};
pub use runtime::health::{live, ready};
pub use runtime::http::router;
pub use runtime::logging::request_id;

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
    pub(crate) pools: runtime::db::Pools,
    pub policy: std::sync::Arc<MarketPolicy>,
    pub reads: std::sync::Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(
        config: Config,
        pools: runtime::db::Pools,
        policy: MarketPolicy,
    ) -> Result<Self, String> {
        validate_config(&config)?;
        if pools.read.options().get_max_connections() != ADMISSION_SLOTS
            || pools.health.options().get_max_connections() != 1
        {
            return Err("Phase 1 requires read/health pools of eight/one".into());
        }
        Ok(Self {
            config: std::sync::Arc::new(config),
            pools,
            policy: std::sync::Arc::new(policy),
            reads: std::sync::Arc::new(tokio::sync::Semaphore::new(ADMISSION_SLOTS as usize)),
        })
    }
}

#[derive(Debug)]
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
