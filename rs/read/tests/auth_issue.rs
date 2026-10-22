//! Task 3: token issuance, claim reading and cross-stack tokens.
//!
//! Every secret and key in this file is a fixed test value, never a runtime
//! credential. The Python side runs the project interpreter in `venv/`, which
//! the offline CI job and the test host both create, so a Rust-issued token is
//! verified by the code that will accept it during the migration.

use std::io::Write;
use std::process::{Command, Stdio};

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::auth::token_bytes;
use verdaxis_read::auth::{
    AccountState, TokenIssuer, VerificationSettings, cutoff_refusal, read_issuance, read_subject,
    verify_access,
};

const SECRET: &str = "phase3-task3-test-secret-at-least-32-characters";
const PREVIOUS_SECRET: &str = "phase3-task3-previous-secret-at-least-32-chars";
const UNKNOWN_SECRET: &str = "phase3-task3-unknown-secret-at-least-32-characters";
const ISSUER: &str = "verdaxis-test-api";
const AUDIENCE: &str = "verdaxis-test-web";
const SUBJECT: &str = "a550c69b-2934-4b36-87f5-e4f9976d1af0";

fn subject() -> Uuid {
    Uuid::parse_str(SUBJECT).unwrap()
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_792_497_600, 123_456_000).unwrap()
}

fn issuer() -> TokenIssuer {
    TokenIssuer::new(SECRET, ISSUER, AUDIENCE)
}

/// Decode without claim validation so a test can inspect exactly what was
/// signed. Signature validation stays on; these are not forged tokens.
fn claims(token: &str) -> Value {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.required_spec_claims.clear();
    validation.leeway = 0;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    decode::<Value>(
        token,
        &DecodingKey::from_secret(SECRET.as_bytes()),
        &validation,
    )
    .expect("the test secret signed this token")
    .claims
}

