//! Offline checks for the container routes: request ordering before any
//! database work. Row effects are proven by the twin-database scenario.
use axum::{
    body::Body,
    http::{Method, Request},
};
use serde_json::Value;
use tower::ServiceExt;

#[path = "support/watchlist_app.rs"]
mod watchlist_app;
use watchlist_app::app;

async fn call(method: Method, path: &str, body: &str) -> (u16, Value) {
    let response = app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn create_refuses_json_syntax_before_authentication() {
    let (status, body) = call(Method::POST, "/api/watchlists", "{").await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["type"], "json_invalid");
}

#[tokio::test]
async fn create_requires_a_bearer_before_body_validation() {
    for body in ["{}", "{\"name\":\"\"}", "null", ""] {
        let (status, _) = call(Method::POST, "/api/watchlists", body).await;
        assert_eq!(status, 401, "body {body:?}");
    }
}

#[tokio::test]
async fn radar_and_delete_require_a_bearer_before_path_validation() {
    assert_eq!(call(Method::POST, "/api/watchlists/me", "").await.0, 401);
    let (status, _) = call(Method::DELETE, "/api/watchlists/not-a-uuid", "").await;
    assert_eq!(status, 401);
}
