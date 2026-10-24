//! Phase 4 Task 4: administrator onboarding attention (N02) and audit read (N03).
//!
//! The stage classifier, the read caps and the query grammar run without a
//! database. The projection, ordering, truncation markers and authority order
//! need disposable PostgreSQL; each such case provisions its own fixture schema
//! and is `#[ignore]`d unless the explicit disposable authority is present, so
//! an offline `cargo test --workspace` stays green.
//!
//! The two components are compiled here from their plan paths, before the
//! integration owner publishes the `business` module declaration.

pub use verdaxis_read::*;

#[path = "../src/business/attention.rs"]
mod attention;
#[path = "../src/business/audit_query.rs"]
mod audit_query;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use std::{collections::BTreeMap, net::SocketAddr};
use tower::ServiceExt;
use uuid::Uuid;

use verdaxis_read::{auth::VerificationSettings, business::error::refusal};

const SECRET: &str = "operator-reads-test-secret-at-least-32-characters";
const ISSUER: &str = "operator-reads-test";
const FORWARDED: &str = "203.0.113.41";

fn verification() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), ISSUER.into())
}

fn token(user: Uuid) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({
            "sub": user,
            "type": "access",
            "iss": ISSUER,
            "aud": ISSUER,
            "jti": Uuid::new_v4(),
            "iat": Utc::now().timestamp() - 1,
            "exp": Utc::now().timestamp() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn config(url: String) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 20,
        configured_max_connections: 100,
    }
}

fn app(pool: sqlx::PgPool, url: String, limits: auth::limits::SharedLimits) -> Router {
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&url)
        .unwrap();
    let state = AppState::new(
        config(url),
        runtime::db::Pools { read: pool, health },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    Router::new()
        .merge(attention::router(
            state.clone(),
            verification(),
            limits.clone(),
        ))
        .merge(audit_query::router(state, verification(), limits))
}

/// The serving server always supplies the peer address. `oneshot` does not, so
/// the test inserts the loopback peer and the forwarded client that Caddy sends.
fn send(path: &str, bearer: Option<&str>, forwarded: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41_234))));
    request
        .headers_mut()
        .insert("x-forwarded-for", forwarded.parse().unwrap());
    if let Some(bearer) = bearer {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
    }
    request
}

async fn get(
    app: &Router,
    path: &str,
    bearer: Option<&str>,
    forwarded: &str,
) -> (StatusCode, Value, Option<String>) {
    let response = app
        .clone()
        .oneshot(send(path, bearer, forwarded))
        .await
        .unwrap();
    let status = response.status();
    let truncated = response
        .headers()
        .get("x-result-truncated")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1 << 22).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body, truncated)
}

// ---------------------------------------------------------------------------
// Python's stage classifier, without a database.
// ---------------------------------------------------------------------------

const NOW: (i32, u32, u32, u32, u32, u32) = (2026, 8, 4, 8, 0, 0);

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(NOW.0, NOW.1, NOW.2, NOW.3, NOW.4, NOW.5)
        .unwrap()
}

fn at(seconds: i64) -> DateTime<Utc> {
    now() + Duration::seconds(seconds)
}

#[derive(Clone)]
struct CandidateFixture {
    key: &'static str,
    email: &'static str,
    first_name: Option<&'static str>,
    last_name: Option<&'static str>,
    role: Option<&'static str>,
    created_at: DateTime<Utc>,
    email_verified: bool,
    verification_expires_at: Option<DateTime<Utc>>,
    user_status: Option<&'static str>,
    organization_name: Option<&'static str>,
    organization_status: Option<&'static str>,
    organization_provenance: Option<&'static str>,
    membership_status: Option<&'static str>,
    membership_reviewed_at: Option<DateTime<Utc>>,
    user_approved_at: Option<DateTime<Utc>>,
    organization_approved_at: Option<DateTime<Utc>>,
    last_login: Option<DateTime<Utc>>,
    pending_registration_expires_at: Option<DateTime<Utc>>,
}

impl CandidateFixture {
    /// The Python test helper's default: an approved, verified buyer whose
    /// final gate completed three hours ago and who has not yet logged in.
    fn approved() -> Self {
        Self {
            key: "user:1",
            email: "person@example.com",
            first_name: Some("Pat"),
            last_name: Some("Lee"),
            role: Some("BUYER"),
            created_at: at(-86_400),
            email_verified: true,
            verification_expires_at: None,
            user_status: Some("APPROVED"),
            organization_name: Some("Example Energy"),
            organization_status: Some("APPROVED"),
            organization_provenance: Some("REAL"),
            membership_status: Some("APPROVED"),
            membership_reviewed_at: Some(at(-3 * 3600)),
            user_approved_at: Some(at(-3 * 3600)),
            organization_approved_at: Some(at(-3 * 3600)),
            last_login: None,
            pending_registration_expires_at: None,
        }
    }
    fn candidate(&self) -> attention::Candidate {
        attention::Candidate {
            key: self.key.into(),
            email: self.email.into(),
            first_name: self.first_name.map(str::to_owned),
            last_name: self.last_name.map(str::to_owned),
            role: self.role.map(str::to_owned),
            created_at: self.created_at,
            email_verified: self.email_verified,
            verification_expires_at: self.verification_expires_at,
            user_status: self.user_status.map(str::to_owned),
            organization_name: self.organization_name.map(str::to_owned),
            organization_status: self.organization_status.map(str::to_owned),
            organization_provenance: self.organization_provenance.map(str::to_owned),
            membership_status: self.membership_status.map(str::to_owned),
            membership_reviewed_at: self.membership_reviewed_at,
            user_approved_at: self.user_approved_at,
            organization_approved_at: self.organization_approved_at,
            last_login: self.last_login,
            pending_registration_expires_at: self.pending_registration_expires_at,
        }
    }
}

