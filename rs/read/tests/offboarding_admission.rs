//! The offboarding marker is an absolute deny for every Rust token consumer.
//!
//! These cases need no database. The shared account rules are checked with
//! real signed tokens, and a source inventory checks that every account
//! projection, credential query and token consumer reads the marker or hands
//! the token to a shared admission that does.
//! `offboarding_admission_postgres` checks the same rules on the routes.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::auth::{
    AccountState, AuthError, TokenIssuance, TokenIssuer, VerificationSettings, cutoff_refusal,
    validate_account, validate_account_for_path, verify_access,
};
use verdaxis_read::streams::auth::{StreamError, admit_account, verify_stream};

const SECRET: &str = "offboarding-admission-secret-at-least-32-characters";
const ISSUER: &str = "verdaxis-offboarding-test";
const AUDIENCE: &str = "verdaxis-offboarding-web";
const ENVIRONMENT: &str = "test";

fn settings() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into())
}

/// A fixed instant with a microsecond part keeps every comparison exact.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_792_497_600, 123_456_000).unwrap()
}

fn subject() -> Uuid {
    Uuid::parse_str("5b0f6d2e-63f3-4a8e-9a43-6f1c2a7d9e10").unwrap()
}

fn account(status: &str, cutoff: Option<DateTime<Utc>>, offboarded: bool) -> AccountState {
    AccountState {
        exists: true,
        status: status.into(),
        must_change_password: false,
        password_changed_at: cutoff.map(|at| at.timestamp_micros()),
        authentication_revoked_at: cutoff.map(|at| at.timestamp_micros()),
        offboarded,
    }
}

