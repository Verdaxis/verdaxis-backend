//! Task 4: the auth HTTP boundary.
//!
//! Covers the two session cookies, the trusted-origin rule, the auth error
//! envelope, the pre-auth and endpoint limits of the auth routes, the auth
//! body limit, and the client address those limits count by.

use std::collections::HashSet;
use std::net::SocketAddr;

use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use futures_util::stream;
use serde_json::{Value, json};
use tower::ServiceExt;
use verdaxis_read::auth::cookies::{
    AuthCookies, COOKIE_MAX_AGE_SECONDS, DEVICE_COOKIE, Presented, REFRESH_COOKIE,
};
use verdaxis_read::auth::error::{AUTH_BODY_LIMIT_BYTES, AuthCode, AuthFailure, read_auth_body};
use verdaxis_read::auth::limits::{Phase2Limits, Refusal, SharedLimits};
use verdaxis_read::auth::origin::{require_trusted_origin, trusted_origins};
use verdaxis_read::auth::token_bytes::generate_token;
use verdaxis_read::runtime::http::{allowed_browser_origin, middleware, middleware_with_limits};

/// A refresh token stand-in: three base64url segments, as a JWT has.
const REFRESH_VALUE: &str = "eyJhbGciOiJIUzI1NiJ9.eyJ0eXBlIjoicmVmcmVzaCJ9.c2lnbmF0dXJl";

/// The `Set-Cookie` headers a jar renders, raw and parsed.
fn set_cookies(jar: CookieJar) -> Vec<(String, Cookie<'static>)> {
    jar.into_response()
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| {
            let raw = value.to_str().unwrap().to_owned();
            (raw.clone(), Cookie::parse(raw).unwrap())
        })
        .collect()
}

fn named<'a>(
    cookies: &'a [(String, Cookie<'static>)],
    name: &str,
) -> (&'a str, &'a Cookie<'static>) {
    let mut matches = cookies.iter().filter(|(_, cookie)| cookie.name() == name);
    let (raw, cookie) = matches.next().unwrap_or_else(|| panic!("{name} is set"));
    assert!(matches.next().is_none(), "{name} is set once");
    (raw.as_str(), cookie)
}

/// The attribute set both session cookies share, set or cleared.
fn assert_session_attributes(raw: &str, cookie: &Cookie<'_>, secure: bool) {
    assert_eq!(cookie.http_only(), Some(true), "{raw}");
    assert_eq!(cookie.secure().unwrap_or(false), secure, "{raw}");
    assert_eq!(raw.contains("Secure"), secure, "{raw}");
    assert_eq!(cookie.same_site(), Some(SameSite::Lax), "{raw}");
    assert_eq!(cookie.path(), Some("/api/auth"), "{raw}");
    assert_eq!(cookie.domain(), None, "{raw}");
}

fn max_age_seconds(cookie: &Cookie<'_>) -> Option<i64> {
    cookie.max_age().map(|age| age.whole_seconds())
}

fn expires_in_the_past(cookie: &Cookie<'_>) -> bool {
    cookie
        .expires_datetime()
        .is_some_and(|expires| expires.unix_timestamp() < chrono::Utc::now().timestamp())
}

fn cookie_headers(cookie: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
    headers
}

#[test]
fn refresh_cookie_attributes_match_environment() {
    assert_eq!(COOKIE_MAX_AGE_SECONDS, 604_800);
    let device = generate_token();
    // Only the exact `development` name drops `Secure`; an unknown or
    // differently spelled name keeps it.
    for (environment, secure) in [
        ("production", true),
        ("staging", true),
        ("test", true),
        ("development", false),
        ("Development", true),
        ("unknown", true),
    ] {
        let cookies = AuthCookies::for_environment(environment);

        let set = set_cookies(cookies.set(CookieJar::new(), REFRESH_VALUE, &device));
        assert_eq!(set.len(), 2, "{environment}: both cookies are set");
        let (raw, refresh) = named(&set, REFRESH_COOKIE);
        assert_eq!(refresh.value(), REFRESH_VALUE, "{environment}");
        assert_session_attributes(raw, refresh, secure);
        assert_eq!(max_age_seconds(refresh), Some(604_800), "{raw}");

        let cleared = set_cookies(cookies.clear(CookieJar::new()));
        assert_eq!(cleared.len(), 2, "{environment}: both cookies are cleared");
        let (raw, refresh) = named(&cleared, REFRESH_COOKIE);
        assert_eq!(refresh.value(), "", "{raw}");
        assert_session_attributes(raw, refresh, secure);
        assert_eq!(max_age_seconds(refresh), Some(0), "{raw}");
        assert!(expires_in_the_past(refresh), "{raw}");
    }
}

