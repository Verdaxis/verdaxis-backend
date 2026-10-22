//! Stream token validation and current account/organization admission.
//!
//! Every refusal is a bounded class that is known before a response header
//! exists. Fixed test values only; no key or claim here is a runtime
//! credential. `postgres_admission` needs the disposable PostgreSQL harness
//! (`tests/postgres/test_phase6_authorization.py`) and is ignored otherwise.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;
use verdaxis_read::auth::{
    AccountState, AuthError, TokenIssuer, VerificationSettings, cutoff_refusal, verify_access,
};
use verdaxis_read::streams::auth::{
    MAX_TOKEN_BYTES, StreamError, StreamPrincipal, admit_account, admit_organization, revalidate,
    verify_stream,
};

const SECRET: &str = "stream-admission-current-secret-at-least-32-characters";
const PREVIOUS_SECRET: &str = "stream-admission-previous-secret-at-least-32-characters";
const UNKNOWN_SECRET: &str = "stream-admission-unknown-secret-at-least-32-characters";
const ISSUER: &str = "verdaxis-test-api";
const AUDIENCE: &str = "verdaxis-test-web";
const ENVIRONMENT: &str = "test";
const SUBJECT: &str = "a550c69b-2934-4b36-87f5-e4f9976d1af0";
const ORGANIZATION: &str = "0f9f0d4c-0d43-4b57-a0d7-6a1b9f5f2f4e";
/// Base64url of `{"alg":"none","typ":"JWT"}`: a header that claims no signature.
const UNSIGNED_HEADER: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";
/// 10000-01-01T00:00:00Z, the first instant after the bounded date range.
const YEAR_10000_SECONDS: i64 = 253_402_300_800;

fn subject() -> Uuid {
    Uuid::parse_str(SUBJECT).unwrap()
}

fn organization() -> Uuid {
    Uuid::parse_str(ORGANIZATION).unwrap()
}

