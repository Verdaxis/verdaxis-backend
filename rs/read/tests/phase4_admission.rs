use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    auth::{CurrentActor, VerificationSettings, verify_access},
    business::{
        admission::{
            self, AdmissionEvents, ApprovalMail, ApprovalMailMarker, ApproveAccount,
            FrozenApprovalPayload, RejectAccount, RejectionRoute, review_queue,
        },
        transaction::{AttemptError, CommandFuture, run_ordered_write},
    },
};

struct Events;
impl AdmissionEvents for Events {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            Ok(
                sqlx::query_scalar("SELECT status='APPROVED' FROM users WHERE id=$1")
                    .bind(user)
                    .fetch_one(conn)
                    .await?,
            )
        })
    }
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            sqlx::query("INSERT INTO market_event_outbox VALUES($1,$2,$3,$4)")
                .bind(Uuid::new_v4())
                .bind(user)
                .bind(organization)
                .bind(public)
                .execute(conn)
                .await?;
            Ok(())
        })
    }
}

// This producer double is limited to CF4_01's frozen marker contract.
struct Mail;
impl ApprovalMail for Mail {
    fn freeze_approval_mail<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        marker: &'a ApprovalMailMarker,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            sqlx::query("UPDATE users SET pending_approval_email_transition_id=$2,pending_approval_email_payload=$3,pending_approval_email_retry_at=$4 WHERE id=$1")
                .bind(marker.user_id).bind(marker.transition_id).bind(serde_json::to_value(&marker.payload).unwrap()).bind(marker.retry_at).execute(conn).await?;
            Ok(())
        })
    }
}

fn payload() -> FrozenApprovalPayload {
    FrozenApprovalPayload {
        from: "team@example.test".into(),
        to: ["recipient@example.test".into()],
        subject: "Approved".into(),
        html: "<p>Approved</p>".into(),
        reply_to: None,
    }
}

