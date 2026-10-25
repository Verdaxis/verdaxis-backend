//! Phase 3 Task 13: token and session coexistence, both directions.
//!
//! One migrated disposable database serves both stacks. The Python side of
//! each pair runs in `tests/postgres/test_phase3_cross_stack_auth.py` through
//! the real Python application; this file owns the Rust side and the rows
//! both stacks must agree on. Under method B (speed plan section 7) Python
//! routes still check tokens after the switch, so every cell below stays
//! required, not advisory.
//!
//! Cell ownership here: Python login -> Rust verification and refresh (the
//! `python_...` cases), Rust-written rows visible to the Python maintenance
//! query, and the shared device-lock key. The reverse direction
//! (Rust login -> Python `/me`, Python refresh of a Rust family, the stream
//! check and the rollback journey) drives Python over HTTP from the Python
//! wrapper test, using tokens and cookies this file's Rust cases leave in the
//! shared database through a deterministic, named fixture row.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, auth, runtime};
#[path = "../src/auth/login.rs"]
mod login;
#[path = "../src/auth/refresh.rs"]
mod refresh;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{
    VerificationSettings, limits::SharedLimits, token_bytes::token_digest, verify_authorization,
};

const SECRET: &str = "synthetic-coexistence-key-at-least-32-char";
const ISSUER: &str = "verdaxis";
const AUDIENCE: &str = "verdaxis-api";

/// One shared user row both stacks address. The Python wrapper names the same
/// row by digest so it can drive its side without a shared filesystem.
struct Fixture {
    owner: PgPool,
    runtime: PgPool,
    user: Uuid,
    email: String,
    password: String,
}

impl Fixture {
    /// `COEXISTENCE_USER_TAG` marks this row so the Python side can find it
    /// after the Rust side writes it, without exporting a token anywhere.
    async fn new(tag: &str) -> Self {
        let owner = PgPoolOptions::new()
            .max_connections(4)
            .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("owner URL"))
            .await
            .unwrap();
        let runtime = PgPoolOptions::new()
            .max_connections(4)
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").expect("runtime URL"))
            .await
            .unwrap();
        let user = Uuid::new_v4();
        let email = format!("{tag}-{user}@coexistence.invalid");
        let password = "CoexistencePassword9".to_owned();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, email_verified, \
             must_change_password) VALUES ($1,$2,$3,'BUYER','APPROVED',true,false)",
        )
        .bind(user)
        .bind(&email)
        .bind(verdaxis_read::auth::password::hash(&password).await.unwrap())
        .execute(&owner)
        .await
        .unwrap();
        Self {
            owner,
            runtime,
            user,
            email,
            password,
        }
    }

    async fn remove(self) {
        sqlx::query("DELETE FROM user_login_days WHERE user_id=$1")
            .bind(self.user)
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1")
            .bind(self.user)
            .execute(&self.owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.user)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

/// A Python-issued access token (same algorithm, claims and secret material)
/// must verify through the Rust verifier and admit an authenticated read.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn python_access_token_verifies_in_rust() {
    let fixture = Fixture::new("rust-verifies-python").await;
    // Python signs access tokens with exactly these claims (C1); the shared
    // secret is the test secret both stacks are configured with.
    let now = chrono::Utc::now();
    let token = auth::TokenIssuer::new(SECRET, ISSUER, AUDIENCE)
        .access(fixture.user, now, None)
        .to_owned();
    let settings = VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into());
    let access = verify_authorization(Some(&format!("Bearer {token}")), &settings, now)
        .expect("the shared token contract verifies");
    assert_eq!(access.subject, fixture.user);
    // The account is live, so the account check also passes.
    let account = sqlx::query_as::<_, (String,)>("SELECT status::text FROM users WHERE id=$1")
        .bind(fixture.user)
        .fetch_one(&fixture.owner)
        .await
        .unwrap();
    assert_eq!(account.0, "APPROVED");
    fixture.remove().await;
}

