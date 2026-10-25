//! Phase 3 Task 14 (slimmed under method B): the session race suite.
//!
//! Kept races (speed plan section 7): every single-stack schedule the shared
//! database still allows, plus the schedules against the Python
//! auth-maintenance timer, which keeps writing this database after the Phase 3
//! switch. Dropped races: any schedule whose other writer is a Python
//! login/refresh/logout/password route, because those routes stop serving in
//! the one switch and never write these rows again.
//!
//! Each schedule uses independent pool connections and the components' own
//! barrier hooks or `FOR UPDATE` serialization; no sleep orders anything.
//! `tests/postgres/test_phase3_races.py` drives the Python-maintenance cell
//! and requires every case in this file to pass on the same database.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, auth, runtime};
#[path = "../src/auth/login.rs"]
mod login;
#[path = "../src/auth/logout.rs"]
mod logout;
#[path = "../src/auth/refresh.rs"]
mod refresh;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{
    limits::SharedLimits,
    store::{NewRefreshRow, insert_refresh},
    token_bytes::{generate_token, token_digest},
};

const SECRET: &str = "synthetic-races-key-at-least-32-characters";
const FRONTEND: &str = "https://app.verdaxis.exchange";

struct RaceFixture {
    owner: PgPool,
    runtime: PgPool,
    users: Vec<Uuid>,
}

impl RaceFixture {
    /// Two approved users on one migrated database; `runtime` serves every
    /// request and `owner` seeds and observes rows. Independent connections
    /// come from separate pool acquisition calls, never one shared session.
    async fn new(users: usize, tag: &str) -> Self {
        let owner = PgPoolOptions::new()
            .max_connections(8)
            .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("owner URL"))
            .await
            .unwrap();
        let runtime = PgPoolOptions::new()
            .max_connections(8)
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").expect("runtime URL"))
            .await
            .unwrap();
        let mut ids = Vec::new();
        for index in 0..users {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO users (id,email,password_hash,role,status,email_verified,\
                 must_change_password) VALUES ($1,$2,$3,'BUYER','APPROVED',true,false)",
            )
            .bind(id)
            .bind(format!("{tag}-{index}-{id}@races.invalid"))
            .bind(
                verdaxis_read::auth::password::hash("RacePassword123")
                    .await
                    .unwrap(),
            )
            .execute(&owner)
            .await
            .unwrap();
            ids.push(id);
        }
        Self {
            owner,
            runtime,
            users: ids,
        }
    }

    async fn remove(self) {
        for id in &self.users {
            sqlx::query("DELETE FROM user_login_days WHERE user_id=$1")
                .bind(id)
                .execute(&self.owner)
                .await
                .unwrap();
            sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1")
                .bind(id)
                .execute(&self.owner)
                .await
                .unwrap();
            sqlx::query("DELETE FROM users WHERE id=$1")
                .bind(id)
                .execute(&self.owner)
                .await
                .unwrap();
        }
    }

    async fn live_families(&self, user: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(DISTINCT family_id) FROM refresh_sessions \
             WHERE user_id=$1 AND revoked=false AND expires_at > now()",
        )
        .bind(user)
        .fetch_one(&self.owner)
        .await
        .unwrap()
    }

    async fn live_rows(&self, user: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM refresh_sessions \
             WHERE user_id=$1 AND revoked=false AND expires_at > now()",
        )
        .bind(user)
        .fetch_one(&self.owner)
        .await
        .unwrap()
    }
}

fn refresh_state(pool: &PgPool, hooks: refresh::RefreshHooks) -> refresh::RefreshState {
    let mut state = refresh::RefreshState::new(
        pool.clone(),
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    );
    state.hooks = hooks;
    state
}

async fn login_state(pool: &PgPool) -> login::LoginState {
    login::LoginState::new(
        pool.clone(),
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    )
    .await
}

