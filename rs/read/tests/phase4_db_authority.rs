//! Acceptance against the migrated disposable schema and committed ACL bundle.
use axum::{body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{Config, compiled_sha, connect, kyc::keyring::KeyRing};

const REVISION: &str = "sse_20261021_replay_floor";
const APP_ROLE: &str = "verdaxis_app_staging";
const MIGRATOR_ROLE: &str = "verdaxis_migrator_staging";

struct Database {
    app: PgConnection,
    migrator: PgConnection,
    app_url: String,
}

impl Database {
    async fn open() -> Self {
        let app_url = disposable_url("RUST_TEST_DATABASE_URL", APP_ROLE);
        let migrator_url = disposable_url("RUST_TEST_MIGRATOR_DATABASE_URL", MIGRATOR_ROLE);
        let app_target = url::Url::parse(&app_url).unwrap();
        let migrator_target = url::Url::parse(&migrator_url).unwrap();
        assert_eq!(app_target.host_str(), migrator_target.host_str());
        assert_eq!(app_target.port(), migrator_target.port());
        assert_eq!(app_target.path(), migrator_target.path());
        let mut app = PgConnection::connect(&app_url).await.unwrap();
        let mut migrator = PgConnection::connect(&migrator_url).await.unwrap();
        for (conn, expected) in [(&mut app, APP_ROLE), (&mut migrator, MIGRATOR_ROLE)] {
            let (session, current, superuser, create_role, bypass_rls): (
                String,
                String,
                bool,
                bool,
                bool,
            ) = sqlx::query_as(
                "SELECT session_user::text,current_user::text,rolsuper,rolcreaterole,rolbypassrls \
                 FROM pg_roles WHERE rolname=current_user",
            )
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(session, expected);
            assert_eq!(current, expected);
            assert!(!superuser && !create_role && !bypass_rls);
        }
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut migrator)
            .await
            .unwrap();
        assert_eq!(revision, REVISION);
        let owns_objects: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_class WHERE relnamespace='public'::regnamespace \
             AND relowner=(SELECT oid FROM pg_roles WHERE rolname=current_user))",
        )
        .fetch_one(&mut app)
        .await
        .unwrap();
        assert!(!owns_objects, "runtime must not own migrated objects");
        Self {
            app,
            migrator,
            app_url,
        }
    }

    async fn parties(&mut self) -> (Uuid, Uuid) {
        let organization = Uuid::new_v4();
        let user = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations(id,name,type) VALUES($1,'Authority acceptance','FUEL_BUYER')")
            .bind(organization).execute(&mut self.migrator).await.unwrap();
        sqlx::query("INSERT INTO users(id,email,password_hash,role,organization_id) VALUES($1,$2,'unused','BUYER',$3)")
            .bind(user).bind(format!("{user}@authority.example")).bind(organization)
            .execute(&mut self.migrator).await.unwrap();
        (organization, user)
    }

    async fn document(&mut self, organization: Uuid, user: Uuid) -> Uuid {
        let document = Uuid::new_v4();
        sqlx::query("INSERT INTO kyc_documents(id,submission_id,user_id,organization_id,kind,content_type,size_bytes,key_id,wrapped_key,key_nonce,data_nonce,ciphertext) VALUES($1,$2,$3,$4,'IDENTITY','application/pdf',1,'synthetic',$5,$5,$5,$5)")
            .bind(document).bind(Uuid::new_v4()).bind(user).bind(organization)
            .bind(vec![0_u8]).execute(&mut self.app).await.unwrap();
        document
    }
}

fn disposable_url(variable: &str, role: &str) -> String {
    let raw = std::env::var(variable).expect("explicit disposable database URL required");
    let parsed = url::Url::parse(&raw).expect("valid disposable URL");
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.port().is_some_and(|port| port != 5432));
    assert!(parsed.path().ends_with("_test"));
    assert_eq!(parsed.username(), role);
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    raw
}

fn sqlstate(error: sqlx::Error, expected: &str) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some(expected)
    );
}