#[test]
fn device_cookie_attributes_and_clear() {
    let device = generate_token();
    let cookies = AuthCookies::for_environment("production");

    let set = set_cookies(cookies.set(CookieJar::new(), REFRESH_VALUE, &device));
    let (raw, cookie) = named(&set, DEVICE_COOKIE);
    assert_eq!(cookie.value(), device);
    assert_session_attributes(raw, cookie, true);
    assert_eq!(max_age_seconds(cookie), Some(604_800), "{raw}");

    // Clearing a jar that presented both cookies sends both removals with the
    // attributes that were used to set them.
    let presented = CookieJar::from_headers(&cookie_headers(&format!(
        "{REFRESH_COOKIE}={REFRESH_VALUE}; {DEVICE_COOKIE}={device}"
    )));
    let cleared = set_cookies(cookies.clear(presented));
    assert_eq!(cleared.len(), 2);
    for name in [REFRESH_COOKIE, DEVICE_COOKIE] {
        let (raw, cookie) = named(&cleared, name);
        assert_eq!(cookie.value(), "", "{raw}");
        assert_session_attributes(raw, cookie, true);
        assert_eq!(max_age_seconds(cookie), Some(0), "{raw}");
        assert!(expires_in_the_past(cookie), "{raw}");
    }

    // Reading: both values, the device shape check and the origin condition.
    let read = AuthCookies::read(&CookieJar::from_headers(&cookie_headers(&format!(
        "{REFRESH_COOKIE}={REFRESH_VALUE}; {DEVICE_COOKIE}={device}"
    ))));
    assert_eq!(read.refresh.as_deref(), Some(REFRESH_VALUE));
    assert_eq!(read.device.as_deref(), Some(device.as_str()));
    assert_eq!(read.valid_device(), Some(device.as_str()));
    assert!(read.cookie_authenticated());

    let malformed = AuthCookies::read(&CookieJar::from_headers(&cookie_headers(&format!(
        "{DEVICE_COOKIE}=short"
    ))));
    assert_eq!(malformed.device.as_deref(), Some("short"));
    assert_eq!(malformed.valid_device(), None);
    assert!(!malformed.cookie_authenticated());

    // An empty value counts as absent.
    let empty = AuthCookies::read(&CookieJar::from_headers(&cookie_headers(&format!(
        "{REFRESH_COOKIE}=; {DEVICE_COOKIE}="
    ))));
    assert_eq!(empty, Presented::default());
    assert!(!empty.cookie_authenticated());
    assert_eq!(
        AuthCookies::read(&CookieJar::from_headers(&HeaderMap::new())),
        Presented::default()
    );

    // Debug output never carries a cookie value.
    let printed = format!("{read:?}");
    assert!(!printed.contains(REFRESH_VALUE), "{printed}");
    assert!(!printed.contains(&device), "{printed}");
    assert!(printed.contains("[REDACTED]"), "{printed}");
}

fn origin_headers(origins: &[&[u8]]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for origin in origins {
        headers.append(header::ORIGIN, HeaderValue::from_bytes(origin).unwrap());
    }
    headers
}

fn assert_origin_rejected(result: Result<(), AuthFailure>, case: &str) {
    let failure = result.expect_err(case);
    assert_eq!(failure.code(), AuthCode::BrowserOriginRejected, "{case}");
    assert_eq!(failure.status(), StatusCode::FORBIDDEN, "{case}");
    assert_eq!(
        failure.detail()["code"],
        "BROWSER_ORIGIN_REJECTED",
        "{case}"
    );
}

