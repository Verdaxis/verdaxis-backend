//! Captured JSON logs.
//!
//! This lives in its own test target: `tracing` caches each callsite's interest
//! the first time it is hit, so a shared binary that reaches the request-log
//! callsite before any subscriber is installed would never deliver the event.
use axum::{Router, body::Body, http::Request, routing::get};
use tower::ServiceExt;

#[test]
fn request_logs_are_bounded_and_secret_free() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    use verdaxis_read::runtime::http::middleware;
    let app = middleware(Router::new().route("/fixture", get(|| async { "ok" })));
    // Reach the library callsite once with the subscriber installed.
    tracing::info!("capture-ready");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    // A current-thread runtime keeps the future on this thread, where the
    // subscriber default is installed.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        app.oneshot(
            Request::builder()
                .uri("/fixture?token=sesame-sentinel")
                .header("authorization", "Bearer sesame-sentinel")
                .header("x-request-id", "client-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    });
    let captured = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let event: serde_json::Value = serde_json::from_str(captured.trim()).unwrap();
    let fields = &event["fields"];
    assert_eq!(event["target"], "verdaxis_read::runtime::http");
    assert_eq!(fields["request_id"], "client-1");
    assert_eq!(fields["method"], "GET");
    assert_eq!(fields["route"], "/fixture");
    assert_eq!(fields["status"], 200);
    assert!(fields["duration_ms"].is_number());
    assert!(fields.get("error_class").is_some());
    assert!(fields.get("sqlstate").is_some());
    assert!(
        !captured.contains("sesame-sentinel"),
        "secrets reached the log"
    );
    assert!(
        !captured.contains("token="),
        "query strings reached the log"
    );
}
