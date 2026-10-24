use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub use verdaxis_read::business::{audit, error, transaction};
pub use verdaxis_read::{ApiError, AppState, auth};
#[path = "../src/business/joins.rs"]
pub mod joins;
#[path = "../src/business/membership.rs"]
pub mod membership;
#[path = "../src/business/organizations.rs"]
pub mod organizations;

// The pending producer is represented only at its declared connection boundary.
pub mod market_write {
    use super::*;
    use rust_decimal::Decimal;
    pub struct InvalidateOrganization {
        pub organization_id: Uuid,
        pub actor_id: Uuid,
        pub now: DateTime<Utc>,
    }
    pub struct InvalidationResult {
        pub withdrawn_orders: Vec<Uuid>,
        pub preserved_trade_ids: Vec<Uuid>,
        pub released_uncommitted_quantity: Decimal,
        pub outbox_ids: Vec<Uuid>,
    }
    pub type InvalidationError = transaction::AttemptError;
    pub async fn invalidate_organization(
        conn: &mut PgConnection,
        command: &InvalidateOrganization,
    ) -> Result<InvalidationResult, InvalidationError> {
        // This double records the call; it does not implement market mutations.
        sqlx::query("INSERT INTO invalidator_calls VALUES($1,$2,$3)")
            .bind(command.organization_id)
            .bind(command.actor_id)
            .bind(command.now)
            .execute(&mut *conn)
            .await?;
        let refuse: bool =
            sqlx::query_scalar("SELECT refuse_invalidation FROM organizations WHERE id=$1")
                .bind(command.organization_id)
                .fetch_one(&mut *conn)
                .await?;
        if refuse {
            return Err(error::refusal(
                axum::http::StatusCode::CONFLICT,
                "MARKET_INVALIDATION_BOUND",
                "The organization requires a bounded review.",
            )
            .into());
        }
        let ids = sqlx::query_scalar(
            "SELECT id FROM settlement_records WHERE organization_id=$1 ORDER BY id",
        )
        .bind(command.organization_id)
        .fetch_all(&mut *conn)
        .await?;
        Ok(InvalidationResult {
            withdrawn_orders: vec![],
            preserved_trade_ids: ids,
            released_uncommitted_quantity: Decimal::ZERO,
            outbox_ids: vec![],
        })
    }
}

#[test]
fn p4_t8_c11() {
    let source = include_str!("../src/business/organizations.rs");
    assert!(source.contains("order_write::{InvalidateOrganization, invalidate_organization}"));
    assert!(source.contains("policy: Arc::clone(&self.policy)"));
    assert!(source.contains("config: Arc::clone(&self.config)"));
}

