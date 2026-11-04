//! Phase 10 Task 6: `migrate status` and adoption of an Alembic-built database
//! into the sqlx history, on the disposable PostgreSQL of `run_postgres.sh`.
//!
//! Each case clones `RUST_ADOPTION_TEMPLATE_DATABASE` (Alembic-built at the
//! baseline head and owned by the production migrator name) or creates an empty
//! database, works as the migrator through `SET ROLE`, and drops its database.
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use std::str::FromStr;
use verdaxis_read::cli::migrate::{
    Adoption, AdoptionRefusal, MIGRATOR, RunOutcome, RunRefusal, TransitionPolicy, adopt,
    compiled_baseline, fingerprint, run_to_target, run_transition, status, status_with,
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

/// Task 7: the deploy runs one approved transition from any current literal.
/// An Alembic literal to itself is a verify-only deploy; the baseline head to
/// `sqlx_1` is adoption; any other Alembic upgrade is refused, because it
/// needs the Python schema tool. Nothing is written by a refusal or a dry run.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn deploy_transitions_cover_alembic_no_change_and_adoption() {
    let database = alembic_database("transition").await;
    let head = adoption_head();
    let older = "p5_20261020_read_indexes";
    let rules = policy(&[
        (head.as_str(), head.as_str()),
        (head.as_str(), "sqlx_1"),
        (older, older),
        (older, head.as_str()),
    ]);
    let transition = |current: &str, target: &str, apply: bool| {
        let (database, rules, current, target) = (
            database.clone(),
            &rules,
            current.to_owned(),
            target.to_owned(),
        );
        async move {
            run_transition(
                &mut as_migrator(&database).await,
                &MIGRATOR,
                rules,
                &compiled_baseline(),
                &current,
                &target,
                apply,
            )
            .await
        }
    };

    assert_eq!(
        transition(&head, &head, true).await,
        Ok(RunOutcome::AtTarget)
    );
    assert_eq!(
        transition(older, older, true).await,
        Err(RunRefusal::CurrentRevision),
        "a no-change deploy checks the exact current literal"
    );
    assert_eq!(
        transition(older, &head, true).await,
        Err(RunRefusal::Target),
        "an Alembic upgrade needs the Python schema tool"
    );
    assert_eq!(
        transition(&head, "sqlx_2", true).await,
        Err(RunRefusal::Transition),
        "an unlisted pair is refused before anything else"
    );
    assert_eq!(
        transition(&head, "sqlx_1", false).await,
        Ok(RunOutcome::WouldApply(vec![1]))
    );
    assert!(!history_exists(&database).await, "no write before adoption");

    assert_eq!(
        transition(&head, "sqlx_1", true).await,
        Ok(RunOutcome::Applied(vec![1]))
    );
    assert_eq!(history_rows(&database).await.len(), 1);
    drop_database(&database).await;
}

