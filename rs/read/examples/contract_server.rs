//! Test-only contract server: one route group, health, identity, and echo.
//!
//! Compiled only with the `contract-tests` feature, so it can never reach a
//! release binary. It refuses any environment other than `test` and any
//! non-loopback bind, and serves the disposable identity endpoint so the
//! external comparator can attest it before it sends any case.
//!
//! Unimplemented route modules are merged as empty routers, so their paths
//! answer 404 until their owning task lands. That keeps a group's corpus test
//! failing honestly rather than passing on a missing route.

use axum::{
    Router,
    extract::Query,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use std::collections::HashMap;
use verdaxis_read::{AppState, Config, connect};

#[path = "../tests/support/live_benchmarks_http.rs"]
mod live_benchmarks_http;

#[path = "../tests/support/curve_query_metrics.rs"]
mod curve_query_metrics;

const IDENTITY_PATH: &str = "/.well-known/verdaxis-disposable-test";
const ECHO_PATH: &str = "/__contract__/echo";
const PURPOSE: &str = "verdaxis-integration-test";

use verdaxis_read::runtime::policy::committed_policy;

/// The test-only benchmark projection route, owned by no public group.
///
/// The disposable server mounts it for the `live_benchmarks` group and for the
/// combined `all` gate, which must serve every group's corpus cases from one
/// process. The serving binary never mounts it.
fn live_benchmarks_helper() -> Router<AppState> {
    Router::new().route(
        "/__contract__/live-benchmarks",
        get(live_benchmarks_http::project),
    )
}

/// Merge the modules one group owns. Health and echo need no data route.
fn group_router(group: &str) -> Router<AppState> {
    use verdaxis_read::routes as r;
    match group {
        "all" => verdaxis_read::runtime::http::public_routes().merge(live_benchmarks_helper()),
        "health" | "echo" => Router::new(),
        "live_benchmarks" => live_benchmarks_helper(),
        "simple_reads" => r::ports::routes()
            .merge(r::catalog::routes())
            .merge(r::news::routes()),
        "orderbook_snapshot" => r::orderbook_snapshot::routes(),
        "orderbook_lists" => r::orderbook_lists::routes().merge(r::listings::routes()),
        "orderbook_map" => r::orderbook_map::routes().merge(r::listings::routes()),
        "prices" => r::prices::routes().merge(r::trade_tape::routes()),
        "curves_legacy" => r::curves_legacy::routes(),
        "curves_canonical" => r::curves_canonical::routes(),
        "mcp" => verdaxis_read::mcp::mcp_router(),
        _ => Router::new(),
    }
}

async fn identity() -> Response {
    let token = std::env::var("DISPOSABLE_TEST_TOKEN").unwrap_or_default();
    let body = json!({"disposable": true, "purpose": PURPOSE, "token": token});
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
}

async fn echo(Query(params): Query<HashMap<String, String>>) -> Response {
    let value = params.get("value").cloned();
    axum::Json(json!({"echo": value, "method": "GET"})).into_response()
}

#[tokio::main]
async fn main() {
    if let Err(reason) = serve().await {
        eprintln!("contract server failed: {reason}");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), String> {
    let raw = std::env::var("RUST_CONFIG").map_err(|_| "RUST_CONFIG is required")?;
    let config: Config = serde_json::from_str(&raw).map_err(|_| "invalid RUST_CONFIG")?;
    if config.environment != "test" {
        return Err("the contract server requires ENVIRONMENT=test".into());
    }
    if !config.bind.ip().is_loopback() {
        return Err("the contract server requires a loopback bind".into());
    }
    let token = std::env::var("DISPOSABLE_TEST_TOKEN").map_err(|_| "token is required")?;
    if token.len() < 32 {
        return Err("the disposable token is too short".into());
    }
    let group = std::env::var("CONTRACT_GROUP").map_err(|_| "CONTRACT_GROUP is required")?;

    // The combined `all` gate serves the canonical curve group too, so it needs
    // the same disposable measurements the single-group run installs.
    if group == "curves_canonical" || group == "all" {
        curve_query_metrics::install();
    }
    let pools = connect(&config).await.map_err(|_| "connect failed")?;
    let policy = committed_policy()?;
    let state = AppState::new(config.clone(), pools, policy)?;
    // Route through the serving router so fallbacks, trailing-slash redirects,
    // and middleware match the real binary exactly.
    let extra = group_router(&group)
        .route(IDENTITY_PATH, get(identity))
        .route(ECHO_PATH, get(echo));
    let app = verdaxis_read::runtime::http::router_with(extra, state);

    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|_| "bind failed")?;
    println!(
        "contract_server ready group={group} addr={}",
        listener.local_addr().map_err(|_| "no address")?
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|_| "serve failed".to_string())
}