fn now() -> DateTime<Utc> {
    // A fixed instant with a microsecond part keeps every assertion exact.
    DateTime::from_timestamp(1_792_497_600, 123_456_000).unwrap()
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

fn sign_with(secret: &str, claims: &Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

fn sign(claims: &Value) -> String {
    sign_with(SECRET, claims)
}

/// The claims the stream-token issuer writes.
fn stream_claims() -> Value {
    let issued_at = now();
    json!({
        "sub": SUBJECT,
        "org_id": ORGANIZATION,
        "environment": ENVIRONMENT,
        "exp": (issued_at + Duration::seconds(60)).timestamp(),
        "type": "stream",
        "iat": issued_at.timestamp(),
        "iat_us": issued_at.timestamp_micros(),
        "iss": ISSUER,
        "aud": AUDIENCE,
    })
}

fn stream_with(claim: &str, value: Value) -> Value {
    let mut claims = stream_claims();
    claims[claim] = value;
    claims
}

fn stream_without(claim: &str) -> Value {
    let mut claims = stream_claims();
    claims.as_object_mut().unwrap().remove(claim);
    claims
}

fn access_claims() -> Value {
    let issued_at = now();
    json!({
        "sub": SUBJECT,
        "exp": (issued_at + Duration::minutes(15)).timestamp(),
        "type": "access",
        "iat": issued_at.timestamp(),
        "iat_us": issued_at.timestamp_micros(),
        "iss": ISSUER,
        "aud": AUDIENCE,
    })
}

fn verify(token: &str) -> Result<StreamPrincipal, StreamError> {
    verify_stream(token, &current_only(), ENVIRONMENT, now())
}

fn principal() -> StreamPrincipal {
    verify(&sign(&stream_claims())).expect("the issued stream claims are valid")
}

fn account(
    status: &str,
    must_change_password: bool,
    password_changed_at: Option<i64>,
    authentication_revoked_at: Option<i64>,
) -> AccountState {
    AccountState {
        exists: true,
        status: status.into(),
        must_change_password,
        password_changed_at,
        authentication_revoked_at,
    }
}

fn approved() -> AccountState {
    account("APPROVED", false, None, None)
}

/// Assert that every named claim set is refused as an invalid stream token.
fn assert_refused(vectors: &[(&str, Value)]) {
    for (name, claims) in vectors {
        let error = verify(&sign(claims)).expect_err(name);
        assert_eq!(error, StreamError::TokenInvalid, "{name}");
        assert_eq!(error.status(), 401, "{name}");
        assert_eq!(error.code(), "STREAM_TOKEN_INVALID", "{name}");
    }
}

#[test]
fn malformed_stream_claims_are_401_before_headers() {
    let instant = now().timestamp();
    assert_refused(&[
        ("missing subject", stream_without("sub")),
        ("numeric subject", stream_with("sub", json!(7))),
        ("invalid subject", stream_with("sub", json!("not-a-uuid"))),
        ("missing organization", stream_without("org_id")),
        ("null organization", stream_with("org_id", Value::Null)),
        (
            "invalid organization",
            stream_with("org_id", json!("not-a-uuid")),
        ),
        ("missing environment", stream_without("environment")),
        (
            "other environment",
            stream_with("environment", json!("production")),
        ),
        (
            "unnormalized environment",
            stream_with("environment", json!(" TEST ")),
        ),
        ("missing type", stream_without("type")),
        ("access type", stream_with("type", json!("access"))),
        ("missing expiry", stream_without("exp")),
        ("null expiry", stream_with("exp", Value::Null)),
        ("expired", stream_with("exp", json!(instant - 1))),
        (
            "issued in the future",
            stream_with("iat", json!(instant + 1)),
        ),
        ("not yet valid", stream_with("nbf", json!(instant + 1))),
        ("claims that are not an object", json!(["stream", SUBJECT])),
    ]);
    for token in ["not-a-token", "two.segments", "a.b.c", "..", "\u{0}"] {
        assert_eq!(
            verify(token).unwrap_err(),
            StreamError::TokenInvalid,
            "{token:?}"
        );
    }
    let required = verify("").unwrap_err();
    assert_eq!(required, StreamError::TokenRequired);
    assert_eq!(required.status(), 401);
    assert_eq!(required.code(), "STREAM_TOKEN_REQUIRED");
}

#[test]
fn required_claims_are_typed() {
    let expiry = now().timestamp() + 60;
    assert_refused(&[
        (
            "string expiry",
            stream_with("exp", json!(expiry.to_string())),
        ),
        ("boolean expiry", stream_with("exp", json!(true))),
        (
            "fractional expiry",
            stream_with("exp", json!(expiry as f64 + 0.5)),
        ),
        ("float expiry", stream_with("exp", json!(expiry as f64))),
        ("numeric organization", stream_with("org_id", json!(7))),
        (
            "array organization",
            stream_with("org_id", json!([ORGANIZATION])),
        ),
        ("numeric environment", stream_with("environment", json!(7))),
        (
            "array environment",
            stream_with("environment", json!([ENVIRONMENT])),
        ),
        (
            "boolean environment",
            stream_with("environment", json!(true)),
        ),
        ("array subject", stream_with("sub", json!([SUBJECT]))),
        ("object subject", stream_with("sub", json!({"id": SUBJECT}))),
        ("numeric type", stream_with("type", json!(7))),
        ("boolean type", stream_with("type", json!(true))),
    ]);
    // Control: the typed issuer claims are accepted.
    let principal = principal();
    assert_eq!(principal.user_id(), subject());
    assert_eq!(principal.organization_id(), organization());
}

#[test]
fn bounded_token_dates() {
    let instant = now().timestamp();
    assert_refused(&[
        ("i64 maximum expiry", stream_with("exp", json!(i64::MAX))),
        ("i64 minimum expiry", stream_with("exp", json!(i64::MIN))),
        ("expiry beyond u64", stream_with("exp", json!(1e30))),
        (
            "expiry after year 9999",
            stream_with("exp", json!(YEAR_10000_SECONDS)),
        ),
        (
            "expiry at the current second",
            stream_with("exp", json!(instant)),
        ),
        (
            "expired one second ago",
            stream_with("exp", json!(instant - 1)),
        ),
    ]);

    // The expiry is the signed instant, and the last second of year 9999 is
    // still inside the bounded range.
    assert_eq!(
        principal().expires_at(),
        DateTime::from_timestamp(instant + 60, 0).unwrap()
    );
    let last = verify(&sign(&stream_with("exp", json!(YEAR_10000_SECONDS - 1))))
        .expect("the last second of year 9999 is a bounded expiry");
    assert_eq!(
        last.expires_at(),
        DateTime::from_timestamp(YEAR_10000_SECONDS - 1, 0).unwrap()
    );

    // The issuance time is exact to the microsecond.
    assert_eq!(principal().issuance().issued_at, Some(now()));
    // An issuance time outside the range verifies, but no account cutoff can
    // admit it: it is refused whenever a cutoff exists.
    for (name, value) in [
        ("after year 9999", json!(YEAR_10000_SECONDS * 1_000_000)),
        ("beyond i64", json!(u64::MAX)),
    ] {
        let principal = verify(&sign(&stream_with("iat_us", value)))
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert!(principal.issuance().out_of_range, "{name}");
        assert_eq!(admit_account(&principal, &approved()), Ok(()), "{name}");
        let cutoff = now().timestamp_micros() - 1_000_000;
        assert_eq!(
            admit_account(&principal, &account("APPROVED", false, Some(cutoff), None)),
            Err(StreamError::TokenInvalid),
            "{name}"
        );
    }
}

#[test]
fn token_profile_restrictions() {
    let mut refresh = access_claims();
    refresh["type"] = json!("refresh");
    refresh["jti"] = json!("a-refresh-token-identifier");
    assert_refused(&[
        ("access token", access_claims()),
        ("refresh token", refresh),
        (
            "another issuer",
            stream_with("iss", json!("another-issuer")),
        ),
        ("missing issuer", stream_without("iss")),
        (
            "another audience",
            stream_with("aud", json!("another-audience")),
        ),
        ("missing audience", stream_without("aud")),
    ]);

    // Only HS256 signatures are accepted.
    let claims = stream_claims();
    let hs512 = encode(
        &Header::new(Algorithm::HS512),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    assert_eq!(verify(&hs512).unwrap_err(), StreamError::TokenInvalid);
    let signed = sign(&claims);
    let parts: Vec<&str> = signed.split('.').collect();
    for unsigned in [
        format!("{UNSIGNED_HEADER}.{}.", parts[1]),
        format!("{UNSIGNED_HEADER}.{}.{}", parts[1], parts[2]),
    ] {
        assert_eq!(verify(&unsigned).unwrap_err(), StreamError::TokenInvalid);
    }

    // The configured environment is normalized the way the issuer writes it.
    assert!(verify_stream(&signed, &current_only(), " Test ", now()).is_ok());
    assert_eq!(
        verify_stream(&signed, &current_only(), "staging", now()).unwrap_err(),
        StreamError::TokenInvalid
    );
}

#[test]
fn current_and_previous_keys() {
    let current = sign_with(SECRET, &stream_claims());
    let previous = sign_with(PREVIOUS_SECRET, &stream_claims());
    let unknown = sign_with(UNKNOWN_SECRET, &stream_claims());
    let with_rotation = |token: &str| verify_stream(token, &rotation(), ENVIRONMENT, now());

    assert!(verify(&current).is_ok());
    assert!(with_rotation(&current).is_ok());
    assert!(with_rotation(&previous).is_ok());
    assert_eq!(verify(&previous).unwrap_err(), StreamError::TokenInvalid);
    assert_eq!(
        with_rotation(&unknown).unwrap_err(),
        StreamError::TokenInvalid
    );
}

#[test]
fn uuid_claims_required() {
    let simple = SUBJECT.replace('-', "");
    for value in [
        SUBJECT.to_owned(),
        simple.clone(),
        format!("{{{SUBJECT}}}"),
        format!("urn:uuid:{SUBJECT}"),
    ] {
        let principal = verify(&sign(&stream_with("sub", json!(value))))
            .unwrap_or_else(|error| panic!("{value} must be a subject: {error:?}"));
        assert_eq!(principal.user_id(), subject());
    }
    let organization_simple = ORGANIZATION.replace('-', "");
    for value in [
        ORGANIZATION.to_owned(),
        organization_simple,
        format!("{{{ORGANIZATION}}}"),
        format!("urn:uuid:{ORGANIZATION}"),
    ] {
        let principal = verify(&sign(&stream_with("org_id", json!(value))))
            .unwrap_or_else(|error| panic!("{value} must be an organization: {error:?}"));
        assert_eq!(principal.organization_id(), organization());
    }
    assert_refused(&[
        ("empty subject", stream_with("sub", json!(""))),
        ("short subject", stream_with("sub", json!(&simple[1..]))),
        ("empty organization", stream_with("org_id", json!(""))),
        ("object organization", stream_with("org_id", json!({}))),
        (
            "long organization",
            stream_with("org_id", json!(format!("{ORGANIZATION}0"))),
        ),
    ]);
}

#[test]
fn oversized_token_is_refused_before_parsing() {
    // Pad otherwise valid claims so the signed token lands on each side of the
    // bound: the bound alone decides the outcome.
    let below = sign(&stream_with("padding", json!("p".repeat(11_000))));
    let above = sign(&stream_with("padding", json!("p".repeat(12_500))));
    assert!(below.len() <= MAX_TOKEN_BYTES, "{}", below.len());
    assert!(above.len() > MAX_TOKEN_BYTES, "{}", above.len());
    assert!(verify(&below).is_ok());
    assert_eq!(verify(&above).unwrap_err(), StreamError::TokenInvalid);
    assert_eq!(
        verify(&"a".repeat(MAX_TOKEN_BYTES + 1)).unwrap_err(),
        StreamError::TokenInvalid
    );
}

/// A stream token over the issued claims plus a padding claim, grown until it
/// is exactly `length` bytes long.
fn signed_token_of_length(length: usize) -> String {
    let padded = |padding: usize| sign(&stream_with("padding", json!("p".repeat(padding))));
    // Base64url writes 4 bytes for every 3 claim bytes: start a little below
    // that estimate and grow the claim one byte at a time.
    let mut padding = (length - padded(0).len()) * 3 / 4 - 3;
    loop {
        let token = padded(padding);
        if token.len() >= length {
            assert_eq!(
                token.len(),
                length,
                "no padding gives exactly {length} bytes"
            );
            return token;
        }
        padding += 1;
    }
}

#[test]
fn token_size_bound_is_inclusive() {
    // Both tokens carry valid claims and signatures, so only the size bound
    // decides.
    let at_bound = signed_token_of_length(MAX_TOKEN_BYTES);
    assert!(
        verify(&at_bound).is_ok(),
        "a token of exactly the bound is parsed"
    );
    let over_bound = signed_token_of_length(MAX_TOKEN_BYTES + 1);
    assert_eq!(verify(&over_bound).unwrap_err(), StreamError::TokenInvalid);
}

#[test]
fn principal_debug_names_no_identifier() {
    let debug = format!("{:?}", principal());
    assert!(!debug.contains(SUBJECT), "{debug}");
    assert!(!debug.contains(ORGANIZATION), "{debug}");
}

#[test]
fn cutoff_is_inclusive() {
    let principal = principal();
    let issued = now().timestamp_micros();

    let at_password_change = account("APPROVED", false, Some(issued), None);
    assert_eq!(
        admit_account(&principal, &at_password_change),
        Err(StreamError::TokenInvalid),
        "a token issued at the password cutoff is cut off"
    );
    let before_password_change = account("APPROVED", false, Some(issued - 1), None);
    assert_eq!(admit_account(&principal, &before_password_change), Ok(()));

    let at_revocation = account("APPROVED", false, None, Some(issued));
    assert_eq!(
        admit_account(&principal, &at_revocation),
        Err(StreamError::TokenInvalid)
    );
    let before_revocation = account("APPROVED", false, None, Some(issued - 1));
    assert_eq!(admit_account(&principal, &before_revocation), Ok(()));

    // A token without any issuance time is refused under an active cutoff.
    let mut undated_claims = stream_without("iat_us");
    undated_claims.as_object_mut().unwrap().remove("iat");
    let undated = verify(&sign(&undated_claims)).expect("issuance claims are optional");
    assert_eq!(undated.issuance().issued_at, None);
    assert_eq!(admit_account(&undated, &approved()), Ok(()));
    assert_eq!(
        admit_account(&undated, &before_password_change),
        Err(StreamError::TokenInvalid)
    );
}

#[test]
fn password_cutoff_precedes_revocation() {
    let principal = principal();
    let issued = now().timestamp_micros();
    let both = account("APPROVED", false, Some(issued), Some(issued - 10));
    assert_eq!(
        cutoff_refusal(principal.issuance(), &both),
        Some(AuthError::PasswordChanged)
    );
    assert_eq!(
        admit_account(&principal, &both),
        Err(StreamError::TokenInvalid)
    );

    let revoked_only = account("APPROVED", false, Some(issued - 10), Some(issued));
    assert_eq!(
        cutoff_refusal(principal.issuance(), &revoked_only),
        Some(AuthError::SessionRevoked)
    );
    assert_eq!(
        admit_account(&principal, &revoked_only),
        Err(StreamError::TokenInvalid)
    );

    // Both cutoffs precede the issuance: the token stays usable.
    let before = account("APPROVED", false, Some(issued - 2), Some(issued - 1));
    assert_eq!(cutoff_refusal(principal.issuance(), &before), None);
    assert_eq!(admit_account(&principal, &before), Ok(()));
}

#[test]
fn current_account_required() {
    let principal = principal();
    let mut missing = approved();
    missing.exists = false;
    assert_eq!(
        admit_account(&principal, &missing),
        Err(StreamError::TokenInvalid)
    );
    for status in ["PENDING", "REJECTED", "approved", "SUSPENDED", ""] {
        assert_eq!(
            admit_account(&principal, &account(status, false, None, None)),
            Err(StreamError::TokenInvalid),
            "{status}"
        );
    }
    assert_eq!(
        admit_account(&principal, &account("APPROVED", true, None, None)),
        Err(StreamError::TokenInvalid),
        "a required password change refuses the stream"
    );
    assert_eq!(admit_account(&principal, &approved()), Ok(()));
}

#[test]
fn current_organization_required() {
    for status in [
        None,
        Some("PENDING"),
        Some("REJECTED"),
        Some("approved"),
        Some(""),
    ] {
        assert_eq!(
            admit_organization(status),
            Err(StreamError::TokenInvalid),
            "{status:?}"
        );
    }
    assert_eq!(admit_organization(Some("APPROVED")), Ok(()));
}

#[test]
fn auth_store_unavailable_before_headers() {
    // A failed store read is its own class, so an HTTP owner answers 503
    // before it builds an event stream; `postgres_admission` produces it from
    // a terminated database session.
    let unavailable = StreamError::AuthorizationUnavailable;
    assert_eq!(unavailable.status(), 503);
    assert_eq!(unavailable.code(), "STREAM_AUTH_UNAVAILABLE");
    for refusal in [StreamError::TokenRequired, StreamError::TokenInvalid] {
        assert_ne!(unavailable.status(), refusal.status());
        assert_ne!(unavailable.code(), refusal.code());
    }
}

#[test]
fn no_access_or_refresh_fallback() {
    let access = sign(&access_claims());
    let mut refresh = access_claims();
    refresh["type"] = json!("refresh");
    let refresh = sign(&refresh);
    let stream = sign(&stream_claims());
    let untyped = sign(&stream_without("type"));

    for (name, token) in [
        ("access", &access),
        ("refresh", &refresh),
        ("untyped", &untyped),
    ] {
        assert_eq!(
            verify(token).unwrap_err(),
            StreamError::TokenInvalid,
            "{name}"
        );
    }
    // The stream token authenticates nothing outside stream admission.
    for (name, token) in [("stream", &stream), ("untyped", &untyped)] {
        assert!(
            matches!(
                verify_access(token, &current_only(), now()),
                Err(AuthError::Credentials)
            ),
            "{name}"
        );
    }
    // Control: the access token is still an access token.
    assert!(verify_access(&access, &current_only(), now()).is_ok());
}

#[test]
fn no_execution_gate() {
    // The exhaustive match stops compiling if admission gains a refusal class,
    // for example a settlement, trade or KYC state.
    for class in [
        StreamError::TokenRequired,
        StreamError::TokenInvalid,
        StreamError::AuthorizationUnavailable,
    ] {
        match class {
            StreamError::TokenRequired | StreamError::TokenInvalid => {
                assert_eq!(class.status(), 401)
            }
            StreamError::AuthorizationUnavailable => assert_eq!(class.status(), 503),
        }
    }
    // An approved account is admitted from account state alone;
    // `postgres_admission` also admits an account whose KYC was rejected.
    assert_eq!(admit_account(&principal(), &approved()), Ok(()));
}

/// The rows that `tests/postgres/test_phase6_authorization.py` seeds. The
/// harness passes their identifiers in the environment.
struct Fixture {
    url: String,
    issued_at: DateTime<Utc>,
    approved_org: Uuid,
    other_org: Uuid,
    pending_org: Uuid,
    issuer: TokenIssuer,
}

impl Fixture {
    fn from_env() -> Self {
        let issued_at_us: i64 = env_value("STREAM_AUTH_TEST_ISSUED_AT_US").parse().unwrap();
        Self {
            url: env_value("STREAM_AUTH_TEST_DATABASE_URL"),
            issued_at: DateTime::from_timestamp_micros(issued_at_us).unwrap(),
            approved_org: env_id("STREAM_AUTH_TEST_APPROVED_ORG"),
            other_org: env_id("STREAM_AUTH_TEST_OTHER_ORG"),
            pending_org: env_id("STREAM_AUTH_TEST_PENDING_ORG"),
            issuer: TokenIssuer::new(SECRET, ISSUER, AUDIENCE),
        }
    }

    /// The principal of the seeded user `name` with a token for
    /// `organization`. The real issuer writes the token, so issuance and
    /// verification agree.
    fn principal(&self, name: &str, organization: Uuid) -> StreamPrincipal {
        let user = env_id(&format!("STREAM_AUTH_TEST_{name}_USER"));
        let token = self
            .issuer
            .stream(user, organization, ENVIRONMENT, self.issued_at);
        verify_stream(&token, &current_only(), ENVIRONMENT, self.issued_at)
            .expect("an issued stream token verifies")
    }

    async fn connect(&self) -> PgConnection {
        PgConnection::connect(&self.url).await.unwrap()
    }
}

fn env_value(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is required: run through tests/postgres/test_phase6_authorization.py")
    })
}

fn env_id(name: &str) -> Uuid {
    Uuid::parse_str(&env_value(name)).unwrap()
}

/// Current account and organization state read from PostgreSQL.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness: tests/postgres/test_phase6_authorization.py"]
async fn postgres_admission() {
    let fixture = Fixture::from_env();
    let checks = admission_checks(&fixture).await;
    println!("stream admission postgres coverage: {checks} checks");
}

/// Each seeded account and organization state, then a failed store read.
/// Returns the number of checks.
async fn admission_checks(fixture: &Fixture) -> usize {
    let approved_org = fixture.approved_org;
    let admitted = [
        ("approved", fixture.principal("APPROVED", approved_org)),
        (
            "password change before issuance",
            fixture.principal("CUTOFF_BEFORE", approved_org),
        ),
        (
            "rejected KYC",
            fixture.principal("KYC_REJECTED", approved_org),
        ),
        // The second organization admits its own members, so the refusals of
        // the moved user and the user without an organization below come
        // from the membership rule alone.
        (
            "member of the second organization",
            fixture.principal("OTHER_ORG", fixture.other_org),
        ),
    ];
    let refused = [
        ("missing user", fixture.principal("MISSING", approved_org)),
        ("rejected user", fixture.principal("REJECTED", approved_org)),
        ("pending user", fixture.principal("PENDING", approved_org)),
        (
            "password change required",
            fixture.principal("MUST_CHANGE", approved_org),
        ),
        (
            "moved to another organization",
            fixture.principal("MOVED", approved_org),
        ),
        (
            "no organization",
            fixture.principal("NO_ORGANIZATION", approved_org),
        ),
        (
            "password change at issuance",
            fixture.principal("CUTOFF_EQUAL", approved_org),
        ),
        (
            "revoked at issuance",
            fixture.principal("REVOKED", approved_org),
        ),
        (
            "organization not approved",
            fixture.principal("UNAPPROVED_ORG", fixture.pending_org),
        ),
    ];

    // Every case runs before the assertion, so a failure names all cases
    // that went wrong.
    let mut connection = fixture.connect().await;
    let mut unexpected = Vec::new();
    for (name, principal) in &admitted {
        let outcome = revalidate(&mut connection, principal).await;
        if outcome != Ok(()) {
            unexpected.push(format!("{name}: {outcome:?}"));
        }
    }
    for (name, principal) in &refused {
        let outcome = revalidate(&mut connection, principal).await;
        if outcome != Err(StreamError::TokenInvalid) {
            unexpected.push(format!("{name}: {outcome:?}"));
        }
    }
    assert!(
        unexpected.is_empty(),
        "unexpected outcomes: {unexpected:#?}"
    );

    // A read on a terminated session is the bounded unavailable class.
    let mut control = fixture.connect().await;
    let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(backend)
        .fetch_one(&mut control)
        .await
        .unwrap();
    assert!(terminated);
    control.close().await.unwrap();
    let approved = &admitted[0].1;
    let mut outcome = revalidate(&mut connection, approved).await;
    // Termination is asynchronous; allow the backend a bounded moment to exit.
    for _ in 0..40 {
        if outcome.is_err() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        outcome = revalidate(&mut connection, approved).await;
    }
    assert_eq!(outcome, Err(StreamError::AuthorizationUnavailable));

    admitted.len() + refused.len() + 1
}
