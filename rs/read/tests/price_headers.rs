use axum::{Router, body::Body, http::Request, routing::get};
use tower::ServiceExt;
use verdaxis_read::runtime::http::{middleware, unhandled_server_error};

#[tokio::test]
async fn only_unhandled_responses_skip_the_complete_header_matrix() {
    let app = middleware(
        Router::new()
            .route("/unhandled", get(|| async { unhandled_server_error() }))
            .route(
                "/handled",
                get(|| async { verdaxis_read::ApiError::internal() }),
            )
            .route("/busy", get(|| async { verdaxis_read::busy() })),
    );
    for origin in [
        None,
        Some("https://staging.verdaxis.exchange"),
        Some("https://denied.invalid"),
    ] {
        for path in ["/unhandled", "/handled", "/busy"] {
            let mut request = Request::builder()
                .uri(path)
                .header("x-request-id", "price-header-test");
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let handled = path != "/unhandled";
            let cors = handled && origin.is_some();
            let allowed = cors && origin == Some("https://staging.verdaxis.exchange");
            assert_eq!(response.headers().contains_key("x-request-id"), handled);
            if handled {
                assert_eq!(response.headers()["x-request-id"], "price-header-test");
            }
            for header in [
                "access-control-allow-credentials",
                "access-control-expose-headers",
            ] {
                assert_eq!(
                    response.headers().contains_key(header),
                    cors,
                    "{path}: {header}"
                );
            }
            for header in ["access-control-allow-origin", "vary"] {
                assert_eq!(
                    response.headers().contains_key(header),
                    allowed,
                    "{path}: {header}"
                );
            }
            assert_eq!(
                response.headers().contains_key("retry-after"),
                path == "/busy"
            );
        }
    }
}
