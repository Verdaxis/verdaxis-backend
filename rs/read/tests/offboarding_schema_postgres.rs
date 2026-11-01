//! Database behavior of the individual-offboarding revision.
//!
//! These tests are `#[ignore]`d so a plain `cargo test` reports them as not
//! run. `rs/read/tests/run_offboarding_schema.sh` provides the disposable
//! databases and runs them with `--include-ignored`:
//!
//! * `RUST_TEST_DATABASE_URL` and `RUST_TEST_MIGRATOR_DATABASE_URL` name a
//!   database at the Alembic head with the runtime ACL bundle applied;
//! * `RUST_TEST_OFFBOARDING_MIGRATOR_DATABASE_URL` names a second database at
//!   the head, owned by the migrator, on which the downgrade case runs
//!   migrations in both directions;
//! * `RUST_TEST_PYTHON` is the interpreter that carries the Alembic dependencies.
//!
//! Every case uses its own identifiers, so cases do not share rows.

use std::process::{Command, Output};

use sqlx::{Connection, PgConnection};
use uuid::Uuid;

const PARENT: &str = "p89_20261022_trade_freeze";
const REVISION: &str = "ob_20261031_offboarding";
const APP_ROLE: &str = "verdaxis_app_staging";
const MIGRATOR_ROLE: &str = "verdaxis_migrator_staging";
const BACKUP_ROLE: &str = "verdaxis_backup_staging";

const REJECTED_CONSTRAINT: &str = "ck_users_offboarded_requires_rejected";
const DETACHED_CONSTRAINT: &str = "ck_users_offboarded_requires_detached";

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

struct Database {
    app: PgConnection,
    migrator: PgConnection,
}

impl Database {
    async fn open() -> Self {
        let app = PgConnection::connect(&disposable_url("RUST_TEST_DATABASE_URL", APP_ROLE))
            .await
            .unwrap();
        let mut migrator = PgConnection::connect(&disposable_url(
            "RUST_TEST_MIGRATOR_DATABASE_URL",
            MIGRATOR_ROLE,
        ))
        .await
        .unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&mut migrator)
            .await
            .unwrap();
        assert_eq!(revision, REVISION);
        Self { app, migrator }
    }

    async fn organization(&mut self) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations(id,name,type) VALUES($1,'Offboarding schema','FUEL_BUYER')",
        )
        .bind(id)
        .execute(&mut self.migrator)
        .await
        .unwrap();
        id
    }

    async fn user(&mut self, status: &str, organization: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users(id,email,password_hash,role,status,organization_id) \
             VALUES($1,$2,'unused','BUYER',$3,$4)",
        )
        .bind(id)
        .bind(format!("{id}@offboarding.example"))
        .bind(status)
        .bind(organization)
        .execute(&mut self.migrator)
        .await
        .unwrap();
        id
    }
}

fn sqlstate_and_constraint(error: sqlx::Error) -> (String, Option<String>) {
    let database = error.as_database_error().expect("a database error");
    (
        database.code().expect("an SQLSTATE").to_string(),
        database.constraint().map(str::to_owned),
    )
}

/// The statement must fail on exactly the named constraint.
async fn refused_by(
    conn: &mut PgConnection,
    statement: &'static str,
    user: Uuid,
    constraint: &str,
) {
    let error = sqlx::query(statement)
        .bind(user)
        .execute(conn)
        .await
        .expect_err("the marker constraints must refuse this write");
    let (code, name) = sqlstate_and_constraint(error);
    assert_eq!(code, "23514", "{statement}");
    assert_eq!(name.as_deref(), Some(constraint), "{statement}");
}