/// Carried from Task 6. Adoption has three publication boundaries: inside its
/// one transaction (an interruption rolls everything back), after its commit
/// (the deploy has not recorded success) and after the deploy published the
/// new revision. A rerun with the same approval adopts once and then reports
/// `AtTarget` without a write; a stale no-change approval is refused.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn adoption_commit_then_interruption_resumes_each_publication_boundary() {
    let database = alembic_database("interrupt").await;
    let head = adoption_head();
    let rules = policy(&[(head.as_str(), head.as_str()), (head.as_str(), "sqlx_1")]);
    let adopt_once = || {
        let (database, rules, head) = (database.clone(), &rules, head.clone());
        async move {
            run_transition(
                &mut as_migrator(&database).await,
                &MIGRATOR,
                rules,
                &compiled_baseline(),
                &head,
                "sqlx_1",
                true,
            )
            .await
        }
    };

    // Boundary 1: the process dies inside the adoption transaction, after the
    // history table and row exist but before COMMIT.
    {
        let mut connection = as_migrator(&database).await;
        let mut transaction = connection.begin().await.expect("begin");
        sqlx::raw_sql(
            "CREATE TABLE public._sqlx_migrations (version BIGINT PRIMARY KEY, \
             description TEXT NOT NULL, installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), \
             success BOOLEAN NOT NULL, checksum BYTEA NOT NULL, execution_time BIGINT NOT NULL); \
             INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (1, 'baseline', true, '\\x00', 0);",
        )
        .execute(&mut *transaction)
        .await
        .expect("the interrupted adoption writes");
        drop(transaction);
        drop(connection);
    }
    assert!(
        !history_exists(&database).await,
        "an uncommitted adoption leaves nothing"
    );
    assert_eq!(adopt_once().await, Ok(RunOutcome::Applied(vec![1])));
    let adopted = history_rows(&database).await;

    // Boundaries 2 and 3: after COMMIT, and after the deploy published
    // `sqlx_1`. The same approval resumes without a write.
    for _ in 0..2 {
        assert_eq!(adopt_once().await, Ok(RunOutcome::AtTarget));
        assert_eq!(history_rows(&database).await, adopted, "a resume wrote");
    }
    // A stale no-change approval for the old head no longer matches.
    let stale = run_transition(
        &mut as_migrator(&database).await,
        &MIGRATOR,
        &rules,
        &compiled_baseline(),
        &head,
        &head,
        true,
    )
    .await;
    assert_eq!(stale, Err(RunRefusal::CurrentRevision));
    drop_database(&database).await;
}

/// The embedded baseline plus a second migration, in a fresh folder.
async fn migrator_with_second_version() -> Migrator {
    migrator_with_second_migration("CREATE TABLE public.migrate_probe (id integer);\n").await
}

/// The embedded baseline plus a second migration with `sql`, in a fresh folder.
async fn migrator_with_second_migration(sql: &str) -> Migrator {
    let folder = std::env::temp_dir().join(format!(
        "verdaxis-migrations-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&folder).expect("migration folder");
    let baseline = MIGRATOR.iter().next().expect("the baseline migration");
    std::fs::write(folder.join("1_baseline.sql"), baseline.sql.as_str())
        .expect("copy the baseline");
    std::fs::write(folder.join("2_migrate_probe.sql"), sql).expect("write the second migration");
    let migrator = Migrator::new(folder.as_path())
        .await
        .expect("read the migration folder");
    std::fs::remove_dir_all(&folder).expect("remove the migration folder");
    migrator
}

fn policy(rows: &[(&str, &str)]) -> TransitionPolicy {
    let text: String = rows
        .iter()
        .map(|(current, target)| format!("{current}\t{target}\n"))
        .collect();
    TransitionPolicy::parse(&text).expect("a test policy")
}

/// An Alembic-built database adopted at `sqlx_1`.
async fn adopted_database(label: &str) -> String {
    let database = alembic_database(label).await;
    adopt(
        &mut as_migrator(&database).await,
        &adoption_head(),
        &compiled_baseline(),
    )
    .await
    .expect("adopt");
    database
}

async fn probe_exists(database: &str) -> bool {
    let mut connection = connect(database).await;
    sqlx::query_scalar("SELECT to_regclass('public.migrate_probe') IS NOT NULL")
        .fetch_one(&mut connection)
        .await
        .expect("probe lookup")
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn run_to_exact_approved_target_is_idempotent() {
    let database = adopted_database("run").await;
    let second = migrator_with_second_version().await;
    let rules = policy(&[
        ("sqlx_1", "sqlx_1"),
        ("sqlx_1", "sqlx_2"),
        ("sqlx_2", "sqlx_2"),
    ]);

    let at_baseline = run_to_target(
        &mut as_migrator(&database).await,
        &MIGRATOR,
        &rules,
        "sqlx_1",
        "sqlx_1",
        true,
    )
    .await;
    assert_eq!(at_baseline, Ok(RunOutcome::AtTarget));

    let before = history_rows(&database).await;
    let dry = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_1",
        "sqlx_2",
        false,
    )
    .await;
    assert_eq!(dry, Ok(RunOutcome::WouldApply(vec![2])));
    assert_eq!(
        history_rows(&database).await,
        before,
        "a dry run wrote history"
    );
    assert!(
        !probe_exists(&database).await,
        "a dry run applied the migration"
    );

    let applied = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_1",
        "sqlx_2",
        true,
    )
    .await;
    assert_eq!(applied, Ok(RunOutcome::Applied(vec![2])));
    assert!(probe_exists(&database).await);
    let report = status_with(&mut connect(&database).await, &second)
        .await
        .expect("status");
    assert_eq!(report.revision(), Some("sqlx_2".into()));
    assert!(report.pending.is_empty() && report.foreign.is_empty());

    let after = history_rows(&database).await;
    let repeat = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_2",
        "sqlx_2",
        true,
    )
    .await;
    assert_eq!(repeat, Ok(RunOutcome::AtTarget));
    assert_eq!(
        history_rows(&database).await,
        after,
        "a repeat changed the history"
    );
    // Task 7: a deploy interrupted after the migration committed reruns with
    // the same approval (sqlx_1 -> sqlx_2). The database is at the target, so
    // the rerun reports it without a write.
    let resumed = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_1",
        "sqlx_2",
        true,
    )
    .await;
    assert_eq!(resumed, Ok(RunOutcome::AtTarget));
    assert_eq!(
        history_rows(&database).await,
        after,
        "a resume changed the history"
    );
    drop_database(&database).await;
}