/// An access token with exactly these issuance claims. `None` omits a claim.
fn access_token(iat: Option<Value>, iat_us: Option<Value>) -> String {
    let mut claims = json!({
        "sub": subject().to_string(),
        "type": "access",
        "exp": (now() + Duration::minutes(15)).timestamp(),
        "iss": ISSUER,
        "aud": AUDIENCE,
    });
    if let Some(iat) = iat {
        claims["iat"] = iat;
    }
    if let Some(iat_us) = iat_us {
        claims["iat_us"] = iat_us;
    }
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

/// Issuance shapes around a cutoff one minute before `now()`: before it, equal
/// to it, after it (a token that every cutoff admits), missing, and outside
/// the representable range.
fn issuance_cases(cutoff: DateTime<Utc>) -> Vec<(&'static str, String)> {
    let micros = |at: DateTime<Utc>| json!(at.timestamp_micros());
    let seconds = |at: DateTime<Utc>| json!(at.timestamp());
    vec![
        (
            "before the cutoff",
            access_token(
                Some(seconds(cutoff - Duration::seconds(5))),
                Some(micros(cutoff - Duration::seconds(5))),
            ),
        ),
        (
            "equal to the cutoff",
            access_token(Some(seconds(cutoff)), Some(micros(cutoff))),
        ),
        (
            "after the cutoff",
            access_token(
                Some(seconds(cutoff + Duration::seconds(30))),
                Some(micros(cutoff + Duration::seconds(30))),
            ),
        ),
        ("no issuance time", access_token(None, None)),
        (
            "issuance outside the representable range",
            access_token(Some(seconds(now())), Some(json!(u64::MAX))),
        ),
    ]
}

#[test]
fn marker_refuses_every_issuance_time() {
    let cutoff = now() - Duration::minutes(1);
    for (case, token) in issuance_cases(cutoff) {
        let access = verify_access(&token, &settings(), now()).expect(case);
        let issuance = TokenIssuance {
            issued_at: access.issued_at,
            out_of_range: case.contains("outside"),
        };
        for stored_cutoff in [None, Some(cutoff)] {
            let marked = account("APPROVED", stored_cutoff, true);
            assert_eq!(
                cutoff_refusal(issuance, &marked),
                Some(AuthError::SessionRevoked),
                "{case}, cutoff {stored_cutoff:?}"
            );
            assert_eq!(
                validate_account(&access, &marked),
                Err(AuthError::SessionRevoked),
                "{case}, cutoff {stored_cutoff:?}"
            );
        }
    }
    // The same token after the cutoff is admitted when no marker is set, so
    // the refusals above come from the marker alone.
    let admitted = verify_access(&issuance_cases(cutoff)[2].1, &settings(), now()).unwrap();
    assert_eq!(
        validate_account(&admitted, &account("APPROVED", Some(cutoff), false)),
        Ok(())
    );
}

#[test]
fn marker_precedes_status_and_forced_change_rules() {
    let token = TokenIssuer::new(SECRET, ISSUER, AUDIENCE).access(subject(), now(), None);
    let access = verify_access(&token, &settings(), now()).unwrap();
    for (status, unmarked) in [
        ("APPROVED", Ok(())),
        ("PENDING", Err(AuthError::Pending)),
        ("REJECTED", Err(AuthError::Rejected)),
        ("UNKNOWN", Err(AuthError::Credentials)),
    ] {
        assert_eq!(
            validate_account(&access, &account(status, None, false)),
            unmarked,
            "{status}"
        );
        assert_eq!(
            validate_account(&access, &account(status, None, true)),
            Err(AuthError::SessionRevoked),
            "{status}"
        );
    }
    // The forced-password-change exception never admits a marked account.
    let mut forced = account("APPROVED", None, true);
    forced.must_change_password = true;
    for path in ["/api/auth/me", "/api/auth/me/password"] {
        assert_eq!(
            validate_account_for_path(&access, &forced, path),
            Err(AuthError::SessionRevoked),
            "{path}"
        );
    }
    // A missing account stays the plain credential refusal.
    let mut missing = account("APPROVED", None, true);
    missing.exists = false;
    assert_eq!(
        validate_account(&access, &missing),
        Err(AuthError::Credentials)
    );
}

/// The shape a cleanup admission uses for an ordinarily rejected account: the
/// status is not part of the check and the revocation cutoff is relaxed, so
/// only the password cutoff and the marker remain.
fn cleanup_projection(password_cutoff: Option<DateTime<Utc>>, offboarded: bool) -> AccountState {
    AccountState {
        exists: true,
        status: String::new(),
        must_change_password: false,
        password_changed_at: password_cutoff.map(|at| at.timestamp_micros()),
        authentication_revoked_at: None,
        offboarded,
    }
}

/// Every cleanup path calls the shared refusal; an ordinarily rejected account
/// keeps its settlement access and an offboarded one never has it.
#[test]
fn marked_rejected_cleanup_denied_but_unmarked_rejected_settles() {
    let issued = TokenIssuance {
        issued_at: Some(now()),
        out_of_range: false,
    };
    assert_eq!(
        cutoff_refusal(issued, &cleanup_projection(None, false)),
        None
    );
    assert_eq!(
        cutoff_refusal(issued, &cleanup_projection(None, true)),
        Some(AuthError::SessionRevoked)
    );
    // A token after the password cutoff still settles for the unmarked account.
    let earlier = Some(now() - Duration::minutes(5));
    assert_eq!(
        cutoff_refusal(issued, &cleanup_projection(earlier, false)),
        None
    );
    assert_eq!(
        cutoff_refusal(issued, &cleanup_projection(earlier, true)),
        Some(AuthError::SessionRevoked)
    );
}

/// The narrow exception for an account detached by its organization's
/// rejection projects no organization and no status; the marker still refuses.
#[test]
fn marked_detached_exception_denied() {
    for issuance in [
        TokenIssuance {
            issued_at: Some(now()),
            out_of_range: false,
        },
        TokenIssuance::default(),
        TokenIssuance {
            issued_at: None,
            out_of_range: true,
        },
    ] {
        assert_eq!(
            cutoff_refusal(issuance, &cleanup_projection(None, true)),
            Some(AuthError::SessionRevoked),
            "{issuance:?}"
        );
    }
}

#[test]
fn marked_stream_principal_refused() {
    let organization = Uuid::new_v4();
    let token = TokenIssuer::new(SECRET, ISSUER, AUDIENCE).stream(
        subject(),
        organization,
        ENVIRONMENT,
        Utc::now(),
    );
    let principal = verify_stream(&token, &settings(), ENVIRONMENT, Utc::now()).unwrap();
    assert_eq!(
        admit_account(&principal, &account("APPROVED", None, false)),
        Ok(())
    );
    assert_eq!(
        admit_account(&principal, &account("APPROVED", None, true)),
        Err(StreamError::TokenInvalid)
    );
}

// ---------------------------------------------------------------------------
// Source inventory
// ---------------------------------------------------------------------------

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/`, as (path relative to `src/`, contents).
fn sources() -> Vec<(String, String)> {
    fn walk(directory: &Path, root: &Path, files: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                let contents = std::fs::read_to_string(&path).unwrap();
                files.push((relative.replace('\\', "/"), contents));
            }
        }
    }
    let root = source_root();
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    assert!(files.len() > 100, "the source tree was not found");
    files
}

fn source(relative: &str) -> String {
    std::fs::read_to_string(source_root().join(relative)).unwrap()
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The number of `#` marks when a raw string (`r"`, `r#"`, `br#"`, ...) starts
/// at `index`.
fn raw_string_hashes(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes[index] != b'r' {
        return None;
    }
    let prefix_start = if index > 0 && bytes[index - 1] == b'b' {
        index - 1
    } else {
        index
    };
    if prefix_start > 0 && is_identifier_byte(bytes[prefix_start - 1]) {
        return None;
    }
    let hashes = bytes[index + 1..]
        .iter()
        .take_while(|&&byte| byte == b'#')
        .count();
    (bytes.get(index + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

/// The string literals of one Rust source file, in order. Comments, character
/// literals and lifetimes are skipped; raw strings keep their contents.
fn string_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &bytes[index..];
        if rest.starts_with(b"//") {
            index += rest
                .iter()
                .position(|&byte| byte == b'\n')
                .unwrap_or(rest.len());
        } else if rest.starts_with(b"/*") {
            index += source[index..].find("*/").map_or(rest.len(), |end| end + 2);
        } else if let Some(hashes) = raw_string_hashes(bytes, index) {
            let start = index + 2 + hashes;
            let terminator = format!("\"{}", "#".repeat(hashes));
            let end = start + source[start..].find(&terminator).unwrap();
            literals.push(source[start..end].to_owned());
            index = end + terminator.len();
        } else if bytes[index] == b'"' {
            let start = index + 1;
            let mut end = start;
            while bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            literals.push(source[start..end].to_owned());
            index = end + 1;
        } else if bytes[index] == b'\'' {
            // A character literal is skipped whole; a lifetime is one quote.
            // After a backslash the escaped character is at offset 2, so the
            // closing quote is searched from offset 3.
            index += match rest.get(1) {
                Some(b'\\') => 3 + rest[3..].iter().position(|&byte| byte == b'\'').unwrap() + 1,
                Some(_) if rest.get(2) == Some(&b'\'') => 3,
                _ => 1,
            };
        } else {
            index += 1;
        }
    }
    literals
}

/// The files whose SQL reads the account cutoffs to admit a token. Each such
/// statement must read the marker in the same literal; a new file here needs
/// review before it joins this list.
const ACCOUNT_PROJECTIONS: [&str; 8] = [
    "auth/store.rs",
    "auth/transaction.rs",
    "business/membership.rs",
    "business/transaction.rs",
    "kyc/review.rs",
    "order_write/admission.rs",
    "order_write/support.rs",
    "streams/auth.rs",
];

/// A statement that writes the cutoffs rather than reading them to admit a
/// token. A locking read (`SELECT ... FOR UPDATE`) is still a projection.
fn writes_cutoffs(literal: &str) -> bool {
    let statement = literal.trim_start();
    statement.starts_with("UPDATE") || statement.starts_with("SET ") || statement.contains(" SET ")
}

#[test]
fn every_account_projection_reads_the_marker() {
    let mut projecting = Vec::new();
    for (path, contents) in sources() {
        for literal in string_literals(&contents) {
            // A bare column name is a row accessor, not a statement.
            let reads_cutoff = literal.contains("authentication_revoked_at")
                && literal != "authentication_revoked_at"
                && !writes_cutoffs(&literal);
            if reads_cutoff {
                assert!(
                    literal.contains("offboarded_at"),
                    "{path}: an account projection omits the marker: {literal}"
                );
                projecting.push(path.clone());
            }
        }
    }
    projecting.sort();
    projecting.dedup();
    assert_eq!(projecting, ACCOUNT_PROJECTIONS);
}

#[test]
fn projection_scan_includes_locking_reads() {
    // The user lock of every authenticated write reads the cutoffs with
    // `FOR UPDATE`; the scan must check it, not skip it as a write.
    assert!(!writes_cutoffs(
        "SELECT status, authentication_revoked_at, offboarded_at FROM users WHERE id = $1 FOR UPDATE"
    ));
    assert!(writes_cutoffs(
        "UPDATE users SET authentication_revoked_at = $2 WHERE id = $1"
    ));
    assert!(writes_cutoffs(
        " SET authentication_revoked_at = GREATEST(authentication_revoked_at, $2)"
    ));
    let locking = string_literals(&source("auth/transaction.rs"))
        .into_iter()
        .filter(|literal| {
            literal.contains("authentication_revoked_at") && literal.contains("FOR UPDATE")
        })
        .collect::<Vec<_>>();
    assert!(!locking.is_empty(), "the locking account read is present");
    for literal in locking {
        assert!(literal.contains("offboarded_at"), "{literal}");
    }
}

#[test]
fn every_account_state_takes_the_marker_from_the_row() {
    // The required field forces every projection to set it; this keeps the
    // value from being a constant instead of the column.
    for (path, contents) in sources() {
        assert!(
            !contents.contains("offboarded: false"),
            "{path}: an account state sets the marker to a constant"
        );
    }
}

/// The calls that admit a token through a marker-aware rule.
const ADMISSIONS: [&str; 15] = [
    "run_authenticated(",
    "run_organization_write(",
    "run_ordered_write(",
    "run_controlled_write(",
    "validate_account(",
    "validate_account_for_path(",
    "cutoff_refusal(",
    "admit_account(",
    "revalidate(",
    "revalidate_from_pool(",
    "::admit(",
    "authenticate_shared(",
    "authenticate_preview(",
    "require_administrator(",
    "run_market_command(",
];

/// A token consumer that hands the verified token to another module, and the
/// modules that admit it there.
const DELEGATES: [(&str, &[&str]); 12] = [
    (
        "runtime/phase4.rs",
        &["business/organizations.rs", "business/joins.rs"],
    ),
    ("crud/notification_authority.rs", &["crud/notifications.rs"]),
    ("streams/http.rs", &["streams/replay.rs"]),
    // The market-write routes verify the token and pass it to the order-write
    // owner, which admits it under the user lock before any receipt replay.
    ("routes/market_write/amend.rs", &["order_write/amend.rs"]),
    (
        "routes/market_write/commission.rs",
        &["order_write/commission.rs"],
    ),
    ("routes/market_write/freeze.rs", &["order_write/freeze.rs"]),
    (
        "routes/market_write/inventory.rs",
        &["order_write/inventory.rs"],
    ),
    (
        "routes/market_write/order_create.rs",
        &["order_write/order_create.rs"],
    ),
    ("routes/market_write/orders.rs", &["order_write/orders.rs"]),
    (
        "routes/market_write/support.rs",
        &["order_write/support.rs"],
    ),
    (
        "routes/market_write/trade_lifecycle.rs",
        &["order_write/trade_lifecycle.rs"],
    ),
    ("routes/market_write/trades.rs", &["order_write/trades.rs"]),
];

/// Files that call a verifier without admitting a request: the verifier's own
/// module and a unit test of the composed settings.
const NOT_CONSUMERS: [&str; 2] = ["auth/token.rs", "runtime/phase2.rs"];

fn admits(contents: &str) -> bool {
    ADMISSIONS.iter().any(|call| contents.contains(call))
}

#[test]
fn every_token_consumer_admits_through_the_marker() {
    let verifier_call = |contents: &str| {
        [
            "verify_authorization(",
            "verify_authorization_with_token(",
            "verify_access(",
            "verify_stream(",
        ]
        .iter()
        .any(|call| {
            contents
                .match_indices(call)
                .any(|(at, _)| !contents[..at].ends_with("fn "))
        })
    };
    let mut consumers = 0;
    for (path, contents) in sources() {
        if !verifier_call(&contents) || NOT_CONSUMERS.contains(&path.as_str()) {
            continue;
        }
        consumers += 1;
        if admits(&contents) {
            continue;
        }
        let (_, targets) = DELEGATES
            .iter()
            .find(|(delegate, _)| *delegate == path)
            .unwrap_or_else(|| panic!("{path}: a token consumer without a marker-aware admission"));
        for target in *targets {
            assert!(admits(&source(target)), "{path} -> {target}");
        }
    }
    assert!(
        consumers >= 25,
        "only {consumers} token consumers were found"
    );
    // The test-only verifier call stays inside the unit-test module.
    let composed = source("runtime/phase2.rs");
    let tests = composed.find("#[cfg(test)]").unwrap();
    assert!(!composed[..tests].contains("verify_access("));
}

#[test]
fn credential_queries_exclude_the_marker() {
    let literal_with = |path: &str, needle: &str| {
        string_literals(&source(path))
            .into_iter()
            .filter(|literal| literal.contains(needle))
            .collect::<Vec<_>>()
    };
    // The invitation lookup projects the marker for the eligibility rule.
    let invitation = literal_with(
        "auth/invitation_accept.rs",
        "password_reset_token_hash = $1",
    );
    assert_eq!(invitation.len(), 1);
    assert!(invitation[0].contains("u.offboarded_at"));
    // Verification and its resend never select an offboarded account.
    let verification = literal_with("auth/email_verification.rs", "NOT email_verified");
    assert_eq!(verification.len(), 2);
    for statement in verification {
        assert!(statement.contains("offboarded_at IS NULL"), "{statement}");
    }
    // Neither review-queue read returns an offboarded account.
    let queue = literal_with("business/review_queue.rs", "FROM users u WHERE");
    assert_eq!(queue.len(), 2);
    for statement in queue {
        assert!(statement.contains("offboarded_at IS NULL"), "{statement}");
    }
    // Account approval reads the marker before it writes.
    let approval = literal_with("business/admission.rs", "organization_id,email_verified");
    assert_eq!(approval.len(), 1);
    assert!(approval[0].contains("offboarded_at"));
}

/// The MCP endpoint serves anonymous public market reads only. It reads no
/// account and verifies no token, so it has no account state to deny.
#[test]
fn mcp_has_no_authenticated_consumer() {
    let mut files = 0;
    for (path, contents) in sources() {
        if !path.starts_with("mcp/") {
            continue;
        }
        files += 1;
        for forbidden in [
            "verify_authorization",
            "verify_access",
            "verify_token",
            "run_authenticated",
            "FROM users",
            "authorization",
        ] {
            assert!(
                !contents.to_lowercase().contains(&forbidden.to_lowercase()),
                "{path} contains {forbidden}"
            );
        }
    }
    assert!(files >= 5);
}
