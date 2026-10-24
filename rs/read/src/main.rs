use std::collections::HashMap;
use std::path::Path;
use verdaxis_read::runtime::config::{config_from_deployed_environment, deployed_arguments};
use verdaxis_read::runtime::policy::committed_policy;
use verdaxis_read::{AppState, Config, connect, router};

/// The largest prepared budget artifact the binary will read. The real document
/// is a few hundred bytes; a bigger file is not the deploy's artifact.
const MAX_BUDGET_BYTES: u64 = 64 * 1024;

#[tokio::main]
async fn main() {
    // The version document is printed before any database or policy work so an
    // operator and the contract job can attest the exact binary without serving
    // it. The release identity check itself lives in runtime::artifact.
    if std::env::args().nth(1).as_deref() == Some("--version-json") {
        println!("{}", verdaxis_read::version_json());
        return;
    }
    tracing_subscriber::fmt().json().init();
    if serve().await.is_err() {
        tracing::error!(error_class = "startup", "read service startup failed");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), ()> {
    let mut config = load_config().map_err(|_| ())?;
    verdaxis_read::validate_config(&config).map_err(|_| ())?;
    let working_directory = std::env::current_dir().map_err(|_| ())?;
    config.fleet_artifact_path = verdaxis_read::runtime::config::resolve_fleet_artifact_path(
        &config.fleet_artifact_path,
        &working_directory,
    )
    .map_err(|_| ())?;
    verdaxis_read::runtime::config::fleet_startup_probe(&config.fleet_artifact_path).await;
    let pools = connect(&config).await.map_err(|_| ())?;
    let bind = config.bind;
    let policy = committed_policy().map_err(|_| ())?;
    let state = AppState::new(config, pools, policy).map_err(|_| ())?;
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(|_| ())?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|_| ())
}

/// The contract harness supplies one `RUST_CONFIG` document; the reviewed
/// staging unit supplies the flat deployed environment plus `--bind` and the
/// `--budget-file` path the deploy transaction wrote. Both routes are built by
/// the same validated `Config` constructor.
fn load_config() -> Result<Config, String> {
    // Check raw environment encoding before any optional value can default.
    if std::env::var_os("FLEET_ARTIFACT_PATH").is_some_and(|value| value.into_string().is_err()) {
        return Err("invalid FLEET_ARTIFACT_PATH".into());
    }
    if let Ok(document) = std::env::var("RUST_CONFIG") {
        return serde_json::from_str(&document).map_err(|_| "invalid RUST_CONFIG".to_owned());
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (bind, budget_file) = deployed_arguments(&arguments)?;
    let budget = read_prepared_budget(&budget_file)?;
    let environment: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    config_from_deployed_environment(&environment, bind, &budget)
}

/// Read the deploy-written budget artifact: a small regular file, never a
/// symlink that could redirect the read elsewhere.
fn read_prepared_budget(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "the prepared budget artifact is missing".to_owned())?;
    if !metadata.file_type().is_file() {
        return Err("the prepared budget artifact must be a regular file".into());
    }
    if metadata.len() > MAX_BUDGET_BYTES {
        return Err("the prepared budget artifact is too large".into());
    }
    std::fs::read_to_string(path).map_err(|_| "the prepared budget artifact is unreadable".into())
}