async fn denied(conn: &mut PgConnection, statement: &'static str) {
    let error = sqlx::query(statement)
        .execute(conn)
        .await
        .expect_err("runtime write must be denied");
    assert_eq!(sqlstate_and_constraint(error).0, "42501", "{statement}");
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marker_requires_rejected_and_detached() {
    let mut db = Database::open().await;
    let organization = db.organization().await;

    // Detached but approved: only the status rule refuses the marker.
    let approved_detached = db.user("APPROVED", None).await;
    refused_by(
        &mut db.app,
        "UPDATE users SET offboarded_at=now() WHERE id=$1",
        approved_detached,
        REJECTED_CONSTRAINT,
    )
    .await;

    // Rejected but still attached: only the attachment rule refuses it.
    let rejected_attached = db.user("REJECTED", Some(organization)).await;
    refused_by(
        &mut db.app,
        "UPDATE users SET offboarded_at=now() WHERE id=$1",
        rejected_attached,
        DETACHED_CONSTRAINT,
    )
    .await;

    // The runtime can write the full marked state in one statement.
    let subject = db.user("APPROVED", Some(organization)).await;
    sqlx::query(
        "UPDATE users SET status='REJECTED', organization_id=NULL, offboarded_at=now() WHERE id=$1",
    )
    .bind(subject)
    .execute(&mut db.app)
    .await
    .unwrap();

    // Approval, attachment and a plain pending reset all fail while the marker is set.
    for (statement, constraint) in [
        (
            "UPDATE users SET status='APPROVED' WHERE id=$1",
            REJECTED_CONSTRAINT,
        ),
        (
            "UPDATE users SET status='PENDING' WHERE id=$1",
            REJECTED_CONSTRAINT,
        ),
    ] {
        refused_by(&mut db.app, statement, subject, constraint).await;
    }
    let join = sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
        .bind(subject)
        .bind(organization)
        .execute(&mut db.app)
        .await
        .expect_err("a join must not reattach a marked user");
    assert_eq!(
        sqlstate_and_constraint(join),
        ("23514".to_owned(), Some(DETACHED_CONSTRAINT.to_owned()))
    );
    // Approval together with attachment fails on the first rule it breaks, never silently.
    let both = sqlx::query("UPDATE users SET status='APPROVED', organization_id=$2 WHERE id=$1")
        .bind(subject)
        .bind(organization)
        .execute(&mut db.app)
        .await;
    assert!(both.is_err());

    // A new marked row must satisfy both rules at insert time as well.
    for (status, attached) in [("APPROVED", false), ("REJECTED", true)] {
        let id = Uuid::new_v4();
        let error = sqlx::query(
            "INSERT INTO users(id,email,password_hash,role,status,organization_id,offboarded_at) \
             VALUES($1,$2,'unused','BUYER',$3,$4,now())",
        )
        .bind(id)
        .bind(format!("{id}@offboarding.example"))
        .bind(status)
        .bind(attached.then_some(organization))
        .execute(&mut db.app)
        .await
        .expect_err("marked inserts must satisfy both rules");
        assert_eq!(sqlstate_and_constraint(error).0, "23514");
    }

    // A reviewed reinstatement clears the marker in the same statement that restores state.
    sqlx::query("UPDATE users SET offboarded_at=NULL, status='APPROVED' WHERE id=$1")
        .bind(subject)
        .execute(&mut db.app)
        .await
        .unwrap();
    let marker: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT offboarded_at FROM users WHERE id=$1")
            .bind(subject)
            .fetch_one(&mut db.migrator)
            .await
            .unwrap();
    assert!(marker.is_none());
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn null_marker_keeps_existing_states() {
    let mut db = Database::open().await;
    let organization = db.organization().await;

    let (column_type, nullable, default): (String, String, Option<String>) = sqlx::query_as(
        "SELECT data_type::text, is_nullable::text, column_default::text \
         FROM information_schema.columns \
         WHERE table_schema='public' AND table_name='users' AND column_name='offboarded_at'",
    )
    .fetch_one(&mut db.migrator)
    .await
    .unwrap();
    assert_eq!(column_type, "timestamp with time zone");
    assert_eq!(nullable, "YES");
    assert!(default.is_none(), "an unmarked row is the default");

    let validated: Vec<(String, bool)> = sqlx::query_as(
        "SELECT conname::text, convalidated FROM pg_constraint \
         WHERE conrelid='public.users'::regclass AND contype='c' \
           AND conname = ANY($1) ORDER BY conname",
    )
    .bind(vec![REJECTED_CONSTRAINT, DETACHED_CONSTRAINT])
    .fetch_all(&mut db.migrator)
    .await
    .unwrap();
    assert_eq!(
        validated,
        [
            (DETACHED_CONSTRAINT.to_owned(), true),
            (REJECTED_CONSTRAINT.to_owned(), true)
        ]
    );

    // Every existing combination of status and attachment is still accepted unmarked.
    for status in ["PENDING", "APPROVED", "REJECTED"] {
        for attached in [false, true] {
            let user = db.user(status, attached.then_some(organization)).await;
            let marker: Option<chrono::DateTime<chrono::Utc>> =
                sqlx::query_scalar("SELECT offboarded_at FROM users WHERE id=$1")
                    .bind(user)
                    .fetch_one(&mut db.migrator)
                    .await
                    .unwrap();
            assert!(marker.is_none(), "{status} attached={attached}");
        }
    }

    // The ordinary lifecycle of an unmarked user is unchanged: approve, reject,
    // detach, rejoin and approve again.
    let user = db.user("PENDING", Some(organization)).await;
    for statement in [
        "UPDATE users SET status='APPROVED' WHERE id=$1",
        "UPDATE users SET status='REJECTED' WHERE id=$1",
        "UPDATE users SET organization_id=NULL WHERE id=$1",
        "UPDATE users SET status='PENDING' WHERE id=$1",
        "UPDATE users SET status='APPROVED' WHERE id=$1",
    ] {
        sqlx::query(statement)
            .bind(user)
            .execute(&mut db.app)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn offboarding_audit_is_append_only() {
    let mut db = Database::open().await;
    let organization = db.organization().await;
    let subject = db.user("REJECTED", None).await;
    let actor = db.user("APPROVED", Some(organization)).await;

    let insert = "INSERT INTO individual_offboardings(operation_id,subject_user_id,\
                  former_organization_id,actor_user_id,occurred_at,reason_code,\
                  evidence_case_ref,urgent) VALUES($1,$2,$3,$4,now(),$5,$6,$7)";
    let operation = Uuid::new_v4();
    sqlx::query(insert)
        .bind(operation)
        .bind(subject)
        .bind(Some(organization))
        .bind(actor)
        .bind("LEFT_COMPANY")
        .bind("CASE-2026-0001")
        .bind(false)
        .execute(&mut db.app)
        .await
        .unwrap();
    // A detached former organization is allowed: a detached target has none.
    sqlx::query(insert)
        .bind(Uuid::new_v4())
        .bind(subject)
        .bind(None::<Uuid>)
        .bind(actor)
        .bind("LEFT_COMPANY")
        .bind("CASE-2026-0002")
        .bind(true)
        .execute(&mut db.app)
        .await
        .unwrap();
    let urgent: bool =
        sqlx::query_scalar("SELECT urgent FROM individual_offboardings WHERE operation_id=$1")
            .bind(operation)
            .fetch_one(&mut db.app)
            .await
            .unwrap();
    assert!(!urgent);

    // The runtime role can neither rewrite nor remove history.
    for statement in [
        "UPDATE individual_offboardings SET urgent=true",
        "UPDATE individual_offboardings SET reason_code='OTHER'",
        "DELETE FROM individual_offboardings",
        "TRUNCATE individual_offboardings",
    ] {
        denied(&mut db.app, statement).await;
    }

    // Exact grants by role: runtime SELECT and INSERT, backup SELECT, nobody else.
    for (role, privilege, expected) in [
        (APP_ROLE, "SELECT", true),
        (APP_ROLE, "INSERT", true),
        (APP_ROLE, "UPDATE", false),
        (APP_ROLE, "DELETE", false),
        (APP_ROLE, "TRUNCATE", false),
        (APP_ROLE, "REFERENCES", false),
        (APP_ROLE, "TRIGGER", false),
        (BACKUP_ROLE, "SELECT", true),
        (BACKUP_ROLE, "INSERT", false),
        (BACKUP_ROLE, "UPDATE", false),
        (BACKUP_ROLE, "DELETE", false),
        (BACKUP_ROLE, "TRUNCATE", false),
    ] {
        let granted: bool = sqlx::query_scalar(
            "SELECT has_table_privilege($1::name,'public.individual_offboardings',$2::text)",
        )
        .bind(role)
        .bind(privilege)
        .fetch_one(&mut db.migrator)
        .await
        .unwrap();
        assert_eq!(granted, expected, "{role} {privilege}");
    }
    let column_update: bool = sqlx::query_scalar(
        "SELECT bool_or(has_column_privilege($1::name,'public.individual_offboardings',attname,'UPDATE')) \
         FROM pg_attribute WHERE attrelid='public.individual_offboardings'::regclass \
           AND attnum>0 AND NOT attisdropped",
    )
    .bind(APP_ROLE)
    .fetch_one(&mut db.migrator)
    .await
    .unwrap();
    assert!(!column_update, "no column carries update authority");
    let public_grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c, aclexplode(COALESCE(c.relacl, acldefault('r', c.relowner))) a \
         WHERE c.oid='public.individual_offboardings'::regclass AND a.grantee=0",
    )
    .fetch_one(&mut db.migrator)
    .await
    .unwrap();
    assert_eq!(public_grants, 0, "PUBLIC holds no privilege");

    // The record bounds are enforced by the database, whoever inserts.
    let long_reference = "x".repeat(65);
    let attempts: [(Uuid, Uuid, &str, &str, &str); 6] = [
        (
            subject,
            subject,
            "LEFT_COMPANY",
            "CASE-1",
            "self offboarding",
        ),
        (subject, actor, "left company", "CASE-1", "reason grammar"),
        (subject, actor, "L", "CASE-1", "reason length"),
        (
            subject,
            actor,
            "LEFT_COMPANY",
            "has a space",
            "case reference whitespace",
        ),
        (subject, actor, "LEFT_COMPANY", "", "case reference empty"),
        (
            subject,
            actor,
            "LEFT_COMPANY",
            &long_reference,
            "case reference length",
        ),
    ];
    for (subject_id, actor_id, reason, reference, label) in attempts {
        let error = sqlx::query(insert)
            .bind(Uuid::new_v4())
            .bind(subject_id)
            .bind(None::<Uuid>)
            .bind(actor_id)
            .bind(reason)
            .bind(reference)
            .bind(false)
            .execute(&mut db.app)
            .await
            .expect_err(label);
        assert_eq!(sqlstate_and_constraint(error).0, "23514", "{label}");
    }
    // Identity must exist, and a user with retained history cannot be deleted.
    let unknown = sqlx::query(insert)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(None::<Uuid>)
        .bind(actor)
        .bind("LEFT_COMPANY")
        .bind("CASE-3")
        .bind(false)
        .execute(&mut db.app)
        .await
        .expect_err("unknown subject");
    assert_eq!(sqlstate_and_constraint(unknown).0, "23503");
    // The former organization is retained too: an organization with no members, named only by a record.
    let former = db.organization().await;
    sqlx::query(insert)
        .bind(Uuid::new_v4())
        .bind(subject)
        .bind(Some(former))
        .bind(actor)
        .bind("LEFT_COMPANY")
        .bind("CASE-4")
        .bind(false)
        .execute(&mut db.app)
        .await
        .unwrap();
    let removals: [(&str, &'static str, Uuid); 3] = [
        ("users", "DELETE FROM users WHERE id=$1", subject),
        ("users", "DELETE FROM users WHERE id=$1", actor),
        (
            "organizations",
            "DELETE FROM organizations WHERE id=$1",
            former,
        ),
    ];
    for (table, statement, id) in removals {
        let error = sqlx::query(statement)
            .bind(id)
            .execute(&mut db.migrator)
            .await
            .expect_err("history restricts deletion");
        let (code, constraint) = sqlstate_and_constraint(error);
        assert_eq!(code, "23503", "{table}");
        assert!(
            constraint.is_some_and(|name| name.starts_with("fk_individual_offboardings_")),
            "{table}"
        );
    }
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn existing_runtime_grants_are_preserved() {
    let mut db = Database::open().await;
    let all = ["SELECT", "INSERT", "UPDATE", "DELETE"];
    let expected: [(&str, &[&str]); 12] = [
        ("users", &all),
        ("audit_logs", &["SELECT", "INSERT"]),
        ("user_status_transitions", &["SELECT", "INSERT"]),
        ("refresh_sessions", &all),
        ("password_reset_tokens", &["SELECT", "DELETE"]),
        ("orderbook_orders", &all),
        ("trades", &all),
        ("market_event_outbox", &["SELECT", "INSERT", "UPDATE"]),
        ("market_support_contexts", &["SELECT", "INSERT"]),
        ("negotiations", &["SELECT", "INSERT", "DELETE"]),
        ("organizations", &["SELECT", "DELETE"]),
        ("kyc_documents", &["SELECT", "INSERT"]),
    ];
    for (table, granted) in expected {
        for privilege in [
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "TRUNCATE",
            "REFERENCES",
            "TRIGGER",
        ] {
            let actual: bool =
                sqlx::query_scalar("SELECT has_table_privilege($1::name,$2::text,$3::text)")
                    .bind(APP_ROLE)
                    .bind(format!("public.{table}"))
                    .bind(privilege)
                    .fetch_one(&mut db.migrator)
                    .await
                    .unwrap();
            assert_eq!(actual, granted.contains(&privilege), "{table} {privilege}");
        }
    }
    // The runtime role writes the new column through its existing users authority only.
    let column_write: bool = sqlx::query_scalar(
        "SELECT has_column_privilege($1::name,'public.users','offboarded_at','UPDATE')",
    )
    .bind(APP_ROLE)
    .fetch_one(&mut db.migrator)
    .await
    .unwrap();
    assert!(column_write);
}

/// Run Alembic as the migrator against the downgrade database.
fn alembic(url: &str, arguments: &[&str]) -> Output {
    let python =
        std::env::var("RUST_TEST_PYTHON").expect("explicit project Python interpreter required");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sqlalchemy_url = url.replacen("postgres://", "postgresql+asyncpg://", 1);
    Command::new(python)
        .args(["-m", "alembic"])
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
        .env("DATABASE_URL", &sqlalchemy_url)
        .env("MIGRATOR_DATABASE_URL", &sqlalchemy_url)
        .output()
        .expect("run alembic")
}

async fn current_revision(conn: &mut PgConnection) -> String {
    sqlx::query_scalar("SELECT version_num FROM alembic_version")
        .fetch_one(conn)
        .await
        .unwrap()
}

async fn column_and_table_exist(conn: &mut PgConnection) -> (bool, bool) {
    sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM information_schema.columns \
                       WHERE table_schema='public' AND table_name='users' AND column_name='offboarded_at'), \
                to_regclass('public.individual_offboardings') IS NOT NULL",
    )
    .fetch_one(conn)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn downgrade_cannot_restore_access_or_erase_evidence() {
    let url = disposable_url("RUST_TEST_OFFBOARDING_MIGRATOR_DATABASE_URL", MIGRATOR_ROLE);
    let mut conn = PgConnection::connect(&url).await.unwrap();
    assert_eq!(current_revision(&mut conn).await, REVISION);

    let refused = |output: &Output| {
        assert!(!output.status.success(), "the downgrade must refuse");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot downgrade individual offboarding"),
            "refusal reason missing: {stderr}"
        );
    };

    // 1. A marked user alone blocks the downgrade; nothing changes.
    let marked = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,email,password_hash,role,status,offboarded_at) \
         VALUES($1,$2,'unused','BUYER','REJECTED',now())",
    )
    .bind(marked)
    .bind(format!("{marked}@offboarding.example"))
    .execute(&mut conn)
    .await
    .unwrap();
    refused(&alembic(&url, &["downgrade", PARENT]));
    assert_eq!(current_revision(&mut conn).await, REVISION);
    assert_eq!(column_and_table_exist(&mut conn).await, (true, true));
    let still_marked: bool =
        sqlx::query_scalar("SELECT offboarded_at IS NOT NULL FROM users WHERE id=$1")
            .bind(marked)
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert!(still_marked);

    // 2. Retained evidence alone blocks it, even after the marker is cleared by a reinstatement.
    let actor = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,role,status) VALUES($1,$2,'unused','ADMIN','APPROVED')")
        .bind(actor)
        .bind(format!("{actor}@offboarding.example"))
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO individual_offboardings(operation_id,subject_user_id,actor_user_id,occurred_at,\
         reason_code,evidence_case_ref,urgent) VALUES($1,$2,$3,now(),'LEFT_COMPANY','CASE-DOWN-1',false)",
    )
    .bind(Uuid::new_v4())
    .bind(marked)
    .bind(actor)
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET offboarded_at=NULL WHERE id=$1")
        .bind(marked)
        .execute(&mut conn)
        .await
        .unwrap();
    refused(&alembic(&url, &["downgrade", PARENT]));
    assert_eq!(current_revision(&mut conn).await, REVISION);
    assert_eq!(column_and_table_exist(&mut conn).await, (true, true));
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM individual_offboardings")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(records, 1);

    // 3. With neither marker nor record the downgrade runs and removes exactly the new objects.
    sqlx::query("DELETE FROM individual_offboardings")
        .execute(&mut conn)
        .await
        .unwrap();
    let output = alembic(&url, &["downgrade", PARENT]);
    assert!(
        output.status.success(),
        "downgrade failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(current_revision(&mut conn).await, PARENT);
    assert_eq!(column_and_table_exist(&mut conn).await, (false, false));
    let leftover: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE conrelid='public.users'::regclass AND conname = ANY($1)",
    )
    .bind(vec![REJECTED_CONSTRAINT, DETACHED_CONSTRAINT])
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert_eq!(leftover, 0);

    // 4. Users that exist before the upgrade, in every existing state, survive it unchanged.
    let organization = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO organizations(id,name,type) VALUES($1,'Offboarding upgrade','FUEL_BUYER')",
    )
    .bind(organization)
    .execute(&mut conn)
    .await
    .unwrap();
    let mut existing = Vec::new();
    for status in ["PENDING", "APPROVED", "REJECTED"] {
        for attached in [false, true] {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO users(id,email,password_hash,role,status,organization_id) \
                 VALUES($1,$2,'unused','BUYER',$3,$4)",
            )
            .bind(id)
            .bind(format!("{id}@offboarding.example"))
            .bind(status)
            .bind(attached.then_some(organization))
            .execute(&mut conn)
            .await
            .unwrap();
            existing.push((id, status, attached));
        }
    }
    let output = alembic(&url, &["upgrade", REVISION]);
    assert!(
        output.status.success(),
        "upgrade failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(current_revision(&mut conn).await, REVISION);
    assert_eq!(column_and_table_exist(&mut conn).await, (true, true));
    for (id, status, attached) in existing {
        let (stored, marker, member): (String, bool, bool) = sqlx::query_as(
            "SELECT status, offboarded_at IS NOT NULL, organization_id IS NOT NULL FROM users WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!((stored.as_str(), marker, member), (status, false, attached));
    }
}
