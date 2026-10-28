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
    let config = load_config().map_err(|_| ())?;
    let pools = connect(&config).await.map_err(|_| ())?;
    let bind = config.bind;
    let policy = committed_policy().map_err(|_| ())?;
    let mut state = AppState::new(config, pools, policy).map_err(|_| ())?;
    log_host_policy(&state.host_policy);
    let streams = verdaxis_read::streams::runtime::start_streams(
        &mut state,
        verdaxis_read::streams::runtime::CapacityConfig::default(),
    )
    .await;
    // Disabled caching starts no task; an enabled cache starts the authority
    // refresh and invalidation loop before the service accepts traffic.
    state.start_trading_cache();
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(|_| ())?;
    let result = axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        if let Some(streams) = streams {
            streams.shutdown().await;
        }
    })
    .await;
    result.map_err(|_| ())
}

/// Say at startup whether the Host check is on, so a missing `ALLOWED_HOSTS`
/// is visible in the journal instead of a silent default.
fn log_host_policy(policy: &verdaxis_read::runtime::host::HostPolicy) {
    if policy.is_enforced() {
        tracing::info!(
            allowed_hosts = policy.configured_hosts().join(","),
            "Host validation is enforced; loopback hosts are always allowed"
        );
    } else {
        tracing::warn!("Host validation is off: ALLOWED_HOSTS is not set");
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("termination signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// The contract harness supplies one `RUST_CONFIG` document; the reviewed
/// staging unit supplies the flat deployed environment plus `--bind` and the
/// `--budget-file` path the deploy transaction wrote. Both routes are built by
/// the same validated `Config` constructor.
fn load_config() -> Result<Config, String> {
    if let Ok(document) = std::env::var("RUST_CONFIG") {
        return serde_json::from_str(&document).map_err(|_| "invalid RUST_CONFIG".to_owned());
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (bind, budget_file) = deployed_arguments(&arguments)?;
    let budget = read_prepared_budget(&budget_file)?;
    // A non-UTF-8 value cannot name any required key; dropping it makes the
    // missing-key error report it instead of aborting the process.
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
