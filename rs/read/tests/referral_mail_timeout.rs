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

/// HTTPX completes the response body before `post()` returns, so a provider
/// that sends a status line and then stalls its body is a transport failure for
/// Python. A status-only reader would call the same response accepted.
#[tokio::test]
async fn provider_body_stall_after_headers_is_a_failed_attempt() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        // Read the whole synthetic request before answering, so the client is
        // waiting on the response body and not still writing its own body.
        while !received.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            received.extend_from_slice(&buffer[..count]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n")
            .await
            .unwrap();
        stream.flush().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client =
        MailClient::loopback(format!("http://{address}/emails"), "test-key".into()).unwrap();
    let sent = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.send(&json!({"to":["synthetic@example.com"]})),
    )
    .await;
    assert!(
        !sent.expect("a stalled body must honor the ten-second deadline"),
        "a 200 status with an unfinished body is not an accepted send"
    );
    server.abort();
    let _ = server.await;
}