#[test]
fn origin_guard_matrix() {
    // The closed sets, exactly.
    assert_eq!(
        trusted_origins("production"),
        [
            "https://app.verdaxis.exchange",
            "https://verdaxis.exchange",
            "https://canary.verdaxis.exchange",
        ]
    );
    assert_eq!(
        trusted_origins("staging"),
        ["https://staging.verdaxis.exchange"]
    );
    assert_eq!(
        trusted_origins("development"),
        [
            "http://localhost:5173",
            "http://localhost:5174",
            "http://127.0.0.1:5173",
            "http://127.0.0.1:5174",
        ]
    );
    assert_eq!(trusted_origins("test"), ["https://test"]);
    for unknown in ["", "qa", "Production", "production ", "prod"] {
        assert!(trusted_origins(unknown).is_empty(), "{unknown:?}");
    }

    // Every trusted origin passes, with or without a refresh cookie.
    for environment in ["production", "staging", "development", "test"] {
        for origin in trusted_origins(environment) {
            assert!(allowed_browser_origin(environment, origin));
            for cookie in [false, true] {
                let headers = origin_headers(&[origin.as_bytes()]);
                assert_eq!(
                    require_trusted_origin(&headers, environment, cookie),
                    Ok(()),
                    "{environment} {origin} cookie={cookie}"
                );
            }
        }
    }

    // An absent origin passes only without a refresh cookie, in any
    // environment.
    for environment in ["production", "staging", "development", "test", "qa"] {
        assert_eq!(
            require_trusted_origin(&HeaderMap::new(), environment, false),
            Ok(()),
            "{environment}"
        );
        assert_origin_rejected(
            require_trusted_origin(&HeaderMap::new(), environment, true),
            &format!("{environment}: absent origin with a refresh cookie"),
        );
    }

    // A present origin must equal a trusted entry byte for byte.
    let production_rejections: &[(&str, &[u8])] = &[
        ("scheme", b"http://app.verdaxis.exchange"),
        ("host", b"https://evil.example"),
        ("host suffix", b"https://app.verdaxis.exchange.evil.example"),
        ("subdomain", b"https://www.app.verdaxis.exchange"),
        ("port", b"https://app.verdaxis.exchange:8443"),
        ("default port", b"https://app.verdaxis.exchange:443"),
        ("root path", b"https://app.verdaxis.exchange/"),
        ("path", b"https://app.verdaxis.exchange/login"),
        ("query", b"https://app.verdaxis.exchange?next=/"),
        ("fragment", b"https://app.verdaxis.exchange#top"),
        ("userinfo", b"https://user@app.verdaxis.exchange"),
        (
            "userinfo password",
            b"https://user:pass@app.verdaxis.exchange",
        ),
        (
            "userinfo host",
            b"https://app.verdaxis.exchange@evil.example",
        ),
        ("case", b"HTTPS://APP.VERDAXIS.EXCHANGE"),
        ("trailing space", b"https://app.verdaxis.exchange "),
        ("other environment", b"https://staging.verdaxis.exchange"),
        ("opaque", b"null"),
        ("empty", b""),
        ("not utf-8", b"https://app.verdaxis.exchange\xff"),
    ];
    for &(case, origin) in production_rejections {
        for cookie in [false, true] {
            assert_origin_rejected(
                require_trusted_origin(&origin_headers(&[origin]), "production", cookie),
                &format!("{case} cookie={cookie}"),
            );
        }
    }
    for (environment, origin) in [
        ("staging", "https://app.verdaxis.exchange"),
        ("development", "http://localhost:3000"),
        ("development", "https://localhost:5173"),
        ("test", "http://test"),
        ("qa", "https://app.verdaxis.exchange"),
        ("qa", "https://staging.verdaxis.exchange"),
    ] {
        assert_origin_rejected(
            require_trusted_origin(&origin_headers(&[origin.as_bytes()]), environment, false),
            &format!("{environment} {origin}"),
        );
    }

    // More than one Origin header is refused, even when each is trusted.
    let app = b"https://app.verdaxis.exchange".as_slice();
    for origins in [[app, app], [app, b"https://evil.example".as_slice()]] {
        assert_origin_rejected(
            require_trusted_origin(&origin_headers(&origins), "production", false),
            "two origin headers",
        );
    }
}

/// Lazy pools keep this HTTP policy test independent of a database server.
fn cors_app(environment: &str) -> Router {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        phase2: Default::default(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.read_admission_slots)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    let mut state = verdaxis_read::AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    // Select only the HTTP environment policy, without deployed DB/budget setup.
    std::sync::Arc::make_mut(&mut state.config).environment = environment.into();
    verdaxis_read::runtime::http::middleware_with_state(
        Router::new().route("/api/auth/logout", post(|| async { "ok" })),
        state,
    )
}

