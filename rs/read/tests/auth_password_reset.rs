//! E1/E2 component checks.
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/password_reset.rs"]
pub mod password_reset;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[tokio::test]
async fn e1_e2_validation_and_quota() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let settings = email_verification::EmailSettings::new(
        pool,
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    let app = password_reset::router(settings);
    for body in [
        r#"{"token":"","new_password":"long-enough"}"#,
        r#"{"token":"token","new_password":"short"}"#,
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/auth/reset-password")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
    }
    let limits = auth::limits::SharedLimits::default();
    for _ in 0..3 {
        limits
            .check_endpoint("client", "/api/auth/forgot-password", 1)
            .unwrap();
    }
    assert!(
        limits
            .check_endpoint("client", "/api/auth/forgot-password/", 1)
            .is_err()
    );
    for _ in 0..15 {
        limits
            .preauth_check("client", "/api/auth/forgot-password", 1)
            .unwrap();
    }
    assert!(
        limits
            .preauth_check("client", "/api/auth/forgot-password", 1)
            .is_err()
    );
}

#[test]
fn e2_mail_link_is_encoded() {
    let payload = email::render(
        &email::AuthMail::Reset {
            to: "recipient@example.com".into(),
            name: "<Name>".into(),
            token: "a&b".into(),
        },
        "sender@example.com",
        "https://app.example.com",
    );
    assert!(payload["html"].as_str().unwrap().contains("token=a%26b"));
    assert!(payload["html"].as_str().unwrap().contains("&lt;Name&gt;"));
}