fn sign(claims: Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn account(
    password_changed_at: Option<i64>,
    authentication_revoked_at: Option<i64>,
) -> AccountState {
    AccountState {
        exists: true,
        status: "APPROVED".into(),
        must_change_password: false,
        password_changed_at,
        authentication_revoked_at,
    }
}

fn sorted_keys(claims: &Value) -> Vec<String> {
    let mut keys: Vec<String> = claims.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[test]
fn access_token_claims_are_stable() {
    let issued_at = now();
    let token = issuer().access(subject(), issued_at, None);
    let claims = claims(&token);

    assert_eq!(
        sorted_keys(&claims),
        ["aud", "exp", "iat", "iat_us", "iss", "sub", "type"].map(String::from)
    );
    assert_eq!(claims["sub"], SUBJECT);
    assert_eq!(claims["type"], "access");
    assert_eq!(claims["iss"], ISSUER);
    assert_eq!(claims["aud"], AUDIENCE);
    assert_eq!(claims["iat"], issued_at.timestamp());
    assert_eq!(claims["iat_us"], issued_at.timestamp_micros());
    assert_eq!(
        claims["exp"],
        (issued_at + Duration::minutes(15)).timestamp()
    );
    // No authorization claim travels in the token; the role and status are
    // read from the database on every request.
    for forbidden in ["role", "status", "organization_id", "org_id"] {
        assert!(
            claims.get(forbidden).is_none(),
            "{forbidden} must not be a claim"
        );
    }

    let verified = verify_access(
        &token,
        &VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into()),
        issued_at,
    )
    .unwrap();
    assert_eq!(verified.subject, subject());
    assert_eq!(verified.issued_at, Some(issued_at));
}

#[test]
fn issued_after_advances_past_cutoff() {
    let issued_at = now();
    for cutoff in [issued_at, issued_at + Duration::seconds(5)] {
        for token in [
            issuer().access(subject(), issued_at, Some(cutoff)),
            issuer().refresh(
                subject(),
                Uuid::new_v4(),
                &"A".repeat(43),
                issued_at,
                Some(cutoff),
            ),
        ] {
            let claims = claims(&token);
            assert_eq!(
                claims["iat_us"],
                cutoff.timestamp_micros() + 1,
                "a token minted at or before its cutoff moves one microsecond past it"
            );
            assert_eq!(
                claims["iat"],
                (cutoff + Duration::microseconds(1)).timestamp()
            );
        }
    }

    // A cutoff in the past does not move the issuance time forward.
    let earlier = issued_at - Duration::seconds(5);
    let token = issuer().access(subject(), issued_at, Some(earlier));
    assert_eq!(claims(&token)["iat_us"], issued_at.timestamp_micros());
}

#[test]
fn previous_key_verifies_during_rotation() {
    let issued_at = now();
    let rotation = VerificationSettings::new(
        SECRET.into(),
        Some(PREVIOUS_SECRET.into()),
        ISSUER.into(),
        AUDIENCE.into(),
    );

    let current = TokenIssuer::new(SECRET, ISSUER, AUDIENCE).access(subject(), issued_at, None);
    assert!(verify_access(&current, &rotation, issued_at).is_ok());

    let previous =
        TokenIssuer::new(PREVIOUS_SECRET, ISSUER, AUDIENCE).access(subject(), issued_at, None);
    assert!(verify_access(&previous, &rotation, issued_at).is_ok());

    let unknown =
        TokenIssuer::new(UNKNOWN_SECRET, ISSUER, AUDIENCE).access(subject(), issued_at, None);
    assert!(
        verify_access(&unknown, &rotation, issued_at).is_err(),
        "an unknown key must not verify during rotation"
    );
}

#[test]
fn refresh_token_is_43_url_safe_chars_and_hashes_to_sha256_hex() {
    let jti = token_bytes::generate_token();
    assert_eq!(jti.len(), 43);
    assert!(token_bytes::is_token_shaped(&jti), "{jti}");
    assert!(
        jti.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );
    assert!(!jti.contains('='));

    let family = Uuid::new_v4();
    let token = issuer().refresh(subject(), family, &jti, now(), None);
    let claims = claims(&token);
    assert_eq!(claims["type"], "refresh");
    assert_eq!(claims["sub"], SUBJECT);
    assert_eq!(claims["jti"], jti);
    assert_eq!(claims["family_id"], family.to_string());
    assert_eq!(claims["exp"], (now() + Duration::days(7)).timestamp());
    assert_eq!(claims["iat_us"], now().timestamp_micros());

    // SHA-256 hex, the storage form Python also computes for `jti_hash`.
    assert_eq!(
        token_bytes::token_digest("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let digest = token_bytes::token_digest(&jti);
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(digest, token_bytes::token_digest(&jti));

    // The device cookie has the same shape and never repeats.
    let device = token_bytes::generate_token();
    assert!(token_bytes::is_token_shaped(&device));
    assert_ne!(jti, device);
    assert!(!token_bytes::is_token_shaped("short"));
    assert!(!token_bytes::is_token_shaped(&"A".repeat(44)));
    assert!(!token_bytes::is_token_shaped(&format!(
        "{}=",
        &"A".repeat(42)
    )));
}

#[test]
fn stream_token_carries_org_and_environment() {
    let organization = Uuid::new_v4();
    let issued_at = now();
    let token = issuer().stream(subject(), organization, " STAGING ", issued_at);
    let claims = claims(&token);

    assert_eq!(
        sorted_keys(&claims),
        [
            "aud",
            "environment",
            "exp",
            "iat",
            "iat_us",
            "iss",
            "org_id",
            "sub",
            "type"
        ]
        .map(String::from)
    );
    assert_eq!(claims["sub"], SUBJECT);
    assert_eq!(claims["org_id"], organization.to_string());
    assert_eq!(claims["environment"], "staging");
    assert_eq!(claims["type"], "stream");
    assert_eq!(claims["iat_us"], issued_at.timestamp_micros());
    assert_eq!(
        claims["exp"],
        (issued_at + Duration::seconds(60)).timestamp()
    );
}

#[test]
fn pd7_access_token_iat_out_of_range_is_401() {
    let issued_at = now();
    let settings = VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into());
    let vectors = [
        ("i64::MAX", json!(i64::MAX)),
        ("u64::MAX", json!(u64::MAX)),
        ("negative", json!(-1)),
        ("float", json!(1.5)),
        ("string", json!("1792497600")),
        ("boolean", json!(true)),
    ];
    for (name, iat_us) in vectors {
        let token = sign(json!({
            "sub": SUBJECT,
            "exp": (issued_at + Duration::minutes(15)).timestamp(),
            "type": "access",
            "iat": (issued_at - Duration::seconds(60)).timestamp(),
            "iat_us": iat_us,
            "iss": ISSUER,
            "aud": AUDIENCE,
        }));
        let verified = verify_access(&token, &settings, issued_at).unwrap_or_else(|error| {
            panic!("{name}: the verifier must not reject the signature: {error:?}")
        });

        let password_cutoff = account(Some(issued_at.timestamp_micros()), None);
        let refused = verdaxis_read::auth::validate_account(&verified, &password_cutoff)
            .expect_err("an unusable issuance time fails an active password cutoff");
        assert_eq!(refused.status(), 401, "{name}");
        assert_eq!(
            refused.body()["detail"]["code"],
            "AUTH_PASSWORD_CHANGED",
            "{name}"
        );

        let revocation_cutoff = account(None, Some(issued_at.timestamp_micros()));
        let refused = verdaxis_read::auth::validate_account(&verified, &revocation_cutoff)
            .expect_err("an unusable issuance time fails an active revocation cutoff");
        assert_eq!(refused.status(), 401, "{name}");
        assert_eq!(
            refused.body()["detail"]["code"],
            "AUTH_SESSION_REVOKED",
            "{name}"
        );

        // With no cutoff the token is accepted, as Python accepts it.
        assert!(
            verdaxis_read::auth::validate_account(&verified, &account(None, None)).is_ok(),
            "{name}"
        );
    }
}

#[test]
fn pd7_refresh_iat_out_of_range_is_401() {
    let issued_at = now();
    let vectors = [
        ("i64::MAX", json!(i64::MAX)),
        ("u64::MAX", json!(u64::MAX)),
        ("negative", json!(-1)),
        ("float", json!(1.5)),
        ("string", json!("1792497600")),
        ("boolean", json!(true)),
    ];
    for (name, iat_us) in vectors {
        let token = sign(json!({
            "sub": SUBJECT,
            "exp": (issued_at + Duration::days(7)).timestamp(),
            "type": "refresh",
            "iat": (issued_at - Duration::seconds(60)).timestamp(),
            "iat_us": iat_us,
            "jti": "A".repeat(43),
            "family_id": Uuid::new_v4().to_string(),
            "iss": ISSUER,
            "aud": AUDIENCE,
        }));
        let issuance = read_issuance(&claims(&token));

        let refused = cutoff_refusal(issuance, &account(Some(issued_at.timestamp_micros()), None))
            .expect("an unusable refresh issuance time fails an active password cutoff");
        assert_eq!(refused.status(), 401, "{name}");
        assert_eq!(
            refused.body()["detail"]["code"],
            "AUTH_PASSWORD_CHANGED",
            "{name}"
        );

        let refused = cutoff_refusal(issuance, &account(None, Some(issued_at.timestamp_micros())))
            .expect("an unusable refresh issuance time fails an active revocation cutoff");
        assert_eq!(refused.status(), 401, "{name}");
        assert_eq!(
            refused.body()["detail"]["code"],
            "AUTH_SESSION_REVOKED",
            "{name}"
        );

        assert_eq!(
            cutoff_refusal(issuance, &account(None, None)),
            None,
            "{name}: no cutoff means no refusal"
        );
    }
}

/// The shared reader that refresh uses applies exactly the access verifier's
/// issuance rule, so one token gets the same issuance time on both paths.
#[test]
fn shared_issuance_reader_matches_the_access_verifier() {
    let issued_at = now();
    let settings = VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into());
    let earlier = issued_at - Duration::seconds(60);
    // A usable `iat` behind each `iat_us` vector, to show when it is used.
    let fallback = || Some(json!(earlier.timestamp()));
    // Integer literals outside both i64 and u64.
    let beyond_u64: Value = serde_json::from_str("100000000000000000000").unwrap();
    let below_i64: Value = serde_json::from_str("-100000000000000000000").unwrap();
    let vectors: [(&str, Option<Value>, Option<Value>); 15] = [
        ("integer", Some(json!(earlier.timestamp_micros())), None),
        ("true", Some(json!(true)), fallback()),
        ("false", Some(json!(false)), fallback()),
        ("i64::MAX", Some(json!(i64::MAX)), fallback()),
        ("u64::MAX", Some(json!(u64::MAX)), fallback()),
        ("beyond u64", Some(beyond_u64), fallback()),
        ("below i64", Some(below_i64), fallback()),
        ("negative", Some(json!(-1)), fallback()),
        ("float", Some(json!(1.5)), fallback()),
        ("string", Some(json!("1")), fallback()),
        ("null", Some(Value::Null), fallback()),
        ("fractional iat", None, Some(json!(1_792_497_540.25))),
        ("huge negative iat", None, Some(json!(-1e300))),
        ("string iat", None, Some(json!("1792497540"))),
        ("no issuance time", None, None),
    ];
    // A cutoff before every usable time: only a missing or unusable issuance
    // time is refused.
    let cutoff = account(Some(0), None);
    for (name, iat_us, iat) in vectors {
        let mut claims = json!({
            "sub": SUBJECT,
            "exp": (issued_at + Duration::minutes(15)).timestamp(),
            "type": "access",
            "iss": ISSUER,
            "aud": AUDIENCE,
        });
        if let Some(iat_us) = iat_us {
            claims["iat_us"] = iat_us;
        }
        if let Some(iat) = iat {
            claims["iat"] = iat;
        }
        let verified = verify_access(&sign(claims.clone()), &settings, issued_at)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let issuance = read_issuance(&claims);

        assert_eq!(issuance.issued_at, verified.issued_at, "{name}");
        assert_eq!(
            cutoff_refusal(issuance, &cutoff),
            verdaxis_read::auth::validate_account(&verified, &cutoff).err(),
            "{name}"
        );
    }

    // An `iat_us` that does not fit in i64 is unusable; it never falls back
    // to a later `iat`.
    let unusable = read_issuance(&json!({"iat_us": u64::MAX, "iat": issued_at.timestamp()}));
    assert!(unusable.out_of_range);
    assert_eq!(unusable.issued_at, None);
}

#[test]
fn d1_non_string_subject_is_401() {
    let issued_at = now();
    let settings = VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into());
    let vectors = [
        ("number", json!(42)),
        ("array", json!(["a550c69b-2934-4b36-87f5-e4f9976d1af0"])),
        (
            "object",
            json!({"id": "a550c69b-2934-4b36-87f5-e4f9976d1af0"}),
        ),
        ("null", Value::Null),
        ("boolean", json!(true)),
        ("unparsable string", json!("not-a-uuid")),
    ];
    for (name, sub) in vectors {
        let access = sign(json!({
            "sub": sub,
            "exp": (issued_at + Duration::minutes(15)).timestamp(),
            "type": "access",
            "iat": (issued_at - Duration::seconds(60)).timestamp(),
            "iat_us": (issued_at - Duration::seconds(60)).timestamp_micros(),
            "iss": ISSUER,
            "aud": AUDIENCE,
        }));
        let refused = verify_access(&access, &settings, issued_at)
            .expect_err("a non-string subject is not a credential");
        assert_eq!(refused.status(), 401, "access {name}");
        assert_eq!(
            refused.body()["detail"],
            "Could not validate credentials",
            "access {name}"
        );

        let refresh_claims = json!({
            "sub": sub,
            "exp": (issued_at + Duration::days(7)).timestamp(),
            "type": "refresh",
            "iat": (issued_at - Duration::seconds(60)).timestamp(),
            "iat_us": (issued_at - Duration::seconds(60)).timestamp_micros(),
            "jti": "A".repeat(43),
            "family_id": Uuid::new_v4().to_string(),
            "iss": ISSUER,
            "aud": AUDIENCE,
        });
        let refused = read_subject(&refresh_claims)
            .expect_err("a non-string refresh subject is not a credential");
        assert_eq!(refused.status(), 401, "refresh {name}");
        assert_eq!(
            refused.body()["detail"],
            "Could not validate credentials",
            "refresh {name}"
        );
    }

    // The shared reader accepts the canonical UUID string.
    assert_eq!(read_subject(&json!({"sub": SUBJECT})).unwrap(), subject());
}