fn pending_fixture() -> CandidateFixture {
    CandidateFixture {
        key: "pending:1",
        email: "pending@example.com",
        first_name: None,
        last_name: None,
        role: Some("BUYER"),
        created_at: at(-60),
        email_verified: false,
        verification_expires_at: None,
        user_status: None,
        organization_name: None,
        organization_status: None,
        organization_provenance: None,
        membership_status: None,
        membership_reviewed_at: None,
        user_approved_at: None,
        organization_approved_at: None,
        last_login: None,
        pending_registration_expires_at: Some(at(300)),
    }
}

fn stage(candidate: &CandidateFixture) -> Option<attention::Attention> {
    attention::classify(&candidate.candidate(), now())
}

#[test]
fn p4_t4_c50() {
    // A rejected party outranks every later stage, including an unverified,
    // stalled account, and the reported instant is the account creation time.
    for status in ["user_status", "organization_status", "membership_status"] {
        let mut candidate = CandidateFixture::approved();
        candidate.email_verified = false;
        match status {
            "user_status" => candidate.user_status = Some("REJECTED"),
            "organization_status" => candidate.organization_status = Some("REJECTED"),
            _ => candidate.membership_status = Some("REJECTED"),
        }
        let attention = stage(&candidate).expect("rejection is reported");
        assert_eq!(attention.stage, attention::AttentionStage::Rejected);
        assert_eq!(attention.since, candidate.created_at);
    }
}

#[test]
fn p4_t4_c51() {
    // A pending registration alerts only inside the five-minute setup warning;
    // past it there is still time, and the reported instant is its expiry.
    let mut candidate = pending_fixture();
    let attention = stage(&candidate).expect("expiring setup is reported");
    assert_eq!(
        attention.stage,
        attention::AttentionStage::OrganizationSetupExpiring
    );
    assert_eq!(attention.since, at(300));

    candidate.pending_registration_expires_at = Some(at(301));
    assert_eq!(stage(&candidate), None);
    candidate.pending_registration_expires_at = Some(at(-1));
    assert_eq!(
        stage(&candidate).unwrap().stage,
        attention::AttentionStage::OrganizationSetupExpiring
    );
}

#[test]
fn p4_t4_c52() {
    // An unverified account stalls after its own verification expiry, or after
    // 24 hours from creation when the token expiry is absent.
    let mut candidate = CandidateFixture::approved();
    candidate.email_verified = false;
    candidate.verification_expires_at = Some(at(-1));
    let attention = stage(&candidate).expect("stalled verification is reported");
    assert_eq!(
        attention.stage,
        attention::AttentionStage::VerificationStalled
    );
    assert_eq!(attention.since, at(-1));

    candidate.verification_expires_at = Some(now());
    assert_eq!(
        stage(&candidate).unwrap().stage,
        attention::AttentionStage::VerificationStalled
    );

    candidate.verification_expires_at = Some(at(1));
    assert_eq!(stage(&candidate), None);

    candidate.verification_expires_at = None;
    candidate.created_at = at(-86_400);
    assert_eq!(
        stage(&candidate).unwrap().stage,
        attention::AttentionStage::VerificationStalled
    );
    assert_eq!(stage(&candidate).unwrap().since, now());

    candidate.created_at = at(-86_400 + 1);
    assert_eq!(stage(&candidate), None);
}

#[test]
fn p4_t4_c53() {
    // A verified account needs all three approvals; each missing one reports
    // approval_required at creation time.
    for missing in ["user_status", "organization_status", "membership_status"] {
        let mut candidate = CandidateFixture::approved();
        match missing {
            "user_status" => candidate.user_status = Some("PENDING"),
            "organization_status" => candidate.organization_status = Some("PENDING"),
            _ => candidate.membership_status = None,
        }
        let attention = stage(&candidate).expect("an open gate is reported");
        assert_eq!(attention.stage, attention::AttentionStage::ApprovalRequired);
        assert_eq!(attention.since, candidate.created_at);
    }
    // A missing organization row is an open organization gate.
    let mut candidate = CandidateFixture::approved();
    candidate.organization_name = None;
    candidate.organization_status = None;
    candidate.organization_provenance = None;
    assert_eq!(
        stage(&candidate).unwrap().stage,
        attention::AttentionStage::ApprovalRequired
    );
}

#[test]
fn p4_t4_c54() {
    // The first-login clock starts at the latest gate, and the two-hour
    // boundary is inclusive.
    let mut candidate = CandidateFixture::approved();
    candidate.membership_reviewed_at = Some(at(-3600 - 3540));
    assert_eq!(stage(&candidate), None);
    candidate.membership_reviewed_at = Some(at(-7200));
    let attention = stage(&candidate).expect("two hours without a login");
    assert_eq!(
        attention.stage,
        attention::AttentionStage::FirstLoginOverdue
    );
    assert_eq!(attention.since, at(-7200));

    // The latest of the four gate instants is the one that counts.
    candidate.membership_reviewed_at = Some(at(-7200));
    candidate.user_approved_at = Some(at(-3600));
    assert_eq!(stage(&candidate), None);
}

#[test]
fn p4_t4_c55() {
    let mut candidate = CandidateFixture::approved();
    candidate.last_login = Some(at(-60));
    assert_eq!(stage(&candidate), None);
}

#[test]
fn p4_t4_c56() {
    let mut admin = CandidateFixture::approved();
    admin.role = Some("ADMIN");
    assert_eq!(stage(&admin), None);

    let mut canary = CandidateFixture::approved();
    canary.email = "canary+prod@prod.canary.verdaxis.exchange";
    assert_eq!(stage(&canary), None);

    for provenance in ["DEMO", "TEST", "CANARY"] {
        let mut candidate = CandidateFixture::approved();
        candidate.organization_provenance = Some(provenance);
        assert_eq!(stage(&candidate), None);
    }
    // Only the canary shape is excluded; an ordinary plus-address is not.
    let mut ordinary = CandidateFixture::approved();
    ordinary.email = "canary+prod@example.com";
    assert!(stage(&ordinary).is_some());
}

