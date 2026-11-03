//! Phase 10 Task 6: `migrate status` and adoption of an Alembic-built database
//! into the sqlx history, on the disposable PostgreSQL of `run_postgres.sh`.
//!
//! Each case clones `RUST_ADOPTION_TEMPLATE_DATABASE` (Alembic-built at the
//! baseline head and owned by the production migrator name) or creates an empty
//! database, works as the migrator through `SET ROLE`, and drops its database.
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use std::str::FromStr;
use verdaxis_read::cli::migrate::{
    Adoption, AdoptionRefusal, MIGRATOR, adopt, compiled_baseline, fingerprint, status,
};

const MIGRATOR_ROLE: &str = "verdaxis_migrator";

fn admin_url() -> String {
    std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server")
}

fn adoption_head() -> String {
    std::env::var("RUST_ADOPTION_ALEMBIC_HEAD").expect("RUST_ADOPTION_ALEMBIC_HEAD")
}

async fn connect(database: &str) -> PgConnection {
    let options = PgConnectOptions::from_str(&admin_url())
        .expect("parse the administrator URL")
        .database(database);
    PgConnection::connect_with(&options)
        .await
        .expect("connect to the disposable server")
}

/// A connection to `database` acting as the migrator role.
async fn as_migrator(database: &str) -> PgConnection {
    let mut connection = connect(database).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET ROLE {MIGRATOR_ROLE}")))
        .execute(&mut connection)
        .await
        .expect("become the migrator");
    connection
}

fn case_name(label: &str) -> String {
    format!("verdaxis_migrate_{label}_{}", uuid::Uuid::new_v4().simple())
}

/// A copy of the Alembic-built template.
async fn alembic_database(label: &str) -> String {
    let template =
        std::env::var("RUST_ADOPTION_TEMPLATE_DATABASE").expect("RUST_ADOPTION_TEMPLATE_DATABASE");
    let name = case_name(label);
    let mut admin = connect("postgres").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} TEMPLATE {template} OWNER {MIGRATOR_ROLE}"
    )))
    .execute(&mut admin)
    .await
    .expect("clone the Alembic-built template");
    name
}

/// An empty database prepared as on the deployed hosts: PostGIS created by the
/// administrator, the schema owned by the migrator.
async fn empty_database(label: &str) -> String {
    let name = case_name(label);
    let mut admin = connect("postgres").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER {MIGRATOR_ROLE}"
    )))
    .execute(&mut admin)
    .await
    .expect("create an empty database");
    let mut database = connect(&name).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE EXTENSION IF NOT EXISTS postgis; ALTER SCHEMA public OWNER TO {MIGRATOR_ROLE};"
    )))
    .execute(&mut database)
    .await
    .expect("prepare the empty database");
    name
}

async fn drop_database(name: &str) {
    let mut admin = connect("postgres").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
    )))
    .execute(&mut admin)
    .await
    .expect("drop the case database");
}

/// The catalog of the history table: columns, constraints and indexes.
async fn history_catalog(database: &str) -> Vec<String> {
    let mut connection = connect(database).await;
    sqlx::query_scalar(
        "SELECT format('column|%s|%s|%s|%s', a.attname, format_type(a.atttypid, a.atttypmod), \
                       a.attnotnull, COALESCE(pg_get_expr(d.adbin, d.adrelid), '')) \
         FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = 'public._sqlx_migrations'::regclass AND a.attnum > 0 AND NOT a.attisdropped \
         UNION ALL \
         SELECT format('constraint|%s|%s', contype, pg_get_constraintdef(oid)) \
         FROM pg_constraint WHERE conrelid = 'public._sqlx_migrations'::regclass \
         UNION ALL \
         SELECT format('index|%s', regexp_replace(pg_get_indexdef(indexrelid), '^CREATE UNIQUE INDEX \\S+', 'CREATE UNIQUE INDEX')) \
         FROM pg_index WHERE indrelid = 'public._sqlx_migrations'::regclass \
         ORDER BY 1",
    )
    .fetch_all(&mut connection)
    .await
    .expect("history table catalog")
}