#[tokio::test]
async fn cors_uses_the_environment_origin_set() {
    let environments = ["production", "staging", "development", "test", "qa"];
    let origins: Vec<_> = environments
        .iter()
        .flat_map(|environment| trusted_origins(environment).iter().copied())
        .chain(["https://untrusted.example"])
        .collect();
    for environment in environments {
        let app = cors_app(environment);
        for origin in &origins {
            let allowed = trusted_origins(environment).contains(origin);
            assert_eq!(
                require_trusted_origin(&origin_headers(&[origin.as_bytes()]), environment, true)
                    .is_ok(),
                allowed,
            );
            for preflight in [true, false] {
                let mut request = Request::builder()
                    .method(if preflight { "OPTIONS" } else { "POST" })
                    .uri("/api/auth/logout")
                    .header(header::ORIGIN, *origin);
                if preflight {
                    request = request
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type");
                } else {
                    request = request.header(header::COOKIE, "refresh_token=fixture");
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let case = format!("{environment} {origin} preflight={preflight}");
                assert_eq!(
                    response.status(),
                    if preflight && !allowed {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::OK
                    },
                    "{case}",
                );
                assert_eq!(
                    response
                        .headers()
                        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                        .map(|value| value.to_str().unwrap()),
                    allowed.then_some(*origin),
                    "{case}",
                );
                assert_eq!(
                    response.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS],
                    "true",
                    "{case}"
                );
            }
        }
    }
}

async fn render(response: Response) -> (StatusCode, HeaderMap, Value) {
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&body).unwrap())
}

fn is_upper_snake(code: &str) -> bool {
    code.starts_with(|first: char| first.is_ascii_uppercase())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[tokio::test]
async fn api_errors_preserve_auth_responses() {
    for code in AuthCode::ALL {
        let direct = render(AuthFailure::new(code).into_response()).await;
        let converted =
            render(verdaxis_read::ApiError::from(AuthFailure::new(code)).into_response()).await;
        assert_eq!(converted.0, code.status(), "{}", code.as_str());
        assert_eq!(converted.2, direct.2, "{}", code.as_str());
        for headers in [&direct.1, &converted.1] {
            assert_eq!(
                headers
                    .get(header::WWW_AUTHENTICATE)
                    .map(|value| value.to_str().unwrap()),
                (code.status() == StatusCode::UNAUTHORIZED).then_some("Bearer"),
                "{}: WWW-Authenticate only on 401",
                code.as_str(),
            );
        }
    }
}

#[tokio::test]
async fn auth_errors_use_the_detail_code_envelope() {
    let mut wire_codes = HashSet::new();
    for code in AuthCode::ALL {
        let wire = code.as_str();
        assert!(wire_codes.insert(wire), "{wire} is listed once");
        assert!(is_upper_snake(wire), "{wire}");
        assert!(!code.message().is_empty(), "{wire}");

        let (status, headers, body) = render(AuthFailure::new(code).into_response()).await;
        assert_eq!(status, code.status(), "{wire}");
        assert_eq!(
            body,
            json!({"detail": {"code": wire, "message": code.message()}}),
            "{wire}"
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/json", "{wire}");
        let challenge = headers
            .get(header::WWW_AUTHENTICATE)
            .map(|value| value.to_str().unwrap());
        assert_eq!(
            challenge,
            (status == StatusCode::UNAUTHORIZED).then_some("Bearer"),
            "{wire}: WWW-Authenticate only on 401"
        );
        assert!(headers.get(header::RETRY_AFTER).is_none(), "{wire}");
    }

    // Every code the pinned client contract names exists, with the status the
    // contract gives it.
    let contract: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase3/client-contract.json"
    ))
    .unwrap();
    let mut contract_codes = 0;
    for surface in contract["surfaces"].as_array().unwrap() {
        let Some(errors) = surface["errors"].as_array() else {
            continue;
        };
        for error in errors {
            let status = error["status"].as_u64().unwrap();
            for wire in error["codes"].as_array().unwrap() {
                let wire = wire.as_str().unwrap();
                let code = AuthCode::ALL
                    .into_iter()
                    .find(|code| code.as_str() == wire)
                    .unwrap_or_else(|| panic!("{wire} has an AuthCode"));
                assert_eq!(
                    u64::from(code.status().as_u16()),
                    status,
                    "{} {wire}",
                    surface["id"]
                );
                contract_codes += 1;
            }
        }
    }
    assert!(contract_codes >= 19, "the contract lists its codes");

    // A retry delay travels in the detail and in `Retry-After`.
    let rotation = AuthFailure::new(AuthCode::RefreshRotationInProgress).with_retry_after(1);
    let (status, headers, body) = render(rotation.into_response()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        json!({"detail": {
            "code": "REFRESH_ROTATION_IN_PROGRESS",
            "message": AuthCode::RefreshRotationInProgress.message(),
            "retry_after_seconds": 1,
        }})
    );
    assert_eq!(headers[header::RETRY_AFTER], "1");
    assert!(headers.get(header::WWW_AUTHENTICATE).is_none());

    // A message override keeps the code and its status.
    let validation =
        AuthFailure::new(AuthCode::ValidationFailed).with_message("Password is too short");
    let (status, _, body) = render(validation.into_response()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        json!({"detail": {"code": "VALIDATION_FAILED", "message": "Password is too short"}})
    );

    // The `ApiError` conversion renders the same envelope and `Retry-After`.
    let limited = AuthFailure::new(AuthCode::RateLimited).with_retry_after(42);
    let (status, headers, body) =
        render(verdaxis_read::ApiError::from(limited).into_response()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body, json!({"detail": limited.detail()}));
    assert_eq!(body["detail"]["code"], "RATE_LIMITED");
    assert_eq!(body["detail"]["retry_after_seconds"], 42);
    assert_eq!(headers[header::RETRY_AFTER], "42");
}

