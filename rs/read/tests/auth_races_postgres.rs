//! Independent-connection session schedules retained by method B.
#![cfg(feature = "contract-tests")]
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use tokio::sync::Barrier;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::{
    self,
    limits::SharedLimits,
    token_bytes::{generate_token, token_digest},
};

const SECRET: &str = "synthetic-race-key-at-least-32-characters";
const ORIGIN: &str = "https://app.verdaxis.exchange";

struct Fixture {
    owner: PgPool,
    runtime: PgPool,
    user: Uuid,
    email: String,
    device: String,
    token: String,
}
impl Fixture {
    async fn new() -> Self {
        let tag = format!("session-race-{}", Uuid::new_v4());
        let observer_tag = tag.clone();
        let runtime_tag = tag.clone();
        let owner = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |connection, _| {
                let tag = observer_tag.clone();
                Box::pin(async move { sqlx::query("SELECT set_config('application_name',$1,false)").bind(tag).execute(connection).await?; Ok(()) })
            })
            .connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
            .await
            .unwrap();
        let runtime = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |connection, _| {
                let tag = runtime_tag.clone();
                Box::pin(async move { sqlx::query("SELECT set_config('application_name',$1,false)").bind(tag).execute(connection).await?; Ok(()) })
            })
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let user = Uuid::new_v4();
        let email = format!("race-{user}@example.com");
        sqlx::query("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password) VALUES ($1,$2,$3,'BUYER','APPROVED',true,false)")
            .bind(user).bind(&email).bind(auth::password::hash("RacePassword123").await.unwrap()).execute(&owner).await.unwrap();
        let device = generate_token();
        let jti = generate_token();
        let family = Uuid::new_v4();
        let now = Utc::now();
        auth::store::insert_refresh(
            &mut owner.acquire().await.unwrap(),
            &auth::store::NewRefreshRow {
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
            .refresh(user, family, &jti, now, None);
        Self {
            owner,
            runtime,
            user,
            email,
            device,
            token,
        }
    }
    fn refresh(&self, held: Option<Arc<Barrier>>) -> auth::refresh::RefreshState {
        let mut state = auth::refresh::RefreshState::new(
            self.runtime.clone(),
            "production",
            SECRET,
            None,
            "verdaxis",
            "verdaxis-api",
            SharedLimits::default(),
        );
        state.hooks.user_locked = held;
        state
    }
    async fn live(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked AND expires_at>now()")
            .bind(self.user).fetch_one(&self.owner).await.unwrap()
    }
    async fn remove(self) {
        for statement in [
            // Migrator-only cleanup of this disposable fixture's audit rows.
            "DELETE FROM audit_logs WHERE user_id=$1",
            "DELETE FROM user_login_days WHERE user_id=$1",
            "DELETE FROM refresh_sessions WHERE user_id=$1",
        ] {
            sqlx::query(statement)
                .bind(self.user)
                .execute(&self.owner)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.user)
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

async fn send(
    router: axum::Router,
    path: &str,
    token: Option<&str>,
    device: &str,
    body: Value,
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(if path.ends_with("password") {
            "PUT"
        } else {
            "POST"
        })
        .uri(path)
        .header("origin", ORIGIN)
        .header("content-type", "application/json")
        .header("cookie", format!("device_session={device}"));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 16384).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn refresh(
    state: auth::refresh::RefreshState,
    token: String,
    device: String,
) -> (u16, Value) {
    let response = auth::refresh::router(state)
        .oneshot(
            Request::post("/api/auth/refresh")
                .header("origin", ORIGIN)
                .header(
                    "cookie",
                    format!("refresh_token={token}; device_session={device}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap(),
    )
}
async fn blocked(pool: &PgPool) {
    blocked_count(pool, 1).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r1_concurrent_refresh_same_token_yields_one_successor() {
    let f = Fixture::new().await;
    let held = Arc::new(Barrier::new(2));
    let first = tokio::spawn(refresh(
        f.refresh(Some(held.clone())),
        f.token.clone(),
        f.device.clone(),
    ));
    held.wait().await;
    let second = tokio::spawn(refresh(f.refresh(None), f.token.clone(), f.device.clone()));
    blocked(&f.owner).await;
    held.wait().await;
    assert_eq!(first.await.unwrap().0, 200);
    let loser = second.await.unwrap();
    assert_eq!(loser.0, 409);
    assert_eq!(loser.1["detail"]["code"], "REFRESH_ROTATION_IN_PROGRESS");
    assert_eq!(f.live().await, 1);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r2_refresh_replay_after_grace_revokes_family() {
    let f = Fixture::new().await;
    assert_eq!(
        refresh(f.refresh(None), f.token.clone(), f.device.clone())
            .await
            .0,
        200
    );
    sqlx::query("UPDATE refresh_sessions SET rotation_grace_until=now()-interval '1 second' WHERE user_id=$1 AND replaced_by_jti_hash IS NOT NULL")
        .bind(f.user).execute(&f.owner).await.unwrap();
    let replay = refresh(f.refresh(None), f.token.clone(), f.device.clone()).await;
    assert_eq!(replay.0, 401);
    assert_eq!(replay.1["detail"]["code"], "REFRESH_TOKEN_REPLAYED");
    assert_eq!(f.live().await, 0);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r3_refresh_vs_logout_revokes_successor() {
    for refresh_first in [true, false] {
        let f = Fixture::new().await;
        let logout = auth::logout::router(auth::logout::LogoutState::new(
            f.runtime.clone(),
            "production",
            SECRET,
            None,
            "verdaxis",
            "verdaxis-api",
        ));
        if refresh_first {
            let held = Arc::new(Barrier::new(2));
            let rotation = tokio::spawn(refresh(
                f.refresh(Some(held.clone())),
                f.token.clone(),
                f.device.clone(),
            ));
            held.wait().await;
            let device = f.device.clone();
            let logout = tokio::spawn(async move {
                send(logout, "/api/auth/logout", None, &device, Value::Null).await
            });
            blocked(&f.owner).await;
            held.wait().await;
            assert_eq!(rotation.await.unwrap().0, 200);
            assert_eq!(logout.await.unwrap().0, 204);
        } else {
            // Hold the same device lock for the logout transaction, then let
            // refresh reach PostgreSQL before the logout commits.
            let held = Arc::new(Barrier::new(2));
            let mut state = auth::logout::LogoutState::new(f.runtime.clone(),"production",SECRET,None,"verdaxis","verdaxis-api");
            state.device_locked = Some(held.clone());
            let device = f.device.clone();
            let logging_out = tokio::spawn(async move { send(auth::logout::router(state),"/api/auth/logout",None,&device,Value::Null).await });
            held.wait().await;
            let rotation = tokio::spawn(refresh(f.refresh(None), f.token.clone(), f.device.clone()));
            blocked(&f.owner).await;
            held.wait().await;
            assert_eq!(logging_out.await.unwrap().0,204);
            assert_eq!(rotation.await.unwrap().0,401);
        }
        assert_eq!(f.live().await, 0);
        f.remove().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r4_refresh_vs_password_change_never_issues_after_cutoff() {
    let f = Fixture::new().await;
    let held = Arc::new(Barrier::new(2));
    let rotation = tokio::spawn(refresh(
        f.refresh(Some(held.clone())),
        f.token.clone(),
        f.device.clone(),
    ));
    held.wait().await;
    let access =
        auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api").access(f.user, Utc::now(), None);
    let change = auth::password_change::router(auth::password_change::PasswordChangeState::new(
        f.runtime.clone(),
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    ));
    let device = f.device.clone();
    let password = tokio::spawn(async move {
        send(
            change,
            "/api/auth/me/password",
            Some(&access),
            &device,
            json!({"current_password":"RacePassword123","new_password":"ChangedPassword456"}),
        )
        .await
    });
    blocked(&f.owner).await;
    held.wait().await;
    let issued = rotation.await.unwrap();
    assert_eq!(issued.0, 200);
    assert_eq!(password.await.unwrap().0, 200);
    assert_eq!(f.live().await, 1);
    let cutoff: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT password_changed_at FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    let settings = auth::VerificationSettings::new(
        SECRET.into(),
        None,
        "verdaxis".into(),
        "verdaxis-api".into(),
    );
    let access = auth::verify_access(
        issued.1["access_token"].as_str().unwrap(),
        &settings,
        Utc::now(),
    )
    .unwrap();
    assert!(
        auth::validate_account(
            &access,
            &auth::AccountState {
                exists: true,
                status: "APPROVED".into(),
                must_change_password: false,
                password_changed_at: Some(cutoff.timestamp_micros()),
                authentication_revoked_at: None
            }
        )
        .is_err()
    );
    assert_eq!(
        refresh(f.refresh(None), f.token.clone(), f.device.clone())
            .await
            .0,
        401
    );
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r5_refresh_vs_admin_revocation_never_issues_after_cutoff() {
    for refresh_first in [true, false] {
        let f = Fixture::new().await;
        if refresh_first {
            let held = Arc::new(Barrier::new(2));
            let rotation = tokio::spawn(refresh(
                f.refresh(Some(held.clone())),
                f.token.clone(),
                f.device.clone(),
            ));
            held.wait().await;
            let pool = f.owner.clone();
            let user = f.user;
            let revoke = tokio::spawn(async move {
                let mut tx = pool.begin().await.unwrap();
                auth::store::revoke_user_authentication(&mut tx, user, Utc::now())
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            });
            blocked(&f.owner).await;
            held.wait().await;
            assert_eq!(rotation.await.unwrap().0, 200);
            revoke.await.unwrap();
        } else {
            let mut tx = f.owner.begin().await.unwrap();
            auth::store::revoke_user_authentication(&mut tx, f.user, Utc::now())
                .await
                .unwrap();
            let rotation =
                tokio::spawn(refresh(f.refresh(None), f.token.clone(), f.device.clone()));
            blocked(&f.owner).await;
            tx.commit().await.unwrap();
            assert_eq!(rotation.await.unwrap().0, 401);
        }
        assert_eq!(f.live().await, 0);
        f.remove().await;
    }
}

async fn login(pool: PgPool, email: String, device: String, held: Option<Arc<Barrier>>) -> u16 {
    let mut state = auth::login::LoginState::new(
        pool,
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    )
    .await;
    state.hooks.user_locked = held;
    auth::login::router(state)
        .oneshot(
            Request::post("/api/auth/login")
                .header("origin", ORIGIN)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", format!("device_session={device}"))
                .body(Body::from(format!(
                    "username={}&password=RacePassword123",
                    email.replace('@', "%40")
                )))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r6_login_vs_account_status_change_writes_no_session() {
    let f = Fixture::new().await;
    let mut tx = f.owner.begin().await.unwrap();
    sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
        .bind(f.user)
        .execute(&mut *tx)
        .await
        .unwrap();
    auth::store::revoke_user_authentication(&mut tx, f.user, Utc::now())
        .await
        .unwrap();
    let waiting = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        f.device.clone(),
        None,
    ));
    blocked(&f.owner).await;
    tx.commit().await.unwrap();
    assert_eq!(waiting.await.unwrap(), 403);
    assert_eq!(f.live().await, 0);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r7_device_session_limit_is_one_live_family() {
    for reverse in [false, true] {
        let a = Fixture::new().await;
        let b = Fixture::new().await;
        let (first, second) = if reverse { (&b, &a) } else { (&a, &b) };
        let device = a.device.clone();
        let held = Arc::new(Barrier::new(2));
        let winner = tokio::spawn(login(a.runtime.clone(),first.email.clone(),device.clone(),Some(held.clone())));
        held.wait().await;
        let later = tokio::spawn(login(a.runtime.clone(),second.email.clone(),device.clone(),None));
        blocked(&a.owner).await;
        held.wait().await;
        assert_eq!(winner.await.unwrap(),200);
        assert_eq!(later.await.unwrap(),200);
        let rows: Vec<(Uuid,bool)> = sqlx::query_as("SELECT user_id,revoked FROM refresh_sessions WHERE device_id_hash=$1")
            .bind(token_digest(&device)).fetch_all(&a.owner).await.unwrap();
        assert!(rows.iter().filter(|(id,_)| *id==first.user).all(|(_,revoked)| *revoked));
        assert_eq!(rows.iter().filter(|(id,revoked)| *id==second.user && !revoked).count(),1);
        a.remove().await; b.remove().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r8_login_vs_login_different_devices_both_live() {
    let f = Fixture::new().await;
    // Remove the seed family so the two logins are the only live families.
    sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1")
        .bind(f.user)
        .execute(&f.owner)
        .await
        .unwrap();
    let held = Arc::new(Barrier::new(2));
    let first = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        generate_token(),
        Some(held.clone()),
    ));
    held.wait().await;
    let second = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        generate_token(),
        None,
    ));
    blocked(&f.owner).await;
    held.wait().await;
    assert_eq!(first.await.unwrap(), 200);
    assert_eq!(second.await.unwrap(), 200);
    assert_eq!(f.live().await, 2);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r12_cutoff_downgrade_fails_fast() {
    let f = Fixture::new().await;
    let later = Utc::now() + Duration::seconds(30);
    let mut tx = f.owner.begin().await.unwrap();
    auth::store::revoke_user_authentication(&mut tx, f.user, later)
        .await
        .unwrap();
    let pool = f.owner.clone();
    let user = f.user;
    let waiting = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let cutoff = auth::store::revoke_user_authentication(&mut tx, user, Utc::now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        cutoff
    });
    blocked(&f.owner).await;
    tx.commit().await.unwrap();
    assert!(waiting.await.unwrap() > later);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r9_refresh_vs_invitation_accept_serializes() {
    let f = Fixture::new().await;
    let organization = Uuid::new_v4();
    let invitation = generate_token();
    sqlx::query("INSERT INTO organizations (id,name,type,verification_status,provenance) VALUES ($1,'Race fixture','FUEL_BUYER','APPROVED','UNKNOWN')")
        .bind(organization).execute(&f.owner).await.unwrap();
    sqlx::query("UPDATE users SET organization_id=$2,email_verified=false,must_change_password=true,password_reset_token_hash=$3,password_reset_expires=now()+interval '1 hour' WHERE id=$1")
        .bind(f.user).bind(organization).bind(token_digest(&invitation)).execute(&f.owner).await.unwrap();
    let held = Arc::new(Barrier::new(2));
    let rotation = tokio::spawn(refresh(
        f.refresh(Some(held.clone())),
        f.token.clone(),
        f.device.clone(),
    ));
    held.wait().await;
    let accept =
        auth::invitation_accept::router(auth::invitation_accept::InvitationAcceptState::new(
            f.runtime.clone(),
            "production",
            SECRET,
            "verdaxis",
            "verdaxis-api",
            ORIGIN,
            SharedLimits::default(),
        ));
    let device = f.device.clone();
    let accepting = tokio::spawn(async move {
        send(
            accept,
            "/api/auth/invitations/accept",
            None,
            &device,
            json!({"token":invitation,"new_password":"AcceptedPassword123","accept_terms":true}),
        )
        .await
    });
    blocked(&f.owner).await;
    held.wait().await;
    assert_eq!(rotation.await.unwrap().0, 200);
    assert_eq!(accepting.await.unwrap().0, 200);
    assert_eq!(f.live().await, 1);
    assert_eq!(
        refresh(f.refresh(None), f.token.clone(), f.device.clone())
            .await
            .0,
        401
    );
    let pool = f.owner.clone();
    f.remove().await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(organization)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn r10_login_vs_password_reset_serializes() {
    let f = Fixture::new().await;
    let token = generate_token();
    sqlx::query("INSERT INTO password_reset_tokens (user_id,token_hash,expires_at) VALUES ($1,$2,now()+interval '1 hour')")
        .bind(f.user).bind(token_digest(&token)).execute(&f.owner).await.unwrap();
    // Hold the login after its user lock while reset reaches that same row.
    let held = Arc::new(Barrier::new(2));
    let logging_in = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        f.device.clone(),
        Some(held.clone()),
    ));
    held.wait().await;
    let settings = auth::email_verification::EmailSettings::new(
        f.runtime.clone(),
        auth::email::MailDispatcher::new(
            verdaxis_read::crud::referrals::invite::MailClient::disabled(),
        ),
    );
    let resetting = tokio::spawn(async move {
        auth::password_reset::reset(&settings, &token, "ResetPassword456").await
    });
    blocked(&f.owner).await;
    held.wait().await;
    assert_eq!(logging_in.await.unwrap(), 200);
    assert!(resetting.await.unwrap().is_ok());
    assert_eq!(f.live().await, 0);
    assert_eq!(
        login(f.runtime.clone(), f.email.clone(), f.device.clone(), None).await,
        401
    );
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn password_change_before_refresh_keeps_only_its_successor() {
    let f = Fixture::new().await;
    let mut blocker = f.owner.begin().await.unwrap();
    auth::store::lock_user(&mut blocker, f.user).await.unwrap();
    let access =
        auth::TokenIssuer::new(SECRET, "verdaxis", "verdaxis-api").access(f.user, Utc::now(), None);
    let change = auth::password_change::router(auth::password_change::PasswordChangeState::new(
        f.runtime.clone(),
        "production",
        SECRET,
        None,
        "verdaxis",
        "verdaxis-api",
        SharedLimits::default(),
    ));
    let device = f.device.clone();
    let changing = tokio::spawn(async move {
        send(
            change,
            "/api/auth/me/password",
            Some(&access),
            &device,
            json!({"current_password":"RacePassword123","new_password":"ChangedPassword456"}),
        )
        .await
    });
    blocked(&f.owner).await;
    let rotating = tokio::spawn(refresh(f.refresh(None), f.token.clone(), f.device.clone()));
    blocked_count(&f.owner, 2).await;
    blocker.commit().await.unwrap();
    assert_eq!(changing.await.unwrap().0, 200);
    assert_eq!(rotating.await.unwrap().0, 401);
    assert_eq!(f.live().await, 1);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn login_before_rejection_leaves_no_live_session() {
    let f = Fixture::new().await;
    let held = Arc::new(Barrier::new(2));
    let logging_in = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        f.device.clone(),
        Some(held.clone()),
    ));
    held.wait().await;
    let pool = f.owner.clone();
    let user = f.user;
    let rejecting = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
        auth::store::revoke_user_authentication(&mut tx, user, Utc::now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    blocked(&f.owner).await;
    held.wait().await;
    assert_eq!(logging_in.await.unwrap(), 200);
    rejecting.await.unwrap();
    assert_eq!(f.live().await, 0);
    f.remove().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn invitation_accept_before_refresh_keeps_only_its_family() {
    let f = Fixture::new().await;
    let organization = Uuid::new_v4();
    let invitation = generate_token();
    sqlx::query("INSERT INTO organizations (id,name,type,verification_status,provenance) VALUES ($1,'Race fixture','FUEL_BUYER','APPROVED','UNKNOWN')").bind(organization).execute(&f.owner).await.unwrap();
    sqlx::query("UPDATE users SET organization_id=$2,email_verified=false,must_change_password=true,password_reset_token_hash=$3,password_reset_expires=now()+interval '1 hour' WHERE id=$1").bind(f.user).bind(organization).bind(token_digest(&invitation)).execute(&f.owner).await.unwrap();
    let mut blocker = f.owner.begin().await.unwrap();
    auth::store::lock_user(&mut blocker, f.user).await.unwrap();
    let accept =
        auth::invitation_accept::router(auth::invitation_accept::InvitationAcceptState::new(
            f.runtime.clone(),
            "production",
            SECRET,
            "verdaxis",
            "verdaxis-api",
            ORIGIN,
            SharedLimits::default(),
        ));
    let device = f.device.clone();
    let accepting = tokio::spawn(async move {
        send(
            accept,
            "/api/auth/invitations/accept",
            None,
            &device,
            json!({"token":invitation,"new_password":"AcceptedPassword123","accept_terms":true}),
        )
        .await
    });
    blocked(&f.owner).await;
    let rotating = tokio::spawn(refresh(f.refresh(None), f.token.clone(), f.device.clone()));
    blocked_count(&f.owner, 2).await;
    blocker.commit().await.unwrap();
    assert_eq!(accepting.await.unwrap().0, 200);
    assert_eq!(rotating.await.unwrap().0, 401);
    assert_eq!(f.live().await, 1);
    let pool = f.owner.clone();
    f.remove().await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(organization)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn password_reset_before_login_refuses_the_old_password() {
    let f = Fixture::new().await;
    let token = generate_token();
    sqlx::query("INSERT INTO password_reset_tokens (user_id,token_hash,expires_at) VALUES ($1,$2,now()+interval '1 hour')").bind(f.user).bind(token_digest(&token)).execute(&f.owner).await.unwrap();
    let mut blocker = f.owner.begin().await.unwrap();
    auth::store::lock_user(&mut blocker, f.user).await.unwrap();
    let settings = auth::email_verification::EmailSettings::new(
        f.runtime.clone(),
        auth::email::MailDispatcher::new(
            verdaxis_read::crud::referrals::invite::MailClient::disabled(),
        ),
    );
    let resetting = tokio::spawn(async move {
        auth::password_reset::reset(&settings, &token, "ResetPassword456").await
    });
    blocked(&f.owner).await;
    let logging_in = tokio::spawn(login(
        f.runtime.clone(),
        f.email.clone(),
        f.device.clone(),
        None,
    ));
    blocked_count(&f.owner, 2).await;
    blocker.commit().await.unwrap();
    assert!(resetting.await.unwrap().is_ok());
    assert_eq!(logging_in.await.unwrap(), 401);
    assert_eq!(f.live().await, 0);
    f.remove().await;
}

async fn blocked_count(pool: &PgPool, count: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(15),async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity a WHERE a.datname=current_database() AND a.application_name=$1 AND a.wait_event_type='Lock' AND EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid=a.pid AND NOT l.granted AND (l.locktype='advisory' OR l.locktype='transactionid') )").bind(sqlx::query_scalar::<_, String>("SHOW application_name").fetch_one(pool).await.unwrap()).fetch_one(pool).await.unwrap();
            if waiting >= count { return; }
            tokio::task::yield_now().await;
        }
    }).await.expect("both independent operations must wait before releasing the row");
}
