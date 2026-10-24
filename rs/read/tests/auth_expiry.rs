use chrono::{DateTime, TimeZone, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use verdaxis_read::auth::{
    verify_access, verify_authorization, AuthError, TokenIssuer, VerificationSettings,
};

const SECRET: &str = "synthetic-expiry-key-at-least-32-characters";

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_792_800_000, 0).unwrap()
}

fn settings() -> VerificationSettings {
    VerificationSettings::new(
        SECRET.into(),
        None,
        "verdaxis".into(),
        "verdaxis-api".into(),
    )
}

fn claims() -> Value {
    json!({"sub":"20000000-0000-4000-8000-000000000001", "type":"access", "iss":"verdaxis", "aud":"verdaxis-api", "iat":now().timestamp(), "iat_us":now().timestamp_micros()})
}

fn sign(claims: Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn assert_refused(token: &str) {
    for result in [
        verify_access(token, &settings(), now()),
        verify_authorization(Some(&format!("Bearer {token}")), &settings(), now()),
    ] {
        let error = result.expect_err("a token requires a future expiry");
        assert_eq!(error, AuthError::Credentials);
        assert_eq!(error.status(), 401);
        assert_eq!(error.body()["detail"]["code"], "AUTH_CREDENTIALS_INVALID");
    }
}

#[test]
fn access_requires_expiry() {
    assert_refused(&sign(claims()));
}

#[test]
fn access_expiry_boundary_is_inclusive() {
    for expiry in [now().timestamp() - 1, now().timestamp()] {
        let mut claims = claims();
        claims["exp"] = json!(expiry);
        assert_refused(&sign(claims));
    }
}

#[test]
fn access_issuers_set_future_expiry() {
    let issuer = TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api");
    let mut python_claims = claims();
    python_claims["exp"] = json!(now().timestamp() + 900);
    for token in [
        issuer.access(
            "20000000-0000-4000-8000-000000000001".parse().unwrap(),
            now(),
            None,
        ),
        sign(python_claims),
    ] {
        assert!(verify_access(&token, &settings(), now()).is_ok());
        assert!(verify_authorization(Some(&format!("Bearer {token}")), &settings(), now()).is_ok());
    }
}