#[test]
fn preauth_and_endpoint_limits_share_one_counter() {
    // Pre-auth buckets: every path under a prefix shares one counter.
    let preauth_buckets: [(&[&str], usize); 3] = [
        (&["/api/auth/login", "/api/auth/login/"], 30),
        (
            &["/api/auth/forgot-password", "/api/auth/forgot-password/"],
            15,
        ),
        (
            &[
                "/api/auth/register",
                "/api/auth/register/",
                "/api/auth/register-with-org",
                "/api/auth/register-with-org/",
            ],
            30,
        ),
    ];
    for (paths, limit) in preauth_buckets {
        let mut limits = Phase2Limits::default();
        for index in 0..limit {
            let path = paths[index % paths.len()];
            assert_eq!(
                limits.preauth_check("192.0.2.10", path, 0),
                Ok(()),
                "{path} #{index}"
            );
        }
        for path in paths {
            assert_eq!(
                limits.preauth_check("192.0.2.10", path, 0),
                Err(Refusal {
                    retry_after_seconds: 60
                }),
                "{path} at the threshold"
            );
        }
        assert_eq!(
            limits.preauth_check("192.0.2.10", paths[0], 45),
            Err(Refusal {
                retry_after_seconds: 15
            })
        );
        assert_eq!(
            limits.preauth_check("192.0.2.11", paths[0], 45),
            Ok(()),
            "each client has its own counter"
        );
        assert_eq!(
            limits.preauth_check("192.0.2.10", paths[0], 60),
            Ok(()),
            "{} resets after one minute",
            paths[0]
        );
    }

    // The other auth routes have no pre-auth bucket.
    let mut limits = Phase2Limits::default();
    for path in [
        "/api/auth/refresh",
        "/api/auth/logout",
        "/api/auth/stream-token",
        "/api/auth/me/password",
        "/api/auth/invitations/accept",
        "/api/auth/reset-password",
        "/api/auth/verify-email",
        "/api/auth/resend-verification-email",
    ] {
        for _ in 0..100 {
            assert_eq!(limits.preauth_check("192.0.2.10", path, 0), Ok(()), "{path}");
        }
    }

    // Endpoint limits: both slash forms of a route share one counter.
    for (route, limit) in [
        ("/api/auth/login", 5),
        ("/api/auth/refresh", 10),
        ("/api/auth/stream-token", 30),
        ("/api/auth/me/password", 3),
        ("/api/auth/invitations/accept", 5),
        ("/api/auth/forgot-password", 3),
        ("/api/auth/reset-password", 5),
        ("/api/auth/verify-email", 10),
        ("/api/auth/resend-verification-email", 3),
        ("/api/auth/register", 5),
        ("/api/auth/register-with-org", 5),
    ] {
        let limits = SharedLimits::default();
        let slash = format!("{route}/");
        for index in 0..limit {
            let path = if index % 2 == 0 {
                route
            } else {
                slash.as_str()
            };
            assert!(
                limits.check_endpoint("client", path, 0).is_ok(),
                "{path} #{index}"
            );
        }
        for path in [route, slash.as_str()] {
            let refused = limits.check_endpoint("client", path, 0).unwrap_err();
            assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{path}");
            assert_eq!(refused.detail["code"], "RATE_LIMITED", "{path}");
            assert_eq!(refused.retry_after, Some(60), "{path}");
        }
        assert_eq!(
            limits
                .check_endpoint("client", route, 59)
                .unwrap_err()
                .retry_after,
            Some(1)
        );
        assert!(
            limits.check_endpoint("client", route, 60).is_ok(),
            "{route} resets after one minute"
        );
    }

    // Registration routes share a pre-auth bucket but not an endpoint bucket.
    let limits = SharedLimits::default();
    for _ in 0..5 {
        assert!(
            limits
                .check_endpoint("client", "/api/auth/register", 0)
                .is_ok()
        );
    }
    assert!(
        limits
            .check_endpoint("client", "/api/auth/register", 0)
            .is_err()
    );
    assert!(
        limits
            .check_endpoint("client", "/api/auth/register-with-org", 0)
            .is_ok()
    );

    // Logout has no endpoint limit.
    for _ in 0..100 {
        assert!(
            limits
                .check_endpoint("client", "/api/auth/logout", 0)
                .is_ok()
        );
    }

    // One request counts once in each layer: five admitted logins use five of
    // the thirty pre-auth requests.
    let mut limits = Phase2Limits::default();
    for _ in 0..5 {
        assert!(limits.preauth("client", "/api/auth/login", 0));
        assert!(limits.endpoint("client", "/api/auth/login", 0));
    }
    assert!(!limits.endpoint("client", "/api/auth/login", 0));
    for _ in 5..30 {
        assert!(limits.preauth("client", "/api/auth/login", 0));
    }
    assert!(!limits.preauth("client", "/api/auth/login", 0));
}