async fn history_rows(database: &str) -> Vec<(i64, String, bool, Vec<u8>, String)> {
    let mut connection = connect(database).await;
    sqlx::query(
        "SELECT version, description, success, checksum, installed_on::text AS installed_on \
         FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("history rows")
    .iter()
    .map(|row| {
        (
            row.get("version"),
            row.get("description"),
            row.get("success"),
            row.get("checksum"),
            row.get("installed_on"),
        )
    })
    .collect()
}

async fn history_exists(database: &str) -> bool {
    let mut connection = connect(database).await;
    sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut connection)
        .await
        .expect("history table lookup")
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn adoption_row_matches_sqlx_created_table() {
    // sqlx creates its own history on an empty database...
    let fresh = empty_database("fresh").await;
    MIGRATOR
        .run(&mut as_migrator(&fresh).await)
        .await
        .expect("sqlx runs the baseline on an empty database");
    // ...and adoption creates it on an Alembic-built one.
    let adopted = alembic_database("adopted").await;
    let outcome = adopt(
        &mut as_migrator(&adopted).await,
        &adoption_head(),
        &compiled_baseline(),
    )
    .await
    .expect("adopt the Alembic-built database");
    assert_eq!(outcome, Adoption::Adopted);
    assert_eq!(
        history_catalog(&adopted).await,
        history_catalog(&fresh).await
    );
    drop_database(&fresh).await;
    drop_database(&adopted).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn adoption_checksum_matches_sqlx() {
    let embedded = MIGRATOR.iter().next().expect("the baseline migration");
    let fresh = empty_database("checksum_fresh").await;
    MIGRATOR
        .run(&mut as_migrator(&fresh).await)
        .await
        .expect("sqlx runs the baseline");
    let adopted = alembic_database("checksum_adopted").await;
    adopt(
        &mut as_migrator(&adopted).await,
        &adoption_head(),
        &compiled_baseline(),
    )
    .await
    .expect("adopt");
    let sqlx_row = history_rows(&fresh).await.remove(0);
    let adopted_row = history_rows(&adopted).await.remove(0);
    assert_eq!(adopted_row.3, &*embedded.checksum, "adoption checksum");
    assert_eq!(adopted_row.3, sqlx_row.3, "sqlx's own row");
    assert_eq!(
        (adopted_row.0, &adopted_row.1, adopted_row.2),
        (sqlx_row.0, &sqlx_row.1, sqlx_row.2)
    );
    drop_database(&fresh).await;
    drop_database(&adopted).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn migrate_status_is_read_only_and_reports_pending() {
    let database = alembic_database("status").await;
    let before = fingerprint(&mut connect(&database).await)
        .await
        .expect("fingerprint");
    let report = status(&mut as_migrator(&database).await)
        .await
        .expect("status");
    assert_eq!(report.alembic, vec![adoption_head()]);
    assert!(report.history.is_empty());
    assert_eq!(report.pending, vec![1]);
    assert!(report.foreign.is_empty());
    assert_eq!(report.revision(), Some(adoption_head()));
    assert!(
        !history_exists(&database).await,
        "status created the history table"
    );
    assert_eq!(
        fingerprint(&mut connect(&database).await)
            .await
            .expect("fingerprint"),
        before
    );
    drop_database(&database).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn baseline_refuses_wrong_head_fingerprint_or_foreign_failed_history() {
    let baseline = compiled_baseline();

    let database = alembic_database("wrong_head").await;
    let refused = adopt(
        &mut as_migrator(&database).await,
        "p5_20261020_read_indexes",
        &baseline,
    )
    .await;
    assert_eq!(refused, Err(AdoptionRefusal::BaselineHead));
    let mut other_head = baseline.clone();
    other_head.alembic_head = "p5_20261020_read_indexes".into();
    let refused = adopt(
        &mut as_migrator(&database).await,
        "p5_20261020_read_indexes",
        &other_head,
    )
    .await;
    assert_eq!(refused, Err(AdoptionRefusal::AlembicRevision));
    assert!(!history_exists(&database).await);
    drop_database(&database).await;

    let database = alembic_database("catalog").await;
    sqlx::raw_sql("ALTER TABLE public.products ADD COLUMN adoption_probe integer")
        .execute(&mut as_migrator(&database).await)
        .await
        .expect("change the catalog");
    let refused = adopt(
        &mut as_migrator(&database).await,
        &adoption_head(),
        &baseline,
    )
    .await;
    assert_eq!(refused, Err(AdoptionRefusal::Fingerprint));
    assert!(!history_exists(&database).await);
    drop_database(&database).await;

    let embedded = MIGRATOR.iter().next().expect("the baseline migration");
    for (label, version, success, checksum) in [
        ("failed", 1_i64, false, embedded.checksum.to_vec()),
        ("foreign", 2, true, vec![0_u8; 48]),
        ("checksum", 1, true, vec![0_u8; 48]),
    ] {
        let database = alembic_database(label).await;
        let mut connection = as_migrator(&database).await;
        sqlx::raw_sql(
            "CREATE TABLE public._sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
             installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), success BOOLEAN NOT NULL, \
             checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL)",
        )
        .execute(&mut connection)
        .await
        .expect("a pre-existing history");
        sqlx::query(
            "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES ($1, 'baseline', $2, $3, 0)",
        )
        .bind(version)
        .bind(success)
        .bind(&checksum)
        .execute(&mut connection)
        .await
        .expect("seed the history row");
        let before = history_rows(&database).await;
        let refused = adopt(
            &mut as_migrator(&database).await,
            &adoption_head(),
            &baseline,
        )
        .await;
        assert_eq!(refused, Err(AdoptionRefusal::History), "{label}");
        assert_eq!(
            history_rows(&database).await,
            before,
            "{label}: history changed"
        );
        drop_database(&database).await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn exact_adoption_resume_is_read_only_and_idempotent() {
    let database = alembic_database("resume").await;
    let baseline = compiled_baseline();
    assert_eq!(
        adopt(
            &mut as_migrator(&database).await,
            &adoption_head(),
            &baseline
        )
        .await,
        Ok(Adoption::Adopted)
    );
    let first = history_rows(&database).await;
    assert_eq!(first.len(), 1);
    for _ in 0..2 {
        assert_eq!(
            adopt(
                &mut as_migrator(&database).await,
                &adoption_head(),
                &baseline
            )
            .await,
            Ok(Adoption::AlreadyAdopted)
        );
        assert_eq!(
            history_rows(&database).await,
            first,
            "a resume changed the history"
        );
    }
    let report = status(&mut connect(&database).await).await.expect("status");
    assert!(report.pending.is_empty() && report.foreign.is_empty());
    assert_eq!(report.revision(), Some("sqlx_1".into()));
    drop_database(&database).await;
}