async fn send_refresh(
    state: refresh::RefreshState,
    token: String,
    device: String,
) -> (u16, Option<Value>) {
    let refresh_token = token.as_str();
    let device = device.as_str();
    let response = refresh::router(state)
        .oneshot(
            Request::post("/api/auth/refresh")
                .header("origin", FRONTEND)
                .header(
                    "cookie",
                    format!("refresh_token={refresh_token}; device_session={device}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 8192)
        .await
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok());
    (status, body)
}

async fn send_login(state: login::LoginState, email: String, password: &str, device: String) -> u16 {
    let form = format!(
        "username={}&password={password}",
        email.replace('@', "%40")
    );
    let response = login::router(state)
        .oneshot(
            Request::post("/api/auth/login")
                .header("origin", FRONTEND)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("device_session={device}"))
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    response.status().as_u16()
}

/// R1 (rust): two concurrent refreshes of one token yield one successor; the
/// loser gets 409 inside the five-second grace, never a second live row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL"]
async fn r1_concurrent_refresh_same_token_yields_one_successor() {
    let f = RaceFixture::new(1, "r1").await;
    let user = f.users[0];
    let family = Uuid::new_v4();
    let jti = generate_token();
    let device = generate_token();
    let now = Utc::now();
    insert_refresh(
        &mut f.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let token = auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api")
        .refresh(user, family, &jti, now, None)
        .to_owned();
    let state = refresh_state(&f.runtime, refresh::RefreshHooks::default());
    let a = tokio::spawn(send_refresh(state.clone(), token.clone(), device.clone()));
    let b = tokio::spawn(send_refresh(state, token, device));
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    let mut statuses = [a.0, b.0];
    statuses.sort_unstable();
    assert!(
        statuses == [200, 200] || statuses == [200, 409],
        "unexpected pair: {statuses:?}"
    );
    assert_eq!(f.live_rows(user).await, 1, "exactly one live row");
    assert_eq!(f.live_families(user).await, 1, "one family, never two");
    f.remove().await;
}

/// R2 (rust): a replay after the grace revokes the whole family and the
/// successor is no longer usable.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r2_refresh_replay_after_grace_revokes_family() {
    let f = RaceFixture::new(1, "r2").await;
    let user = f.users[0];
    let family = Uuid::new_v4();
    let jti = generate_token();
    let device = generate_token();
    let now = Utc::now();
    insert_refresh(
        &mut f.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let issuer = auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api");
    let token = issuer.refresh(user, family, &jti, now, None);
    let state = refresh_state(&f.runtime, refresh::RefreshHooks::default());
    let (status, _) = send_refresh(state.clone(), token.clone(), device.clone()).await;
    assert_eq!(status, 200);
    // Advance the predecessor's grace past its deadline; no clock is changed,
    // the committed deadline is moved back deterministically.
    sqlx::query("UPDATE refresh_sessions SET rotation_grace_until = now() - interval '1 second' WHERE jti_hash=$1")
        .bind(token_digest(&jti))
        .execute(&f.owner)
        .await
        .unwrap();
    let (replay, body) = send_refresh(state, token, device).await;
    assert_eq!(replay, 401, "{body:?}");
    assert_eq!(
        body.as_ref().and_then(|b| b["detail"]["code"].as_str()),
        Some("REFRESH_TOKEN_REPLAYED"),
        "{body:?}"
    );
    assert_eq!(f.live_rows(user).await, 0, "the family is fully revoked");
    f.remove().await;
}

/// R3 (rust): refresh completing its successor, then logout, leaves no live
/// row and refuses the successor.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r3_refresh_vs_logout_revokes_successor() {
    let f = RaceFixture::new(1, "r3").await;
    let user = f.users[0];
    let family = Uuid::new_v4();
    let jti = generate_token();
    let device = generate_token();
    let now = Utc::now();
    insert_refresh(
        &mut f.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let issuer = auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api");
    let token = issuer.refresh(user, family, &jti, now, None);
    let state = refresh_state(&f.runtime, refresh::RefreshHooks::default());
    let (status, _) = send_refresh(state, token.clone(), device.clone()).await;
    assert_eq!(status, 200);
    // Logout with the same device cookie: the whole device family is revoked.
    let logout = logout::LogoutState::new(
        f.runtime.clone(),
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
    );
    let response = logout::router(logout)
        .oneshot(
            Request::post("/api/auth/logout")
                .header("origin", FRONTEND)
                .header("cookie", format!("device_session={device}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(f.live_rows(user).await, 0);
    f.remove().await;
}

/// R4 (rust): a refresh issued before a password change keeps its 200, but
/// its token is cut off by the inclusive microsecond comparison afterwards.
#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r4_refresh_vs_password_change_never_issues_after_cutoff() {
    let f = RaceFixture::new(1, "r4").await;
    let user = f.users[0];
    let family = Uuid::new_v4();
    let jti = generate_token();
    let device = generate_token();
    let now = Utc::now();
    insert_refresh(
        &mut f.owner.acquire().await.unwrap(),
        &NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let issuer = auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api");
    let token = issuer.refresh(user, family, &jti, now, None);
    let state = refresh_state(&f.runtime, refresh::RefreshHooks::default());
    let (status, body) = send_refresh(state.clone(), token.clone(), device.clone()).await;
    assert_eq!(status, 200, "{body:?}");
    let successor = body.unwrap()["access_token"].as_str().unwrap().to_owned();
    // The password change's own cutoff write, applied directly so the
    // schedule is deterministic: every earlier-issued token is cut.
    let changed_at = Utc::now();
    sqlx::query("UPDATE users SET password_changed_at=$2 WHERE id=$1")
        .bind(user)
        .bind(changed_at)
        .execute(&f.owner)
        .await
        .unwrap();
    sqlx::query("UPDATE refresh_sessions SET revoked=true WHERE user_id=$1")
        .bind(user)
        .execute(&f.owner)
        .await
        .unwrap();
    // The successor no longer verifies: issued at or before the cutoff.
    let settings = auth::VerificationSettings::new(
        SECRET.into(),
        None,
        "verdaxis".into(),
        "verdaxis-api".into(),
    );
    let verified = auth::verify_authorization(Some(&format!("Bearer {successor}")), &settings, Utc::now());
    assert!(verified.is_ok(), "signature and expiry still hold");
    let account = auth::AccountState {
        exists: true,
        status: "APPROVED".into(),
        must_change_password: false,
        password_changed_at: Some(changed_at.timestamp_micros()),
        authentication_revoked_at: None,
    };
    assert!(
        auth::validate_account(&verified.unwrap(), &account).is_err(),
        "issued-at-or-before-cutoff must be refused (C5)"
    );
    f.remove().await;
}

/// R6 (rust): a login racing an account rejection never writes a session row
/// after the rejection commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL"]
async fn r6_login_vs_account_status_change_writes_no_session() {
    let f = RaceFixture::new(1, "r6").await;
    let user = f.users[0];
    let email = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&f.owner)
        .await
        .unwrap();
    // The rejection commits first (status plus the monotonic revocation
    // cutoff); the login then reads the rejected row under its own lock. The
    // login's user-lock barrier rendezvous proves it reached the row read, so
    // a login that succeeded cannot have done so after this point.
    sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
        .bind(user)
        .execute(&f.owner)
        .await
        .unwrap();
    let cutoff = verdaxis_read::auth::store::revoke_user_authentication(
        &mut f.owner.acquire().await.unwrap(),
        user,
        Utc::now(),
    )
    .await
    .unwrap();
    let mut state = login_state(&f.runtime).await;
    state.hooks = login::LoginHooks {
        user_locked: Some(std::sync::Arc::new(tokio::sync::Barrier::new(1))),
        ..Default::default()
    };
    let status = send_login(state, email, "RacePassword123", generate_token()).await;
    assert_eq!(status, 403, "a rejected account must be refused");
    assert_eq!(
        f.live_rows(user).await, 0,
        "no refresh row may be written for the rejected account"
    );
    let stored: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT authentication_revoked_at FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert!(stored.unwrap() >= cutoff, "the cutoff never moved backwards");
    f.remove().await;
}

/// R7 (rust): two logins on one device keep exactly one live family; the
/// earlier account's family is revoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL"]
async fn r7_device_session_limit_is_one_live_family() {
    let f = RaceFixture::new(2, "r7").await;
    let device = generate_token();
    let emails: Vec<String> = {
        let mut rows = sqlx::query_scalar::<_, String>(
            "SELECT email FROM users WHERE id = ANY($1) ORDER BY email",
        )
        .bind(&f.users)
        .fetch_all(&f.owner)
        .await
        .unwrap();
        rows.sort();
        rows
    };
    let a = tokio::spawn(send_login(
        login_state(&f.runtime).await,
        emails[0].clone(),
        "RacePassword123",
        device.clone(),
    ));
    let b = tokio::spawn(send_login(
        login_state(&f.runtime).await,
        emails[1].clone(),
        "RacePassword123",
        device,
    ));
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    assert_eq!(a, 200, "first login");
    assert_eq!(b, 200, "second login");
    let total_live: i64 = f.live_rows(f.users[0]).await + f.live_rows(f.users[1]).await;
    assert_eq!(total_live, 1, "one live family for the device");
    f.remove().await;
}

/// R8 (rust): two logins on two devices keep two live families.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL"]
async fn r8_login_vs_login_different_devices_both_live() {
    let f = RaceFixture::new(1, "r8").await;
    let email = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id=$1")
        .bind(f.users[0])
        .fetch_one(&f.owner)
        .await
        .unwrap();
    let a = tokio::spawn(send_login(
        login_state(&f.runtime).await,
        email.clone(),
        "RacePassword123",
        generate_token(),
    ));
    let b = tokio::spawn(send_login(
        login_state(&f.runtime).await,
        email,
        "RacePassword123",
        generate_token(),
    ));
    assert_eq!(a.await.unwrap(), 200);
    assert_eq!(b.await.unwrap(), 200);
    assert_eq!(f.live_families(f.users[0]).await, 2);
    f.remove().await;
}

/// R12 (rust): a cutoff never moves backwards under concurrent writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL"]
async fn r12_cutoff_downgrade_fails_fast() {
    let f = RaceFixture::new(1, "r12").await;
    let user = f.users[0];
    // A higher cutoff is committed first; every later concurrent call must
    // keep it or advance it, never lower it.
    let later = Utc::now() + Duration::seconds(30);
    verdaxis_read::auth::store::revoke_user_authentication(
        &mut f.owner.acquire().await.unwrap(),
        user,
        later,
    )
    .await
    .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let pool = f.owner.clone();
        tasks.spawn(async move {
            let mut connection = pool.acquire().await.unwrap();
            verdaxis_read::auth::store::revoke_user_authentication(
                &mut connection,
                user,
                Utc::now(),
            )
            .await
            .unwrap()
        });
    }
    let mut minimum = later;
    while let Some(cutoff) = tasks.join_next().await {
        let cutoff = cutoff.unwrap();
        assert!(
            cutoff >= later,
            "a concurrent call reported a downgraded cutoff: {cutoff} < {later}"
        );
        minimum = minimum.min(cutoff);
    }
    let stored: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT authentication_revoked_at FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert!(
        stored >= minimum,
        "the stored cutoff moved backwards: {stored} < {minimum}"
    );
    f.remove().await;
}
