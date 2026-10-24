pub use verdaxis_read::{ApiError, auth, runtime};
#[path = "../src/auth/logout.rs"]
mod logout;
use axum::{body::Body, http::Request};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
fn state() -> logout::LogoutState {
    logout::LogoutState::new(
        PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(100))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap(),
        "production",
        "synthetic-session-key-at-least-32-characters",
        None,
        "verdaxis",
        "verdaxis-api",
    )
}
fn cleared(response: &axum::response::Response) {
    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    for name in ["refresh_token=", "device_session="] {
        let cookie = cookies.iter().find(|c| c.starts_with(name)).unwrap();
        for attr in [
            "Max-Age=0",
            "Path=/api/auth",
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
        ] {
            assert!(cookie.contains(attr));
        }
    }
}
#[tokio::test]
async fn logout_with_expired_token_still_clears_cookies() {
    let id = uuid::Uuid::new_v4();
    let token = auth::TokenIssuer::new(
        "synthetic-session-key-at-least-32-characters",
        "verdaxis",
        "verdaxis-api",
    )
    .refresh(
        id,
        id,
        "synthetic",
        chrono::Utc::now() - chrono::Duration::days(8),
        None,
    );
    let response = logout::router(state())
        .oneshot(
            Request::post("/api/auth/logout")
                .header("origin", "https://app.verdaxis.exchange")
                .header("cookie", format!("refresh_token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    cleared(&response);
}
#[tokio::test]
async fn logout_error_path_clears_cookies() {
    for origin in ["https://other.invalid", "https://app.verdaxis.exchange"] {
        let response = logout::router(state())
            .oneshot(
                Request::post("/api/auth/logout")
                    .header("origin", origin)
                    .header(
                        "cookie",
                        format!("device_session={}", auth::token_bytes::generate_token()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if origin.contains("other") { 403 } else { 503 }
        );
        cleared(&response);
    }
}
