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

#[tokio::test]
async fn split_upstreams_share_one_pre_auth_authority() {
    use axum::routing::get;
    use verdaxis_read::auth::limits::SharedLimits;
    let authority = SharedLimits::default();
    let python_owned =
        Router::new().route("/api/admin/analytics", get(|| async { "python-owned" }));
    let rust_owned = Router::new().route("/api/admin/feedback", get(|| async { "rust-owned" }));
    let app = verdaxis_read::runtime::http::middleware_with_limits(
        python_owned.merge(rust_owned),
        authority.clone(),
    );
    for index in 0..300 {
        let path = if index % 2 == 0 {
            "/api/admin/analytics"
        } else {
            "/api/admin/feedback"
        };
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/admin/feedback")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "60");
    // A successful authenticated and validated endpoint check is separate.
    for _ in 0..5 {
        assert!(authority.check_endpoint("user", "/api/feedback", 0).is_ok());
    }
    assert_eq!(
        authority
            .check_endpoint("user", "/api/feedback", 0)
            .unwrap_err()
            .status,
        429
    );
}

#[test]
fn invalid_authentication_does_not_consume_endpoint_quota_and_floor_is_bounded() {
    use verdaxis_read::auth::limits::{SharedLimits, sample_floor};
    let authority = SharedLimits::default();
    // No endpoint admission is performed for rejected credentials. The route
    // adapter receives this authority only after authentication/validation.
    assert!(
        verdaxis_read::auth::verify_authorization(
            None,
            &verdaxis_read::auth::VerificationSettings::new(
                "synthetic".into(),
                None,
                "issuer".into(),
                "audience".into()
            ),
            chrono::Utc::now()
        )
        .is_err()
    );
    for _ in 0..5 {
        assert!(authority.check_endpoint("user", "/api/feedback", 0).is_ok());
    }
    assert!(
        authority
            .check_endpoint("user", "/api/feedback", 0)
            .is_err()
    );
    assert_eq!(sample_floor(5.0, 60.0), 4);
    assert_eq!(sample_floor(30.0, 60.0), 28);
    assert_eq!(sample_floor(10.0 / 60.0, 3600.0), 9);
    assert_eq!(sample_floor(60.0, 3600.0), 1000);
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