async fn denied(conn: &mut PgConnection, statement: &'static str, id: Uuid) {
    let error = sqlx::query(statement)
        .bind(id)
        .execute(conn)
        .await
        .expect_err("runtime write must be denied");
    sqlstate(error, "42501");
}

#[tokio::test]
async fn runtime_documents_and_history_are_append_only() {
    let mut db = Database::open().await;
    let (organization, user) = db.parties().await;
    let document = db.document(organization, user).await;
    let transition = Uuid::new_v4();
    let audit = Uuid::new_v4();
    sqlx::query("INSERT INTO user_status_transitions(id,user_id,organization_id,role,from_status,to_status,effective_at,provenance) VALUES($1,$2,$3,'BUYER','PENDING','APPROVED',now(),'runtime')")
        .bind(transition).bind(user).bind(organization).execute(&mut db.app).await.unwrap();
    sqlx::query("INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,timestamp) VALUES($1,$2,'admin.user_approved','user',$3,now())")
        .bind(audit).bind(user).bind(user.to_string()).execute(&mut db.app).await.unwrap();
    for (id, select, update, delete) in [
        (
            document,
            "SELECT to_jsonb(t) FROM kyc_documents t WHERE id=$1",
            "UPDATE kyc_documents SET id=id WHERE id=$1",
            "DELETE FROM kyc_documents WHERE id=$1",
        ),
        (
            transition,
            "SELECT to_jsonb(t) FROM user_status_transitions t WHERE id=$1",
            "UPDATE user_status_transitions SET id=id WHERE id=$1",
            "DELETE FROM user_status_transitions WHERE id=$1",
        ),
        (
            audit,
            "SELECT to_jsonb(t) FROM audit_logs t WHERE id=$1",
            "UPDATE audit_logs SET id=id WHERE id=$1",
            "DELETE FROM audit_logs WHERE id=$1",
        ),
    ] {
        let before: Value = sqlx::query_scalar(select)
            .bind(id)
            .fetch_one(&mut db.app)
            .await
            .unwrap();
        denied(&mut db.app, update, id).await;
        denied(&mut db.app, delete, id).await;
        let after: Value = sqlx::query_scalar(select)
            .bind(id)
            .fetch_one(&mut db.app)
            .await
            .unwrap();
        assert_eq!(before, after);
    }
}

#[tokio::test]
async fn runtime_cannot_write_provenance_or_grace() {
    let mut db = Database::open().await;
    let (organization, user) = db.parties().await;
    denied(
        &mut db.app,
        "UPDATE organizations SET provenance='REAL' WHERE id=$1",
        organization,
    )
    .await;
    let grace = Uuid::new_v4();
    sqlx::query("INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES($1,$2,$3,'PENDING',now(),now()+interval '1 day','acceptance')")
        .bind(grace).bind(user).bind(organization).execute(&mut db.migrator).await.unwrap();
    let original: Value =
        sqlx::query_scalar("SELECT to_jsonb(g) FROM kyc_execution_grace g WHERE id=$1")
            .bind(grace)
            .fetch_one(&mut db.app)
            .await
            .unwrap();
    let error = sqlx::query("INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES($1,$2,$3,'PENDING',now(),now()+interval '1 day','acceptance')")
        .bind(Uuid::new_v4()).bind(user).bind(organization).execute(&mut db.app).await.unwrap_err();
    sqlstate(error, "42501");
    denied(
        &mut db.app,
        "UPDATE kyc_execution_grace SET expires_at=now() WHERE id=$1",
        grace,
    )
    .await;
    denied(
        &mut db.app,
        "DELETE FROM kyc_execution_grace WHERE id=$1",
        grace,
    )
    .await;
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(g) FROM kyc_execution_grace g WHERE id=$1")
            .bind(grace)
            .fetch_one(&mut db.app)
            .await
            .unwrap();
    assert_eq!(original, after);
}

