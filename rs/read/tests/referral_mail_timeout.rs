//! Correctness deadlines only; not a latency measurement.
use serde_json::json;
use verdaxis_read::crud::referrals::invite::MailClient;

#[tokio::test]
async fn provider_timeout_records_one_attempt_and_never_replays() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let attempts = Arc::new(AtomicUsize::new(0));
    let ledger = attempts.clone();
    let app = axum::Router::new().route(
        "/emails",
        axum::routing::post(move || {
            let ledger = ledger.clone();
            async move {
                ledger.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<axum::http::StatusCode>().await
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client =
        MailClient::loopback(format!("http://{address}/emails"), "test-key".into()).unwrap();
    let sent = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.send(&json!({"to":["synthetic@example.com"]})),
    )
    .await;
    assert!(!sent.expect("provider call must honor its ten-second deadline"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    server.abort();
    let _ = server.await;
}