#[test]
fn debug_output_never_contains_secret_material() {
    let issuer = TokenIssuer::new(SECRET, ISSUER, AUDIENCE);
    let diagnostic = format!("{issuer:?}");
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(SECRET));

    let settings = VerificationSettings::new(
        SECRET.into(),
        Some(PREVIOUS_SECRET.into()),
        ISSUER.into(),
        AUDIENCE.into(),
    );
    let diagnostic = format!("{settings:?}");
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(SECRET));
    assert!(!diagnostic.contains(PREVIOUS_SECRET));
}

#[test]
fn python_issued_tokens_verify_in_rust() {
    let script = r#"
import json
from datetime import UTC, datetime
from app.config import settings
from app.core.security import create_access_token, create_refresh_token

subject = "a550c69b-2934-4b36-87f5-e4f9976d1af0"
family = "3f1a2b4c-5d6e-4f70-8a91-b2c3d4e5f607"
issued_at = datetime(2026, 10, 20, 12, 0, 0, 123456, tzinfo=UTC)
print(json.dumps({
    "issuer": settings.JWT_ISSUER,
    "audience": settings.JWT_AUDIENCE,
    "issued_at_us": int(issued_at.timestamp()) * 1_000_000 + issued_at.microsecond,
    "access_token": create_access_token(subject=subject, issued_at=issued_at),
    "refresh_token": create_refresh_token(subject=subject, family_id=family, issued_at=issued_at),
}))
"#;
    let output = run_python(script, None);
    let document: Value = serde_json::from_str(&output).unwrap();
    let issued_at =
        DateTime::from_timestamp_micros(document["issued_at_us"].as_i64().unwrap()).unwrap();
    let settings = VerificationSettings::new(
        SECRET.into(),
        None,
        document["issuer"].as_str().unwrap().into(),
        document["audience"].as_str().unwrap().into(),
    );

    let verified = verify_access(
        document["access_token"].as_str().unwrap(),
        &settings,
        issued_at,
    )
    .expect("Rust verifies a Python-issued access token");
    assert_eq!(verified.subject, subject());
    assert_eq!(verified.issued_at, Some(issued_at));

    let refresh_claims = claims(document["refresh_token"].as_str().unwrap());
    assert_eq!(refresh_claims["type"], "refresh");
    assert_eq!(read_subject(&refresh_claims).unwrap(), subject());
    assert_eq!(read_issuance(&refresh_claims).issued_at, Some(issued_at));
    assert_eq!(
        refresh_claims["family_id"],
        "3f1a2b4c-5d6e-4f70-8a91-b2c3d4e5f607"
    );
    assert!(token_bytes::is_token_shaped(
        refresh_claims["jti"].as_str().unwrap()
    ));
}

