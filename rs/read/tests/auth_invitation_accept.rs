//! S6 invitation-acceptance component checks that need no database.
//!
//! The database behaviour - token consumption, family revocation, referral and
//! audit writes - is covered by `auth_invitation_accept_postgres.rs`.
pub use verdaxis_read::{ApiError, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/invitation_accept.rs"]
pub mod invitation_accept;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use invitation_accept::{InvitationAcceptState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "synthetic-invitation-key-at-least-32-chars";

/// A state whose pool refuses connections at once, so a request that reaches
/// the database answers 503 instead of a route decision.
fn state() -> InvitationAcceptState {
    InvitationAcceptState::new(
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(100))
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap(),
        "production",
        SECRET,
        "verdaxis",
        "verdaxis-api",
        "https://app.verdaxis.exchange",
        auth::limits::SharedLimits::default(),
    )
}

fn body(terms: bool, password: &str, token: &str) -> Value {
    json!({"token": token, "new_password": password, "accept_terms": terms})
}

fn token() -> String {
    "a".repeat(43)
}

async fn send(state: InvitationAcceptState, path: &str, payload: &Value) -> (StatusCode, Value) {
    send_with(state, path, payload.to_string(), None, None).await
}

async fn send_with(
    state: InvitationAcceptState,
    path: &str,
    payload: String,
    origin: Option<&str>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = router(state)
        .oneshot(request.body(Body::from(payload)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn invitation_accept_validation_vectors() {
    let origin = Some("https://app.verdaxis.exchange");
    let long = format!("Aa1{}", "b".repeat(1024));
    let vectors = [
        // Terms must be explicitly accepted.
        body(false, "AcceptedPassword9", &token()),
        // 8 to 1024 UTF-8 bytes.
        body(true, "Aa1bbbb", &token()),
        body(true, &long, &token()),
        // At least one uppercase letter and one digit.
        body(true, "alllowercase9", &token()),
        body(true, "NoDigitsHere", &token()),
        // The invitation secret is 32 to 128 bytes.
        body(true, "AcceptedPassword9", &"a".repeat(31)),
        body(true, "AcceptedPassword9", &"a".repeat(129)),
        // A missing field is a validation failure, not a refusal.
        json!({"token": token(), "accept_terms": true}),
        json!({"token": token(), "new_password": "AcceptedPassword9"}),
    ];
    for vector in vectors {
        let (status, body) = send_with(
            state(),
            "/api/auth/invitations/accept",
            vector.to_string(),
            origin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{vector}");
        assert_eq!(body["detail"]["code"], "VALIDATION_FAILED", "{vector}");
    }
    for payload in ["{", "[]", "null", ""] {
        let (status, body) = send_with(
            state(),
            "/api/auth/invitations/accept",
            payload.to_string(),
            origin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{payload}");
        assert_eq!(body["detail"]["code"], "VALIDATION_FAILED", "{payload}");
    }
}

#[tokio::test]
async fn invitation_accept_password_policy_matches_python() {
    for (password, accepted) in [
        ("Aa1bbbbb", true),
        ("AcceptedPassword9", true),
        ("ABC12345", true),
        ("Aa1bbbb", false),
        ("alllowercase9", false),
        ("NoDigitsHere", false),
        ("Aa1 bbbbb", true),
        // Python's `\d` also accepts non-ASCII decimal digits; this rule does
        // not, so it refuses some passwords Python would accept and never
        // accepts one Python would refuse.
        ("AaBbbbb\u{0665}", false),
    ] {
        assert_eq!(
            invitation_accept::strong_password(password),
            accepted,
            "{password}"
        );
    }
    assert!(invitation_accept::strong_password(&format!(
        "Aa1{}",
        "b".repeat(1021)
    )));
    assert!(!invitation_accept::strong_password(&format!(
        "Aa1{}",
        "b".repeat(1022)
    )));
}

#[tokio::test]
async fn invitation_accept_is_origin_checked() {
    let valid = body(true, "AcceptedPassword9", &token());
    for origin in [
        "https://other.invalid",
        "http://app.verdaxis.exchange",
        "https://app.verdaxis.exchange/path",
        "https://app.verdaxis.exchange.evil.invalid",
    ] {
        let (status, body) = send_with(
            state(),
            "/api/auth/invitations/accept",
            valid.to_string(),
            Some(origin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
        assert_eq!(
            body["detail"]["code"], "BROWSER_ORIGIN_REJECTED",
            "{origin}"
        );
    }
    // An absent origin is refused only when the request carries a session
    // cookie; without one the browser rule allows the request through to the
    // route's own validation and storage.
    let (status, body) = send_with(
        state(),
        "/api/auth/invitations/accept",
        valid.to_string(),
        None,
        Some("refresh_token=presented"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["detail"]["code"], "BROWSER_ORIGIN_REJECTED");
    let (status, _) = send_with(
        state(),
        "/api/auth/invitations/accept",
        valid.to_string(),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn invitation_accept_is_limited_to_five_per_minute() {
    let state = state();
    let valid = body(true, "AcceptedPassword9", &token());
    // A validation refusal spends no quota: the pool refuses the connection, so
    // none of these requests can reach the limit.
    for _ in 0..6 {
        let (status, _) = send(
            state.clone(),
            "/api/auth/invitations/accept",
            &body(true, "alllowercase", &token()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, _) = send(state.clone(), "/api/auth/invitations/accept", &valid).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // Five validated requests are admitted; the sixth is refused, on either
    // slash form, because they share one counter.
    for attempt in 0..4 {
        let (status, _) = send(state.clone(), "/api/auth/invitations/accept", &valid).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "attempt {attempt}");
    }
    for path in [
        "/api/auth/invitations/accept",
        "/api/auth/invitations/accept/",
    ] {
        let (status, body) = send(state.clone(), path, &valid).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{path}");
        assert_eq!(body["detail"]["code"], "RATE_LIMITED", "{path}");
        let retry_after = body["detail"]["retry_after_seconds"].as_u64().unwrap();
        assert!(
            (59..=60).contains(&retry_after),
            "{path}: retry_after_seconds {retry_after}"
        );
    }
}

#[tokio::test]
async fn invitation_accept_body_limit() {
    let oversized = format!(
        "{{\"token\":\"{}\",\"new_password\":\"AcceptedPassword9\",\"accept_terms\":true,\"padding\":\"{}\"}}",
        token(),
        "x".repeat(auth::error::AUTH_BODY_LIMIT_BYTES)
    );
    let (status, body) = send_with(
        state(),
        "/api/auth/invitations/accept",
        oversized,
        Some("https://app.verdaxis.exchange"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["detail"]["code"], "REQUEST_TOO_LARGE");
}

#[tokio::test]
async fn invitation_accept_refusal_contract_is_one_code() {
    // The route reports every unusable invitation with one status and code, so
    // it cannot tell a caller which account states exist.
    assert_eq!(
        auth::error::AuthCode::InvitationInvalid.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        auth::error::AuthCode::InvitationInvalid.as_str(),
        "INVITATION_INVALID"
    );
}
