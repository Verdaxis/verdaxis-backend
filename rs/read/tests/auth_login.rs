pub use verdaxis_read::{ApiError, auth, runtime};

#[allow(dead_code)]
#[path = "../src/auth/login.rs"]
mod login;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use verdaxis_read::auth::limits::SharedLimits;

async fn state() -> login::LoginState {
    login::LoginState::new(
        PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap(),
        "production",
        "synthetic-login-key-at-least-32-characters",
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    )
    .await
}

fn request(origin: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::post("/api/auth/login")
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::test]
async fn login_requires_trusted_origin() {
    let app = login::router(state().await);
    for origin in [
        "https://app-staging.verdaxis.exchange",
        "https://app.verdaxis.exchange/",
        "https://other.invalid",
    ] {
        let response = app
            .clone()
            .oneshot(request(Some(origin), "username=a&password=b"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["detail"]["code"],
            "BROWSER_ORIGIN_REJECTED"
        );
    }
    let mut req = request(None, "username=a&password=b");
    req.headers_mut()
        .insert("cookie", "refresh_token=synthetic".parse().unwrap());
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn login_is_rate_limited() {
    let state = state().await;
    let now = chrono::Utc::now().timestamp() as u64;
    for _ in 0..5 {
        state
            .limits
            .check_endpoint_by_address("unknown", "/api/auth/login", now)
            .unwrap();
    }
    let response = login::router(state)
        .oneshot(request(
            Some("https://app.verdaxis.exchange"),
            "username=a&password=b",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn login_quota_threshold_and_reset() {
    let state = state().await;
    let now = chrono::Utc::now().timestamp() as u64;
    for _ in 0..5 {
        state
            .limits
            .check_endpoint_by_address("unknown", "/api/auth/login", now - 60)
            .unwrap();
    }
    let app = login::router(state);
    for number in 1..=6 {
        let response = app
            .clone()
            .oneshot(request(None, "username=a"))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if number <= 5 {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
}

#[tokio::test]
async fn login_form_requires_both_fields_and_bounded_body() {
    let app = login::router(state().await);
    let response = app
        .clone()
        .oneshot(request(None, "username=a"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = app
        .oneshot(request(None, &"x".repeat(128 * 1024 + 1)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