#[test]
fn p4_t4_c57() {
    let candidate = CandidateFixture::approved();
    let attention = stage(&candidate).expect("first login overdue");
    let message = attention::format_message(&candidate.candidate(), &attention, now());
    assert!(message.contains("Pat Lee · person@example.com"));
    assert!(message.contains("Organization: Example Energy"));
    assert!(message.contains("First login overdue"));
    assert!(message.contains("Timing: 3h 0m ago"));
    assert!(!message.to_lowercase().contains("token"));
    assert!(!message.to_lowercase().contains("hash"));
    assert!(!message.to_lowercase().contains("ip address"));

    // The response name drops empty parts, exactly as Python's truthiness
    // filter does; a pair with no content at all reports null.
    assert_eq!(attention::joined_name(Some(""), None), None);
    assert_eq!(
        attention::joined_name(None, Some("Lee")),
        Some("Lee".into())
    );
    assert_eq!(
        attention::joined_name(Some("Pat"), Some("Lee")),
        Some("Pat Lee".into())
    );

    // A whitespace-only part is dropped by the message's own rule, so the
    // message falls back to the email rather than printing blank space.
    let mut fallback = CandidateFixture::approved();
    fallback.email = "fallback@example.com";
    fallback.first_name = Some("  ");
    fallback.last_name = Some("");
    let attention = stage(&fallback).expect("first login overdue");
    let message = attention::format_message(&fallback.candidate(), &attention, now());
    assert!(message.contains("fallback@example.com · fallback@example.com"));
    assert!(!message.contains("  ·"));
    // The response name keeps Python's truthiness-only test, so the same
    // whitespace part stays in it.
    assert_eq!(
        attention::joined_name(Some("  "), Some("")),
        Some("  ".into())
    );
}

