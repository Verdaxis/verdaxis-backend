use verdaxis_read::{AppState, Config, MarketPolicy, compiled_sha, connect, router};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().json().init();
    if serve().await.is_err() {
        tracing::error!(error_class = "startup", "read service startup failed");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), ()> {
    // Explicit JSON avoids reading the Python service's secret-rich .env file.
    let config: Config =
        serde_json::from_str(&std::env::var("RUST_CONFIG").map_err(|_| ())?).map_err(|_| ())?;
    let pools = connect(&config).await.map_err(|_| ())?;
    let bind = config.bind;
    // Task 5 supplies the policy loader; no data routes are mounted yet.
    let policy = MarketPolicy {
        source_sha: compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    let state = AppState::new(config, pools, policy).map_err(|_| ())?;
    let listener = tokio::net::TcpListener::bind(bind).await.map_err(|_| ())?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|_| ())
}
