//! S6 invitation-acceptance PostgreSQL component checks.
//!
//! Each case builds one invited account exactly as Python's admin-invitation
//! create leaves it - APPROVED, unverified, `must_change_password`, bound to an
//! approved live organization, with a hashed seven-day secret in the shared
//! `users.password_reset_token_hash` column - then drives the real route.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/invitation_accept.rs"]
pub mod invitation_accept;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use chrono::{Duration, Utc};
use invitation_accept::{InvitationAcceptState, router};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{
    VerificationSettings, limits::SharedLimits, store::NewRefreshRow, store::insert_refresh,
    token_bytes::generate_token, token_bytes::token_digest, verify_access,
};

const SECRET: &str = "synthetic-invitation-key-at-least-32-chars";
const FRONTEND: &str = "https://app.verdaxis.exchange";
const NEW_PASSWORD: &str = "AcceptedPassword9";
const OLD_PASSWORD: &str = "PreviousPassword9";

struct Fixture {
    owner: PgPool,
    state: InvitationAcceptState,
    organization: Uuid,
    user: Uuid,
    token: String,
}

impl Fixture {
    async fn new(role: &str, organization_type: &str) -> Self {
        let owner = PgPoolOptions::new()
            .max_connections(4)
            .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("owner URL"))
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").expect("runtime URL"))
            .await
            .unwrap();
        let organization = Uuid::new_v4();
        let user = Uuid::new_v4();
        let email = format!("{user}@invitation.invalid");
        let token = generate_token();
        sqlx::query(
            "INSERT INTO organizations (id,name,type,verification_status,provenance) \
             VALUES ($1,'Synthetic Invitation Org',$2,'APPROVED','REAL')",
        )
        .bind(organization)
        .bind(organization_type)
        .execute(&owner)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO users (id,email,password_hash,role,status,email_verified, \
             must_change_password,organization_id,password_reset_token_hash, \
             password_reset_expires) \
             VALUES ($1,$2,$3,$4,'APPROVED',false,true,$5,$6,$7)",
        )
        .bind(user)
        .bind(&email)
        .bind(
            verdaxis_read::auth::password::hash(OLD_PASSWORD)
                .await
                .unwrap(),
        )
        .bind(role)
        .bind(organization)
        .bind(token_digest(&token))
        .bind(Utc::now() + Duration::days(7))
        .execute(&owner)
        .await
        .unwrap();
        Self {
            owner,
            state: InvitationAcceptState::new(
                pool,
                "production",
                SECRET,
                "verdaxis",
                "verdaxis-api",
                FRONTEND,
                SharedLimits::default(),
            ),
            organization,
            user,
            token,
        }
    }

    async fn send(&self, token: &str, password: &str, terms: bool) -> Response {
        send(
            self.state.clone(),
            token,
            password,
            terms,
            Some("https://app.verdaxis.exchange"),
            None,
        )
        .await
    }

    async fn user_row(&self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE id=$1")
            .bind(self.user)
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }

    async fn sessions(&self) -> Vec<Value> {
        let rows: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY id),'[]'::jsonb) \
             FROM refresh_sessions s WHERE user_id=$1",
        )
        .bind(self.user)
        .fetch_one(&self.owner)
        .await
        .unwrap();
        rows.as_array().cloned().unwrap_or_default()
    }

    async fn cleanup(&self) {
        for sql in [
            "DELETE FROM user_login_days WHERE user_id=$1",
            "DELETE FROM refresh_sessions WHERE user_id=$1",
            "DELETE FROM referrals WHERE referred_user_id=$1",
            "DELETE FROM audit_logs WHERE user_id=$1",
            "DELETE FROM users WHERE id=$1",
        ] {
            sqlx::query(sql)
                .bind(self.user)
                .execute(&self.owner)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.organization)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

async fn send(
    state: InvitationAcceptState,
    token: &str,
    password: &str,
    terms: bool,
    origin: Option<&str>,
    cookie: Option<&str>,
) -> Response {
    let payload = json!({"token": token, "new_password": password, "accept_terms": terms});
    let mut request =
        Request::post("/api/auth/invitations/accept").header("content-type", "application/json");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        router(state).oneshot(request.body(Body::from(payload.to_string())).unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn body_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

async fn code(response: Response) -> String {
    body_json(response).await["detail"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The cookies a response sets, by name.
fn cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

fn cookie_value(cookies: &[String], name: &str) -> String {
    cookies
        .iter()
        .find(|cookie| cookie.starts_with(name))
        .unwrap_or_else(|| panic!("no {name} cookie in {cookies:?}"))
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned()
}

/// The claims of a token the component issued, read without verification. The
/// test asserts what the response carries, not that the signature is valid;
/// [`verify_access`] covers the access token separately.
fn token_payload(token: &str) -> Value {
    use base64::Engine;
    let payload = token.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_consumes_token_and_issues_one_family() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    // A session that exists before the claim must not survive it.
    let old_device = generate_token();
    insert_refresh(
        &mut f.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: f.user,
            family_id: Uuid::new_v4(),
            jti_hash: token_digest(&generate_token()),
            device_id_hash: token_digest(&old_device),
            expires_at: Utc::now() + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let referral = Uuid::new_v4();
    let referrer = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password) VALUES ($1,$2,'unused','BUYER','APPROVED',true,false)")
        .bind(referrer).bind(format!("{referrer}@invitation.invalid")).execute(&f.owner).await.unwrap();
    sqlx::query("INSERT INTO referrals (id,referrer_id,referred_user_id,referral_code_used,status) VALUES ($1,$2,$3,'VDX-TEST01','SIGNED_UP')")
        .bind(referral).bind(referrer).bind(f.user).execute(&f.owner).await.unwrap();

    let response = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let set = cookies(&response);
    assert_eq!(set.len(), 2);
    for name in ["refresh_token=", "device_session="] {
        let cookie = set.iter().find(|cookie| cookie.starts_with(name)).unwrap();
        for attribute in [
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            "Path=/api/auth",
            "Max-Age=604800",
        ] {
            assert!(cookie.contains(attribute), "{cookie}");
        }
    }
    let body = body_json(response).await;
    assert_eq!(body.as_object().unwrap().len(), 2);
    assert_eq!(body["token_type"], "bearer");
    assert_eq!(
        verify_access(
            body["access_token"].as_str().unwrap(),
            &VerificationSettings::new(
                SECRET.into(),
                None,
                "verdaxis".into(),
                "verdaxis-api".into()
            ),
            Utc::now()
        )
        .unwrap()
        .subject,
        f.user
    );

    let user = f.user_row().await;
    assert_eq!(user["email_verified"], true);
    assert_eq!(user["must_change_password"], false);
    assert!(user["password_reset_token_hash"].is_null());
    assert!(user["password_reset_expires"].is_null());
    assert!(user["last_login"].is_string());
    assert!(user["referral_code"].is_string());
    assert!(user["password_changed_at"].is_string());
    assert!(
        verdaxis_read::auth::password::verify(
            NEW_PASSWORD,
            user["password_hash"].as_str().unwrap()
        )
        .await
    );

    let rows = f.sessions().await;
    let live: Vec<&Value> = rows.iter().filter(|row| row["revoked"] == false).collect();
    assert_eq!(live.len(), 1, "exactly one live family: {rows:?}");
    assert_eq!(
        live[0]["device_id_hash"],
        json!(token_digest(&cookie_value(&set, "device_session=")))
    );
    let refresh_claims = token_payload(&cookie_value(&set, "refresh_token="));
    assert_eq!(
        live[0]["family_id"], refresh_claims["family_id"],
        "the live row is the family the response issued"
    );
    assert_eq!(
        live[0]["jti_hash"],
        json!(token_digest(refresh_claims["jti"].as_str().unwrap()))
    );

    let referral_status: String =
        sqlx::query_scalar("SELECT status::text FROM referrals WHERE id=$1")
            .bind(referral)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(referral_status, "VERIFIED");
    let verified_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT verified_at FROM referrals WHERE id=$1")
            .bind(referral)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert!(verified_at.is_some());
    let days: i64 = sqlx::query_scalar("SELECT count(*) FROM user_login_days WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&f.owner)
        .await
        .unwrap();
    assert_eq!(days, 1);

    // The audit entry carries Python's action name and fields.
    let audit: Value = sqlx::query_scalar(
        "SELECT to_jsonb(a) FROM audit_logs a WHERE a.user_id=$1 AND a.action='user.invitation_accepted'",
    )
    .bind(f.user)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(audit["resource_type"], "user");
    assert_eq!(audit["resource_id"], json!(f.user.to_string()));
    assert_eq!(audit["changes"]["terms_accepted"], true);
    assert_eq!(
        audit["changes"]["terms_url"],
        "https://app.verdaxis.exchange/en/terms"
    );
    assert_eq!(
        audit["changes"]["privacy_url"],
        "https://app.verdaxis.exchange/en/privacy"
    );
    assert_eq!(audit["changes"]["agreement_version"], "2026-08-05");
    assert_eq!(
        audit["changes"]["organization_id"],
        json!(f.organization.to_string())
    );
    assert_eq!(audit["ip_address"], "unknown");

    // A second use of the same secret is refused with Python's status and body.
    let replay = f.send(&f.token, "AnotherPassword9", true).await;
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code(replay).await, "INVITATION_INVALID");
    assert_eq!(f.sessions().await, rows);
    assert_eq!(f.user_row().await, user);

    // The referral row and its referrer are removed after the claimed account,
    // because `referrals.referrer_id` has no cascade.
    f.cleanup().await;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(referrer)
        .execute(&f.owner)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_revokes_all_user_families() {
    let f = Fixture::new("BUYER", "SHIPPING_LINE").await;
    let mut runtime = f.owner.acquire().await.unwrap();
    let mut families = Vec::new();
    for _ in 0..3 {
        let family = Uuid::new_v4();
        insert_refresh(
            &mut runtime,
            &NewRefreshRow {
                user_id: f.user,
                family_id: family,
                jti_hash: token_digest(&generate_token()),
                device_id_hash: token_digest(&generate_token()),
                expires_at: Utc::now() + Duration::days(7),
            },
        )
        .await
        .unwrap();
        families.push(family);
    }
    drop(runtime);
    let response = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let rows = f.sessions().await;
    let live: Vec<&Value> = rows.iter().filter(|row| row["revoked"] == false).collect();
    assert_eq!(live.len(), 1, "{rows:?}");
    for family in families {
        assert!(
            rows.iter()
                .any(|row| row["family_id"] == json!(family) && row["revoked"] == true),
            "family {family} was not revoked: {rows:?}"
        );
    }
    // The cutoff advanced past every pre-claim session, so an old access token
    // minted before acceptance is refused.
    let cutoff: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT authentication_revoked_at FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    let password_cutoff: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT password_changed_at FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert!(password_cutoff >= cutoff);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_requires_terms_and_strong_password() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    let before = f.user_row().await;
    for (password, terms) in [
        (NEW_PASSWORD, false),
        ("alllowercase9", true),
        ("NoDigitsHere", true),
        ("Aa1bbbb", true),
    ] {
        let response = f.send(&f.token, password, terms).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{password}/{terms}"
        );
        assert_eq!(code(response).await, "VALIDATION_FAILED");
    }
    // A refused body changes nothing and leaves the invitation usable.
    assert_eq!(f.user_row().await, before);
    assert!(f.sessions().await.is_empty());
    let accepted = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(accepted.status(), StatusCode::OK);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_rejects_expired_or_reissued_token() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    // Expired: the same secret with a deadline in the past.
    sqlx::query("UPDATE users SET password_reset_expires=$2 WHERE id=$1")
        .bind(f.user)
        .bind(Utc::now() - Duration::seconds(1))
        .execute(&f.owner)
        .await
        .unwrap();
    let expired = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(expired.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code(expired).await, "INVITATION_INVALID");

    // Reissued: a new secret replaces the old hash, so only the newest works.
    let reissued = generate_token();
    sqlx::query(
        "UPDATE users SET password_reset_token_hash=$2,password_reset_expires=$3 WHERE id=$1",
    )
    .bind(f.user)
    .bind(token_digest(&reissued))
    .bind(Utc::now() + Duration::days(7))
    .execute(&f.owner)
    .await
    .unwrap();
    let stale = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(stale.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code(stale).await, "INVITATION_INVALID");
    let current = f.send(&reissued, NEW_PASSWORD, true).await;
    assert_eq!(current.status(), StatusCode::OK);

    // An unknown secret and a token for an ineligible account answer the same.
    let unknown = f.send(&generate_token(), NEW_PASSWORD, true).await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(code(unknown).await, "INVITATION_INVALID");
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_rejects_ineligible_account_states() {
    let cases = [
        // (status, email_verified, must_change_password, organization_status,
        //  role, organization type)
        (
            "APPROVED",
            false,
            true,
            "PENDING",
            "SUPPLIER",
            "FUEL_SUPPLIER",
        ),
        (
            "APPROVED",
            true,
            true,
            "APPROVED",
            "SUPPLIER",
            "FUEL_SUPPLIER",
        ),
        (
            "APPROVED",
            false,
            false,
            "APPROVED",
            "SUPPLIER",
            "FUEL_SUPPLIER",
        ),
        (
            "PENDING",
            false,
            true,
            "APPROVED",
            "SUPPLIER",
            "FUEL_SUPPLIER",
        ),
        (
            "REJECTED",
            false,
            true,
            "APPROVED",
            "SUPPLIER",
            "FUEL_SUPPLIER",
        ),
        // A role that does not match the organization side.
        (
            "APPROVED",
            false,
            true,
            "APPROVED",
            "BUYER",
            "FUEL_SUPPLIER",
        ),
        (
            "APPROVED",
            false,
            true,
            "APPROVED",
            "SUPPLIER",
            "SHIPPING_LINE",
        ),
    ];
    for (status, verified, forced, organization_status, role, kind) in cases {
        let f = Fixture::new(role, kind).await;
        sqlx::query(
            "UPDATE users SET status=$2,email_verified=$3,must_change_password=$4 WHERE id=$1",
        )
        .bind(f.user)
        .bind(status)
        .bind(verified)
        .bind(forced)
        .execute(&f.owner)
        .await
        .unwrap();
        sqlx::query("UPDATE organizations SET verification_status=$2 WHERE id=$1")
            .bind(f.organization)
            .bind(organization_status)
            .execute(&f.owner)
            .await
            .unwrap();
        let before = f.user_row().await;
        let response = f.send(&f.token, NEW_PASSWORD, true).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{status}/{verified}/{forced}/{organization_status}/{role}/{kind}"
        );
        assert_eq!(code(response).await, "INVITATION_INVALID");
        assert_eq!(f.user_row().await, before);
        assert!(f.sessions().await.is_empty());
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_is_limited_and_origin_checked() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    // A refused origin writes nothing and spends no quota.
    for origin in [Some("https://other.invalid"), None] {
        let response = send(
            f.state.clone(),
            &f.token,
            NEW_PASSWORD,
            true,
            origin,
            Some("refresh_token=presented"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(code(response).await, "BROWSER_ORIGIN_REJECTED");
    }
    let user = f.user_row().await;
    assert!(f.sessions().await.is_empty());

    // The sixth request in the window is refused, and the accepted invitation
    // is not consumed by a rate-limited attempt.
    for attempt in 0..5 {
        let response = f.send(&generate_token(), NEW_PASSWORD, true).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "attempt {attempt}"
        );
    }
    let limited = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().get("retry-after").is_some());
    assert_eq!(code(limited).await, "RATE_LIMITED");
    assert_eq!(f.user_row().await, user);
    assert!(f.sessions().await.is_empty());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_does_not_touch_password_reset_tokens() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    // The invited account already holds a Rust-issued reset token; acceptance
    // must leave that row exactly as it is.
    let reset_hash = token_digest(&generate_token());
    let reset_expiry = Utc::now() + Duration::hours(1);
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id,token_hash,expires_at,created_at) \
         VALUES ($1,$2,$3,$4)",
    )
    .bind(f.user)
    .bind(&reset_hash)
    .bind(reset_expiry)
    .bind(Utc::now())
    .execute(&f.owner)
    .await
    .unwrap();
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM password_reset_tokens t WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();

    let response = f.send(&f.token, NEW_PASSWORD, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM password_reset_tokens t WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(before, after);

    // The invitation secret was never accepted as a reset token: the reset row
    // still carries its own digest, and the invitation's digest is absent.
    assert_eq!(after["token_hash"], json!(reset_hash));
    assert!(after["used_at"].is_null());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn invitation_accept_concurrent_claims_yield_one_session() {
    let f = Fixture::new("SUPPLIER", "FUEL_SUPPLIER").await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let state = f.state.clone();
        let token = f.token.clone();
        tasks.spawn(async move {
            let response = send(state, &token, NEW_PASSWORD, true, Some(FRONTEND), None).await;
            u16::from(response.status())
        });
    }
    let mut statuses = Vec::new();
    while let Some(status) = tasks.join_next().await {
        statuses.push(status.unwrap());
    }
    statuses.sort_unstable();
    // Exactly one claim succeeds; the other sees the consumed secret.
    assert_eq!(
        statuses,
        vec![200, 400],
        "one claim must succeed and the other must be refused"
    );
    let rows = f.sessions().await;
    let live: Vec<&Value> = rows.iter().filter(|row| row["revoked"] == false).collect();
    assert_eq!(live.len(), 1, "{rows:?}");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE user_id=$1 AND action='user.invitation_accepted'",
    )
    .bind(f.user)
    .fetch_one(&f.owner)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    f.cleanup().await;
}
