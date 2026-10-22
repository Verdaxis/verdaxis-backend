//! Phase 5 Task 2a: the public market read boundary.
//!
//! These constructor tests check structured read failures: status, CORS and
//! correlation headers where a test sends the error through the HTTP middleware
//! (without database admission), and a typed error envelope with no partial
//! data or internal text. Demand query-limit validation belongs to the demand
//! route tests, not these constructor tests. The admitted transaction helper
//! has its own tests in `http_contract.rs` and `admission_postgres.rs`.

use verdaxis_read::routes::market_data_error;
use verdaxis_read::{ApiError, ApiErrorKind, runtime};

#[path = "support/phase5/public_fixture.rs"]
mod public_fixture;

use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use public_fixture::{DatabaseFailure, LogCapture, ORIGIN, REQUEST_ID, json_body, request};

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
async fn phase5_invalid_input_constructor() {
    // The handler constructs an error; it does not parse or validate the query.
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
async fn phase5_database_failure_cases() {
    for (source, sqlstate, error_class, retryable) in [
        (sqlx::Error::PoolTimedOut, "", "pool_timeout", true),
        (sqlx::Error::PoolClosed, "", "pool_closed", true),
        (sqlx::Error::RowNotFound, "", "row_not_found", false),
    ]
    .into_iter()
    .chain(
        [
            ("55P03", true),
            ("40P01", true),
            ("40001", true),
            ("57014", true),
            ("23505", false),
        ]
        .into_iter()
        .map(|(code, retryable)| {
            (
                sqlx::Error::Database(Box::new(DatabaseFailure(code))),
                code,
                "database",
                retryable,
            )
        }),
    ) {
        assert_database_failure(source, sqlstate, error_class, retryable).await;
    }
}

async fn assert_database_failure(
    source: sqlx::Error,
    sqlstate: &str,
    error_class: &str,
    retryable: bool,
) {
    let capture = LogCapture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    let error = tracing::subscriber::with_default(subscriber, || {
        market_data_error::database_failure("availability", &source)
    });
    assert_eq!(error.kind, ApiErrorKind::Database, "{source:?}");

    let response = error.into_response();
    let (status, code, message) = if retryable {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "READ_UNAVAILABLE",
            "Market data is temporarily unavailable; retry shortly.",
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "READ_FAILED",
            "Market data could not be read.",
        )
    };
    assert_eq!(response.status(), status, "{source:?}");
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .map(|value| value.to_str().unwrap()),
        retryable.then_some("1"),
        "{source:?}"
    );
    assert_eq!(
        json_body(response).await,
        serde_json::json!({"detail": {"code": code, "message": message}}),
        "{source:?}"
    );

    let events = capture.events();
    assert_eq!(events.len(), 1, "one event per failure: {source:?}");
    let fields = events[0]["fields"].as_object().unwrap();
    let mut keys: Vec<_> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["error_class", "message", "sqlstate", "stage"]);
    assert_eq!(fields["stage"], "availability");
    assert_eq!(fields["error_class"], error_class);
    assert_eq!(fields["message"], "admitted_database_failure");
    assert_eq!(fields["sqlstate"], sqlstate);
    let code = fields["sqlstate"].as_str().unwrap();
    assert!(
        code.is_empty()
            || (code.len() == 5
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    );
}

#[tokio::test]
async fn phase5_diagnostic_fields() {
    // SQLSTATE metadata is either five uppercase ASCII letters/digits or empty.
    for code in ["", "2350", "235050", "abcde", "23é5", "23 05"] {
        assert_database_failure(
            sqlx::Error::Database(Box::new(DatabaseFailure(code))),
            "",
            "database",
            false,
        )
        .await;
    }
}