#[test]
fn p4_t4_c05() {
    // N03's source has only an upper bound; the pinned contract makes a
    // negative or zero limit a request refusal, never a PostgreSQL error.
    for limit in ["-1", "0", "201"] {
        let error = audit_query::AuditQuery::from_pairs(&[("limit", limit)]).unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(error.detail.is_array());
    }
    assert_eq!(
        audit_query::AuditQuery::from_pairs(&[]).unwrap().limit(),
        50
    );
    assert_eq!(
        audit_query::AuditQuery::from_pairs(&[("limit", "1")])
            .unwrap()
            .limit(),
        1
    );
    assert_eq!(
        audit_query::AuditQuery::from_pairs(&[("limit", "200")])
            .unwrap()
            .limit(),
        200
    );
    // The offset cap is an explicit contract change: deeper scans are refused.
    for offset in ["-1", "10001"] {
        let error = audit_query::AuditQuery::from_pairs(&[("offset", offset)]).unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(
        audit_query::AuditQuery::from_pairs(&[("offset", "10000")])
            .unwrap()
            .offset(),
        10000
    );
    // An empty filter is no filter, as in Python's truthiness test.
    let query =
        audit_query::AuditQuery::from_pairs(&[("action", ""), ("resource_type", "")]).unwrap();
    assert!(query.action().is_none());
    assert!(query.resource_type().is_none());
    // Identity filters parse before SQL.
    let error = audit_query::AuditQuery::from_pairs(&[("user_id", "not-a-uuid")]).unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    let id = Uuid::new_v4();
    let query = audit_query::AuditQuery::from_pairs(&[("user_id", &id.to_string())]).unwrap();
    assert_eq!(query.user_id(), Some(id));

    // N02's cap: default 100, 1..=500, offset 0..=10000.
    assert_eq!(
        attention::AttentionQuery::from_pairs(&[]).unwrap().limit(),
        100
    );
    for limit in ["0", "501"] {
        assert_eq!(
            attention::AttentionQuery::from_pairs(&[("limit", limit)])
                .unwrap_err()
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        attention::AttentionQuery::from_pairs(&[("limit", "500")])
            .unwrap()
            .limit(),
        500
    );
    assert_eq!(
        attention::AttentionQuery::from_pairs(&[]).unwrap().offset(),
        0
    );
    assert_eq!(
        attention::AttentionQuery::from_pairs(&[("offset", "10000")])
            .unwrap()
            .offset(),
        10_000
    );
    assert_eq!(
        attention::AttentionQuery::from_pairs(&[("offset", "10001")])
            .unwrap_err()
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // The shared refusal envelope stays the structured detail these routes
    // return for authority and quota refusals.
    let error = refusal(StatusCode::FORBIDDEN, "ADMIN_REQUIRED", "x");
    assert_eq!(error.detail["code"], "ADMIN_REQUIRED");
    assert!(error.detail["message"].is_string());
}

#[tokio::test]
async fn p4_t4_c07() {
    // The component owns exactly its two GET paths, and each refuses an
    // anonymous caller instead of falling through to 404.
    let url = "postgres://offline_app:offline_password@127.0.0.1:1/offline_test";
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(url)
        .unwrap();
    let state = AppState::new(
        config(url.into()),
        runtime::db::Pools {
            read: pool,
            health: PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy(url)
                .unwrap(),
        },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let component = Router::new()
        .merge(attention::router(
            state.clone(),
            verification(),
            auth::limits::SharedLimits::default(),
        ))
        .merge(audit_query::router(
            state.clone(),
            verification(),
            auth::limits::SharedLimits::default(),
        ));
    for path in ["/api/admin/onboarding-attention", "/api/admin/audit-logs"] {
        let response = component
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    // The serving router is unchanged: its health and public routes stay
    // mounted, and neither component path is served without this component.
    let serving = runtime::http::public_router(state);
    for (path, status) in [
        ("/health/live", StatusCode::OK),
        ("/api/admin/onboarding-attention", StatusCode::NOT_FOUND),
        ("/api/admin/audit-logs", StatusCode::NOT_FOUND),
    ] {
        let response = serving
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
    }
}

// ---------------------------------------------------------------------------
// The disposable database projection.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct OrgSpec {
    name: &'static str,
    verification_status: &'static str,
    provenance: &'static str,
    country_code: Option<&'static str>,
    tax_id: Option<&'static str>,
}

#[derive(Clone)]
struct Spec {
    email: String,
    role: Option<&'static str>,
    status: &'static str,
    first_name: Option<&'static str>,
    last_name: Option<&'static str>,
    created_ago: i64,
    email_verified: bool,
    verification_expires_ago: Option<i64>,
    organization: Option<OrgSpec>,
    membership_status: Option<&'static str>,
    membership_reviewed_ago: Option<i64>,
    user_approved_ago: Option<i64>,
    organization_approved_ago: Option<i64>,
    organization_audit_action: &'static str,
    last_login_ago: Option<i64>,
}

impl Spec {
    fn new(email: &str) -> Self {
        Self {
            email: email.into(),
            role: Some("BUYER"),
            status: "APPROVED",
            first_name: Some("Operator"),
            last_name: Some("Case"),
            created_ago: 86_400,
            email_verified: true,
            verification_expires_ago: None,
            organization: Some(OrgSpec {
                name: "Synthetic Energy",
                verification_status: "APPROVED",
                provenance: "REAL",
                country_code: Some("NL"),
                tax_id: Some("NL000000000B01"),
            }),
            membership_status: Some("APPROVED"),
            membership_reviewed_ago: Some(3 * 3600),
            user_approved_ago: Some(3 * 3600),
            organization_approved_ago: Some(3 * 3600),
            organization_audit_action: "admin.organization_approved",
            last_login_ago: None,
        }
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    now: DateTime<Utc>,
    url: String,
}

impl Fixture {
    async fn new() -> Self {
        let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator required");
        let runtime =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime required");
        let m = url::Url::parse(&migrator).unwrap();
        let r = url::Url::parse(&runtime).unwrap();
        assert_eq!(m.host_str(), Some("127.0.0.1"));
        assert_eq!(r.host_str(), m.host_str());
        assert_eq!(r.port(), m.port());
        assert_eq!(r.path(), m.path());
        assert!(r.path().ends_with("_test"));
        assert_eq!(r.username(), "rust_app");
        assert_eq!(m.username(), "rust_migrator");
        assert_ne!(r.port_or_known_default(), Some(5432));
        let mut admin = PgConnection::connect(&migrator).await.unwrap();
        let schema = format!("p4_operator_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        // The production column shape of the facts these two reads touch.
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY,name text,domain text,type text,tax_id text,country_code text,\
             verification_status text NOT NULL DEFAULT 'PENDING',provenance text NOT NULL DEFAULT 'UNKNOWN');\
             CREATE TABLE users(id uuid PRIMARY KEY,email text NOT NULL,first_name text,last_name text,role text,status text NOT NULL DEFAULT 'PENDING',\
             organization_id uuid,email_verified boolean NOT NULL DEFAULT false,email_verification_token_expires_at timestamptz,last_login timestamptz,\
             password_changed_at timestamptz,authentication_revoked_at timestamptz,must_change_password boolean NOT NULL DEFAULT false,\
             created_at timestamptz NOT NULL DEFAULT now());\
             CREATE TABLE organization_join_requests(id uuid PRIMARY KEY,user_id uuid NOT NULL,organization_id uuid NOT NULL,status text NOT NULL DEFAULT 'PENDING',\
             reviewed_by uuid,reviewed_at timestamptz,review_note text,created_at timestamptz NOT NULL DEFAULT now());\
             CREATE TABLE pending_registrations(id uuid PRIMARY KEY,token_hash text NOT NULL DEFAULT '',email text NOT NULL,password_hash text NOT NULL DEFAULT '',\
             first_name text,last_name text,role text,referral_code text,expires_at timestamptz NOT NULL,used_at timestamptz,\
             created_at timestamptz NOT NULL DEFAULT now());\
             CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100) NOT NULL,resource_type varchar(50) NOT NULL,resource_id varchar(100),\
             changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz NOT NULL DEFAULT now());\
             CREATE TABLE user_status_transitions(id uuid PRIMARY KEY,user_id uuid NOT NULL,organization_id uuid,role text,from_status text,to_status text NOT NULL,\
             effective_at timestamptz NOT NULL DEFAULT now(),provenance text NOT NULL DEFAULT 'workflow');\
             CREATE INDEX ix_audit_logs_action ON audit_logs(action);\
             CREATE INDEX ix_audit_logs_timestamp ON audit_logs(timestamp)",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app;\
             GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app;\
             GRANT UPDATE ON users TO rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let sql = search_path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            schema,
            now: Utc::now(),
            url: runtime,
        }
    }

    /// The disposable runtime URL, for the config the component needs.
    fn url(&self) -> String {
        self.url.clone()
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

    fn at(&self, ago: i64) -> DateTime<Utc> {
        self.now - Duration::seconds(ago)
    }

    async fn actor(&mut self, role: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users(id,email,role,status,email_verified,must_change_password,created_at) \
             VALUES($1,$2,$3,'APPROVED',true,false,$4)",
        )
        .bind(id)
        .bind(format!("{id}@example.test"))
        .bind(role)
        .bind(self.now)
        .execute(&mut self.admin)
        .await
        .unwrap();
        id
    }

    /// Insert one candidate exactly as the Python classifier receives it and
    /// return the same values the projection should report.
    async fn seed(&mut self, spec: &Spec) -> (String, attention::Candidate) {
        let id = Uuid::new_v4();
        let organization = match &spec.organization {
            Some(org) => {
                let organization = Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO organizations(id,name,verification_status,provenance,country_code,tax_id) VALUES($1,$2,$3,$4,$5,$6)",
                )
                .bind(organization)
                .bind(org.name)
                .bind(org.verification_status)
                .bind(org.provenance)
                .bind(org.country_code)
                .bind(org.tax_id)
                .execute(&mut self.admin)
                .await
                .unwrap();
                Some(organization)
            }
            None => None,
        };
        let created_at = self.at(spec.created_ago);
        let verification_expires_at = spec.verification_expires_ago.map(|ago| self.at(ago));
        sqlx::query(
            "INSERT INTO users(id,email,first_name,last_name,role,status,organization_id,email_verified,\
             email_verification_token_expires_at,last_login,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(id)
        .bind(&spec.email)
        .bind(spec.first_name)
        .bind(spec.last_name)
        .bind(spec.role)
        .bind(spec.status)
        .bind(organization)
        .bind(spec.email_verified)
        .bind(verification_expires_at)
        .bind(spec.last_login_ago.map(|ago| self.at(ago)))
        .bind(created_at)
        .execute(&mut self.admin)
        .await
        .unwrap();
        let mut membership_reviewed_at = None;
        if let (Some(status), Some(organization)) = (spec.membership_status, organization) {
            let reviewed_at = spec.membership_reviewed_ago.map(|ago| self.at(ago));
            membership_reviewed_at = reviewed_at;
            sqlx::query(
                "INSERT INTO organization_join_requests(id,user_id,organization_id,status,reviewed_at,created_at) VALUES($1,$2,$3,$4,$5,$6)",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(organization)
            .bind(status)
            .bind(reviewed_at)
            .bind(created_at)
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
        let user_approved_at = spec.user_approved_ago.map(|ago| self.at(ago));
        if let Some(effective_at) = user_approved_at {
            sqlx::query(
                "INSERT INTO user_status_transitions(id,user_id,organization_id,role,from_status,to_status,effective_at,provenance) \
                 VALUES($1,$2,$3,$4,'PENDING','APPROVED',$5,'workflow')",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(organization)
            .bind(spec.role)
            .bind(effective_at)
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
        let organization_approved_at = spec.organization_approved_ago.map(|ago| self.at(ago));
        if let (Some(timestamp), Some(organization)) = (organization_approved_at, organization) {
            sqlx::query(
                "INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,changes,ip_address,request_id,timestamp) \
                 VALUES($1,$2,$3,'organization',$4,'{\"status\":{\"to\":\"APPROVED\"}}',$5,$6,$7)",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(spec.organization_audit_action)
            .bind(organization.to_string())
            .bind("203.0.113.10")
            .bind("correlation-1")
            .bind(timestamp)
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
        let candidate = attention::Candidate {
            key: format!("user:{id}"),
            email: spec.email.clone(),
            first_name: spec.first_name.map(str::to_owned),
            last_name: spec.last_name.map(str::to_owned),
            role: spec.role.map(str::to_owned),
            created_at,
            email_verified: spec.email_verified,
            verification_expires_at,
            user_status: Some(spec.status.to_owned()),
            organization_name: spec.organization.as_ref().map(|org| org.name.to_owned()),
            organization_status: spec
                .organization
                .as_ref()
                .map(|org| org.verification_status.to_owned()),
            organization_provenance: spec
                .organization
                .as_ref()
                .map(|org| org.provenance.to_owned()),
            membership_status: spec.membership_status.map(str::to_owned),
            membership_reviewed_at,
            user_approved_at,
            organization_approved_at,
            last_login: spec.last_login_ago.map(|ago| self.at(ago)),
            pending_registration_expires_at: None,
        };
        (spec.email.clone(), candidate)
    }

    async fn seed_pending(
        &mut self,
        email: &str,
        role: Option<&str>,
        expires_at: DateTime<Utc>,
        used: bool,
        created_at: DateTime<Utc>,
    ) -> attention::Candidate {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO pending_registrations(id,email,role,expires_at,used_at,created_at) VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(id)
        .bind(email)
        .bind(role)
        .bind(expires_at)
        .bind(used.then_some(created_at))
        .bind(created_at)
        .execute(&mut self.admin)
        .await
        .unwrap();
        attention::Candidate {
            key: format!("pending:{id}"),
            email: email.into(),
            first_name: None,
            last_name: None,
            role: role.map(str::to_owned),
            created_at,
            email_verified: false,
            verification_expires_at: None,
            user_status: None,
            organization_name: None,
            organization_status: None,
            organization_provenance: None,
            membership_status: None,
            membership_reviewed_at: None,
            user_approved_at: None,
            organization_approved_at: None,
            last_login: None,
            pending_registration_expires_at: Some(expires_at),
        }
    }

    async fn insert_audit(
        &mut self,
        actor: Option<Uuid>,
        action: &str,
        resource_type: &str,
        resource_id: Option<&str>,
        changes: Option<Value>,
        timestamp: DateTime<Utc>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,changes,ip_address,request_id,timestamp) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(id)
        .bind(actor)
        .bind(action)
        .bind(resource_type)
        .bind(resource_id)
        .bind(changes)
        .bind("203.0.113.9")
        .bind("request-1")
        .bind(timestamp)
        .execute(&mut self.admin)
        .await
        .unwrap();
        id
    }
}

fn limits() -> auth::limits::SharedLimits {
    auth::limits::SharedLimits::default()
}

/// Compare the projection with the pure classifier over the same specs.
async fn assert_parity(
    fixture: &mut Fixture,
    seeds: &[Spec],
    pending: &[(String, attention::Candidate)],
) {
    let mut expected: BTreeMap<String, (String, DateTime<Utc>)> = BTreeMap::new();
    for spec in seeds {
        let (email, candidate) = fixture.seed(spec).await;
        if let Some(attention) = attention::classify(&candidate, fixture.now) {
            expected.insert(
                email,
                (attention.stage.as_str().to_owned(), attention.since),
            );
        }
    }
    for (email, candidate) in pending {
        if let Some(attention) = attention::classify(candidate, fixture.now) {
            expected.insert(
                email.clone(),
                (attention.stage.as_str().to_owned(), attention.since),
            );
        }
    }
    let app = app(fixture.pool.clone(), fixture.url(), limits());
    let actor = fixture.actor("ADMIN").await;
    let (status, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention?limit=500",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(truncated.as_deref(), Some("false"));
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), expected.len(), "{body}");
    for item in items {
        let email = item["email"].as_str().unwrap().to_owned();
        let (stage, since) = expected
            .get(&email)
            .unwrap_or_else(|| panic!("unexpected candidate {email}"));
        assert_eq!(item["stage"].as_str(), Some(stage.as_str()), "{email}");
        assert_eq!(
            serde_json::from_value::<DateTime<Utc>>(item["since"].clone()).unwrap(),
            *since,
            "{email}"
        );
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t4_c03() {
    let mut fixture = Fixture::new().await;
    let mut boundary_run = Spec::new("run@example.test");
    boundary_run.membership_status = Some("PENDING");
    let mut admin = Spec::new("admin@example.test");
    admin.role = Some("ADMIN");
    let mut rejected = Spec::new("rejected@example.test");
    rejected.status = "REJECTED";

    let mut pending_expiring = Spec::new("pending-expiring@example.test");
    pending_expiring.organization = None;
    pending_expiring.membership_status = None;
    pending_expiring.membership_reviewed_ago = None;
    pending_expiring.status = "PENDING";
    pending_expiring.email_verified = false;
    pending_expiring.created_ago = 60;
    let mut pending_later = pending_expiring.clone();
    pending_later.email = "pending-later@example.test".into();

    let mut stalled_exact = Spec::new("stalled-exact@example.test");
    stalled_exact.email_verified = false;
    stalled_exact.verification_expires_ago = Some(0);
    let mut stalled_none = Spec::new("stalled-none@example.test");
    stalled_none.email_verified = false;
    stalled_none.created_ago = 86_400;

    let mut no_approval = Spec::new("no-approval@example.test");
    no_approval.user_approved_ago = None;
    no_approval.membership_status = None;
    no_approval.membership_reviewed_ago = None;
    let mut no_org = Spec::new("no-org@example.test");
    no_org.organization = None;
    no_org.membership_status = None;
    no_org.membership_reviewed_ago = None;

    let mut boundary_before = Spec::new("boundary-before@example.test");
    boundary_before.membership_reviewed_ago = Some(7000);
    let mut boundary_at = Spec::new("boundary-at@example.test");
    boundary_at.membership_reviewed_ago = Some(7200);
    let mut logged_in = Spec::new("logged-in@example.test");
    logged_in.last_login_ago = Some(60);

    // Exclusion cases carry the real action name; the SQL must drop them.
    let mut canary = Spec::new("canary+prod@prod.canary.verdaxis.exchange");
    canary.status = "APPROVED";
    canary.membership_status = Some("APPROVED");
    let mut demo = Spec::new("demo@example.test");
    demo.organization = Some(OrgSpec {
        name: "Demo Org",
        verification_status: "APPROVED",
        provenance: "DEMO",
        country_code: None,
        tax_id: None,
    });
    let mut test = Spec::new("test@example.test");
    test.organization = Some(OrgSpec {
        name: "Test Org",
        verification_status: "APPROVED",
        provenance: "TEST",
        country_code: None,
        tax_id: None,
    });
    let mut canary_org = Spec::new("canary-org@example.test");
    canary_org.organization = Some(OrgSpec {
        name: "Canary Org",
        verification_status: "APPROVED",
        provenance: "CANARY",
        country_code: None,
        tax_id: None,
    });

    // Two accounts with the same facts, differing only in the stored audit
    // action spelling. The real dotted action is the approval gate (one hour
    // old, no stage); the underscore spelling is not, so only creation remains
    // and the account is overdue. This pair cannot use the parity helper,
    // because the pure classifier is fed the derived instant, not the action.
    let mut wrong_audit = Spec::new("wrong-audit@example.test");
    wrong_audit.organization_audit_action = "admin_organization_approved";
    wrong_audit.user_approved_ago = None;
    wrong_audit.membership_reviewed_ago = None;
    wrong_audit.organization_approved_ago = Some(3600);
    let mut right_audit = wrong_audit.clone();
    right_audit.email = "right-audit@example.test".into();
    right_audit.organization_audit_action = "admin.organization_approved";

    let seeds = [
        boundary_run,
        admin,
        rejected,
        pending_expiring.clone(),
        pending_later.clone(),
        stalled_exact,
        stalled_none,
        no_approval,
        no_org,
        boundary_before,
        boundary_at,
        logged_in,
        canary,
        demo,
        test,
        canary_org,
    ];
    fixture.seed(&wrong_audit).await;
    fixture.seed(&right_audit).await;
    // The pending rows live outside the user projection and are classified
    // with the same rules.
    let mut expected_pending = Vec::new();
    for (email, expires_at, used) in [
        (
            pending_expiring.email.clone(),
            fixture.now + Duration::minutes(5),
            false,
        ),
        (
            pending_later.email.clone(),
            fixture.now + Duration::hours(1),
            false,
        ),
        (
            "used-pending@example.test".to_owned(),
            fixture.now + Duration::minutes(5),
            true,
        ),
    ] {
        let candidate = fixture
            .seed_pending(
                &email,
                Some("BUYER"),
                expires_at,
                used,
                fixture.now - Duration::seconds(60),
            )
            .await;
        expected_pending.push((email, candidate));
    }

    assert_parity(&mut fixture, &seeds, &expected_pending).await;

    // The action-spelling pair, read back through the route.
    let app = app(fixture.pool.clone(), fixture.url(), limits());
    let actor = fixture.actor("ADMIN").await;
    let (status, body, _) = get(
        &app,
        "/api/admin/onboarding-attention?limit=500",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    let wrong = items
        .iter()
        .find(|item| item["email"] == "wrong-audit@example.test")
        .expect("the underscore action is not an approval gate");
    assert_eq!(wrong["stage"], "first_login_overdue");
    assert!(
        !items
            .iter()
            .any(|item| item["email"] == "right-audit@example.test"),
        "the dotted action is the approval gate"
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t4_c04() {
    let mut fixture = Fixture::new().await;
    let actor = fixture.actor("ADMIN").await;
    let mut excluded = Vec::new();
    let mut admin = Spec::new("excluded-admin@example.test");
    admin.role = Some("ADMIN");
    excluded.push(admin);
    let mut canary = Spec::new("canary+prod@prod.canary.verdaxis.exchange");
    canary.membership_status = Some("APPROVED");
    excluded.push(canary);
    for (name, provenance) in [
        ("excluded-demo@example.test", "DEMO"),
        ("excluded-test@example.test", "TEST"),
        ("excluded-canary@example.test", "CANARY"),
    ] {
        let mut spec = Spec::new(name);
        spec.organization = Some(OrgSpec {
            name: "Excluded",
            verification_status: "APPROVED",
            provenance,
            country_code: None,
            tax_id: None,
        });
        excluded.push(spec);
    }
    for spec in &excluded {
        fixture.seed(spec).await;
    }
    let eligible = Spec::new("eligible@example.test");
    fixture.seed(&eligible).await;

    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role <> 'ADMIN'")
        .fetch_one(&mut fixture.admin)
        .await
        .unwrap();
    assert_eq!(users, 5, "the excluded candidate rows exist");
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&mut fixture.admin)
        .await
        .unwrap();
    assert_eq!(audits, 6);

    let app = app(fixture.pool.clone(), fixture.url(), limits());
    let (status, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(truncated.as_deref(), Some("false"));
    let emails: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["email"].as_str().unwrap())
        .collect();
    assert_eq!(emails, ["eligible@example.test"]);

    // The audit read still returns excluded actors' rows: N03 is a different
    // surface from N02's classifier.
    let (status, body, _) = get(
        &app,
        "/api/admin/audit-logs?resource_type=organization",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 6);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t4_c06() {
    let mut fixture = Fixture::new().await;
    let admin = fixture.actor("ADMIN").await;
    let buyer = fixture.actor("BUYER").await;
    let supplier = fixture.actor("SUPPLIER").await;
    fixture.seed(&Spec::new("visible@example.test")).await;

    let shared = limits();
    let app = app(fixture.pool.clone(), fixture.url(), shared.clone());
    // Wrong-role and non-admin callers get the same refusal on every path
    // shape, including an unrelated path that shares no route state.
    for bearer in [buyer, supplier] {
        let mut first_refusal = None;
        for path in [
            "/api/admin/onboarding-attention",
            "/api/admin/onboarding-attention?offset=10000",
            "/api/admin/audit-logs",
            "/api/admin/audit-logs?action=admin.user_rejected",
        ] {
            let result = get(&app, path, Some(&token(bearer)), FORWARDED).await;
            assert_eq!(result.0, StatusCode::FORBIDDEN, "{path}");
            assert_eq!(result.1["detail"]["code"], "ADMIN_REQUIRED");
            if let Some(expected) = &first_refusal {
                assert_eq!(&result.1, expected);
            }
            first_refusal = Some(result.1);
        }
    }
    // Anonymous callers are refused before any state is read.
    for path in ["/api/admin/onboarding-attention", "/api/admin/audit-logs"] {
        let (status, body, _) = get(&app, path, None, FORWARDED).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["detail"]["code"], "INVALID_CREDENTIALS");
    }

    // Sixty refused attempts from another caller leave the administrator's
    // sixty-request N02 window intact, and N03 is not limited at all.
    let other = "198.51.100.200";
    for _ in 0..60 {
        let _ = get(
            &app,
            "/api/admin/onboarding-attention",
            Some(&token(buyer)),
            other,
        )
        .await;
    }
    for index in 0..60 {
        let (status, _, _) = get(
            &app,
            "/api/admin/onboarding-attention",
            Some(&token(admin)),
            FORWARDED,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "admitted request {index}");
    }
    let (status, body, _) = get(
        &app,
        "/api/admin/onboarding-attention",
        Some(&token(admin)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["detail"]["code"], "RATE_LIMITED");

    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t4_c01() {
    let mut fixture = Fixture::new().await;
    let actor = fixture.actor("ADMIN").await;
    let base = fixture.now - Duration::days(1);
    let count = 501usize;
    let ids: Vec<Uuid> = (0..count)
        .map(|index| Uuid::from_u128(0x1000 + index as u128))
        .collect();
    let organizations: Vec<Uuid> = (0..count)
        .map(|index| Uuid::from_u128(0x5000 + index as u128))
        .collect();
    sqlx::query(
        "INSERT INTO organizations(id,name,verification_status,provenance) \
         SELECT id,'Synthetic Org','APPROVED','REAL' FROM unnest($1::uuid[]) AS id",
    )
    .bind(&organizations)
    .execute(&mut fixture.admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users(id,email,role,status,organization_id,email_verified,created_at) \
         SELECT u.id,'candidate-'||u.ord||'@example.test','BUYER','APPROVED',u.organization,true,\
                $2::timestamptz + ((u.ord - 1) / 2) * interval '1 second' \
         FROM unnest($1::uuid[],$3::uuid[]) WITH ORDINALITY AS u(id,organization,ord)",
    )
    .bind(&ids)
    .bind(base)
    .bind(&organizations)
    .execute(&mut fixture.admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO organization_join_requests(id,user_id,organization_id,status,reviewed_at,created_at) \
         SELECT gen_random_uuid(),id,organization_id,'APPROVED',created_at,created_at FROM users \
         WHERE organization_id IS NOT NULL",
    )
    .execute(&mut fixture.admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_status_transitions(id,user_id,organization_id,role,from_status,to_status,effective_at,provenance) \
         SELECT gen_random_uuid(),id,organization_id,'BUYER','PENDING','APPROVED',created_at,'workflow' FROM users",
    )
    .execute(&mut fixture.admin)
    .await
    .unwrap();
    // A healthy user is a candidate row but never an attention item.
    let mut healthy = Spec::new("healthy@example.test");
    healthy.created_ago = 86_400;
    healthy.last_login_ago = Some(60);
    fixture.seed(&healthy).await;

    let app = app(fixture.pool.clone(), fixture.url(), limits());
    let ordered: Vec<(DateTime<Utc>, String)> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                base + Duration::seconds((index / 2) as i64),
                format!("user:{id}"),
            )
        })
        .collect();
    let mut ordered = ordered;
    ordered.sort();
    let emails: Vec<String> = ordered
        .iter()
        .map(|(_, key)| {
            let id = key.strip_prefix("user:").unwrap();
            let index = ids
                .iter()
                .position(|candidate| candidate.to_string() == id)
                .unwrap();
            format!("candidate-{}@example.test", index + 1)
        })
        .collect();

    let (status, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(truncated.as_deref(), Some("true"));
    assert_eq!(body["truncated"], Value::Bool(true));
    let page: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["email"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page.len(), 100);
    assert_eq!(page[..], emails[..100]);

    // The maximum page, an offset past the first page, and the last page.
    let (_, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention?limit=500",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(truncated.as_deref(), Some("true"));
    assert_eq!(body["truncated"], Value::Bool(true));
    assert_eq!(body["items"].as_array().unwrap().len(), 500);

    let (_, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention?limit=500&offset=400",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(truncated.as_deref(), Some("false"));
    let tail: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["email"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tail, emails[400..]);

    let (_, body, truncated) = get(
        &app,
        "/api/admin/onboarding-attention?offset=501",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(truncated.as_deref(), Some("false"));
    assert!(body["items"].as_array().unwrap().is_empty());

    // Both marker spellings carry the same boolean, and `generated_at` is the
    // server's classification instant.
    let generated: DateTime<Utc> = serde_json::from_value(body["generated_at"].clone()).unwrap();
    assert!((generated - fixture.now).num_seconds().abs() < 60);

    let (status, body, _) = get(
        &app,
        "/api/admin/onboarding-attention?limit=0",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["detail"].is_array());
    let (status, _, _) = get(
        &app,
        "/api/admin/onboarding-attention?offset=10001",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t4_c02() {
    let mut fixture = Fixture::new().await;
    let actor = fixture.actor("ADMIN").await;
    let first = fixture.now - Duration::hours(2);
    let second = first + Duration::minutes(1);
    let third = second + Duration::minutes(1);
    let first_id = fixture
        .insert_audit(
            Some(actor),
            "kyc.approved",
            "user",
            Some(&Uuid::new_v4().to_string()),
            Some(json!({"status":{"from":"SUBMITTED","to":"APPROVED"}})),
            first,
        )
        .await;
    let second_id = fixture
        .insert_audit(
            None,
            "admin.organization_approved",
            "organization",
            None,
            None,
            second,
        )
        .await;
    let third_id = fixture
        .insert_audit(
            Some(actor),
            "kyc.rejected",
            "user",
            Some("not-a-uuid"),
            Some(json!({"reason":"x"})),
            third,
        )
        .await;

    let app = app(fixture.pool.clone(), fixture.url(), limits());
    let (status, body, truncated) = get(
        &app,
        "/api/admin/audit-logs",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(truncated, None, "N03 has no truncation header");
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    // Descending timestamp, so the newest row is first.
    assert_eq!(rows[0]["id"], third_id.to_string());
    assert_eq!(rows[1]["id"], second_id.to_string());
    assert_eq!(rows[2]["id"], first_id.to_string());
    // Every Python field is present with its Python null-ness.
    let newest = &rows[0];
    assert_eq!(newest["user_id"], actor.to_string());
    assert_eq!(newest["action"], "kyc.rejected");
    assert_eq!(newest["resource_type"], "user");
    assert_eq!(newest["resource_id"], "not-a-uuid");
    assert_eq!(newest["changes"], json!({"reason":"x"}));
    assert_eq!(newest["ip_address"], "203.0.113.9");
    assert_eq!(newest["request_id"], "request-1");
    assert_eq!(
        serde_json::from_value::<DateTime<Utc>>(newest["timestamp"].clone()).unwrap(),
        third
    );
    let middle = &rows[1];
    assert_eq!(middle["user_id"], Value::Null);
    assert_eq!(middle["resource_id"], Value::Null);
    assert_eq!(middle["changes"], Value::Null);
    assert_eq!(middle["ip_address"], "203.0.113.9");

    // Each filter narrows the same descending order.
    let (_, body, _) = get(
        &app,
        "/api/admin/audit-logs?action=kyc.approved",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    let filtered = body.as_array().unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["id"], first_id.to_string());

    let (_, body, _) = get(
        &app,
        &format!("/api/admin/audit-logs?resource_type=user&user_id={actor}&limit=1"),
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    let filtered = body.as_array().unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["id"], third_id.to_string());
    let (_, body, _) = get(
        &app,
        &format!("/api/admin/audit-logs?resource_type=user&user_id={actor}&limit=1&offset=1"),
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(body.as_array().unwrap()[0]["id"], first_id.to_string());
    let (_, body, _) = get(
        &app,
        "/api/admin/audit-logs?action=",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 3);

    // The default limit and the defects: negative, zero and over-maximum
    // limits are request refusals, as is an invalid user id.
    assert_eq!(
        audit_query::AuditQuery::from_pairs(&[]).unwrap().limit(),
        50
    );
    for query in [
        "limit=-1",
        "limit=0",
        "limit=201",
        "offset=-1",
        "offset=10001",
        "user_id=not-a-uuid",
    ] {
        let (status, body, _) = get(
            &app,
            &format!("/api/admin/audit-logs?{query}"),
            Some(&token(actor)),
            FORWARDED,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert!(body["detail"].is_array(), "{query}");
    }

    // A 201-row read proves the default is applied before SQL and the array
    // order stays deterministic on duplicate timestamps.
    let tied = fixture.now - Duration::days(1);
    let mut tied_order = Vec::new();
    for _ in 0..201 {
        let id = fixture
            .insert_audit(Some(actor), "order.created", "order", None, None, tied)
            .await;
        tied_order.push(id.to_string());
    }
    tied_order.sort();
    let (_, body, _) = get(
        &app,
        "/api/admin/audit-logs?action=order.created",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    let page: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page.len(), 50);
    assert_eq!(page[..], tied_order[..50]);
    // A zero row count keeps the offset path bounded and empty.
    let (_, body, _) = get(
        &app,
        "/api/admin/audit-logs?action=no.such.action",
        Some(&token(actor)),
        FORWARDED,
    )
    .await;
    assert!(body.as_array().unwrap().is_empty());
    // N03 has no declared limit, so sixty calls all succeed.
    for _ in 0..60 {
        let (status, _, _) = get(
            &app,
            "/api/admin/audit-logs",
            Some(&token(actor)),
            FORWARDED,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    fixture.finish().await;
}