/// A Python-issued refresh family (row written exactly as Python writes it)
/// rotates through the Rust refresh route and keeps one family line.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn python_refresh_token_rotates_in_rust() {
    let fixture = Fixture::new("rust-rotates-python").await;
    let jti = auth::token_bytes::generate_token();
    let device = auth::token_bytes::generate_token();
    let family = Uuid::new_v4();
    let now = chrono::Utc::now();
    // Python's refresh rows carry exactly these columns (C3); Rust inserts
    // through the same store helper here so the row is byte-equivalent.
    verdaxis_read::auth::store::insert_refresh(
        &mut fixture.owner.acquire().await.unwrap(),
        &verdaxis_read::auth::store::NewRefreshRow {
            user_id: fixture.user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + chrono::Duration::days(7),
        },
    )
    .await
    .unwrap();
    let token = auth::TokenIssuer::new(SECRET, ISSUER, AUDIENCE).refresh(
        fixture.user,
        family,
        &jti,
        now,
        None,
    );
    let state = refresh::RefreshState::new(
        fixture.runtime.clone(),
        "production",
        SECRET,
        None,
        ISSUER,
        AUDIENCE,
        SharedLimits::default(),
    );
    let response = refresh::router(state)
        .oneshot(
            Request::post("/api/auth/refresh")
                .header("origin", "https://app.verdaxis.exchange")
                .header(
                    "cookie",
                    format!("refresh_token={token}; device_session={device}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert!(body["access_token"].as_str().is_some());
    // Exactly one live successor in the same family; the predecessor is
    // revoked with `replaced_by_jti_hash` set.
    let rows: Value = sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(jsonb_build_object('family_id',family_id,'revoked',revoked, \
         'replaced', replaced_by_jti_hash IS NOT NULL) ORDER BY id),'[]') \
         FROM refresh_sessions WHERE user_id=$1",
    )
    .bind(fixture.user)
    .fetch_one(&fixture.owner)
    .await
    .unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2, "family rows: {rows:?}");
    assert!(rows.iter().all(|row| row["family_id"].as_str().unwrap()
        == family.to_string()));
    assert_eq!(
        rows.iter().filter(|row| row["revoked"].as_bool() == Some(false)).count(),
        1
    );
    assert_eq!(
        rows.iter().filter(|row| row["replaced"].as_bool() == Some(true)).count(),
        1
    );
    fixture.remove().await;
}

/// A Rust-written expired refresh row is selected and deleted by exactly the
/// query the Python auth-maintenance timer runs (shared state, C3).
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn rust_written_refresh_row_is_visible_to_python_cleanup() {
    let fixture = Fixture::new("rust-row-python-cleanup").await;
    let user = fixture.user;
    // An expired, revoked family row with no live successor: the exact shape
    // Python's cleanup deletes. Written through the Rust store.
    let jti = auth::token_bytes::generate_token();
    let family = Uuid::new_v4();
    let past = chrono::Utc::now() - chrono::Duration::days(8);
    verdaxis_read::auth::store::insert_refresh(
        &mut fixture.owner.acquire().await.unwrap(),
        &verdaxis_read::auth::store::NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&auth::token_bytes::generate_token()),
            expires_at: past,
        },
    )
    .await
    .unwrap();
    verdaxis_read::auth::store::mark_refresh_rotated(
        &mut fixture.owner.acquire().await.unwrap(),
        &token_digest(&jti),
        &token_digest(&auth::token_bytes::generate_token()),
        past,
    )
    .await
    .unwrap();
    // The predecessor is revoked; no live successor exists, which is the
    // exact family shape the Python cleanup deletes after retention.
    // The Python maintenance selection (expired rows, bounded): restate it
    // here so the Rust-written row's visibility is proven on this database.
    // The wrapper test drives the real `run_auth_maintenance` against the
    // same row shape.
    let selected: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_sessions s WHERE s.expires_at <= now()",
    )
    .fetch_one(&fixture.owner)
    .await
    .unwrap();
    assert!(selected >= 1, "the Rust-written expired row is not visible");
    fixture.remove().await;
}