#[tokio::test]
async fn organization_approval_trigger_reads_back_real_and_vd001() {
    let mut db = Database::open().await;
    let (organization, _) = db.parties().await;
    let provenance: String = sqlx::query_scalar(
        "UPDATE organizations SET verification_status='APPROVED' WHERE id=$1 RETURNING provenance",
    )
    .bind(organization)
    .fetch_one(&mut db.app)
    .await
    .unwrap();
    assert_eq!(provenance, "REAL");
    let (blocked, _) = db.parties().await;
    let product = Uuid::new_v4();
    sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade) VALUES($1,'Authority fuel','Methanol','Bio')")
        .bind(product).execute(&mut db.migrator).await.unwrap();
    for status in ["OPEN", "FILLED"] {
        let order = Uuid::new_v4();
        sqlx::query(include_str!("fixtures/phase4_authority_order_insert.sql"))
            .bind(order)
            .bind(blocked)
            .bind(product)
            .bind(status)
            .execute(&mut db.migrator)
            .await
            .unwrap();
        let error =
            sqlx::query("UPDATE organizations SET verification_status='APPROVED' WHERE id=$1")
                .bind(blocked)
                .execute(&mut db.app)
                .await
                .unwrap_err();
        sqlstate(error, "VD001");
        let state: (String, String) =
            sqlx::query_as("SELECT verification_status,provenance FROM organizations WHERE id=$1")
                .bind(blocked)
                .fetch_one(&mut db.app)
                .await
                .unwrap();
        assert_eq!(state, ("PENDING".into(), "UNKNOWN".into()));
        sqlx::query(include_str!("fixtures/phase4_authority_order_cancel.sql"))
            .bind(order)
            .execute(&mut db.migrator)
            .await
            .unwrap();
    }
    let provenance: String = sqlx::query_scalar(
        "UPDATE organizations SET verification_status='APPROVED' WHERE id=$1 RETURNING provenance",
    )
    .bind(blocked)
    .fetch_one(&mut db.app)
    .await
    .unwrap();
    assert_eq!(provenance, "REAL");
}

#[tokio::test]
async fn retained_documents_block_user_and_org_delete() {
    let mut db = Database::open().await;
    let (organization, user) = db.parties().await;
    let document = db.document(organization, user).await;
    // Remove the ordinary membership FK so the document is the retention boundary.
    sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
        .bind(user)
        .execute(&mut db.migrator)
        .await
        .unwrap();
    for (statement, id) in [
        ("DELETE FROM users WHERE id=$1", user),
        ("DELETE FROM organizations WHERE id=$1", organization),
    ] {
        let error = sqlx::query(statement)
            .bind(id)
            .execute(&mut db.migrator)
            .await
            .unwrap_err();
        assert!(
            error
                .as_database_error()
                .unwrap()
                .constraint()
                .unwrap()
                .starts_with("kyc_documents_")
        );
        sqlstate(error, "23503");
    }
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM kyc_documents WHERE id=$1")
        .bind(document)
        .fetch_one(&mut db.app)
        .await
        .unwrap();
    assert_eq!(retained, 1);
}

#[tokio::test]
async fn rust_service_credential_absence_does_not_fail_readiness() {
    let db = Database::open().await;
    for environment in ["test", "staging", "production"] {
        assert!(KeyRing::load_runtime(None, None, environment).is_err());
    }
    let config = Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: REVISION.into(),
        database_url: db.app_url,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 2,
        pool_max: 3,
        python_budget: 52,
        configured_max_connections: 100,
        prepared_budget: None,
        phase2: serde_json::from_value(json!({"jwt_secret":"synthetic-authority-key-at-least-32-characters","jwt_issuer":"test","jwt_audience":"test","frontend_url":"https://app.example.test","email_from":"team@example.test","resend_api_key":"synthetic","auth_domain_datasets":"null"})).unwrap(),
    };
    let pools = connect(&config).await.unwrap();
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    let state = verdaxis_read::AppState::new(config, pools.clone(), policy).unwrap();
    let response = verdaxis_read::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body,
        json!({"status":"ok","db":"ok","environment":"test","release_sha":compiled_sha()})
    );
    pools.read.close().await;
    pools.health.close().await;
}