#[test]
fn p4_t8_c08() {
    assert!(joins::JoinListQuery::parse(Some("PENDING"), 100, 0).is_ok());
    for (status, limit, offset) in [
        (Some("OTHER"), 100, 0),
        (None, 0, 0),
        (None, 501, 0),
        (None, 1, 10001),
    ] {
        let error = joins::JoinListQuery::parse(status, limit, offset).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.detail["code"], "VALIDATION_FAILED");
    }
    for note in ["ab", "   ", &"x".repeat(1001)] {
        assert!(joins::ReviewNote::parse(note).is_err());
    }
    assert_eq!(
        joins::ReviewNote::parse("  checked  ").unwrap().as_str(),
        "checked"
    );
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    org: Uuid,
    user: Uuid,
    join: Uuid,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL");
        let runtime =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
        let parsed = url::Url::parse(&url).unwrap();
        let app = url::Url::parse(&runtime).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), app.port());
        assert_eq!(parsed.path(), app.path());
        assert_eq!(app.host_str(), Some("127.0.0.1"));
        assert_eq!(app.username(), "rust_app");
        assert_ne!(parsed.username(), app.username());
        assert!(parsed.path().ends_with("_test"));
        let mut admin = PgConnection::connect(&url).await.unwrap();
        let schema = format!("p4_t8_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE organizations(id uuid PRIMARY KEY,type text NOT NULL,verification_status text NOT NULL,provenance text NOT NULL DEFAULT 'UNKNOWN',refuse_invalidation boolean NOT NULL DEFAULT false);\
        CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations(id),role text,status text NOT NULL,email_verified boolean NOT NULL DEFAULT true,must_change_password boolean NOT NULL DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,kyc_status text,kyc_organization_id uuid,kyc_rejection_reason text,kyc_external_evidence_reference text,kyc_review_note text,kyc_reviewed_by uuid,kyc_reviewed_at timestamptz);\
        CREATE INDEX ix_users_organization_id_id ON users(organization_id,id);\
        CREATE TABLE organization_join_requests(id uuid PRIMARY KEY,user_id uuid NOT NULL REFERENCES users(id),organization_id uuid NOT NULL REFERENCES organizations(id),status text NOT NULL,reviewed_by uuid,reviewed_at timestamptz,review_note text,created_at timestamptz NOT NULL,UNIQUE(user_id,organization_id));\
        CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100),resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz);\
        CREATE TABLE invalidator_calls(organization_id uuid,actor_id uuid,at timestamptz);\
        CREATE TABLE settlement_records(id uuid PRIMARY KEY,organization_id uuid,state text,participant_id uuid,quantity numeric,fee numeric);\
        CREATE TABLE retained_evidence(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,ciphertext bytea);\
        CREATE TABLE kyc_execution_grace(user_id uuid,organization_id uuid,expires_at timestamptz);\
        CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,event_type text,aggregate_type text,aggregate_id text,participant_org_ids json,payload json,created_at timestamptz DEFAULT now());\
        CREATE FUNCTION approval_guard() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.verification_status='APPROVED' AND OLD.verification_status<>'APPROVED' THEN IF NEW.refuse_invalidation THEN RAISE EXCEPTION 'unresolved' USING ERRCODE='VD001'; END IF; IF OLD.provenance='UNKNOWN' THEN NEW.provenance='REAL'; END IF; END IF; RETURN NEW; END $$;\
        CREATE TRIGGER approval_guard BEFORE UPDATE ON organizations FOR EACH ROW EXECUTE FUNCTION approval_guard();").execute(&mut admin).await.unwrap();
        let actor = Uuid::from_u128(900);
        let org = Uuid::from_u128(100);
        let user = Uuid::from_u128(1);
        let join = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,type,verification_status) VALUES($1,'FUEL_BUYER','APPROVED')").bind(org).execute(&mut admin).await.unwrap();
        for (id, role, organization) in [(actor, "ADMIN", None::<Uuid>), (user, "BUYER", None)] {
            sqlx::query("INSERT INTO users(id,organization_id,role,status,kyc_status) VALUES($1,$2,$3,'APPROVED','PENDING')")
                .bind(id).bind(organization).bind(role).execute(&mut admin).await.unwrap();
        }
        sqlx::query("INSERT INTO organization_join_requests(id,user_id,organization_id,status,created_at) VALUES($1,$2,$3,'PENDING','2026-01-01Z')")
            .bind(join).bind(user).bind(org).execute(&mut admin).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app; GRANT UPDATE(verification_status) ON organizations TO rust_app; GRANT UPDATE(organization_id,kyc_status,kyc_organization_id,kyc_rejection_reason,kyc_external_evidence_reference,kyc_review_note,kyc_reviewed_by,kyc_reviewed_at) ON users TO rust_app; GRANT UPDATE(status,reviewed_by,reviewed_at,review_note) ON organization_join_requests TO rust_app; GRANT INSERT ON audit_logs,invalidator_calls,market_event_outbox TO rust_app;"))).execute(&mut admin).await.unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(path))
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
            org,
            user,
            join,
        }
    }
    fn access(&self) -> auth::VerifiedAccess {
        let key = "test-secret-key-for-testing-minimum-32-chars";
        let claims = json!({"sub":self.actor,"iat":Utc::now().timestamp(),"exp":Utc::now().timestamp()+3600,"type":"access","iss":"test","aud":"test"});
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
        )
        .unwrap();
        auth::verify_access(
            &token,
            &auth::VerificationSettings::new(key.into(), None, "test".into(), "test".into()),
            Utc::now(),
        )
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
    async fn run<C: transaction::BusinessCommand>(
        &self,
        command: C,
    ) -> Result<C::Output, ApiError> {
        transaction::run_ordered_write(
            self.pool.clone(),
            Arc::new(Semaphore::new(4)),
            self.access(),
            command,
        )
        .await
    }
    fn review(&self, approve: bool) -> joins::ReviewJoin {
        joins::ReviewJoin {
            join_request_id: self.join,
            approve,
            note: joins::ReviewNote::parse("checked membership").unwrap(),
            now: Utc::now(),
        }
    }
    fn organization(&self, approve: bool) -> organizations::ReviewOrganization {
        organizations::ReviewOrganization {
            organization_id: self.org,
            approve,
            reason: None,
            now: Utc::now(),
        }
    }
    async fn count(&mut self, table: &str) -> i64 {
        assert!(["audit_logs", "invalidator_calls", "market_event_outbox"].contains(&table));
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c01() {
    let mut f = Fixture::new().await;
    sqlx::query("INSERT INTO users(id,role,status,kyc_status) SELECT gen_random_uuid(),'BUYER','APPROVED','PENDING' FROM generate_series(1,1002)").execute(&mut f.admin).await.unwrap();
    sqlx::query("INSERT INTO organization_join_requests(id,user_id,organization_id,status,created_at) SELECT gen_random_uuid(),id,$1,CASE WHEN row_number() OVER(ORDER BY id)<=501 THEN 'REJECTED' ELSE 'PENDING' END,'2026-01-01Z' FROM users WHERE id<>$2 AND id<>$3")
        .bind(f.org).bind(f.actor).bind(f.user).execute(&mut f.admin).await.unwrap();
    let actor = auth::CurrentActor {
        id: f.actor,
        organization_id: None,
        role: Some("ADMIN".into()),
        email_verified: true,
    };
    let q = joins::JoinListQuery::parse(Some("PENDING"), 500, 0).unwrap();
    let page = joins::list(&mut f.pool.acquire().await.unwrap(), &actor, &q)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 500);
    assert!(page.truncated);
    assert!(
        page.items
            .iter()
            .all(|x| x.status == joins::JoinStatus::Pending)
    );
    assert!(
        page.items
            .windows(2)
            .all(|w| (w[0].created_at, w[0].id) < (w[1].created_at, w[1].id))
    );
    let q = joins::JoinListQuery::parse(Some("PENDING"), 500, 500).unwrap();
    let last = joins::list(&mut f.pool.acquire().await.unwrap(), &actor, &q)
        .await
        .unwrap();
    assert_eq!(last.items.len(), 2);
    assert!(!last.truncated);
    let value = serde_json::to_value(&last.items[0]).unwrap();
    for key in [
        "id",
        "user_id",
        "organization_id",
        "status",
        "reviewed_by",
        "reviewed_at",
        "review_note",
        "created_at",
    ] {
        assert!(value.get(key).is_some());
    }
    assert_eq!(f.count("audit_logs").await, 0);
    let mut member = actor;
    member.role = Some("BUYER".into());
    assert_eq!(
        joins::list(&mut f.pool.acquire().await.unwrap(), &member, &q)
            .await
            .unwrap_err()
            .status,
        axum::http::StatusCode::FORBIDDEN
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c02() {
    let mut f = Fixture::new().await;
    let result = f.run(f.review(false)).await.unwrap();
    assert_eq!(result.status, joins::JoinStatus::Rejected);
    let row=sqlx::query("SELECT u.status,u.organization_id,j.reviewed_by,j.review_note,o.verification_status FROM users u CROSS JOIN organization_join_requests j CROSS JOIN organizations o WHERE u.id=$1 AND j.id=$2 AND o.id=$3").bind(f.user).bind(f.join).bind(f.org).fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(row.get::<String, _>("status"), "APPROVED");
    assert!(row.get::<Option<Uuid>, _>("organization_id").is_none());
    assert_eq!(row.get::<Uuid, _>("reviewed_by"), f.actor);
    assert_eq!(row.get::<String, _>("review_note"), "checked membership");
    assert_eq!(row.get::<String, _>("verification_status"), "APPROVED");
    assert_eq!(f.count("audit_logs").await, 1);
    let error = f.run(f.review(false)).await.unwrap_err();
    assert_eq!(error.detail["code"], "ORGANIZATION_JOIN_ALREADY_REVIEWED");
    assert_eq!(f.count("audit_logs").await, 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c03() {
    let mut f = Fixture::new().await;
    sqlx::query("UPDATE users SET organization_id=$1,kyc_status='APPROVED',kyc_organization_id=$1 WHERE id=$2").bind(f.org).bind(f.user).execute(&mut f.admin).await.unwrap();
    for state in [
        "PENDING_CONFIRMATION",
        "CONFIRMED",
        "DELIVERED",
        "PAID",
        "CANCELLED",
        "DECLINED",
    ] {
        sqlx::query("INSERT INTO settlement_records VALUES($1,$2,$3,$4,10,2)")
            .bind(Uuid::new_v4())
            .bind(f.org)
            .bind(state)
            .bind(f.user)
            .execute(&mut f.admin)
            .await
            .unwrap();
    }
    let before: serde_json::Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM settlement_records t")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    f.run(f.organization(false)).await.unwrap();
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM settlement_records t")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
    assert_eq!(before, after);
    let row = sqlx::query(
        "SELECT organization_id,kyc_status,kyc_organization_id,status FROM users WHERE id=$1",
    )
    .bind(f.user)
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert!(row.get::<Option<Uuid>, _>("organization_id").is_none());
    assert!(row.get::<Option<Uuid>, _>("kyc_organization_id").is_none());
    assert_eq!(row.get::<String, _>("kyc_status"), "PENDING");
    assert_eq!(row.get::<String, _>("status"), "APPROVED");
    assert_eq!(f.count("invalidator_calls").await, 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c04() {
    let mut f = Fixture::new().await;
    for iteration in 0..50 {
        sqlx::raw_sql("UPDATE organizations SET verification_status='APPROVED'; UPDATE users SET organization_id=NULL,kyc_status='PENDING',kyc_organization_id=NULL; UPDATE organization_join_requests SET status='PENDING'").execute(&mut f.admin).await.unwrap();
        let gate = Arc::new(tokio::sync::Barrier::new(3));
        let join = f.review(true);
        let reject = f.organization(false);
        let pool = f.pool.clone();
        let access = f.access();
        let g = gate.clone();
        let a = tokio::spawn(async move {
            g.wait().await;
            transaction::run_ordered_write(pool, Arc::new(Semaphore::new(1)), access, join).await
        });
        let pool = f.pool.clone();
        let access = f.access();
        let g = gate.clone();
        let b = tokio::spawn(async move {
            g.wait().await;
            transaction::run_ordered_write(pool, Arc::new(Semaphore::new(1)), access, reject).await
        });
        gate.wait().await;
        let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            tokio::join!(a, b)
        })
        .await
        .unwrap();
        if let Err(error) = a.unwrap() {
            assert_eq!(
                error.detail["code"], "ORGANIZATION_JOIN_ORG_INELIGIBLE",
                "iteration {iteration}"
            );
        }
        b.unwrap().unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE organization_id=$1")
            .bind(f.org)
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c05() {
    let mut f = Fixture::new().await;
    sqlx::query("UPDATE organizations SET verification_status='PENDING',refuse_invalidation=true")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let error = f.run(f.organization(true)).await.unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.detail["code"], "ORGANIZATION_APPROVAL_CONFLICT");
    assert_eq!(f.count("audit_logs").await, 0);
    let state: String = sqlx::query_scalar("SELECT verification_status FROM organizations")
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(state, "PENDING");
    sqlx::query("UPDATE organizations SET refuse_invalidation=false")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.run(f.organization(true)).await.unwrap();
    let changes: serde_json::Value = sqlx::query_scalar(
        "SELECT changes FROM audit_logs WHERE action='admin.organization_approved'",
    )
    .fetch_one(&mut f.admin)
    .await
    .unwrap();
    assert_eq!(changes["provenance"], json!({"from":"UNKNOWN","to":"REAL"}));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c06() {
    let mut f = Fixture::new().await;
    for (column, value, code) in [
        ("status", "PENDING", "ORGANIZATION_JOIN_USER_INELIGIBLE"),
        (
            "email_verified",
            "false",
            "ORGANIZATION_JOIN_USER_INELIGIBLE",
        ),
        ("role", "SUPPLIER", "ORGANIZATION_JOIN_ROLE_MISMATCH"),
    ] {
        let sql = if column == "email_verified" {
            format!("UPDATE users SET {column}={value} WHERE id=$1")
        } else {
            format!("UPDATE users SET {column}='{value}' WHERE id=$1")
        };
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(f.user)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.run(f.review(true)).await.unwrap_err().detail["code"],
            code
        );
        sqlx::query(
            "UPDATE users SET status='APPROVED',email_verified=true,role='BUYER' WHERE id=$1",
        )
        .bind(f.user)
        .execute(&mut f.admin)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE organizations SET verification_status='PENDING'")
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.run(f.review(true)).await.unwrap_err().detail["code"],
        "ORGANIZATION_JOIN_ORG_INELIGIBLE"
    );
    assert_eq!(f.count("audit_logs").await, 0);
    sqlx::query("UPDATE organizations SET verification_status='APPROVED'")
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.run(f.review(true)).await.unwrap();
    assert_eq!(f.count("audit_logs").await, 1);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c07() {
    let mut f = Fixture::new().await;
    sqlx::query("UPDATE users SET kyc_status='APPROVED',kyc_organization_id=$1,kyc_review_note='review',kyc_reviewed_by=$2,kyc_reviewed_at=now(),kyc_external_evidence_reference='case',kyc_rejection_reason='old' WHERE id=$3").bind(f.org).bind(f.actor).bind(f.user).execute(&mut f.admin).await.unwrap();
    sqlx::query("INSERT INTO retained_evidence VALUES($1,$2,$3,'\\x0102')")
        .bind(Uuid::new_v4())
        .bind(f.user)
        .bind(f.org)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO kyc_execution_grace VALUES($1,$2,now()+interval '1 day')")
        .bind(f.user)
        .bind(f.org)
        .execute(&mut f.admin)
        .await
        .unwrap();
    f.run(f.review(true)).await.unwrap();
    let row = sqlx::query("SELECT * FROM users WHERE id=$1")
        .bind(f.user)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(row.get::<Uuid, _>("organization_id"), f.org);
    assert_eq!(row.get::<String, _>("kyc_status"), "PENDING");
    for key in [
        "kyc_review_note",
        "kyc_external_evidence_reference",
        "kyc_rejection_reason",
    ] {
        assert!(row.get::<Option<String>, _>(key).is_none());
    }
    assert!(row.get::<Option<Uuid>, _>("kyc_reviewed_by").is_none());
    assert!(
        row.get::<Option<DateTime<Utc>>, _>("kyc_reviewed_at")
            .is_none()
    );
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM retained_evidence),(SELECT count(*) FROM kyc_execution_grace)").fetch_one(&mut f.admin).await.unwrap();
    assert_eq!(counts, (1, 1));
    assert_eq!(f.count("audit_logs").await, 2);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c09() {
    let mut f = Fixture::new().await;
    sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
        .bind(f.org)
        .bind(f.user)
        .execute(&mut f.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE organizations SET refuse_invalidation=true WHERE id=$1")
        .bind(f.org)
        .execute(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(
        f.run(f.organization(false)).await.unwrap_err().status,
        axum::http::StatusCode::CONFLICT
    );
    let org: Uuid = sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
        .bind(f.user)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(org, f.org);
    assert_eq!(f.count("audit_logs").await, 0);
    assert_eq!(f.count("invalidator_calls").await, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t8_c10() {
    let mut f = Fixture::new().await;
    for status in ["UNKNOWN", "REVIEW_REQUIRED", ""] {
        sqlx::query("UPDATE users SET kyc_status=$1 WHERE id=$2")
            .bind(status)
            .bind(f.user)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.run(f.review(true)).await.unwrap_err().detail["code"],
            "KYC_STATE_INVALID"
        );
    }
    assert_eq!(f.count("audit_logs").await, 0);
    f.finish().await;
}