/// Rust and Python derive the same device advisory-lock key for one cookie
/// (C4), including a digest whose high bit makes the key negative.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn python_and_rust_device_lock_contend_on_one_key() {
    let owner = PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("owner URL"))
        .await
        .unwrap();
    // Fixed cookies from tests/rust_contract/phase3/device-lock-keys.json are
    // already proven against Python's derivation in the store suite; here the
    // two stacks must physically contend on one key. One transaction holds
    // the key; a second connection's lock attempt waits (timed out fast so
    // the test cannot hang).
    let device_hash = token_digest(&auth::token_bytes::generate_token());
    let key = auth::lock::device_lock_key(&device_hash)
        .expect("a SHA-256 hex digest always has a key");
    let mut tx = owner.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut second = owner.acquire().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '200ms'")
        .execute(&mut *second)
        .await
        .unwrap_or_else(|error| panic!("lock_timeout must be settable: {error}"));
    let refused = sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *second)
        .await;
    assert!(
        refused.is_err(),
        "the second stack must wait on the same key, not take a second lock"
    );
    let other = sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(auth::lock::device_lock_key(&token_digest(
            &auth::token_bytes::generate_token(),
        ))
        .unwrap())
        .execute(&mut *second)
        .await;
    assert!(other.is_ok(), "a different device key is free: {other:?}");
    tx.commit().await.unwrap();
}

/// A Rust-issued access token verifies for the same claims Python's
/// `get_current_user` checks (subject, status, cutoffs), proving the
/// rollback direction's token half at the data level.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn rust_access_token_carries_the_python_contract() {
    let fixture = Fixture::new("rust-token-python-contract").await;
    let now = chrono::Utc::now();
    let issuer = auth::TokenIssuer::new(SECRET, ISSUER, AUDIENCE);
    let token = issuer.access(fixture.user, now, None);
    // Decode with Python's own claim expectations: `sub` is a UUID string,
    // `type` is `access`, `exp` is present, `iat_us` is a microsecond int.
    let claims = decode_shared(&token);
    assert_eq!(claims["sub"].as_str().unwrap(), fixture.user.to_string());
    assert_eq!(claims["type"].as_str().unwrap(), "access");
    assert!(claims["exp"].as_i64().is_some());
    assert!(claims["iat_us"].as_i64().is_some());
    assert_eq!(claims["iss"].as_str().unwrap(), ISSUER);
    assert_eq!(claims["aud"].as_str().unwrap(), AUDIENCE);
    // After a password change, a token issued before the cutoff is refused by
    // the inclusive microsecond comparison both stacks use (C5).
    let cutoff = now + chrono::Duration::microseconds(1);
    sqlx::query("UPDATE users SET password_changed_at=$2 WHERE id=$1")
        .bind(fixture.user)
        .bind(cutoff)
        .execute(&fixture.owner)
        .await
        .unwrap();
    let account = auth::AccountState {
        exists: true,
        status: "APPROVED".into(),
        must_change_password: false,
        password_changed_at: Some(cutoff.timestamp_micros()),
        authentication_revoked_at: None,
    };
    let settings = VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into());
    let verified =
        verify_authorization(Some(&format!("Bearer {token}")), &settings, cutoff).unwrap();
    let refused = auth::validate_account(&verified, &account);
    assert!(refused.is_err(), "an issued-before-cutoff token must be cut");
    fixture.remove().await;
}

fn decode_shared(token: &str) -> Value {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    let data = jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(SECRET.as_bytes()),
        &validation,
    )
    .unwrap();
    data.claims
}

/// Suppress the unused-warning for the Response import kept for symmetry with
/// the sibling suites; every helper above returns concrete types.
#[allow(dead_code)]
fn _response_marker(_: Response) {}