/// label, policy rows, expected current, target and the refusal it must give.
type RefusalCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a str,
    &'a str,
    RunRefusal,
);

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn unapproved_transition_or_pending_version_refuses_before_mutation() {
    let second = migrator_with_second_version().await;
    let database = adopted_database("refuse").await;
    let before = history_rows(&database).await;
    let cases: [RefusalCase<'_>; 4] = [
        (
            "unlisted pair",
            &[("sqlx_1", "sqlx_1")],
            "sqlx_1",
            "sqlx_2",
            RunRefusal::Transition,
        ),
        (
            "wrong current",
            &[("sqlx_2", "sqlx_2")],
            "sqlx_2",
            "sqlx_2",
            RunRefusal::CurrentRevision,
        ),
        (
            "unknown target",
            &[("sqlx_1", "sqlx_3")],
            "sqlx_1",
            "sqlx_3",
            RunRefusal::Target,
        ),
        (
            "alembic current",
            &[("p89_20261022_trade_freeze", "sqlx_1")],
            "p89_20261022_trade_freeze",
            "sqlx_1",
            RunRefusal::Transition,
        ),
    ];
    for (label, rows, current, target, refusal) in cases {
        let outcome = run_to_target(
            &mut as_migrator(&database).await,
            &second,
            &policy(rows),
            current,
            target,
            true,
        )
        .await;
        assert_eq!(outcome, Err(refusal), "{label}");
        assert_eq!(
            history_rows(&database).await,
            before,
            "{label}: history changed"
        );
        assert!(!probe_exists(&database).await, "{label}: a migration ran");
    }
    drop_database(&database).await;

    // A failed row for a pending version makes the history foreign: refused.
    let database = adopted_database("foreign_run").await;
    sqlx::raw_sql(
        "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (2, 'migrate probe', false, '\\x00', 0)",
    )
    .execute(&mut as_migrator(&database).await)
    .await
    .expect("seed a failed row");
    let before = history_rows(&database).await;
    let rules = policy(&[("sqlx_1", "sqlx_2")]);
    let outcome = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_1",
        "sqlx_2",
        true,
    )
    .await;
    assert_eq!(outcome, Err(RunRefusal::History));
    assert_eq!(history_rows(&database).await, before);
    assert!(!probe_exists(&database).await);
    drop_database(&database).await;
}

