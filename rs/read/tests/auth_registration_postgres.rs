//! E5/E6 PostgreSQL component checks.
#![cfg(feature = "contract-tests")]
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;
#[path = "../src/auth/registration.rs"]
pub mod registration;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use registration::{RegistrationSettings, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    owner: PgPool,
    settings: RegistrationSettings,
    mail: Arc<Mutex<Vec<Value>>>,
    release: Arc<Semaphore>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(fail: bool) -> Self {
        let owner = PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_OWNER_URL").unwrap())
            .await
            .unwrap();
        let pool = PgPool::connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let mail = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let received = mail.clone();
        let gate = release.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/",
            post(move |headers: axum::http::HeaderMap, Json(mut payload): Json<Value>| {
                let received = received.clone();
                let gate = gate.clone();
                async move {
                    payload["test_idempotency"] = headers.get("idempotency-key").and_then(|v|v.to_str().ok()).map(Value::from).unwrap_or(Value::Null);
                    received.lock().await.push(payload);
                    gate.acquire().await.unwrap().forget();
                    if fail {
                        StatusCode::BAD_GATEWAY
                    } else {
                        StatusCode::OK
                    }
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dispatcher = email::MailDispatcher::new(
            crud::referrals::invite::MailClient::loopback(
                format!("http://{addr}/"),
                "synthetic".into(),
            )
            .unwrap(),
        );
        Self {
            owner,
            settings: RegistrationSettings::new(pool, dispatcher)
                .with_domain_lists(["gmail.com".into()], Vec::<String>::new()),
            mail,
            release,
            server,
        }
    }
    async fn request(&self, path: &str, body: Value) -> (StatusCode, axum::http::HeaderMap, Value) {
        request(router(self.settings.clone()), path, body).await
    }
    async fn drain(&self) {
        self.release.add_permits(100);
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            self.settings.mail.drain(),
        )
        .await
        .unwrap();
    }
    async fn token(&self, email: &str) -> String {
        self.drain().await;
        let mails = self.mail.lock().await;
        let html = mails
            .iter()
            .rev()
            .find(|mail| mail["to"][0] == email)
            .unwrap()["html"]
            .as_str()
            .unwrap();
        let token = html
            .split("token=")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        token.to_string()
    }
    async fn stop(self) {
        self.drain().await;
        self.server.abort();
        let _ = self.server.await;
    }
}
async fn request(
    app: Router,
    path: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        app.oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    (status, headers, body)
}
fn signup(email: &str, referral: Option<&str>) -> Value {
    json!({"email":email,"password":"valid-registration-password","role":"BUYER","first_name":"Test","last_name":"Account","referral_code":referral})
}
fn organization(token: &str) -> Value {
    json!({"registration_token":token,"organization":{"name":"Synthetic Company","type":"FUEL_BUYER","country_code":"SG"}})
}
async fn count(pool: &PgPool, sql: &'static str, email: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_response_and_delivery_boundary() {
    let f = Fixture::new(false).await;
    let domain = format!("{}.example", Uuid::new_v4());
    let org = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)",
    )
    .bind(org)
    .bind(&domain)
    .execute(&f.owner)
    .await
    .unwrap();
    let mut responses = Vec::new();
    let mut recipients = Vec::new();
    for (index, verified) in [None, Some(true), Some(false), None]
        .into_iter()
        .enumerate()
    {
        let email = if index == 3 {
            format!("new@{}.example", Uuid::new_v4())
        } else {
            format!("user{index}@{domain}")
        };
        if let Some(verified) = verified {
            sqlx::query("INSERT INTO users(id,email,password_hash,status,email_verified) VALUES($1,$2,'unchanged','PENDING',$3)").bind(Uuid::new_v4()).bind(&email).bind(verified).execute(&f.owner).await.unwrap();
        }
        let before: Option<Value> =
            sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE email=$1")
                .bind(&email)
                .fetch_optional(&f.owner)
                .await
                .unwrap();
        let response = f.request("/api/auth/register", signup(&email, None)).await;
        recipients.push((email.clone(), verified.is_some()));
        if let Some(before) = before {
            let after: Value = sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE email=$1")
                .bind(&email)
                .fetch_one(&f.owner)
                .await
                .unwrap();
            assert_eq!(before, after);
            for sql in [
                "SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1",
                "SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1",
                "SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
                "SELECT count(*) FROM user_login_days s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
                "SELECT count(*) FROM refresh_sessions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
            ] {
                assert_eq!(count(&f.owner, sql, &email).await, 0);
            }
        }
        assert_eq!(response.0, StatusCode::ACCEPTED);
        assert_eq!(response.2, json!({"status":"check_email"}));
        assert!(!response.1.contains_key("set-cookie"));
        responses.push((response.0, response.1, response.2));
        if verified.is_some() {
            assert_eq!(
                count(
                    &f.owner,
                    "SELECT count(*) FROM users WHERE email=$1 AND password_hash='unchanged'",
                    &email
                )
                .await,
                1
            );
            assert_eq!(
                count(
                    &f.owner,
                    "SELECT count(*) FROM pending_registrations WHERE email=$1",
                    &email
                )
                .await,
                0
            );
            assert_eq!(count(&f.owner,"SELECT count(*) FROM audit_logs a JOIN users u ON a.user_id=u.id WHERE u.email=$1",&email).await,0);
        }
    }
    assert!(responses.windows(2).all(|pair| pair[0] == pair[1]));
    // Provider completion requires permits that no request has released.
    assert_eq!(f.release.available_permits(), 0);
    f.drain().await;
    {
        let mail = f.mail.lock().await;
        for (address, existing) in recipients {
            let messages: Vec<_> = mail
                .iter()
                .filter(|message| message["to"][0] == address)
                .collect();
            assert_eq!(messages.len(), 1);
            let html = messages[0]["html"].as_str().unwrap();
            if existing {
                assert!(html.contains("/login"));
                assert!(!html.contains("token="));
            } else {
                assert!(html.contains("/verify-email?token="));
            }
        }
    }
    {
        let mail = f.mail.lock().await;
        let alerts: Vec<_> = mail.iter().filter(|m|m["subject"].as_str().is_some_and(|s|s.contains("New Verdaxis account application"))).collect();
        assert_eq!(alerts.len(),1);
        assert!(alerts[0]["subject"].as_str().unwrap().contains("[development]"));
        assert_eq!(alerts[0]["to"][0],"support@verdaxis.exchange");
        let id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email=$1").bind(format!("user0@{domain}")).fetch_one(&f.owner).await.unwrap();
        assert_eq!(alerts[0]["test_idempotency"],format!("signup-alert/development/{id}"));
        assert!(alerts[0]["html"].as_str().unwrap().contains(&format!("user0@{domain}")));
    }
    let joined: (Option<Uuid>,String,i64) = sqlx::query_as("SELECT u.organization_id,u.status::text,(SELECT count(*) FROM organization_join_requests j WHERE j.user_id=u.id AND j.organization_id=$2) FROM users u WHERE email=$1").bind(format!("user0@{domain}")).bind(org).fetch_one(&f.owner).await.unwrap();
    assert_eq!(joined, (None, "PENDING".into(), 1));
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e6_pending_verification_and_single_consumption() {
    let f = Fixture::new(false).await;
    let email = format!("{}@gmail.com", Uuid::new_v4());
    assert_eq!(
        f.request("/api/auth/register", signup(&email, None))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let token = f.token(&email).await;
    let lifetime: i64 = sqlx::query_scalar("SELECT extract(epoch FROM expires_at-created_at)::bigint FROM pending_registrations WHERE email=$1").bind(&email).fetch_one(&f.owner).await.unwrap();
    assert_eq!(lifetime, 1800);
    let verify_settings =
        email_verification::EmailSettings::new(f.settings.pool.clone(), f.settings.mail.clone());
    let response = email_verification::router(verify_settings)
        .oneshot(
            Request::post(format!("/api/auth/verify-email?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["requires_org"], true);
    assert_eq!(body["registration_token"], token);
    assert_eq!(body["email"], email);
    let result = f
        .request("/api/auth/register-with-org", organization(&token))
        .await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.2["email"], email);
    assert_eq!(result.2["status"], "PENDING");
    assert!(result.2["organization_id"].is_null());
    let replay = f
        .request("/api/auth/register-with-org", organization(&token))
        .await;
    assert_eq!(replay.0, StatusCode::BAD_REQUEST);
    assert_eq!(replay.2["detail"]["code"], "REGISTRATION_TOKEN_INVALID");
    assert_eq!(
        count(
            &f.owner,
            "SELECT count(*) FROM pending_registrations WHERE email=$1 AND used_at IS NOT NULL",
            &email
        )
        .await,
        1
    );
    assert_eq!(count(&f.owner,"SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1",&email).await,1);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_e6_atomic_referral_registration() {
    let f = Fixture::new(false).await;
    let domain = format!("{}.example", Uuid::new_v4());
    let code = format!("G{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query("INSERT INTO users(id,email,password_hash,status,referral_code) VALUES($1,$2,'unused','PENDING',$3)").bind(Uuid::new_v4()).bind(format!("referrer@{domain}")).bind(&code).execute(&f.owner).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(&domain)
    .execute(&f.owner)
    .await
    .unwrap();
    let email = format!("rollback@{domain}");
    // An INSERT after attribution fails, so every earlier write must roll back.
    sqlx::query("CREATE OR REPLACE FUNCTION registration_test_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.changes->>'email' LIKE 'rollback@%' THEN RAISE EXCEPTION 'fixture rejection'; END IF; RETURN NEW; END $$").execute(&f.owner).await.unwrap();
    sqlx::query("CREATE TRIGGER registration_test_reject_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION registration_test_reject_audit()").execute(&f.owner).await.unwrap();
    let result = f
        .request("/api/auth/register", signup(&email, Some(&code)))
        .await;
    sqlx::query("DROP TRIGGER registration_test_reject_audit ON audit_logs")
        .execute(&f.owner)
        .await
        .unwrap();
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    for sql in [
        "SELECT count(*) FROM users WHERE email=$1",
        "SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM audit_logs WHERE changes->>'email'=$1",
    ] {
        assert_eq!(count(&f.owner, sql, &email).await, 0);
    }
    assert!(f.mail.lock().await.is_empty());
    let pending_email = format!("rollback@{}.example", Uuid::new_v4());
    assert_eq!(
        f.request("/api/auth/register", signup(&pending_email, Some(&code)))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let token = f.token(&pending_email).await;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(p) FROM pending_registrations p WHERE email=$1")
            .bind(&pending_email)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    let mail_before = f.mail.lock().await.len();
    let org_before: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations")
        .fetch_one(&f.owner)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER registration_test_reject_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION registration_test_reject_audit()").execute(&f.owner).await.unwrap();
    let result = f
        .request("/api/auth/register-with-org", organization(&token))
        .await;
    sqlx::query("DROP TRIGGER registration_test_reject_audit ON audit_logs")
        .execute(&f.owner)
        .await
        .unwrap();
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(p) FROM pending_registrations p WHERE email=$1")
            .bind(&pending_email)
            .fetch_one(&f.owner)
            .await
            .unwrap();
    assert_eq!(before, after);
    let org_after: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations")
        .fetch_one(&f.owner)
        .await
        .unwrap();
    assert_eq!(org_before, org_after);
    for sql in [
        "SELECT count(*) FROM users WHERE email=$1",
        "SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM organization_join_requests j JOIN users u ON j.user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
        "SELECT count(*) FROM audit_logs WHERE changes->>'email'=$1",
        "SELECT count(*) FROM refresh_sessions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",
    ] {
        assert_eq!(count(&f.owner, sql, &pending_email).await, 0);
    }
    f.drain().await;
    assert_eq!(f.mail.lock().await.len(), mail_before);
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_duplicate_registration_counts_referral_once() {
    let f = Fixture::new(false).await;
    let domain = format!("{}.example", Uuid::new_v4());
    let code = format!("G{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query("INSERT INTO users(id,email,password_hash,status,referral_code) VALUES($1,$2,'unused','PENDING',$3)").bind(Uuid::new_v4()).bind(format!("referrer@{domain}")).bind(&code).execute(&f.owner).await.unwrap();
    sqlx::query(
        "INSERT INTO organizations(id,name,type,domain) VALUES($1,'Synthetic','FUEL_BUYER',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(&domain)
    .execute(&f.owner)
    .await
    .unwrap();
    let email = format!("duplicate@{domain}");
    let mut holder = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE referral_code=$1 FOR UPDATE")
        .bind(&code)
        .execute(&mut *holder)
        .await
        .unwrap();
    let mut tasks = Vec::new();
    let mut pids = Vec::new();
    for _ in 0..2 {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::query("SET lock_timeout='15s'")
            .execute(&pool)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        pids.push(pid);
        let mut settings = f.settings.clone();
        settings.pool = pool;
        let app = router(settings);
        let body = signup(&email, Some(&code));
        tasks.push(tokio::spawn(async move {
            request(app, "/api/auth/register", body).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(8),async {
        loop {
            let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid=ANY($1) AND cardinality(pg_blocking_pids(pid))>0").bind(&pids).fetch_one(&f.owner).await.unwrap();
            if waiting==2 {break;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    holder.commit().await.unwrap();
    for task in tasks {
        assert_eq!(task.await.unwrap().0, StatusCode::ACCEPTED);
    }
    assert_eq!(
        count(
            &f.owner,
            "SELECT count(*) FROM users WHERE email=$1",
            &email
        )
        .await,
        1
    );
    assert_eq!(count(&f.owner,"SELECT count(*) FROM referrals r JOIN users u ON r.referred_user_id=u.id WHERE u.email=$1",&email).await,1);
    assert_eq!(count(&f.owner,"SELECT count(*) FROM user_status_transitions s JOIN users u ON s.user_id=u.id WHERE u.email=$1",&email).await,1);
    f.stop().await;
}

#[derive(Clone)]
struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e6_concurrent_consumption_has_one_result() {
    let f = Fixture::new(false).await;
    let email = format!("{}@gmail.com", Uuid::new_v4());
    assert_eq!(
        f.request("/api/auth/register", signup(&email, None))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let token = f.token(&email).await;
    let mut holder = f.owner.begin().await.unwrap();
    sqlx::query("SELECT id FROM pending_registrations WHERE email=$1 FOR UPDATE")
        .bind(&email)
        .execute(&mut *holder)
        .await
        .unwrap();
    let mut tasks = Vec::new();
    let mut pids = Vec::new();
    for _ in 0..2 {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&std::env::var("AUTH_EMAIL_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::query("SET lock_timeout='15s'")
            .execute(&pool)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
        pids.push(pid);
        let mut settings = f.settings.clone();
        settings.pool = pool;
        let app = router(settings);
        let body = organization(&token);
        tasks.push(tokio::spawn(async move {
            request(app, "/api/auth/register-with-org", body).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid=ANY($1) AND cardinality(pg_blocking_pids(pid))>0").bind(&pids).fetch_one(&f.owner).await.unwrap();
            if waiting == 2 { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    holder.commit().await.unwrap();
    let mut statuses = Vec::new();
    for task in tasks {
        let result = task.await.unwrap();
        if result.0 == StatusCode::BAD_REQUEST {
            assert_eq!(result.2["detail"]["code"], "REGISTRATION_TOKEN_INVALID");
        }
        statuses.push(result.0);
    }
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::OK, StatusCode::BAD_REQUEST]);
    assert_eq!(
        count(
            &f.owner,
            "SELECT count(*) FROM users WHERE email=$1",
            &email
        )
        .await,
        1
    );
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_failed_delivery_preserves_response() {
    use tracing::instrument::WithSubscriber;
    let captured = LogCapture(Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let f = Fixture::new(true).await;
    let email = format!("{}@gmail.com", Uuid::new_v4());
    let response = f
        .request("/api/auth/register", signup(&email, None))
        .with_subscriber(subscriber)
        .await;
    assert_eq!(response.0, StatusCode::ACCEPTED);
    assert_eq!(response.2, json!({"status":"check_email"}));
    f.drain().await;
    assert_eq!(f.mail.lock().await.len(), 3);
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert_eq!(log.matches("auth_mail_delivery_failed").count(), 1);
    assert!(log.contains(&auth::token_bytes::token_digest(&email)[..12]));
    assert!(!log.contains(&email));
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e3_e6_pending_expiry_boundaries() {
    let f = Fixture::new(false).await;
    for offset in [-1i64, 0, 60] {
        let email = format!("{}@gmail.com", Uuid::new_v4());
        assert_eq!(
            f.request("/api/auth/register", signup(&email, None))
                .await
                .0,
            StatusCode::ACCEPTED
        );
        let token = f.token(&email).await;
        sqlx::query("UPDATE pending_registrations SET expires_at=clock_timestamp()+make_interval(secs => $2) WHERE email=$1").bind(&email).bind(offset as f64).execute(&f.owner).await.unwrap();
        let before: Value =
            sqlx::query_scalar("SELECT to_jsonb(p) FROM pending_registrations p WHERE email=$1")
                .bind(&email)
                .fetch_one(&f.owner)
                .await
                .unwrap();
        let mail_before = f.mail.lock().await.len();
        let verification = email_verification::EmailSettings::new(
            f.settings.pool.clone(),
            f.settings.mail.clone(),
        );
        let response = email_verification::router(verification)
            .oneshot(
                Request::post(format!("/api/auth/verify-email?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if offset > 0 {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        if offset <= 0 {
            assert_eq!(body["detail"]["code"], "EMAIL_TOKEN_INVALID");
        }
        let result = f
            .request("/api/auth/register-with-org", organization(&token))
            .await;
        if offset > 0 {
            assert_eq!(result.0, StatusCode::OK);
        } else {
            assert_eq!(result.0, StatusCode::BAD_REQUEST);
            assert_eq!(result.2["detail"]["code"], "REGISTRATION_TOKEN_INVALID");
            let after: Value = sqlx::query_scalar(
                "SELECT to_jsonb(p) FROM pending_registrations p WHERE email=$1",
            )
            .bind(&email)
            .fetch_one(&f.owner)
            .await
            .unwrap();
            assert_eq!(before, after);
            assert_eq!(
                count(
                    &f.owner,
                    "SELECT count(*) FROM users WHERE email=$1",
                    &email
                )
                .await,
                0
            );
            f.drain().await;
            assert_eq!(f.mail.lock().await.len(), mail_before);
        }
    }
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_e6_http_limits() {
    let f = Fixture::new(false).await;
    for path in ["/api/auth/register", "/api/auth/register-with-org"] {
        let mut settings = f.settings.clone();
        settings.limits = auth::limits::SharedLimits::default();
        let app = router(settings);
        for index in 0..6 {
            let body = if path.ends_with("register") {
                signup(&format!("{}@gmail.com", Uuid::new_v4()), None)
            } else {
                organization("invalid")
            };
            let response = request(app.clone(), path, body).await;
            assert_eq!(
                response.0,
                if index == 5 {
                    StatusCode::TOO_MANY_REQUESTS
                } else if path.ends_with("register") {
                    StatusCode::ACCEPTED
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
            if index == 5 {
                assert!(response.1.contains_key("retry-after"));
                assert_eq!(response.2["detail"]["code"], "RATE_LIMITED");
            }
        }
    }
    f.stop().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL authority"]
async fn e5_e6_mail_logs_exclude_credentials() {
    use tracing::instrument::WithSubscriber;
    let captured=LogCapture(Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(captured.clone()).finish();
    let f=Fixture::new(true).await;
    let email=format!("{}@gmail.com",Uuid::new_v4());
    f.request("/api/auth/register",signup(&email,None)).with_subscriber(subscriber).await;
    let pending=f.token(&email).await;
    let verification=auth::token_bytes::generate_token();
    let reset=auth::token_bytes::generate_token();
    let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(captured.clone()).finish();
    async {
        f.settings.mail.dispatch(email::AuthMail::Verification{to:email.clone(),name:"Test".into(),token:verification.clone(),pending:None});
        f.settings.mail.dispatch(email::AuthMail::Reset{to:email.clone(),name:"Test".into(),token:reset.clone()});
    }.with_subscriber(subscriber).await;
    f.drain().await;
    let log=String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("auth_mail_delivery_failed"));
    for token in [&pending,&verification,&reset] {assert!(!log.contains(token),"credential appeared in logs");}
    f.stop().await;
}
