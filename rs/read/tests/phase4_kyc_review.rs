pub use verdaxis_read::{ApiError, ApiErrorKind, auth, business, crud, runtime};

#[path = "../src/kyc/review.rs"]
mod review;

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::StatusCode,
    routing::post,
};
use business::{
    admission::AdmissionEvents,
    transaction::{AttemptError, CommandFuture},
};
use chrono::Utc;
use crud::referrals::invite::MailClient;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// The registry values Python stores verbatim in the append-only audit table
/// (`app/services/audit_actions.py`). A compliance query filters on them.
const APPROVED_ACTION: &str = "kyc.approved";
const REJECTED_ACTION: &str = "kyc.rejected";
/// Correlation id and forwarded hop the server middleware would supply. The
/// middleware extension value differs from the header, so the test proves the
/// audit row reuses the minted id instead of deriving a second one.
const REQUEST_ID: &str = "t11b-review-correlation";
const MIDDLEWARE_ID: &str = "t11b-middleware-minted";
const FORWARDED_IP: &str = "203.0.113.7";

#[test]
fn p4_t11b_c01() {
    for body in [
        json!({}),
        json!({"external_evidence_reference":"ab","review_note":"valid review note"}),
        json!({"external_evidence_reference":"case-1","review_note":"short"}),
        json!({"external_evidence_reference":"ab\nc","review_note":"valid review note"}),
    ] {
        assert!(review::Decision::approval(body).is_err());
    }
    let decision = review::Decision::approval(
        json!({"external_evidence_reference":" case-1 ","review_note":" valid review note "}),
    )
    .unwrap();
    assert_eq!(decision.status(), "APPROVED");
    for body in [
        json!({}),
        json!({"reason":"  "}),
        json!({"reason":"x".repeat(1001)}),
    ] {
        assert!(review::Decision::rejection(body).is_err());
    }
    assert_eq!(
        review::Decision::rejection(json!({"reason":" valid reason "}))
            .unwrap()
            .status(),
        "REJECTED"
    );
}

#[derive(Clone, Default)]
struct Probe {
    appends: Arc<AtomicUsize>,
    visibles: Arc<AtomicUsize>,
    status_at_append: Arc<Mutex<Vec<String>>>,
}

