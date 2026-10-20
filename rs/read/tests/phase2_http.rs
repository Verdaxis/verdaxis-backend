use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::post,
};
use tower::ServiceExt;

#[tokio::test]
async fn known_get_and_support_refusals_are_wrapped_in_cors() {
    let routes = Router::new().route("/api/watchlists/me", post(|| async { "unexpected" }));
    let app = verdaxis_read::runtime::http::middleware(routes);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/watchlists/me/")
                .header("origin", "https://staging.verdaxis.exchange")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 405);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://staging.verdaxis.exchange"
    );
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("STATE_CHANGING_GET_NOT_ALLOWED")
    );
}

#[test]
fn exact_endpoint_limits_and_shared_admin_prefix_reset() {
    use verdaxis_read::auth::limits::Phase2Limits;
    let mut limits = Phase2Limits::default();
    for index in 0..300 {
        assert!(limits.preauth(
            "client",
            if index % 2 == 0 {
                "/api/admin/feedback"
            } else {
                "/api/admin/analytics"
            },
            0
        ));
    }
    assert!(!limits.preauth("client", "/api/admin/feedback", 0));
    assert!(limits.preauth("client", "/api/admin/analytics", 60));
    for (path, limit, window) in [
        ("/api/feedback", 5, 60),
        ("/api/users/me/preferences/tutorial", 30, 60),
        ("/api/referrals/invite", 10, 3600),
        ("/api/admin/feedback", 60, 60),
    ] {
        for _ in 0..limit {
            assert!(limits.endpoint("client", path, 0));
        }
        assert!(!limits.endpoint("client", path, 0));
        assert!(limits.endpoint("client", path, window));
    }
}
