//! Offline checks for the target routes: request ordering before any database
//! work. Row effects, snapshots and conflicts are proven by the twin-database scenario.
use axum::{
    body::Body,
    http::{Method, Request},
};
use serde_json::Value;
use tower::ServiceExt;

#[path = "support/watchlist_app.rs"]
mod watchlist_app;
use watchlist_app::app;

const CONTAINER: &str = "70000000-0000-4000-8000-000000000001";
const TARGET: &str = "72000000-0000-4000-8000-000000000001";

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
    let path = format!("/api/watchlists/{CONTAINER}/targets");
    let (status, body) = call(Method::POST, &path, "{").await;
    assert_eq!(status, 422);
    assert_eq!(body["detail"][0]["type"], "json_invalid");
}

#[tokio::test]
async fn create_requires_a_bearer_before_path_and_body_validation() {
    for path in [
        format!("/api/watchlists/{CONTAINER}/targets"),
        "/api/watchlists/not-a-uuid/targets".to_owned(),
    ] {
        for body in ["{}", "{\"target_type\":\"OTHER\"}", "null", ""] {
            let (status, _) = call(Method::POST, &path, body).await;
            assert_eq!(status, 401, "{path} with {body:?}");
        }
    }
}

#[tokio::test]
async fn delete_requires_a_bearer_before_path_validation() {
    for path in [
        format!("/api/watchlists/{CONTAINER}/targets/{TARGET}"),
        "/api/watchlists/not-a-uuid/targets/also-not".to_owned(),
    ] {
        assert_eq!(call(Method::DELETE, &path, "").await.0, 401, "{path}");
    }
}