fn content_length(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(value).unwrap(),
    );
    headers
}

/// A body that fails when it is read, as when a client goes away.
fn failing_body() -> Body {
    Body::from_stream(stream::once(async {
        Err::<Bytes, std::io::Error>(std::io::Error::other("client went away"))
    }))
}

#[tokio::test]
async fn auth_body_limit_is_enforced() {
    assert_eq!(AUTH_BODY_LIMIT_BYTES, 128 * 1024);

    // Exactly the limit is read.
    let at_limit = read_auth_body(
        &content_length(&AUTH_BODY_LIMIT_BYTES.to_string()),
        Body::from(vec![b'a'; AUTH_BODY_LIMIT_BYTES]),
    )
    .await
    .unwrap();
    assert_eq!(at_limit.len(), AUTH_BODY_LIMIT_BYTES);

    // A declared length above the limit is refused before any byte is read:
    // reading this body would fail with a different code.
    let declared = read_auth_body(
        &content_length(&(AUTH_BODY_LIMIT_BYTES + 1).to_string()),
        failing_body(),
    )
    .await
    .unwrap_err();
    assert_eq!(declared.code(), AuthCode::RequestTooLarge);
    assert_eq!(declared.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // A body without a declared length is refused once it passes the limit.
    let streamed = Body::from_stream(stream::iter([
        Ok::<Bytes, std::io::Error>(Bytes::from(vec![b'a'; AUTH_BODY_LIMIT_BYTES])),
        Ok(Bytes::from_static(b"a")),
    ]));
    let refused = read_auth_body(&HeaderMap::new(), streamed)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), AuthCode::RequestTooLarge);

    // A declared length smaller than the body does not lift the limit.
    let understated = read_auth_body(
        &content_length("16"),
        Body::from(vec![b'a'; AUTH_BODY_LIMIT_BYTES + 1]),
    )
    .await
    .unwrap_err();
    assert_eq!(understated.code(), AuthCode::RequestTooLarge);

    // A malformed length and a failed read are 400, never 500.
    for value in ["abc", "-1", "1.5", ""] {
        let invalid = read_auth_body(&content_length(value), Body::empty())
            .await
            .unwrap_err();
        assert_eq!(invalid.code(), AuthCode::RequestInvalid, "{value:?}");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST, "{value:?}");
    }
    let interrupted = read_auth_body(&HeaderMap::new(), failing_body())
        .await
        .unwrap_err();
    assert_eq!(interrupted.code(), AuthCode::RequestInvalid);

    // Through the HTTP stack, an oversized body is the 413 auth envelope.
    let app = middleware(
        Router::new().route(
            "/api/auth/login",
            post(|request: Request<Body>| async move {
                let (parts, body) = request.into_parts();
                match read_auth_body(&parts.headers, body).await {
                    Ok(body) => body.len().to_string().into_response(),
                    Err(failure) => failure.into_response(),
                }
            }),
        ),
        "staging",
    );
    let oversized = || Body::from(vec![b'a'; AUTH_BODY_LIMIT_BYTES + 1]);
    for declare in [true, false] {
        let mut request = Request::post("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if declare {
            request = request.header(header::CONTENT_LENGTH, AUTH_BODY_LIMIT_BYTES + 1);
        }
        let response = app
            .clone()
            .oneshot(request.body(oversized()).unwrap())
            .await
            .unwrap();
        let (status, _, body) = render(response).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "declared={declare}");
        assert_eq!(
            body["detail"]["code"], "REQUEST_TOO_LARGE",
            "declared={declare}"
        );
    }
    let response = app
        .oneshot(
            Request::post("/api/auth/login")
                .body(Body::from("username=a&password=b"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), 64).await.unwrap(), "21");
}

