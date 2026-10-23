//! E3/E4 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
use auth::token_bytes::token_digest;
use chrono::{Duration, Utc};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e3_consumes_and_assigns_once() {
    let pool = sqlx::PgPool::connect(
        &std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("disposable database"),
    )
    .await
    .unwrap();
    let referrer = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',true,false)").bind(referrer).bind(format!("{referrer}@example.com")).execute(&pool).await.unwrap();
    let id = Uuid::new_v4();
    let token = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password,email_verification_token_hash,email_verification_token_expires_at) VALUES ($1,$2,'unused','PENDING',false,false,$3,$4)")
        .bind(id).bind(format!("{id}@example.com")).bind(token_digest(&token)).bind(Utc::now()+Duration::hours(24)).execute(&pool).await.unwrap();
    let settings = email_verification::EmailSettings::new(
        sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    sqlx::query("INSERT INTO referrals(id,referrer_id,referred_user_id,referral_code_used,status) VALUES ($1,$2,$3,'VDX-TEST01','SIGNED_UP')").bind(Uuid::new_v4()).bind(referrer).bind(id).execute(&pool).await.unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let waiting_settings = settings.clone();
    let waiting_token = token.clone();
    let waiting =
        tokio::spawn(
            async move { email_verification::verify(&waiting_settings, &waiting_token).await },
        );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE cardinality(pg_blocking_pids(pid))>0)").fetch_one(&pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    holder.commit().await.unwrap();
    let result = waiting.await.unwrap().unwrap();
    assert_eq!(result["email"], format!("{id}@example.com"));
    assert_eq!(result["requires_org"], false);
    email_verification::resend(&settings, &format!("{id}@example.com"), true)
        .await
        .unwrap();
    let row: (bool,Option<String>,Option<chrono::DateTime<Utc>>, Option<String>) = sqlx::query_as("SELECT email_verified,email_verification_token_hash,email_verification_token_expires_at,referral_code FROM users WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(row.0 && row.1.is_none() && row.2.is_none() && row.3.is_some());
    assert_eq!(
        email_verification::verify(&settings, &token)
            .await
            .unwrap_err()
            .status,
        axum::http::StatusCode::BAD_REQUEST
    );
    let referral: (String, bool) = sqlx::query_as(
        "SELECT status::text,verified_at IS NOT NULL FROM referrals WHERE referred_user_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(referral, ("VERIFIED".into(), true));
    sqlx::query("DELETE FROM referrals WHERE referred_user_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(referrer)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e4_response_and_delivery_boundary() {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
        routing::post,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;
    let pool = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_server = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_server = calls.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                post(move || {
                    let release = release_server.clone();
                    let calls = calls_server.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        release.notified().await;
                        axum::http::StatusCode::OK
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let mail = email::MailDispatcher::new(
        crud::referrals::invite::MailClient::loopback(
            format!("http://{address}/"),
            "synthetic".into(),
        )
        .unwrap(),
    );
    let settings = email_verification::EmailSettings::new(
        sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
        mail.clone(),
    );
    let mut expected = None;
    let mut ids = Vec::new();
    for verified in [Some(false), Some(true), None] {
        let id = Uuid::new_v4();
        let address = format!("{id}@example.com");
        if let Some(verified) = verified {
            sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',$3,false)").bind(id).bind(&address).bind(verified).execute(&pool).await.unwrap();
            ids.push(id);
        }
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            email_verification::router(settings.clone()).oneshot(
                Request::post("/api/auth/resend-verification-email")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"email":address}).to_string()))
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let result = (status, headers, body);
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.notify_waiters();
    mail.drain().await;
    for id in ids {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    server.abort();
    let _ = server.await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e4_write_attempt_restarts() {
    let owner = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let runtime = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let address = format!("{id}@example.com");
    sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',false,false)").bind(id).bind(&address).execute(&owner).await.unwrap();
    let suffix = id.simple().to_string();
    let seq = format!("e4_{suffix}");
    let fun = format!("e4f_{suffix}");
    let trigger = format!("e4t_{suffix}");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SEQUENCE {seq}; GRANT USAGE,SELECT ON SEQUENCE {seq} TO verdaxis_app_test; CREATE FUNCTION {fun}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.id='{id}'::uuid AND nextval('{seq}')<=2 THEN RAISE EXCEPTION 'retry' USING ERRCODE='40001'; END IF; RETURN NEW; END $$; CREATE TRIGGER {trigger} BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION {fun}();"))).execute(&owner).await.unwrap();
    let settings = email_verification::EmailSettings::new(
        runtime,
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    let result = email_verification::resend(&settings, &address, true).await;
    let attempts: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT last_value FROM {seq}")))
        .fetch_one(&owner)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER {trigger} ON users; DROP FUNCTION {fun}(); DROP SEQUENCE {seq};"
    )))
    .execute(&owner)
    .await
    .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await
        .unwrap();
    assert!(result.is_ok());
    assert_eq!(attempts, 3);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e3_e4_serial_schedules() {
    let owner = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let runtime = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&runtime)
        .await
        .unwrap();
    assert_eq!(role, "verdaxis_app_test");
    let settings = email_verification::EmailSettings::new(
        runtime,
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    for verify_first in [true, false] {
        let id = Uuid::new_v4();
        let address = format!("{id}@example.com");
        let old = Uuid::new_v4().to_string();
        let new = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password,email_verification_token_hash,email_verification_token_expires_at) VALUES ($1,$2,'unused','PENDING',false,false,$3,$4)").bind(id).bind(&address).bind(token_digest(&old)).bind(Utc::now()+Duration::hours(24)).execute(&owner).await.unwrap();
        let mut holder = owner.begin().await.unwrap();
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *holder)
            .await
            .unwrap();
        if verify_first {
            sqlx::query("UPDATE users SET email_verified=true,email_verification_token_hash=NULL,email_verification_token_expires_at=NULL WHERE id=$1").bind(id).execute(&mut *holder).await.unwrap();
        } else {
            sqlx::query("UPDATE users SET email_verification_token_hash=$2 WHERE id=$1")
                .bind(id)
                .bind(token_digest(&new))
                .execute(&mut *holder)
                .await
                .unwrap();
        }
        let waiting = settings.clone();
        let address_wait = address.clone();
        let old_wait = old.clone();
        let contender = tokio::spawn(async move {
            if verify_first {
                email_verification::resend(&waiting, &address_wait, true).await
            } else {
                email_verification::verify(&waiting, &old_wait).await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE cardinality(pg_blocking_pids(pid))>0)").fetch_one(&owner).await.unwrap();if blocked {break;}tokio::task::yield_now().await;
        }}).await.unwrap();
        holder.commit().await.unwrap();
        let result = contender.await.unwrap();
        if verify_first {
            assert!(result.is_ok());
            let hash: Option<String> =
                sqlx::query_scalar("SELECT email_verification_token_hash FROM users WHERE id=$1")
                    .bind(id)
                    .fetch_one(&owner)
                    .await
                    .unwrap();
            assert!(hash.is_none());
        } else {
            assert_eq!(
                result.unwrap_err().status,
                axum::http::StatusCode::BAD_REQUEST
            );
            email_verification::verify(&settings, &new).await.unwrap();
            assert!(email_verification::verify(&settings, &new).await.is_err());
        }
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(&owner)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e4_http_canary_admission() {
    use axum::{Router, body::Body, http::Request, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;
    let owner = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let runtime = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let address = format!("canary+{id}@one.canary.verdaxis.exchange");
    sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',false,false)").bind(id).bind(&address).execute(&owner).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                post(move || {
                    let calls = observed.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        axum::http::StatusCode::OK
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let mail = email::MailDispatcher::new(
        crud::referrals::invite::MailClient::loopback(
            format!("http://{endpoint}/"),
            "synthetic".into(),
        )
        .unwrap(),
    );
    let settings = email_verification::EmailSettings::new(runtime, mail.clone())
        .with_monitor_token("monitor-fixture".into());
    for (presented, expected) in [("monitor-fixture", 0), ("other-fixture", 1)] {
        let response = email_verification::router(settings.clone())
            .oneshot(
                Request::post("/api/auth/resend-verification-email")
                    .header("content-type", "application/json")
                    .header("x-monitor-token", presented)
                    .body(Body::from(serde_json::json!({"email":address}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        mail.drain().await;
        assert_eq!(calls.load(Ordering::SeqCst), expected);
    }
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&owner)
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
}