struct Events {
    fail: bool,
    probe: Probe,
}
impl AdmissionEvents for Events {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            self.probe.visibles.fetch_add(1, Ordering::SeqCst);
            Ok(
                sqlx::query_scalar("SELECT kyc_status='APPROVED' FROM users WHERE id=$1")
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
            // Record the decision state this append observes, so a rollback
            // test can prove the failure happened after the user write rather
            // than before any write at all.
            let status: Option<String> =
                sqlx::query_scalar("SELECT kyc_status FROM users WHERE id=$1")
                    .bind(user)
                    .fetch_optional(&mut *conn)
                    .await?
                    .flatten();
            sqlx::query("INSERT INTO market_event_outbox(id,user_id,organization_id,public) VALUES($1,$2,$3,$4)").bind(Uuid::new_v4()).bind(user).bind(organization).bind(public).execute(&mut *conn).await?;
            self.probe.appends.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                self.probe
                    .status_at_append
                    .lock()
                    .expect("probe lock")
                    .push(status.unwrap_or_default());
                return Err(sqlx::Error::Protocol("injected append failure".into()).into());
            }
            Ok(())
        })
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    target: Uuid,
    organization: Uuid,
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
        let schema = format!("p4_review_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE organizations(id uuid PRIMARY KEY,verification_status text); CREATE TABLE organization_join_requests(id uuid PRIMARY KEY); \
            CREATE TABLE users(id uuid PRIMARY KEY, organization_id uuid, role text, status text, email_verified boolean DEFAULT true, must_change_password boolean DEFAULT false, password_changed_at timestamptz, authentication_revoked_at timestamptz, email text DEFAULT 'synthetic@example.test', first_name text DEFAULT 'Synthetic', kyc_status text DEFAULT 'SUBMITTED', kyc_organization_id uuid, kyc_rejection_reason text, kyc_external_evidence_reference text, kyc_review_note text, kyc_reviewed_by uuid, kyc_reviewed_at timestamptz); \
            CREATE TABLE kyc_documents(id uuid PRIMARY KEY,user_id uuid,organization_id uuid); \
            CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100),resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz); \
            CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,public boolean)")
            .execute(&mut admin).await.unwrap();
        // The settlement tables and their rows come from a test-only SQL
        // fixture with the production column shape. Rust literals must not
        // carry order-book DML: the sealed write boundary forbids it even in
        // tests, and a stand-in table would not prove real rows are untouched.
        sqlx::raw_sql(include_str!("fixtures/settlement_orders.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let actor = Uuid::new_v4();
        let target = Uuid::new_v4();
        let organization = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations VALUES($1)")
            .bind(organization)
            .execute(&mut admin)
            .await
            .unwrap();
        for (id, role) in [(actor, "ADMIN"), (target, "BUYER")] {
            sqlx::query("INSERT INTO users(id,organization_id,kyc_organization_id,role,status) VALUES($1,$2,$2,$3,'APPROVED')").bind(id).bind(organization).bind(role).execute(&mut admin).await.unwrap();
        }
        for _ in 0..2 {
            sqlx::query("INSERT INTO kyc_documents VALUES($1,$2,$3)")
                .bind(Uuid::new_v4())
                .bind(target)
                .bind(organization)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app; GRANT UPDATE(verification_status) ON organizations TO rust_app; GRANT UPDATE(kyc_status,kyc_rejection_reason,kyc_external_evidence_reference,kyc_review_note,kyc_reviewed_by,kyc_reviewed_at) ON users TO rust_app; GRANT INSERT ON audit_logs,market_event_outbox TO rust_app"))).execute(&mut admin).await.unwrap();
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
            actor,
            target,
            organization,
        }
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
    fn service(&self, mail: MailClient, fail: bool) -> (review::ReviewService, Probe) {
        let probe = Probe::default();
        let service = review::ReviewService::new(
            self.pool.clone(),
            Arc::new(Semaphore::new(4)),
            Arc::new(settings()),
            auth::limits::SharedLimits::default(),
            Arc::new(Events {
                fail,
                probe: probe.clone(),
            }),
            mail,
            "Verdaxis <test@example.test>".into(),
        );
        (service, probe)
    }
    /// Every settlement row, read back as JSON in a stable order.
    async fn settlement(&mut self) -> Value {
        let mut tables = serde_json::Map::new();
        for table in ["orderbook_orders", "trades", "inventory_items"] {
            let rows: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY t.id),'[]'::jsonb) FROM {table} t"
            )))
            .fetch_one(&mut self.admin)
            .await
            .unwrap();
            tables.insert(table.to_owned(), rows);
        }
        Value::Object(tables)
    }
    async fn audit(&mut self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_logs a")
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn row(&mut self) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(u) FROM users u WHERE id=$1")
            .bind(self.target)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
    async fn count(&mut self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
}
fn settings() -> auth::VerificationSettings {
    auth::VerificationSettings::new(
        "test-secret-key-for-review-32-characters".into(),
        None,
        "review-test".into(),
        "review-test".into(),
    )
}
fn token(user: Uuid) -> String {
    jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":user,"type":"access","iat":Utc::now().timestamp()-1,"exp":Utc::now().timestamp()+3600,"iss":"review-test","aud":"review-test"}),&jsonwebtoken::EncodingKey::from_secret(b"test-secret-key-for-review-32-characters")).unwrap()
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), task)
}
/// Supply the loopback peer the server sees behind Caddy and the correlation
/// id the request-log middleware stores, so the shared client-address rule can
/// trust the final forwarded hop and the audit reuses the minted id.
async fn loopback_peer(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41_234))));
    request.extensions_mut().insert(MIDDLEWARE_ID.to_owned());
    next.run(request).await
}
fn with_peer(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(loopback_peer))
}
async fn request(
    base: &str,
    user: Uuid,
    action: &str,
    bearer: Option<&str>,
    body: Value,
) -> reqwest::Response {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut req = client
        .put(format!("{base}/api/kyc/admin/{user}/{action}"))
        .header("x-request-id", REQUEST_ID)
        .header("x-forwarded-for", FORWARDED_IP)
        .json(&body);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    req.send().await.unwrap()
}
fn approval() -> Value {
    json!({"external_evidence_reference":"external-001","review_note":"Independent evidence reviewed."})
}