#[test]
fn p4_t6_c08() {
    assert!(review_queue::ReviewQuery::new(1).is_ok());
    assert!(review_queue::ReviewQuery::new(100).is_ok());
    for limit in [0, 101, -1] {
        assert_eq!(
            review_queue::ReviewQuery::new(limit)
                .unwrap_err()
                .status
                .as_u16(),
            422
        );
    }
    let value = serde_json::to_value(payload()).unwrap();
    assert_eq!(value["to"].as_array().unwrap().len(), 1);
    assert!(value.get("reply_to").is_none());
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    user: Uuid,
    org: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL");
        let app_url =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
        let migrator = url::Url::parse(&url).unwrap();
        let runtime = url::Url::parse(&app_url).unwrap();
        assert_eq!(migrator.host_str(), Some("127.0.0.1"));
        assert_eq!(runtime.host_str(), migrator.host_str());
        assert_eq!(runtime.port(), migrator.port());
        assert_eq!(runtime.path(), migrator.path());
        assert!(runtime.path().ends_with("_test"));
        assert_eq!(runtime.username(), "rust_app");
        let mut admin = PgConnection::connect(&url).await.unwrap();
        let schema = format!("p4_t6_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE organizations(id uuid PRIMARY KEY,name text,domain text,type text,verification_status text,tax_id text,country_code text,provenance text);\
            CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid,role text,status text,email_verified boolean DEFAULT true,must_change_password boolean DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,email text DEFAULT 'recipient@example.test',password_hash text DEFAULT 'synthetic',email_verification_token_hash text,email_verification_token_expires_at timestamptz,first_name text,last_name text,referral_code text,kyc_status text DEFAULT 'PENDING',kyc_organization_id uuid,kyc_external_evidence_reference text,kyc_review_note text,kyc_reviewed_by uuid,kyc_reviewed_at timestamptz,created_at timestamptz DEFAULT now(),last_login timestamptz,pending_approval_email_transition_id uuid,pending_approval_email_payload jsonb,pending_approval_email_retry_at timestamptz);\
            CREATE TABLE organization_join_requests(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,status text,created_at timestamptz,reviewed_at timestamptz,review_note text);\
            CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action text,resource_type text,resource_id text,changes jsonb,ip_address text,request_id text,timestamp timestamptz);\
            CREATE TABLE user_status_transitions(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,role text,from_status text,to_status text,effective_at timestamptz,provenance text);\
            CREATE TABLE refresh_sessions(id uuid PRIMARY KEY,user_id uuid,revoked boolean);\
            CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,public boolean);\
            CREATE TABLE trades(id uuid PRIMARY KEY,buyer_user_id uuid,seller_user_id uuid,state text,quantity numeric,fee numeric);\
            CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid)").execute(&mut admin).await.unwrap();
        let actor = Uuid::new_v4();
        let user = Uuid::new_v4();
        let org = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,name,type,verification_status) VALUES($1,'Synthetic','BUYER','APPROVED')").bind(org).execute(&mut admin).await.unwrap();
        for (id, role, status) in [(actor, "ADMIN", "APPROVED"), (user, "BUYER", "PENDING")] {
            sqlx::query("INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,$3,$4)")
                .bind(id)
                .bind(org)
                .bind(role)
                .bind(status)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO refresh_sessions VALUES($1,$2,false)")
            .bind(Uuid::new_v4())
            .bind(user)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app; REVOKE UPDATE ON {schema}.audit_logs,{schema}.user_status_transitions,{schema}.market_event_outbox FROM rust_app"))).execute(&mut admin).await.unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let query = search_path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(query))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            schema,
            actor,
            user,
            org,
        }
    }
    fn actor(&self) -> CurrentActor {
        CurrentActor {
            id: self.actor,
            organization_id: Some(self.org),
            role: Some("ADMIN".into()),
            email_verified: true,
        }
    }
    async fn approve(&self) -> admission::ApprovalResult {
        run_ordered_write(
            self.pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(self.actor),
            ApproveAccount {
                user_id: self.user,
                now: Utc::now(),
                payload: payload(),
                mail: Mail,
                events: Events,
            },
        )
        .await
        .unwrap()
    }
    async fn reject(
        &self,
        route: RejectionRoute,
    ) -> Result<admission::RejectionResult, verdaxis_read::ApiError> {
        run_ordered_write(
            self.pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(self.actor),
            admission::AccountRejection {
                command: RejectAccount {
                    user_id: self.user,
                    now: Utc::now(),
                    reason: "review".into(),
                    events: Events,
                },
                route,
            },
        )
        .await
    }
    async fn count(&self, table: &str) -> i64 {
        assert!(
            [
                "audit_logs",
                "user_status_transitions",
                "market_event_outbox"
            ]
            .contains(&table)
        );
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&mut self.admin)
        .await
        .unwrap();
    }
}
fn access(id: Uuid) -> verdaxis_read::auth::VerifiedAccess {
    let now = Utc::now();
    let token=jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),&json!({"sub":id,"type":"access","iat":now.timestamp(),"exp":now.timestamp()+900,"iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(b"synthetic-test-secret-at-least-32-bytes")).unwrap();
    verify_access(
        &token,
        &VerificationSettings::new(
            "synthetic-test-secret-at-least-32-bytes".into(),
            None,
            "test".into(),
            "test".into(),
        ),
        now,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c01() {
    let f = Fixture::new().await;
    let mut conn = f.pool.acquire().await.unwrap();
    let detail = review_queue::detail(&mut conn, f.user).await.unwrap();
    for key in [
        "user_id",
        "email",
        "account_status",
        "current_organization",
        "requested_organizations",
        "kyc_reviewed_at",
    ] {
        assert!(detail.get(key).is_some(), "{key}");
    }
    assert_eq!(detail["current_organization"]["id"], f.org.to_string());
    assert_eq!(
        review_queue::detail(&mut conn, Uuid::new_v4())
            .await
            .unwrap_err()
            .status
            .as_u16(),
        404
    );
    assert_eq!(
        review_queue::detail(&mut conn, f.actor)
            .await
            .unwrap_err()
            .status
            .as_u16(),
        404
    );
    drop(conn);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c02() {
    let f = Fixture::new().await;
    for state in [
        "PENDING_CONFIRMATION",
        "CONFIRMED",
        "DELIVERED",
        "PAID",
        "CANCELLED",
        "DECLINED",
    ] {
        sqlx::query("INSERT INTO trades VALUES($1,$2,$3,$4,25,3.25)")
            .bind(Uuid::new_v4())
            .bind(f.user)
            .bind(f.actor)
            .bind(state)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    let before: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.reject(RejectionRoute::Auth).await.unwrap();
    let after: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE NOT revoked")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        "REJECTED"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c03() {
    for route in [RejectionRoute::Auth, RejectionRoute::Analytics] {
        let f = Fixture::new().await;
        f.reject(route).await.unwrap();
        assert_eq!(f.count("user_status_transitions").await, 1);
        let repeated = f.reject(route).await;
        if route == RejectionRoute::Analytics {
            assert_eq!(repeated.unwrap_err().status.as_u16(), 409);
        } else {
            repeated.unwrap();
        }
        assert_eq!(f.count("user_status_transitions").await, 1);
        f.finish().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c04() {
    let f = Fixture::new().await;
    f.approve().await;
    f.reject(RejectionRoute::Analytics).await.unwrap();
    let row=sqlx::query("SELECT pending_approval_email_transition_id,pending_approval_email_payload,pending_approval_email_retry_at FROM users WHERE id=$1").bind(f.user).fetch_one(&f.pool).await.unwrap();
    assert!(row.get::<Option<Uuid>, _>(0).is_none());
    assert!(row.get::<Option<Value>, _>(1).is_none());
    assert!(row.get::<Option<DateTime<Utc>>, _>(2).is_none());
    assert_eq!(f.count("market_event_outbox").await, 2);
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT bool_and(public) FROM market_event_outbox")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c05() {
    let f = Fixture::new().await;
    for (role, verified) in [("ADMIN", true), ("BUYER", false)] {
        sqlx::query("UPDATE users SET role=$2,email_verified=$3 WHERE id=$1")
            .bind(f.user)
            .bind(role)
            .bind(verified)
            .execute(&f.pool)
            .await
            .unwrap();
        let result = run_ordered_write(
            f.pool.clone(),
            Arc::new(Semaphore::new(1)),
            access(f.actor),
            ApproveAccount {
                user_id: f.user,
                now: Utc::now(),
                payload: payload(),
                mail: Mail,
                events: Events,
            },
        )
        .await;
        assert_eq!(result.unwrap_err().status.as_u16(), 403);
        assert_eq!(f.count("user_status_transitions").await, 0);
        assert_eq!(f.count("audit_logs").await, 0);
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c06() {
    let f = Fixture::new().await;
    let first = f.approve().await.transition_id.unwrap();
    assert!(f.approve().await.transition_id.is_none());
    f.reject(RejectionRoute::Auth).await.unwrap();
    let next = f.approve().await.transition_id.unwrap();
    assert_ne!(first, next);
    let stored: Uuid =
        sqlx::query_scalar("SELECT pending_approval_email_transition_id FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(stored, next);
    assert_eq!(f.count("user_status_transitions").await, 3);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c07() {
    let f = Fixture::new().await;
    for _ in 0..25 {
        sqlx::query(
            "INSERT INTO organization_join_requests VALUES($1,$2,$3,'PENDING',now(),NULL,NULL)",
        )
        .bind(Uuid::new_v4())
        .bind(f.user)
        .bind(f.org)
        .execute(&f.pool)
        .await
        .unwrap();
    }
    let mut conn = f.pool.acquire().await.unwrap();
    let result = review_queue::list(&mut conn, review_queue::ReviewQuery::new(50).unwrap())
        .await
        .unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["items"][0]["requested_organizations"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    drop(conn);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c09() {
    let f = Fixture::new().await;
    sqlx::raw_sql("ALTER TABLE audit_logs ADD CONSTRAINT refuse_fact CHECK (false) NOT VALID")
        .execute(&mut *f.pool.acquire().await.unwrap())
        .await
        .unwrap_err();
    // Trusted fixture writer installs the failure; runtime has no DDL authority.
    let ddl = format!(
        "ALTER TABLE {}.audit_logs ADD CONSTRAINT refuse_fact CHECK (false) NOT VALID",
        f.schema
    );
    let mut owner =
        PgConnection::connect(&std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").unwrap())
            .await
            .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&mut owner)
        .await
        .unwrap();
    assert!(f.reject(RejectionRoute::Auth).await.is_err());
    assert_eq!(f.count("user_status_transitions").await, 0);
    assert_eq!(f.count("market_event_outbox").await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id=$1")
            .bind(f.user)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        "PENDING"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c10() {
    let f = Fixture::new().await;
    let limits = verdaxis_read::auth::limits::SharedLimits::default();
    for route in admission::AdmissionRoute::ALL {
        let mut wrong = f.actor();
        wrong.role = Some("BUYER".into());
        for _ in 0..60 {
            assert_eq!(
                admission::admit_admin(&wrong, &limits, route, "same-client", "same-token", 100)
                    .unwrap_err()
                    .status
                    .as_u16(),
                403
            );
        }
        for _ in 0..60 {
            admission::admit_admin(&f.actor(), &limits, route, "same-client", "same-token", 100)
                .unwrap();
        }
        let error =
            admission::admit_admin(&f.actor(), &limits, route, "same-client", "same-token", 100)
                .unwrap_err();
        assert_eq!(error.status.as_u16(), 429);
        assert!(error.retry_after.unwrap() > 0);
    }
    f.finish().await;
}

struct Template;
impl admission::ApprovalTemplate for Template {
    fn payload(&self, email: &str, _: Option<&str>) -> FrozenApprovalPayload {
        let mut value = payload();
        value.to = [email.into()];
        value
    }
}
fn app(f: &Fixture, limits: verdaxis_read::auth::limits::SharedLimits) -> axum::Router {
    // Readiness is a dedicated one-connection pool; the fixture pool serves reads.
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&std::env::var("RUST_TEST_DATABASE_URL").unwrap())
        .unwrap();
    let state = verdaxis_read::AppState::new(
        verdaxis_read::Config {
            kyc_execution_gate_enabled: true,
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "kycg_20261014_staging".into(),
            database_url: std::env::var("RUST_TEST_DATABASE_URL").unwrap(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 20,
            configured_max_connections: 100,
        },
        verdaxis_read::runtime::db::Pools {
            read: f.pool.clone(),
            health,
        },
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    admission::router(
        state,
        VerificationSettings::new(
            "synthetic-test-secret-at-least-32-bytes".into(),
            None,
            "test".into(),
            "test".into(),
        ),
        limits,
        admission::AdmissionService {
            mail: Arc::new(Mail),
            events: Arc::new(Events),
            template: Arc::new(Template),
        },
    )
}
fn token(user: Uuid) -> String {
    let now = Utc::now();
    jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),&json!({"sub":user,"type":"access","iat":now.timestamp(),"exp":now.timestamp()+900,"iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(b"synthetic-test-secret-at-least-32-bytes")).unwrap()
}
async fn send(
    app: &axum::Router,
    route: admission::AdmissionRoute,
    user: Uuid,
    token: Option<&str>,
) -> (u16, Value) {
    let path = route.template().replace("{user_id}", &user.to_string());
    let mut request = axum::http::Request::builder()
        .method(route.method())
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(axum::body::Body::from("{}")).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), 100_000)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c11() {
    let f = Fixture::new().await;
    sqlx::query("UPDATE users SET status='APPROVED' WHERE id=$1")
        .bind(f.user)
        .execute(&f.pool)
        .await
        .unwrap();
    // A fellow member stays APPROVED on every route, so its refusals are stable.
    let member_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'BUYER','APPROVED')",
    )
    .bind(member_user)
    .bind(f.org)
    .execute(&f.pool)
    .await
    .unwrap();
    let app = app(&f, Default::default());
    let member = token(member_user);
    let admin = token(f.actor);
    for route in admission::AdmissionRoute::ALL {
        for credential in [None, Some("invalid"), Some(member.as_str())] {
            let present = send(&app, route, f.user, credential).await;
            let absent = send(&app, route, Uuid::new_v4(), credential).await;
            assert_eq!(present, absent);
            assert_eq!(
                present.0,
                if credential == Some(member.as_str()) {
                    403
                } else {
                    401
                }
            );
        }
        for _ in 0..60 {
            assert_eq!(send(&app, route, f.user, Some(&member)).await.0, 403);
        }
        let first = send(&app, route, f.user, Some(&admin)).await;
        assert_ne!(first.0, 429);
        assert_eq!(
            first.0,
            if matches!(route, admission::AdmissionRoute::AnalyticsReject) {
                409
            } else {
                200
            },
            "{route:?}: {first:?}"
        );
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t6_c12() {
    let f = Fixture::new().await;
    let first = f.approve().await.transition_id.unwrap();
    let (left, right) = tokio::join!(
        f.reject(RejectionRoute::Auth),
        f.reject(RejectionRoute::Analytics)
    );
    assert!(left.is_ok());
    if let Err(error) = right {
        assert_eq!(error.status.as_u16(), 409);
    }
    assert_eq!(f.count("user_status_transitions").await, 2);
    let new = f.approve().await.transition_id.unwrap();
    assert_ne!(first, new);
    f.finish().await;
}
