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
    let app = verdaxis_read::runtime::http::middleware(routes, "staging");
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
        "staging",
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

#[tokio::test]
async fn request_pipeline_enforces_syntax_auth_validation_quota_order() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use verdaxis_read::auth::limits::SharedLimits;
    use verdaxis_read::auth::request::{RequestAdmission, admit_request};
    let limits = SharedLimits::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let context = RequestAdmission {
        client: "client".into(),
        path: "/api/feedback".into(),
        now_seconds: 0,
        limits: limits.clone(),
    };
    for (raw, authenticated, expected) in [("{", false, 422), ("{}", false, 401), ("{}", true, 422)]
    {
        let calls = calls.clone();
        let result=admit_request(raw.as_bytes(), &context, move || async move {
            calls.fetch_add(1,Ordering::SeqCst);
            if authenticated {Ok(())}else{Err(verdaxis_read::auth::AuthError::Credentials)}
        }, |body| {
            if body.get("message").and_then(serde_json::Value::as_str).is_some() {Ok(body)} else {
                Err(verdaxis_read::ApiError { status:axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    detail:serde_json::json!([{"type":"missing","loc":["body","message"],"msg":"Field required","input":{}}]),
                    retry_after:None,kind:verdaxis_read::ApiErrorKind::Expected })
            }
        }).await;
        assert_eq!(result.unwrap_err().status(), expected);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "JSON syntax must fail before auth"
    );
    for _ in 0..5 {
        assert!(
            admit_request(b"{\"message\":\"ok\"}", &context, || async { Ok(()) }, Ok)
                .await
                .is_ok()
        );
    }
    let rejected = admit_request(b"{\"message\":\"ok\"}", &context, || async { Ok(()) }, Ok)
        .await
        .unwrap_err();
    assert_eq!(rejected.status(), 429);
}

#[tokio::test]
async fn admission_order_is_observed_through_http_and_early_cors() {
    use axum::{extract::Request as AxumRequest, response::IntoResponse};
    use verdaxis_read::auth::{
        limits::SharedLimits,
        request::{RequestAdmission, admit_request},
    };
    let limits = SharedLimits::default();
    let routes = Router::new().route(
        "/api/feedback",
        post(move |request: AxumRequest| {
            let limits = limits.clone();
            async move {
                let authorized = request
                    .headers()
                    .get("authorization")
                    .is_some_and(|v| v == "Bearer test-admitted");
                let body = to_bytes(request.into_body(), 8192).await.unwrap();
                let context = RequestAdmission {
                    client: "client".into(),
                    path: "/api/feedback".into(),
                    now_seconds: 0,
                    limits,
                };
                let outcome = admit_request(
                    &body,
                    &context,
                    || async move {
                        if authorized {
                            Ok(())
                        } else {
                            Err(verdaxis_read::auth::AuthError::Credentials)
                        }
                    },
                    |value| {
                        if value
                            .get("message")
                            .and_then(serde_json::Value::as_str)
                            .is_some()
                        {
                            Ok(value)
                        } else {
                            Err(verdaxis_read::ApiError {
                                status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                                detail: serde_json::json!("missing message"),
                                retry_after: None,
                                kind: verdaxis_read::ApiErrorKind::Expected,
                            })
                        }
                    },
                )
                .await;
                match outcome {
                    Ok(value) => axum::Json(value).into_response(),
                    Err(error) => error.into_response(),
                }
            }
        }),
    );
    let app = verdaxis_read::runtime::http::middleware(routes, "staging");
    for (body, token, expected) in [
        ("{", "bad", 422),
        ("{}", "bad", 401),
        ("{}", "test-admitted", 422),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/feedback")
                    .header("authorization", format!("Bearer {token}"))
                    .header("origin", "https://staging.verdaxis.exchange")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://staging.verdaxis.exchange"
        );
    }
    for index in 0..6 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/feedback")
                    .header("authorization", "Bearer test-admitted")
                    .body(Body::from("{\"message\":\"ok\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if index < 5 { 200 } else { 429 }
        );
    }
}

#[test]
fn approved_json_wording_difference_keeps_status_type_and_absolute_location() {
    let ledger: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/json-syntax-differences.json"
    ))
    .unwrap();
    for vector in ledger["vectors"].as_array().unwrap() {
        let error = verdaxis_read::auth::request::parse_json_body(
            vector["raw"].as_str().unwrap().as_bytes(),
        )
        .unwrap_err();
        let observed = serde_json::json!({"status":error.status.as_u16(),"type":error.detail[0]["type"],"loc":error.detail[0]["loc"]});
        assert_eq!(observed, vector["rust_expected"], "{}", vector["id"]);
        assert_eq!(vector["python"], vector["rust_expected"]);
        assert_eq!(vector["approved_difference"], "msg and ctx.error only");
    }
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
