//! Disposable ingress qualification using the production quota middleware.
use axum::{Router, routing::get};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ENVIRONMENT").as_deref() != Ok("test") {
        return Err("test environment required".into());
    }
    let bind: SocketAddr = std::env::var("PHASE2_BIND")?.parse()?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err("numeric loopback port required".into());
    }
    let routes = Router::new()
        .route("/health/live", get(|| async { "ok" }))
        .route("/api/admin/feedback", get(|| async { "rust-owned" }));
    let app = verdaxis_read::runtime::http::middleware_with_limits(
        routes,
        verdaxis_read::auth::limits::SharedLimits::default(),
        &std::env::var("ENVIRONMENT")?,
    );
    axum::serve(
        tokio::net::TcpListener::bind(bind).await?,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
