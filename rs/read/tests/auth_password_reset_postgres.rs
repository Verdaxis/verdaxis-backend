//! E1/E2 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/password_reset.rs"]
pub mod password_reset;
use auth::token_bytes::token_digest;
use chrono::{Duration, Utc};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e2_consumes_and_revokes() {
    let pool = sqlx::PgPool::connect(
        &std::env::var("AUTH_EMAIL_TEST_OWNER_URL").expect("disposable database"),
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    let address = format!("{id}@example.com");
    let token = Uuid::new_v4().to_string();
    let invitation = token_digest("invitation-fixture");
    sqlx::query("INSERT INTO users (id,email,password_hash,status,email_verified,must_change_password,password_reset_token_hash,password_reset_expires) VALUES ($1,$2,'unused','APPROVED',true,true,$3,$4)").bind(id).bind(&address).bind(&invitation).bind(Utc::now()+Duration::hours(1)).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id,token_hash,expires_at) VALUES ($1,$2,$3)",
    )
    .bind(id)
    .bind(token_digest(&token))
    .bind(Utc::now() + Duration::hours(1))
    .execute(&pool)
    .await
    .unwrap();
    auth::store::insert_refresh(
        &mut pool.acquire().await.unwrap(),
        &auth::store::NewRefreshRow {
            user_id: id,
            family_id: Uuid::new_v4(),
            jti_hash: token_digest(&Uuid::new_v4().to_string()),
            device_id_hash: token_digest("fixture-device"),
            expires_at: Utc::now() + Duration::days(7),
        },
    )
    .await
    .unwrap();
    let settings = email_verification::EmailSettings::new(
        sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    assert!(
        password_reset::reset(&settings, "invitation-fixture", "New-password-123")
            .await
            .is_err()
    );
    password_reset::reset(&settings, &token, "New-password-123")
        .await
        .unwrap();
    assert!(
        password_reset::reset(&settings, &token, "Another-password-123")
            .await
            .is_err()
    );
    let row:(String,bool,Option<chrono::DateTime<Utc>>,String) = sqlx::query_as("SELECT password_hash,must_change_password,password_changed_at,password_reset_token_hash FROM users WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(auth::password::verify("New-password-123", &row.0).await);
    assert!(!row.1 && row.2.is_some());
    assert_eq!(row.3, invitation);
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
    sqlx::query("DELETE FROM refresh_sessions WHERE user_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM audit_logs WHERE user_id=$1")
        .bind(id)
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
async fn e1_write_attempt_restarts() {
    let owner = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
        .await
        .unwrap();
    let runtime = sqlx::PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let address = format!("{id}@example.com");
    sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',true,false)").bind(id).bind(&address).execute(&owner).await.unwrap();
    let suffix = id.simple().to_string();
    let seq = format!("e1_{suffix}");
    let fun = format!("e1f_{suffix}");
    let trigger = format!("e1t_{suffix}");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SEQUENCE {seq}; GRANT USAGE,SELECT ON SEQUENCE {seq} TO verdaxis_app_test; CREATE FUNCTION {fun}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.user_id='{id}'::uuid AND nextval('{seq}')<=2 THEN RAISE EXCEPTION 'retry' USING ERRCODE='40P01'; END IF; RETURN NEW; END $$; CREATE TRIGGER {trigger} BEFORE INSERT OR UPDATE ON password_reset_tokens FOR EACH ROW EXECUTE FUNCTION {fun}();"))).execute(&owner).await.unwrap();
    let settings = email_verification::EmailSettings::new(
        runtime,
        email::MailDispatcher::new(crud::referrals::invite::MailClient::disabled()),
    );
    let result = password_reset::forgot(&settings, &address).await;
    let attempts: i64 =
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT last_value FROM {seq}")))
            .fetch_one(&owner)
            .await
            .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER {trigger} ON password_reset_tokens; DROP FUNCTION {fun}(); DROP SEQUENCE {seq};"))).execute(&owner).await.unwrap();
    sqlx::query("DELETE FROM audit_logs WHERE user_id=$1")
        .bind(id)
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
async fn e1_response_cleanup_and_delivery() {
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let held = release.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                post(move || {
                    let held = held.clone();
                    let calls = counted.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        held.acquire().await.unwrap().forget();
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
    let settings = email_verification::EmailSettings::new(runtime, mail.clone());
    let mut expected = None;
    let mut ids = Vec::new();
    for (index, verified) in [Some(true), Some(false), None, Some(true)]
        .into_iter()
        .enumerate()
    {
        let id = Uuid::new_v4();
        let address = format!("{id}@example.com");
        if let Some(verified) = verified {
            sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) VALUES ($1,$2,'unused','PENDING',$3,false)").bind(id).bind(&address).bind(verified).execute(&owner).await.unwrap();
            ids.push(id);
        }
        let mut request = Request::post("/api/auth/forgot-password")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::json!({"email":address}).to_string()))
            .unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, (index + 1) as u8],
                1234,
            ))));
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            password_reset::router(settings.clone()).oneshot(request),
        )
        .await
        .unwrap()
        .unwrap();
        let result = (
            response.status(),
            response.headers().clone(),
            to_bytes(response.into_body(), 4096).await.unwrap(),
        );
        assert_eq!(result.0, axum::http::StatusCode::OK);
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while calls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    release.add_permits(2);
    mail.drain().await;
    // Sweep is a separate operation and each request removes at most 1000 rows.
    let batch = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified,must_change_password) SELECT md5($1||g)::uuid,$1||g||'@example.com','unused','PENDING',false,false FROM generate_series(1,1500) g").bind(&batch).execute(&owner).await.unwrap();
    sqlx::query("INSERT INTO password_reset_tokens(user_id,token_hash,expires_at) SELECT id,repeat('0',32)||replace(id::text,'-',''),now()-interval '1 hour' FROM users WHERE email LIKE $1").bind(format!("{batch}%")).execute(&owner).await.unwrap();
    password_reset::forgot(&settings, "absent@example.com")
        .await
        .unwrap();
    let remaining:i64=sqlx::query_scalar("SELECT count(*) FROM password_reset_tokens t JOIN users u ON u.id=t.user_id WHERE u.email LIKE $1").bind(format!("{batch}%")).fetch_one(&owner).await.unwrap();
    assert_eq!(remaining, 500);
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{batch}%"))
        .execute(&owner)
        .await
        .unwrap();
    for id in ids {
        sqlx::query("DELETE FROM audit_logs WHERE user_id=$1")
            .bind(id)
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(&owner)
            .await
            .unwrap();
    }
    server.abort();
    let _ = server.await;
}