#[derive(Clone)]
struct MailStub {
    pool: sqlx::PgPool,
    target: Uuid,
    calls: Arc<AtomicUsize>,
    expected: &'static str,
    status: StatusCode,
}
async fn receive(State(stub): State<MailStub>) -> StatusCode {
    let status: String = sqlx::query_scalar("SELECT kyc_status FROM users WHERE id=$1")
        .bind(stub.target)
        .fetch_one(&stub.pool)
        .await
        .unwrap();
    assert_eq!(status, stub.expected);
    // A fresh transaction can lock the target: no review lock spans provider I/O.
    let mut tx = stub.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(stub.target)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    stub.calls.fetch_add(1, Ordering::SeqCst);
    stub.status
}
async fn mail(
    f: &Fixture,
    expected: &'static str,
    status: StatusCode,
) -> (MailClient, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let (base, task) = serve(
        Router::new()
            .route("/mail", post(receive))
            .with_state(MailStub {
                pool: f.pool.clone(),
                target: f.target,
                calls: calls.clone(),
                expected,
                status,
            }),
    )
    .await;
    (
        MailClient::loopback(format!("{base}/mail"), "synthetic-key".into()).unwrap(),
        calls,
        task,
    )
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t11b_c02() {
    let mut f = Fixture::new().await;
    let (mail, calls, mail_task) = mail(&f, "APPROVED", StatusCode::BAD_GATEWAY).await;
    let (service, _probe) = f.service(mail, false);
    let (base, server) = serve(with_peer(service.router())).await;
    let bearer = token(f.actor);
    let settlement = f.settlement().await;
    let response = request(&base, f.target, "approve", Some(&bearer), approval()).await;
    assert_eq!(response.status(), 502);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"]["code"],
        "KYC_NOTIFICATION_FAILED"
    );
    let row = f.row().await;
    assert_eq!(row["kyc_status"], "APPROVED");
    assert_eq!(row["kyc_reviewed_by"], f.actor.to_string());
    assert_eq!(row["kyc_external_evidence_reference"], "external-001");
    // Approval is a user KYC decision. Every settlement row stays exactly as
    // it was, including open order-book rows, all six trade states and the
    // uncommitted reservation.
    assert_eq!(f.settlement().await, settlement);
    assert_eq!(f.count("audit_logs").await, 1);
    assert_eq!(f.count("market_event_outbox").await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let response = request(&base, f.target, "approve", Some(&bearer), approval()).await;
    assert_eq!(response.status(), 409);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let audit = f.audit().await;
    assert_eq!(audit["action"], APPROVED_ACTION);
    assert_eq!(audit["request_id"], MIDDLEWARE_ID);
    assert_eq!(audit["ip_address"], FORWARDED_IP);
    let changes = &audit["changes"];
    assert_eq!(changes["raw_documents_retained_in_platform"], true);
    server.abort();
    mail_task.abort();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t11b_c03() {
    let mut f = Fixture::new().await;
    let (mail, calls, mail_task) = mail(&f, "REJECTED", StatusCode::OK).await;
    let (service, _probe) = f.service(mail, false);
    let (base, server) = serve(with_peer(service.router())).await;
    let bearer = token(f.actor);
    let before = f.row().await;
    let settlement = f.settlement().await;
    let response = request(
        &base,
        f.target,
        "reject",
        Some(&bearer),
        json!({"reason":"Review not accepted"}),
    )
    .await;
    assert_eq!(response.status(), 200);
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(body["kyc_status"], "REJECTED");
    assert_eq!(body["rejection_reason"], "Review not accepted");
    let after = f.row().await;
    for field in [
        "status",
        "organization_id",
        "kyc_organization_id",
        "authentication_revoked_at",
        "password_changed_at",
    ] {
        assert_eq!(before[field], after[field]);
    }
    // User-level KYC rejection performs no tenant cleanup: the real
    // order-book rows, every persisted trade state and the reservation are
    // byte-for-byte the same.
    assert_eq!(f.settlement().await, settlement);
    assert_eq!(f.count("trades").await, 6);
    assert_eq!(f.count("orderbook_orders").await, 2);
    assert_eq!(f.count("kyc_documents").await, 2);
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM orderbook_orders WHERE status='OPEN' AND remaining_quantity_mt=29 AND version=3",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(open, 1);
    let reserved: bool = sqlx::query_scalar("SELECT reserved_stock_mt=17 FROM inventory_items")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert!(reserved);
    assert_eq!(f.count("audit_logs").await, 1);
    assert_eq!(f.count("market_event_outbox").await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.audit().await["action"], REJECTED_ACTION);
    // Python's reject path has no status precondition (PM decision
    // PD4-T11b-1): a second rejection is accepted, re-audited and re-notified,
    // while the actual-change guard keeps its single admission event.
    let response = request(
        &base,
        f.target,
        "reject",
        Some(&bearer),
        json!({"reason":"Review not accepted"}),
    )
    .await;
    assert_eq!(response.status(), 200);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.count("audit_logs").await, 2);
    assert_eq!(f.count("market_event_outbox").await, 1);
    let actions: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_logs ORDER BY timestamp")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(actions, [REJECTED_ACTION, REJECTED_ACTION]);
    server.abort();
    mail_task.abort();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t11b_c04() {
    let mut f = Fixture::new().await;
    let (mail, calls, mail_task) = mail(&f, "APPROVED", StatusCode::OK).await;
    let (service, probe) = f.service(mail, true);
    let (base, server) = serve(with_peer(service.router())).await;
    let bearer = token(f.actor);
    assert_eq!(
        request(&base, f.target, "approve", Some(&bearer), approval())
            .await
            .status(),
        500
    );
    assert_eq!(f.row().await["kyc_status"], "SUBMITTED");
    assert_eq!(f.count("audit_logs").await, 0);
    assert_eq!(f.count("market_event_outbox").await, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // The failure happened inside the event append, after the user write and
    // its audit facts existed in the transaction. Zero surviving rows is
    // therefore a real rollback, not a failure before any write.
    assert_eq!(probe.appends.load(Ordering::SeqCst), 1);
    assert!(probe.visibles.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        *probe.status_at_append.lock().unwrap(),
        vec!["APPROVED".to_owned()]
    );
    server.abort();
    mail_task.abort();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t11b_c05() {
    let mut f = Fixture::new().await;
    let (service, _probe) = f.service(MailClient::disabled(), false);
    let (base, server) = serve(with_peer(service.router())).await;
    for (user, status) in [(f.target, 403), (Uuid::new_v4(), 403)] {
        let nonadmin = token(f.target);
        for action in ["approve", "reject"] {
            assert_eq!(
                request(&base, user, action, None, json!({})).await.status(),
                401
            );
            let response = request(&base, user, action, Some(&nonadmin), json!({})).await;
            assert_eq!(response.status(), status);
            assert_eq!(
                response.json::<Value>().await.unwrap()["detail"]["code"],
                "ADMIN_REQUIRED"
            );
        }
    }
    let bearer = token(f.actor);
    assert_eq!(
        request(&base, Uuid::new_v4(), "approve", Some(&bearer), approval())
            .await
            .status(),
        404
    );
    for state in ["PENDING", "APPROVED", "REJECTED", "UNKNOWN"] {
        sqlx::query("UPDATE users SET kyc_status=$2 WHERE id=$1")
            .bind(f.target)
            .bind(state)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            request(&base, f.target, "approve", Some(&bearer), approval())
                .await
                .status(),
            409
        );
    }
    sqlx::query("UPDATE users SET kyc_status='SUBMITTED',kyc_organization_id=$2 WHERE id=$1")
        .bind(f.target)
        .bind(Uuid::new_v4())
        .execute(&mut f.admin)
        .await
        .unwrap();
    for action in ["approve", "reject"] {
        let body = if action == "approve" {
            approval()
        } else {
            json!({"reason":"Review declined"})
        };
        let response = request(&base, f.target, action, Some(&bearer), body).await;
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.json::<Value>().await.unwrap()["detail"]["code"],
            "KYC_ORGANIZATION_MISMATCH"
        );
    }
    assert_eq!(f.count("audit_logs").await, 0);
    assert_eq!(f.count("market_event_outbox").await, 0);
    server.abort();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t11b_c06() {
    let mut f = Fixture::new().await;
    let (mail, calls, mail_task) = mail(&f, "APPROVED", StatusCode::OK).await;
    let (service, _probe) = f.service(mail, false);
    let (base, server) = serve(with_peer(service.router())).await;
    let bearer = token(f.actor);
    let (a, b) = tokio::join!(
        request(&base, f.target, "approve", Some(&bearer), approval()),
        request(&base, f.target, "approve", Some(&bearer), approval())
    );
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.count("audit_logs").await, 1);
    assert_eq!(f.count("market_event_outbox").await, 1);
    assert_eq!(
        f.row().await["kyc_organization_id"],
        f.organization.to_string()
    );
    server.abort();
    mail_task.abort();
    f.finish().await;
}