#[test]
fn compiled_policy_parses_and_keeps_the_alembic_rows() {
    let compiled = TransitionPolicy::compiled();
    assert!(compiled.allows(
        "p5_20261020_read_indexes",
        "p89_20261022_watchlist_producer"
    ));
    assert!(compiled.allows("p89_20261022_trade_freeze", "p89_20261022_trade_freeze"));
    assert!(!compiled.allows("p89_20261022_trade_freeze", "head"));
    assert!(
        TransitionPolicy::parse("a\tHEAD\n").is_none(),
        "an alias or upper case parsed"
    );
}

#[test]
fn deploy_owned_requires_exact_state_nonce_sha_and_both_locks() {
    use std::os::unix::fs::PermissionsExt;
    use verdaxis_read::cli::migrate::{DeployOwnership, require_deploy_ownership};
    use verdaxis_read::cli::state::StateLock;
    use verdaxis_read::cli::{Environment, Exit};

    let directory =
        std::env::temp_dir().join(format!("verdaxis-deploy-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&directory).expect("deploy directory");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).expect("private");
    let sha = "a".repeat(40);
    let nonce = "0123456789abcdef0123456789abcdef";
    let state = |phase: &str, env: &str, sha: &str, nonce: &str| {
        let path = directory.join("staging.state");
        std::fs::write(&path, format!("DEPLOYMENT_STATE={phase}\nENVIRONMENT={env}\nRELEASE_SHA={sha}\nDEPLOY_NONCE={nonce}\n"))
            .expect("write state");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("state mode");
    };
    let owner = DeployOwnership {
        deploy_directory: &directory,
        environment: Environment::Staging,
        release_sha: &sha,
        nonce,
    };

    // Missing lock files: refused, and the check creates none.
    state("blocked", "staging", &sha, nonce);
    assert_eq!(require_deploy_ownership(&owner), Err(Exit::Precondition));
    assert!(
        !directory.join("staging.lock").exists(),
        "the check created a lock file"
    );

    let main = StateLock::try_acquire(&directory.join("staging.lock"))
        .expect("main")
        .expect("free");
    // Only the main lock held: refused.
    {
        let restart_path = directory.join("staging.restart.lock");
        drop(
            StateLock::try_acquire(&restart_path)
                .expect("create")
                .expect("free"),
        );
        assert_eq!(require_deploy_ownership(&owner), Err(Exit::Precondition));
    }
    let restart = StateLock::try_acquire(&directory.join("staging.restart.lock"))
        .expect("restart")
        .expect("free");
    assert_eq!(
        require_deploy_ownership(&owner),
        Ok(()),
        "exact state and both locks"
    );

    let other_sha = "b".repeat(40);
    for (label, phase, env, state_sha, state_nonce) in [
        (
            "restart phase",
            "restart-authorized",
            "staging",
            sha.as_str(),
            nonce,
        ),
        (
            "other environment",
            "blocked",
            "production",
            sha.as_str(),
            nonce,
        ),
        (
            "other release",
            "blocked",
            "staging",
            other_sha.as_str(),
            nonce,
        ),
        (
            "other nonce",
            "blocked",
            "staging",
            sha.as_str(),
            "ffffffffffffffffffffffffffffffff",
        ),
    ] {
        state(phase, env, state_sha, state_nonce);
        assert_eq!(
            require_deploy_ownership(&owner),
            Err(Exit::Precondition),
            "{label}"
        );
    }
    state("blocked", "staging", &sha, nonce);
    let short = DeployOwnership {
        nonce: "abc",
        ..owner
    };
    assert_eq!(
        require_deploy_ownership(&short),
        Err(Exit::Usage),
        "a short nonce"
    );

    drop(restart);
    drop(main);
    let owner = DeployOwnership {
        deploy_directory: &directory,
        environment: Environment::Staging,
        release_sha: &sha,
        nonce,
    };
    assert_eq!(
        require_deploy_ownership(&owner),
        Err(Exit::Precondition),
        "no lock held"
    );
    std::fs::remove_dir_all(&directory).expect("clean up");
}

#[test]
fn production_literal_and_allowlist_move_together() {
    use verdaxis_read::runtime::artifact::ALLOWED_REVISIONS;
    use verdaxis_read::runtime::config::PRODUCTION_REVISIONS;

    let allowlist: Vec<String> =
        serde_json::from_str(include_str!("../../../deploy/rust/allowed-revisions.json"))
            .expect("the reviewed allowlist is a JSON list");
    assert_eq!(
        allowlist, ALLOWED_REVISIONS,
        "the binary allowlist drifted from the file"
    );
    for revision in PRODUCTION_REVISIONS {
        assert!(
            allowlist.iter().any(|allowed| allowed == revision),
            "{revision} is not allowed"
        );
    }
    // The sqlx cutover moves the production literal and the allowlist together:
    // a sqlx literal is in both or in neither, and names an embedded version.
    let sqlx_production: Vec<&str> = PRODUCTION_REVISIONS
        .iter()
        .copied()
        .filter(|revision| revision.starts_with("sqlx_"))
        .collect();
    let sqlx_allowed: Vec<&str> = allowlist
        .iter()
        .map(String::as_str)
        .filter(|revision| revision.starts_with("sqlx_"))
        .collect();
    assert_eq!(sqlx_production, sqlx_allowed);
    for revision in sqlx_allowed {
        let version: i64 = revision["sqlx_".len()..].parse().expect("a sqlx version");
        assert!(
            MIGRATOR
                .iter()
                .any(|migration| migration.version == version),
            "{revision} names no embedded migration"
        );
    }
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn migrate_and_app_request_do_not_half_apply() {
    let two_changes = "CREATE TABLE public.migrate_probe (id integer);\n\
                       ALTER TABLE public.products ADD COLUMN migrate_probe_flag boolean;\n";
    let second = migrator_with_second_migration(two_changes).await;
    let rules = policy(&[("sqlx_1", "sqlx_2")]);
    // Both changes, read by the application in one statement (one snapshot).
    // Both facts are read from the catalog tables under the statement's one
    // snapshot. `to_regclass` would consult the catalog cache instead, whose
    // view can differ from the snapshot and show a torn pair on its own.
    let observe = "SELECT EXISTS (SELECT 1 FROM pg_class c \
                                  JOIN pg_namespace n ON n.oid = c.relnamespace \
                                  WHERE n.nspname = 'public' AND c.relname = 'migrate_probe'), \
                   EXISTS (SELECT 1 FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
                           JOIN pg_namespace n ON n.oid = c.relnamespace \
                           WHERE n.nspname = 'public' AND c.relname = 'products' \
                             AND a.attname = 'migrate_probe_flag' AND NOT a.attisdropped)";

    // 1. An application lock blocks the migration: it fails whole, with no
    //    history row and neither change.
    let database = adopted_database("blocked").await;
    let mut app = connect(&database).await;
    sqlx::raw_sql("BEGIN; SELECT count(*) FROM public.products;")
        .execute(&mut app)
        .await
        .expect("the application holds a lock on products");
    let mut migrator = as_migrator(&database).await;
    sqlx::raw_sql("SET lock_timeout = '500ms'")
        .execute(&mut migrator)
        .await
        .expect("a short lock wait");
    let blocked = run_to_target(&mut migrator, &second, &rules, "sqlx_1", "sqlx_2", true).await;
    assert_eq!(
        blocked,
        Err(RunRefusal::Database),
        "the blocked migration must fail"
    );
    // sqlx keeps its session advisory lock after a failed run; the `migrate`
    // command is one process per run, so its exit ends that session. Here the
    // failed session is closed the same way before the retry.
    drop(migrator);
    sqlx::raw_sql("COMMIT")
        .execute(&mut app)
        .await
        .expect("release");
    let (table, column): (bool, bool) = sqlx::query_as(observe)
        .fetch_one(&mut connect(&database).await)
        .await
        .expect("observe");
    assert_eq!(
        (table, column),
        (false, false),
        "a failed migration left part of its change"
    );
    let report = status_with(&mut connect(&database).await, &second)
        .await
        .expect("status");
    assert_eq!(report.revision(), Some("sqlx_1".into()));
    assert_eq!(report.pending, vec![2]);
    assert!(report.foreign.is_empty(), "a failed run left a history row");

    // 2. Once released, a retry applies both changes, and an application that
    //    reads throughout sees neither or both, never one.
    let watcher_database = database.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_stop = stop.clone();
    let watcher = tokio::spawn(async move {
        let mut app = connect(&watcher_database).await;
        let mut seen = std::collections::BTreeSet::new();
        while !watcher_stop.load(std::sync::atomic::Ordering::SeqCst) {
            let pair: (bool, bool) = sqlx::query_as(observe)
                .fetch_one(&mut app)
                .await
                .expect("observe");
            seen.insert(pair);
        }
        let last: (bool, bool) = sqlx::query_as(observe)
            .fetch_one(&mut app)
            .await
            .expect("observe");
        seen.insert(last);
        seen
    });
    let applied = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_1",
        "sqlx_2",
        true,
    )
    .await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = watcher.await.expect("the application watcher");
    assert_eq!(applied, Ok(RunOutcome::Applied(vec![2])));
    for pair in &seen {
        assert!(
            pair.0 == pair.1,
            "the application saw half of the migration: {pair:?}"
        );
    }
    assert!(
        seen.contains(&(true, true)),
        "the application never saw the applied schema"
    );
    drop(app);
    drop_database(&database).await;
}

#[tokio::test]
async fn migrate_command_refuses_before_any_connection() {
    use std::collections::HashMap;
    use verdaxis_read::cli::migrate::run_with;
    use verdaxis_read::cli::{Environment, Exit, IdentityArgs, MigrateArgs, MigrationAction};

    let args = |action, apply: bool, current: Option<&str>, target: Option<&str>| MigrateArgs {
        action,
        identity: IdentityArgs {
            environment: Environment::Staging,
            release_sha: "a".repeat(40),
        },
        check: false,
        expected_current: current.map(str::to_owned),
        target: target.map(str::to_owned),
        apply,
        deploy_nonce: None,
        environment_file: None,
    };
    let never_opened = |_: Environment, _: String| async {
        panic!("a refused migrate command opened a connection")
    };
    let no_vars = HashMap::new();
    let head = "p89_20261022_trade_freeze";
    for (label, request, expected) in [
        (
            "status with --apply",
            args(MigrationAction::Status, true, None, None),
            Exit::Usage,
        ),
        (
            "baseline without --apply",
            args(MigrationAction::Baseline, false, Some(head), None),
            Exit::Usage,
        ),
        (
            "baseline without a current",
            args(MigrationAction::Baseline, true, None, None),
            Exit::Usage,
        ),
        (
            "run without a target",
            args(MigrationAction::Run, false, Some("sqlx_1"), None),
            Exit::Usage,
        ),
        (
            "baseline without deploy state",
            args(MigrationAction::Baseline, true, Some(head), None),
            Exit::Precondition,
        ),
        (
            "run --apply without deploy state",
            args(MigrationAction::Run, true, Some("sqlx_1"), Some("sqlx_1")),
            Exit::Precondition,
        ),
        (
            "verify",
            args(MigrationAction::Verify, false, None, None),
            Exit::Precondition,
        ),
        (
            "status --check without a literal",
            MigrateArgs {
                check: true,
                ..args(MigrationAction::Status, false, None, None)
            },
            Exit::Usage,
        ),
        (
            "--check on run",
            MigrateArgs {
                check: true,
                ..args(MigrationAction::Run, false, Some("sqlx_1"), Some("sqlx_1"))
            },
            Exit::Usage,
        ),
    ] {
        assert_eq!(
            run_with(&request, &no_vars, never_opened).await,
            expected,
            "{label}"
        );
    }
}