#[test]
fn rust_issued_tokens_verify_in_python() {
    let organization = Uuid::new_v4();
    let family = Uuid::new_v4();
    let jti = token_bytes::generate_token();
    let issued_at = Utc::now();
    let document = json!({
        "access_token": issuer().access(subject(), issued_at, None),
        "refresh_token": issuer().refresh(subject(), family, &jti, issued_at, None),
        "stream_token": issuer().stream(subject(), organization, "test", issued_at),
    });
    let script = r#"
import json, sys
from app.core.security import decode_token

document = json.load(sys.stdin)
def claims(token):
    payload = decode_token(token)
    return {
        "sub": payload["sub"],
        "type": payload["type"],
        "jti": payload.get("jti"),
        "family_id": payload.get("family_id"),
        "org_id": payload.get("org_id"),
        "environment": payload.get("environment"),
        "iat_us": payload.get("iat_us"),
    }
print(json.dumps({name: claims(document[name]) for name in
                  ("access_token", "refresh_token", "stream_token")}))
"#;
    let output = run_python(script, Some(&document.to_string()));
    let verified: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(verified["access_token"]["sub"], SUBJECT);
    assert_eq!(verified["access_token"]["type"], "access");
    assert_eq!(
        verified["access_token"]["iat_us"],
        issued_at.timestamp_micros()
    );
    assert_eq!(verified["refresh_token"]["type"], "refresh");
    assert_eq!(verified["refresh_token"]["jti"], jti);
    assert_eq!(verified["refresh_token"]["family_id"], family.to_string());
    assert_eq!(verified["stream_token"]["type"], "stream");
    assert_eq!(verified["stream_token"]["org_id"], organization.to_string());
    assert_eq!(verified["stream_token"]["environment"], "test");
}

fn run_python(script: &str, input: Option<&str>) -> String {
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "venv/bin/python".into());
    let mut child = Command::new(python)
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", SECRET)
        .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
        .args(["-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the project Python interpreter at venv/bin/python runs");
    if let Some(input) = input {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