async fn forgot_password(app: &Router, peer: &str, forwarded: Option<&str>) -> Response {
    let mut request = Request::post("/api/auth/forgot-password");
    if let Some(forwarded) = forwarded {
        request = request.header("x-forwarded-for", forwarded);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn forwarded_client_address_uses_last_loopback_hop() {
    // The forgot-password pre-auth bucket allows 15 requests per client.
    let app = middleware_with_limits(
        Router::new().route("/api/auth/forgot-password", post(|| async { "ok" })),
        SharedLimits::default(),
        "staging",
    );

    // A direct client cannot choose its address: a forwarded header from a
    // non-loopback peer is ignored, so every request counts for the peer.
    for index in 0..15 {
        let spoofed = format!("198.51.100.{index}");
        let response = forgot_password(&app, "192.0.2.10:40000", Some(&spoofed)).await;
        assert_eq!(response.status(), StatusCode::OK, "#{index}");
    }
    let refused = forgot_password(&app, "192.0.2.10:40001", Some("198.51.100.200")).await;
    let (status, headers, body) = render(refused).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["detail"]["code"], "RATE_LIMITED");
    let retry_after: u64 = headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(body["detail"]["retry_after_seconds"], retry_after);
    assert_eq!(
        forgot_password(&app, "192.0.2.11:40000", None)
            .await
            .status(),
        StatusCode::OK,
        "another peer has its own counter"
    );

    // Behind the local proxy the client is the last forwarded hop.
    assert_eq!(
        forgot_password(&app, "127.0.0.1:50000", Some("192.0.2.10"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the proxied client is the exhausted peer"
    );
    for index in 0..15 {
        let response =
            forgot_password(&app, "127.0.0.1:50000", Some("192.0.2.10, 203.0.113.7")).await;
        assert_eq!(response.status(), StatusCode::OK, "#{index}");
    }
    assert_eq!(
        forgot_password(&app, "127.0.0.1:50000", Some("192.0.2.99, 203.0.113.7"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "only the last hop names the client"
    );
    assert_eq!(
        forgot_password(&app, "[::1]:50000", Some("203.0.113.8"))
            .await
            .status(),
        StatusCode::OK,
        "an IPv6 loopback peer also uses the last hop"
    );
}

#[tokio::test]
async fn state_free_cors_uses_explicit_environment() {
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;
    for environment in ["production", "unknown"] {
        for limited in [false, true] {
            let routes = Router::new().route("/probe", get(|| async { "ok" }));
            let app = if limited {
                verdaxis_read::runtime::http::middleware_with_limits(
                    routes,
                    Default::default(),
                    environment,
                )
            } else {
                verdaxis_read::runtime::http::middleware(routes, environment)
            };
            for origin in [
                "https://app.verdaxis.exchange",
                "https://staging.verdaxis.exchange",
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::get("/probe")
                            .header("origin", origin)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    response
                        .headers()
                        .contains_key("access-control-allow-origin"),
                    environment == "production" && origin == "https://app.verdaxis.exchange"
                );
            }
        }
    }
}
