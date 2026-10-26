//! The shared signed-token verifier, exercised through its public interface.
//!
//! Every profile goes through one cryptographic path: HS256 with the current
//! and previous keys, the configured issuer and audience, the time claims and
//! the expected token type. Fixed test values only; no key here is a runtime
//! credential.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::auth::{
    AuthError, TokenType, VerificationSettings, verify_access, verify_token,
};

const SECRET: &str = "shared-verifier-current-secret-at-least-32-characters";
const PREVIOUS_SECRET: &str = "shared-verifier-previous-secret-at-least-32-characters";
const UNKNOWN_SECRET: &str = "shared-verifier-unknown-secret-at-least-32-characters";
const ISSUER: &str = "verdaxis-test-api";
const AUDIENCE: &str = "verdaxis-test-web";
const SUBJECT: &str = "a550c69b-2934-4b36-87f5-e4f9976d1af0";
const ORGANIZATION: &str = "0f9f0d4c-0d43-4b57-a0d7-6a1b9f5f2f4e";
const PROFILES: [TokenType; 2] = [TokenType::Access, TokenType::Stream];
/// Base64url of `{"alg":"none","typ":"JWT"}`: a header that claims no signature.
const UNSIGNED_HEADER: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";

fn now() -> DateTime<Utc> {
    // A fixed instant with a microsecond part keeps every assertion exact.
    DateTime::from_timestamp(1_792_497_600, 123_456_000).unwrap()
}

fn subject() -> Uuid {
    Uuid::parse_str(SUBJECT).unwrap()
}

fn current_only() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into())
}

fn rotation() -> VerificationSettings {
    VerificationSettings::new(
        SECRET.into(),
        Some(PREVIOUS_SECRET.into()),
        ISSUER.into(),
        AUDIENCE.into(),
    )
}

