//! Support helpers for the Phase 5 public read tests.
//!
//! The helpers send a request through the real middleware stack and decode the
//! body, so a test asserts the boundary the client sees: response status,
//! headers and the error envelope. They hold no business data.

use axum::{body::Body, http::Request, response::Response};
use serde_json::Value;
use sqlx::error::DatabaseError;
use tower::ServiceExt;

/// The credentialed browser origin the boundary answers with CORS headers.
pub const ORIGIN: &str = "https://test";
/// Correlation ID the request carries; the boundary echoes it back.
pub const REQUEST_ID: &str = "phase5-public-case";

/// Send one GET through the composed router without a network hop.
pub async fn request(router: axum::Router, uri: &str) -> Response {
    router
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("origin", ORIGIN)
                .header("x-request-id", REQUEST_ID)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Read the response body as JSON. The body bound keeps a runaway response
/// from hiding behind the test allocation.
pub async fn json_body(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// A database failure supplied through SQLx's public error interface.
#[derive(Debug)]
pub struct DatabaseFailure(pub &'static str);

impl std::fmt::Display for DatabaseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for DatabaseFailure {}

impl sqlx::error::DatabaseError for DatabaseFailure {
    fn message(&self) -> &str {
        "Database operation failed."
    }

    fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(std::borrow::Cow::Borrowed(self.0))
    }

    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }
}

/// In-memory writer for a `tracing_subscriber` that a test wants to inspect.
#[derive(Clone, Default)]
pub struct LogCapture(pub std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogCapture {
    /// Parse the captured events, one JSON document per line.
    pub fn events(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
