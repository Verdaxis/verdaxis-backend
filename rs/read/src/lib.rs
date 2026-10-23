pub mod auth;
pub mod business;
pub mod crud;
pub mod market;
pub mod mcp;
pub mod routes;
pub mod runtime;

// ---------------------------------------------------------------------------
// Phase 2 Task 20 registration block.
//
// The one shared-file change the order-book write boundary needs: the module
// must be reachable for `rs/read/tests/order_write_boundary.rs`. This module
// mounts no route. The integration owner merges this block into
// `feat/rust-phase-2`; nothing else in this file belongs to Task 20.
pub mod order_write;
// ----------------------- end Task 20 registration block --------------------

pub use market::decimal::decimal_text;
pub use market::provenance::MarketEvidenceScope;
pub use market::terms::{
    QualificationInput, canonical_terms_from_row, digest_terms, normalize_certification_scheme,
    order_is_execution_qualified, order_is_execution_qualified_at, redact_terms,
    validate_fame_terms, validate_lng_terms,
};
pub use market::visibility::{
    PublicOrderFilters, TradeEvidenceFilters, VisibilityClock, order_query_sql_projected,
    public_order_sql, select_public_order_ids, select_trade_evidence_ids, trade_evidence_sql,
};
pub use market::windows::{
    normalize_window, window_display_label, window_sort_key, window_start_date,
};
pub use runtime::admission::{DEFAULT_READ_ADMISSION_SLOTS, run_admitted, selected_read, try_read};
pub use runtime::artifact::{
    ARTIFACT_VERSION, LIBC_MIN, verify_artifact_identity, verify_compiled_identity, version_json,
};
pub use runtime::config::{check_revision, connection_budget, validate_budget, validate_config};
pub use runtime::db::connect;
pub use runtime::error::{busy, database_error};
pub use runtime::health::{live, ready};
pub use runtime::http::router;
pub use runtime::logging::request_id;

impl ApiError {
    /// Internal classification only; REST status/body remain unchanged.
    pub fn database() -> Self {
        Self {
            kind: ApiErrorKind::Database,
            ..Self::internal()
        }
    }

    pub fn internal() -> Self {
        Self {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            detail: serde_json::json!("Database operation failed."),
            retry_after: None,
            kind: ApiErrorKind::Unexpected,
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
    pub reference_cache:
        std::sync::Arc<runtime::cache::ReferenceCache<runtime::cache::ReferenceKey>>,
}

impl AppState {
    pub fn new(
        config: Config,
        pools: runtime::db::Pools,
        policy: MarketPolicy,
    ) -> Result<Self, String> {
        validate_config(&config)?;
        if pools.read.options().get_max_connections() != config.read_admission_slots
            || pools.health.options().get_max_connections() != 1
        {
            return Err("read pool must match admission slots; health pool must be one".into());
        }
        let reads = std::sync::Arc::new(tokio::sync::Semaphore::new(
            config.read_admission_slots as usize,
        ));
        Ok(Self {
            config: std::sync::Arc::new(config),
            pools,
            policy: std::sync::Arc::new(policy),
            reads,
            reference_cache: std::sync::Arc::new(runtime::cache::ReferenceCache::default()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    Expected,
    Database,
    Unexpected,
}

#[derive(Debug)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub status: axum::http::StatusCode,
    pub detail: Json,
    pub retry_after: Option<u32>,
}

#[derive(Clone, serde::Deserialize)]
pub struct Config {
    /// Runtime-owned KYC_EXECUTION_GATE_ENABLED; never a request filter.
    #[serde(default = "kyc_gate_default", alias = "KYC_EXECUTION_GATE_ENABLED")]
    pub kyc_execution_gate_enabled: bool,
    pub environment: String,
    pub release_sha: String,
    pub migration_revision: String,
    pub database_url: String,
    /// Required database session policy; never inherit SQLx's implicit UTC.
    #[serde(alias = "DB_SESSION_TIME_ZONE")]
    pub db_session_time_zone: String,
    /// The prepared budget artifact text (`budget.json`). The staging unit
    /// passes its path with `--budget-file`; the contract harness may carry the
    /// document itself in `RUST_CONFIG`. Required for a staging start.
    #[serde(default)]
    pub prepared_budget: Option<String>,
    pub bind: std::net::SocketAddr,
    #[serde(default = "read_admission_default", alias = "READ_ADMISSION_SLOTS")]
    pub read_admission_slots: u32,
    /// Total Rust connections: read admission slots plus one readiness connection.
    pub pool_max: u32,
    pub python_budget: u32,
    pub configured_max_connections: u32,
}

fn read_admission_default() -> u32 {
    DEFAULT_READ_ADMISSION_SLOTS
}

fn kyc_gate_default() -> bool {
    true
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
    /// Fixed-biofuel ASK contract (`BIOFUEL_SPECIFICATION_STANDARDS` in the
    /// Python oracle). Absent for every product without a fixed standard; it
    /// is data, so no executable source names a product.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specification_standard: Option<String>,
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