fn sign(secret: &str, claims: &Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

/// The claims each issuer writes for one profile.
fn claims(profile: TokenType) -> Value {
    let issued_at = now();
    let mut claims = json!({
        "sub": SUBJECT,
        "iat": issued_at.timestamp(),
        "iat_us": issued_at.timestamp_micros(),
        "iss": ISSUER,
        "aud": AUDIENCE,
    });
    match profile {
        TokenType::Access => {
            claims["type"] = json!("access");
            claims["exp"] = json!((issued_at + Duration::minutes(15)).timestamp());
        }
        TokenType::Stream => {
            claims["type"] = json!("stream");
            claims["exp"] = json!((issued_at + Duration::seconds(60)).timestamp());
            claims["org_id"] = json!(ORGANIZATION);
            claims["environment"] = json!("test");
        }
    }
    claims
}

fn other(profile: TokenType) -> TokenType {
    match profile {
        TokenType::Access => TokenType::Stream,
        TokenType::Stream => TokenType::Access,
    }
}

fn refused(token: &str, settings: &VerificationSettings, profile: TokenType) -> bool {
    matches!(
        verify_token(token, settings, profile, now()),
        Err(AuthError::Credentials)
    )
}

#[test]
fn each_profile_accepts_only_its_own_token_type() {
    for profile in PROFILES {
        let token = sign(SECRET, &claims(profile));
        let verified = verify_token(&token, &current_only(), profile, now())
            .unwrap_or_else(|error| panic!("{profile:?} must accept its own token: {error:?}"));
        assert_eq!(verified.subject(), subject());
        assert!(
            refused(&token, &current_only(), other(profile)),
            "a {profile:?} token must be refused by the other profile"
        );
    }
}

#[test]
fn access_verification_refuses_a_stream_token() {
    let token = sign(SECRET, &claims(TokenType::Stream));
    assert!(matches!(
        verify_access(&token, &current_only(), now()),
        Err(AuthError::Credentials)
    ));
}

#[test]
fn stream_verification_refuses_an_access_token() {
    let token = sign(SECRET, &claims(TokenType::Access));
    assert!(refused(&token, &current_only(), TokenType::Stream));
    // Control: the same token is still a valid access token.
    assert_eq!(
        verify_access(&token, &current_only(), now())
            .unwrap()
            .subject,
        subject()
    );
}

#[test]
fn refresh_or_untyped_tokens_match_no_profile() {
    let types = [
        ("refresh", Some(json!("refresh"))),
        ("missing", None),
        ("numeric", Some(json!(7))),
        ("upper case", Some(json!("STREAM"))),
    ];
    for (name, value) in types {
        for profile in PROFILES {
            let mut claims = claims(profile);
            match &value {
                Some(value) => claims["type"] = value.clone(),
                None => {
                    claims.as_object_mut().unwrap().remove("type");
                }
            }
            let token = sign(SECRET, &claims);
            assert!(
                refused(&token, &current_only(), profile),
                "{name} type, {profile:?} profile"
            );
        }
    }
}

#[test]
fn wrong_issuer_or_audience_is_refused_for_every_profile() {
    let mutations = [
        ("another issuer", "iss", Some(json!("another-issuer"))),
        ("null issuer", "iss", Some(Value::Null)),
        ("missing issuer", "iss", None),
        ("another audience", "aud", Some(json!("another-audience"))),
        (
            "another audience list",
            "aud",
            Some(json!(["another-audience"])),
        ),
        (
            "non-string audience member",
            "aud",
            Some(json!([AUDIENCE, 7])),
        ),
        ("missing audience", "aud", None),
    ];
    for profile in PROFILES {
        for (name, claim, value) in &mutations {
            let mut claims = claims(profile);
            match value {
                Some(value) => claims[*claim] = value.clone(),
                None => {
                    claims.as_object_mut().unwrap().remove(*claim);
                }
            }
            let token = sign(SECRET, &claims);
            assert!(
                refused(&token, &current_only(), profile),
                "{name}, {profile:?} profile"
            );
        }
        // Control: an audience list that names the configured audience.
        let mut claims = claims(profile);
        claims["aud"] = json!(["another-audience", AUDIENCE]);
        let token = sign(SECRET, &claims);
        assert!(verify_token(&token, &current_only(), profile, now()).is_ok());
    }
}

#[test]
fn previous_key_follows_the_rotation_rule_for_every_profile() {
    for profile in PROFILES {
        let current = sign(SECRET, &claims(profile));
        let previous = sign(PREVIOUS_SECRET, &claims(profile));
        let unknown = sign(UNKNOWN_SECRET, &claims(profile));
        assert!(verify_token(&current, &current_only(), profile, now()).is_ok());
        assert!(verify_token(&current, &rotation(), profile, now()).is_ok());
        assert!(verify_token(&previous, &rotation(), profile, now()).is_ok());
        assert!(
            refused(&previous, &current_only(), profile),
            "the previous key is not a key of a current-only configuration"
        );
        assert!(refused(&unknown, &rotation(), profile));
    }
    // The access entry point applies the same rotation rule.
    let previous = sign(PREVIOUS_SECRET, &claims(TokenType::Access));
    assert!(verify_access(&previous, &rotation(), now()).is_ok());
    assert!(verify_access(&previous, &current_only(), now()).is_err());
}

#[test]
fn time_claims_are_checked_for_every_profile() {
    let instant = now().timestamp();
    let mutations = [
        ("expiry at the current second", "exp", json!(instant)),
        ("issued in the future", "iat", json!(instant + 1)),
        ("not yet valid", "nbf", json!(instant + 1)),
        ("non-numeric expiry", "exp", json!("soon")),
    ];
    for profile in PROFILES {
        for (name, claim, value) in &mutations {
            let mut claims = claims(profile);
            claims[*claim] = value.clone();
            let token = sign(SECRET, &claims);
            assert!(
                refused(&token, &current_only(), profile),
                "{name}, {profile:?} profile"
            );
        }
    }
}

#[test]
fn only_hs256_signatures_are_accepted() {
    for profile in PROFILES {
        let claims = claims(profile);
        let hs512 = encode(
            &Header::new(Algorithm::HS512),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        assert!(refused(&hs512, &current_only(), profile), "HS512");

        let signed = sign(SECRET, &claims);
        let mut parts = signed.split('.');
        let (_, payload, signature) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        for unsigned in [
            format!("{UNSIGNED_HEADER}.{payload}."),
            format!("{UNSIGNED_HEADER}.{payload}.{signature}"),
        ] {
            assert!(refused(&unsigned, &current_only(), profile), "{unsigned}");
        }
    }
}

#[test]
fn verified_token_exposes_subject_issuance_and_profile_claims() {
    let token = sign(SECRET, &claims(TokenType::Stream));
    let verified = verify_token(&token, &current_only(), TokenType::Stream, now()).unwrap();
    assert_eq!(verified.subject(), subject());
    assert_eq!(verified.issuance().issued_at, Some(now()));
    assert!(!verified.issuance().out_of_range);
    assert_eq!(verified.claim("org_id"), Some(&json!(ORGANIZATION)));
    assert_eq!(verified.claim("environment"), Some(&json!("test")));
    assert_eq!(verified.claim("absent"), None);
    // Debug output names no identifier, so an accidental log stays bounded.
    let debug = format!("{verified:?}");
    assert!(!debug.contains(SUBJECT), "{debug}");
    assert!(!debug.contains(ORGANIZATION), "{debug}");
}

#[test]
fn every_profile_requires_a_present_expiry() {
    // Mandatory `exp` now applies to both profiles through the shared
    // verifier; removing it must refuse, not pass through as before.
    for profile in [TokenType::Access, TokenType::Stream] {
        let mut claims = claims(profile);
        claims.as_object_mut().unwrap().remove("exp");
        let token = sign(SECRET, &claims);
        assert!(
            verify_token(&token, &current_only(), profile, now()).is_err(),
            "{profile:?} token without exp must be refused"
        );
    }
}

#[test]
fn verified_token_exposes_the_verified_expiry() {
    // JWT NumericDate is whole seconds, so compare at that precision.
    let expected =
        DateTime::<Utc>::from_timestamp((now() + Duration::minutes(15)).timestamp(), 0).unwrap();
    let mut claims = claims(TokenType::Access);
    claims["exp"] = json!(expected.timestamp());
    let token = sign(SECRET, &claims);
    let verified = verify_token(&token, &current_only(), TokenType::Access, now()).unwrap();
    assert_eq!(verified.expires_at(), expected);
}
