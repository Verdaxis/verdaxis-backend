//! E5/E6 component checks.
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/registration.rs"]
pub mod registration;

use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use email::MailDispatcher;
use crud::referrals::invite::MailClient;
use registration::{RegistrationSettings, router};
use serde_json::{Value, json};
use tower::ServiceExt;

fn settings() -> RegistrationSettings {
    RegistrationSettings::new(
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused").unwrap(),
        MailDispatcher::new(MailClient::disabled()),
    )
}

#[tokio::test]
async fn e5_e6_validation_precedes_storage() {
    for body in [
        json!({"email":"invalid","password":"valid-password","role":"BUYER"}),
        json!({"email":"one@example.com","password":"short","role":"BUYER"}),
        json!({"email":"one@example.com","password":"valid-password","role":"ADMIN"}),
        json!({"email":"one@example.com","password":"x".repeat(1025),"role":"BUYER"}),
    ] {
        let response = router(settings()).oneshot(Request::post("/api/auth/register")
            .header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(),4096).await.unwrap()).unwrap();
        assert_eq!(body["detail"]["code"], "VALIDATION_FAILED");
    }
    for org_type in ["FUEL_TRADER", "ADMIN", ""] {
        let response = router(settings()).oneshot(Request::post("/api/auth/register-with-org")
            .header("content-type", "application/json").body(Body::from(json!({"registration_token":"opaque", "organization":{"name":"Company","type":org_type}}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[test]
fn e5_domain_policy_is_closed_without_datasets() {
    assert_eq!(settings().organization_domain("one@company.example"),None);
    let settings = settings().with_domain_lists(["gmail.com".into()], ["disposable.example".into()]);
    for email in ["one@gmail.com", "one@sub.gmail.com", "one@disposable.example"] {
        assert_eq!(settings.organization_domain(email),None);
    }
    assert_eq!(settings.organization_domain("one@company.example"),Some("company.example".into()));
}

#[test]
fn e5_e6_quota_policy_covers_slash_forms() {
    for path in ["/api/auth/register", "/api/auth/register/", "/api/auth/register-with-org", "/api/auth/register-with-org/"] {
        let limits = auth::limits::SharedLimits::default();
        for _ in 0..5 { assert!(limits.check_endpoint("client",path,1).is_ok()); }
        let error = limits.check_endpoint("client",path,1).unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(error.retry_after.is_some());
        let limits = auth::limits::SharedLimits::default();
        for _ in 0..30 { assert!(limits.preauth_check("client",path,1).is_ok()); }
        assert!(limits.preauth_check("client",path,1).is_err());
    }
}

#[test]
fn e5_account_template_has_sign_in_link() {
    let html = include_str!("../src/auth/templates/account_exists.html");
    assert!(html.contains("{{URL}}"));
    assert!(!html.contains("{{TOKEN}}"));
}
