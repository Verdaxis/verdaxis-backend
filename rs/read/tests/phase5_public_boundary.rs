//! Phase 5 Task 2a: the public market read boundary.
//!
//! These tests exercise the structured read failures through the real
//! middleware composition, so they assert what a browser client receives:
//! status, CORS and correlation headers, and a typed error envelope that holds
//! no partial data and no internal text.
//!
//! `routes::market_data_error` enters the library through the integration
//! owner's `routes/mod.rs` registration, a file this lane may not edit. The
//! test compiles that same source file and supplies the crate-root names the
//! module resolves against the library, so the one file is exercised here until
//! the registration lands.

pub use verdaxis_read::{ApiError, ApiErrorKind, Json, runtime};

#[path = "../src/routes/market_data_error.rs"]
mod market_data_error;

#[path = "support/phase5/public_fixture.rs"]
mod public_fixture;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use public_fixture::{LogCapture, ORIGIN, REQUEST_ID, json_body, request};

fn read_router(path: &'static str, error: fn() -> ApiError) -> Router {
    Router::new().route(path, get(move || async move { error().into_response() }))
}

#[tokio::test]
async fn phase5_public_boundary_cases() {
    let response = request(
        runtime::http::middleware(read_router(
            "/api/availability",
            market_data_error::unavailable,
        )),
        "/api/availability",
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(response.headers()["x-request-id"], REQUEST_ID);
    assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
    assert_eq!(
        response.headers()["access-control-allow-credentials"],
        "true"
    );
    let exposed = response.headers()["access-control-expose-headers"]
        .to_str()
        .unwrap();
    assert!(exposed.contains("Retry-After"), "exposed: {exposed}");
    assert!(exposed.contains("X-Request-ID"), "exposed: {exposed}");

    let body = json_body(response).await;
    assert_eq!(
        body.as_object().unwrap().len(),
        1,
        "one envelope key: {body}"
    );
    let detail = body["detail"].as_object().unwrap();
    let mut keys: Vec<_> = detail.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["code", "message"]);
    assert_eq!(detail["code"], "READ_UNAVAILABLE");
    assert!(!detail["message"].as_str().unwrap().is_empty());
    // A refused read carries no list, so nothing can be read as a short answer.
    assert!(body.get("items").is_none(), "no partial body: {body}");
}

#[tokio::test]
async fn phase5_public_boundary_result_cap() {
    let response = request(
        runtime::http::middleware(read_router(
            "/api/producers",
            market_data_error::result_limit,
        )),
        "/api/producers",
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let body = json_body(response).await;
    assert_eq!(body["detail"]["code"], "READ_RESULT_LIMIT");
    assert!(body.get("items").is_none(), "no partial body: {body}");

    // A statement asks for one row past its bound: a complete page is accepted
    // and the first row above it refuses the whole response.
    for (count, accepted) in [
        (0_u64, true),
        (1, true),
        (512, true),
        (513, false),
        (u64::MAX, false),
    ] {
        assert_eq!(
            market_data_error::within_limit(count, 512).is_ok(),
            accepted,
            "count {count} against a bound of 512"
        );
    }
    assert_eq!(market_data_error::probe_limit(512), 513);
    assert_eq!(market_data_error::probe_limit(5000), 5001);
    let refused = market_data_error::within_limit(513, 512)
        .unwrap_err()
        .into_response();
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        json_body(refused).await["detail"]["code"],
        "READ_RESULT_LIMIT"
    );
}

#[tokio::test]
async fn phase5_public_boundary_invalid_input() {
    let response = request(
        runtime::http::middleware(read_router("/api/demand", || {
            market_data_error::invalid_input("limit must be between 1 and 512")
        })),
        "/api/demand?limit=513",
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.headers().get("retry-after").is_none());
    let body = json_body(response).await;
    assert_eq!(body["detail"]["code"], "INVALID_INPUT");
    assert_eq!(body["detail"]["message"], "limit must be between 1 and 512");
}

#[tokio::test]
async fn phase5_public_boundary_keeps_phase1_envelopes() {
    // Phase 1 market reads keep their current string detail; the Phase 5
    // contract only adds the typed envelope on the new reads.
    let busy = request(
        Router::new().route(
            "/api/prices",
            get(|| async { verdaxis_read::busy().into_response() }),
        ),
        "/api/prices",
    )
    .await;
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(busy.headers()["retry-after"], "1");
    assert!(json_body(busy).await["detail"].is_string());

    let internal = ApiError::internal().into_response();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(json_body(internal).await["detail"].is_string());
}

#[tokio::test]
async fn phase5_diagnostic_fields() {
    let capture = LogCapture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();

    let error = tracing::subscriber::with_default(subscriber, || {
        market_data_error::database_failure("availability", &sqlx::Error::PoolTimedOut)
    });
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(response).await;
    assert_eq!(body["detail"]["code"], "READ_UNAVAILABLE");
    let exposed = body.to_string();
    assert!(
        !exposed.contains("availability"),
        "no stage in body: {exposed}"
    );
    assert!(!exposed.contains("pool"), "no class in body: {exposed}");

    let events = capture.events();
    assert_eq!(events.len(), 1, "one event per failure: {events:?}");
    let fields = events[0]["fields"].as_object().unwrap();
    let mut keys: Vec<_> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["error_class", "message", "sqlstate", "stage"]);
    assert_eq!(fields["stage"], "availability");
    assert_eq!(fields["error_class"], "pool_timeout");
    assert_eq!(fields["sqlstate"], "");
    assert_eq!(fields["message"], "admitted_database_failure");

    // An unexpected database result still uses the scoped structured envelope.
    let error = market_data_error::database_failure("availability", &sqlx::Error::RowNotFound);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await;
    assert_eq!(body["detail"]["code"], "READ_FAILED");
    assert!(!body["detail"]["message"].as_str().unwrap().is_empty());
}
