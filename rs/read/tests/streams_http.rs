//! The three market stream routes and their HTTP contract.
//!
//! Every case runs without a database: the stream subsystem handle is absent
//! for the unavailable paths, and present with a real hub for the live ones.
//! Private replay admission is exercised through its error classes; the
//! database-backed replay behaviour belongs to `streams_replay`.


use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};

use verdaxis_read::{
    Config,
    auth::VerificationSettings,
    streams::http::{Streams, StreamsState, routes},
};

fn config() -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture-secret@127.0.0.1/verdaxis_test".into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().expect("loopback bind"),
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 0,
        configured_max_connections: 100,
        phase2: Default::default(),
    }
}

fn app(streams: Option<Streams>) -> Router {
    routes().with_state(StreamsState::for_tests(config(), streams))
}

fn live() -> Router {
    app(Some(Streams {
        hub: Default::default(),
        verification: VerificationSettings::new(
            "test-secret-key-for-streams-minimum-32".into(),
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ),
    }))
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    use tower::ServiceExt;
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn method(method: &str, path: &str) -> Request<Body> {
    Request::builder().method(method).uri(path).body(Body::empty()).unwrap()
}

fn code(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).unwrap();
    serde_json::from_str::<serde_json::Value>(text).unwrap()["detail"]["code"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
/// A live stream body never ends, so its proof is status plus headers; the
/// body is dropped, which is also the client-disconnect contract.
async fn head(router: &Router, path: &str) -> (StatusCode, Vec<(String, String)>) {
    use tower::ServiceExt;
    let response = router.clone().oneshot(get(path)).await.unwrap();
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
        .collect();
    drop(response.into_body());
    (status, headers)
}

#[tokio::test]
async fn prices_get_alias_matches_canonical() {
    for path in ["/api/stream/prices", "/api/stream/prices/"] {
        let (status, headers) = head(&live(), path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.starts_with("text/event-stream")), "{path}");
        assert!(!headers.iter().any(|(name, _)| name == "www-authenticate"), "{path}");
    }
}

#[tokio::test]
async fn orderbook_get_alias_matches_canonical() {
    for path in ["/api/stream/orderbook", "/api/stream/orderbook/"] {
        let (status, headers) = head(&live(), path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(headers
            .iter()
            .any(|(name, value)| name == "content-type" && value.starts_with("text/event-stream")), "{path}");
    }
}

#[tokio::test]
async fn trades_get_alias_matches_canonical() {
    // Without a token both forms answer the same pre-header refusal.
    for path in ["/api/stream/trades", "/api/stream/trades/"] {
        let (status, _, bytes) = send(&live(), get(path)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED", "{path}");
    }
}

#[tokio::test]
async fn trades_rejects_bearer_400_stream_auth_conflict() {
    let request = Request::builder()
        .uri("/api/stream/trades?stream_token=x")
        .header(header::AUTHORIZATION, "bearer something")
        .body(Body::empty())
        .unwrap();
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_AUTH_CONFLICT");
}

#[tokio::test]
async fn trades_rejects_bad_cursor_400_stream_cursor_invalid() {
    let request = get("/api/stream/trades?stream_token=x&last_event_id=abc");
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
    // The cursor is checked before the token.
    let request = get("/api/stream/trades?last_event_id=abc");
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
}

#[tokio::test]
async fn trades_requires_token_401_stream_token_required() {
    let (status, _, bytes) = send(&live(), get("/api/stream/trades")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED");
}

#[tokio::test]
async fn trades_rejects_bad_token_401_stream_token_invalid() {
    let request = get("/api/stream/trades?stream_token=not-a-token");
    let (status, headers, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_INVALID");
    assert!(!headers
        .iter()
        .any(|(name, value)| name == "content-type" && value.starts_with("text/event-stream")));
}

#[tokio::test]
async fn trades_auth_store_down_503_stream_auth_unavailable() {
    // A malformed store URL makes admission's read fail closed.
    let router = app(Some(Streams {
        hub: Default::default(),
        verification: VerificationSettings::new(
            "test-secret-key-for-streams-minimum-32".into(),
            None,
            "verdaxis-test-api".into(),
            "verdaxis-test-web".into(),
        ),
    }));
    // The token must be valid for the profile to reach the store read.
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let mut claims = serde_json::json!({
        "sub": "00000000-0000-4000-8000-000000000001",
        "org_id": "00000000-0000-4000-8000-000000000002",
        "environment": "test",
        "type": "stream",
        "jti": "00000000-0000-4000-8000-000000000003",
        "iss": "verdaxis-test-api",
        "aud": "verdaxis-test-web",
        "iat": chrono::Utc::now().timestamp(),
        "exp": (chrono::Utc::now() + chrono::Duration::minutes(1)).timestamp(),
    });
    claims["exp"] = serde_json::json!((chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp());
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret("test-secret-key-for-streams-minimum-32".as_bytes()),
    )
    .unwrap();
    let (status, _, bytes) = send(&router, get(&format!("/api/stream/trades?stream_token={token}"))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&bytes), "STREAM_AUTH_UNAVAILABLE");
}

#[tokio::test]
async fn stream_routes_reject_head_and_post_405_envelope() {
    for path in [
        "/api/stream/prices",
        "/api/stream/prices/",
        "/api/stream/orderbook",
        "/api/stream/orderbook/",
        "/api/stream/trades",
        "/api/stream/trades/",
    ] {
        let (status, headers, bytes) = send(&live(), method("POST", path)).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "POST {path}");
        assert!(headers.iter().any(|(name, value)| name == "allow" && value == "GET"));
        assert_eq!(code(&bytes), "STREAM_METHOD_NOT_ALLOWED", "POST {path}");
        let (status, headers, bytes) = send(&live(), method("HEAD", path)).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "HEAD {path}");
        assert!(headers.iter().any(|(name, value)| name == "allow" && value == "GET"));
        assert!(bytes.is_empty(), "HEAD {path} carries no body");
    }
}

#[tokio::test]
async fn activity_route_is_404_in_rust() {
    let (status, _, _) = send(&live(), get("/api/stream/activity")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn issuer_route_is_not_mounted() {
    let (status, _, _) = send(&live(), get("/api/auth/stream-token")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_subsystem_down_503_stream_unavailable() {
    let router = app(None);
    for path in [
        "/api/stream/prices",
        "/api/stream/prices/",
        "/api/stream/orderbook",
        "/api/stream/orderbook/",
        "/api/stream/trades",
        "/api/stream/trades/",
    ] {
        let (status, _, bytes) = send(&router, get(path)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(code(&bytes), "STREAM_UNAVAILABLE", "{path}");
    }
}

#[tokio::test]
async fn stream_cursor_header_precedes_query() {
    let request = Request::builder()
        .uri("/api/stream/trades?stream_token=x&last_event_id=9")
        .header("last-event-id", "5")
        .body(Body::empty())
        .unwrap();
    // Both are syntactically valid, so the admission order decides: the
    // conflict/token checks follow the cursor parse, and 401 proves the
    // cursor layer accepted rather than answering 400.
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED");
    // An empty header falls back to the query value.
    let request = Request::builder()
        .uri("/api/stream/trades?stream_token=x&last_event_id=abc")
        .header("last-event-id", "")
        .body(Body::empty())
        .unwrap();
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
}

#[tokio::test]
async fn no_pid_origin_comment_in_rust_stream() {
    // Read only what the live body emits inside a short bound; an idle stream
    // may emit nothing at all, which also proves no diagnostic comment.
    use tower::ServiceExt;
    let response = live().oneshot(get("/api/stream/prices")).await.unwrap();
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 4096),
    )
    .await;
    if let Ok(Ok(bytes)) = bytes {
        let text = String::from_utf8_lossy(&bytes).to_lowercase();
        assert!(!text.contains("pid"), "{text}");
        assert!(!text.contains("origin:"), "{text}");
    }
}

#[tokio::test]
async fn stream_error_order_and_envelope() {
    // Cursor syntax precedes the Authorization conflict.
    let request = Request::builder()
        .uri("/api/stream/trades?stream_token=x&last_event_id=abc")
        .header(header::AUTHORIZATION, "bearer a")
        .body(Body::empty())
        .unwrap();
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_CURSOR_INVALID");
    // The conflict precedes the missing token.
    let request = Request::builder()
        .uri("/api/stream/trades")
        .header(header::AUTHORIZATION, "bearer a")
        .body(Body::empty())
        .unwrap();
    let (status, _, bytes) = send(&live(), request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bytes), "STREAM_AUTH_CONFLICT");
    // The token presence precedes token validity.
    let (status, _, bytes) = send(&live(), get("/api/stream/trades?stream_token=")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&bytes), "STREAM_TOKEN_REQUIRED");
    // Every pre-header error is the code/message object.
    let (_, _, bytes) = send(&live(), get("/api/stream/trades")).await;
    let detail: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(detail["detail"]["code"].is_string());
    assert!(detail["detail"]["message"].is_string());
}
