//! `migrate status` and `migrate run` on the disposable PostgreSQL of
//! `run_postgres.sh`: the sqlx history, the approved transitions and every
//! refusal. Adoption of an Alembic-built database is gone; a runner database
//! is initialized with the embedded baseline by plain sqlx migration runs.
//!
//! Each case creates an empty database, works as the migrator through
//! `SET ROLE`, and drops its database.
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use std::str::FromStr;
use verdaxis_read::cli::migrate::{
    MIGRATOR, RunOutcome, RunRefusal, TransitionPolicy, run_to_target, run_transition, status_with,
};

const MIGRATOR_ROLE: &str = "verdaxis_migrator";

fn admin_url() -> String {
    std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
        .expect("RUST_TEMPLATE_ADMIN_DATABASE_URL must name the disposable server")
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

/// The sqlx baseline leaves no Alembic history behind: a database built from
/// the embedded migrations reports only sqlx revisions.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn migrate_status_is_read_only_and_reports_pending() {
    let database = empty_database("status").await;
    MIGRATOR
        .run(&mut as_migrator(&database).await)
        .await
        .expect("apply the embedded migrations");
    let report = status_with(&mut connect(&database).await, &MIGRATOR)
        .await
        .expect("status");
    assert_eq!(report.revision(), Some("sqlx_2".into()));
    assert!(report.pending.is_empty());
    assert!(report.foreign.is_empty());
    drop_database(&database).await;

    // An empty database has no history: everything is pending, and reading
    // the status writes nothing.
    let database = empty_database("status_empty").await;
    let report = status_with(&mut as_migrator(&database).await, &MIGRATOR)
        .await
        .expect("status");
    assert_eq!(report.revision(), None);
    assert_eq!(report.pending, vec![1, 2]);
    assert!(report.foreign.is_empty());
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut connect(&database).await)
            .await
            .expect("history table lookup"),
        "status created the history table"
    );
    drop_database(&database).await;
}

/// The embedded baseline plus a second migration, in a fresh folder.
async fn migrator_with_second_version() -> Migrator {
    migrator_with_second_migration("CREATE TABLE public.migrate_probe (id integer);\n").await
}

/// The embedded migrations plus one more with `sql`, in a fresh folder.
async fn migrator_with_second_migration(sql: &str) -> Migrator {
    let folder = std::env::temp_dir().join(format!(
        "verdaxis-migrations-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&folder).expect("migration folder");
    for migration in MIGRATOR.iter() {
        let name = format!(
            "{}_{}.sql",
            migration.version,
            migration.description.replace(' ', "_")
        );
        std::fs::write(folder.join(name), migration.sql.as_str())
            .expect("copy an embedded migration");
    }
    std::fs::write(folder.join("9_migrate_probe.sql"), sql).expect("write the extra migration");
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

async fn probe_exists(database: &str) -> bool {
    let mut connection = connect(database).await;
    sqlx::query_scalar("SELECT to_regclass('public.migrate_probe') IS NOT NULL")
        .fetch_one(&mut connection)
        .await
        .expect("probe lookup")
}

/// A database at the newest embedded migration.
async fn migrated_database(label: &str) -> String {
    let database = empty_database(label).await;
    MIGRATOR
        .run(&mut as_migrator(&database).await)
        .await
        .expect("apply the embedded migrations");
    database
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn run_to_exact_approved_target_is_idempotent() {
    let database = migrated_database("run").await;
    let head = format!("sqlx_{}", MIGRATOR.iter().map(|m| m.version).max().unwrap());
    let second = migrator_with_second_version().await;
    let rules = policy(&[(&head, &head), (&head, "sqlx_9"), ("sqlx_9", "sqlx_9")]);

    let at_head = run_to_target(
        &mut as_migrator(&database).await,
        &MIGRATOR,
        &rules,
        &head,
        &head,
        true,
    )
    .await;
    assert_eq!(at_head, Ok(RunOutcome::AtTarget));

    let before = history_rows(&database).await;
    let dry = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        &head,
        "sqlx_9",
        false,
    )
    .await;
    assert_eq!(dry, Ok(RunOutcome::WouldApply(vec![9])));
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
        &head,
        "sqlx_9",
        true,
    )
    .await;
    assert_eq!(applied, Ok(RunOutcome::Applied(vec![9])));
    assert!(probe_exists(&database).await);
    let report = status_with(&mut connect(&database).await, &second)
        .await
        .expect("status");
    assert_eq!(report.revision(), Some("sqlx_9".into()));
    assert!(report.pending.is_empty() && report.foreign.is_empty());

    let after = history_rows(&database).await;
    let repeat = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        "sqlx_9",
        "sqlx_9",
        true,
    )
    .await;
    assert_eq!(repeat, Ok(RunOutcome::AtTarget));
    assert_eq!(
        history_rows(&database).await,
        after,
        "a repeat changed the history"
    );
    // A deploy interrupted after the migration committed reruns with the same
    // approval. The database is at the target, so the rerun reports it without
    // a write.
    let resumed = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        &head,
        "sqlx_9",
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
    let head = format!("sqlx_{}", MIGRATOR.iter().map(|m| m.version).max().unwrap());
    let second = migrator_with_second_version().await;
    let database = migrated_database("refuse").await;
    let before = history_rows(&database).await;
    let cases: [RefusalCase<'_>; 4] = [
        (
            "unlisted pair",
            &[(&head, &head)],
            &head,
            "sqlx_9",
            RunRefusal::Transition,
        ),
        (
            "wrong current",
            &[("sqlx_9", "sqlx_9")],
            "sqlx_9",
            "sqlx_9",
            RunRefusal::CurrentRevision,
        ),
        (
            "unknown target",
            &[(&head, "sqlx_99")],
            &head,
            "sqlx_99",
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
    let database = migrated_database("foreign_run").await;
    sqlx::raw_sql(
        "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (9, 'migrate probe', false, '\\x00', 0)",
    )
    .execute(&mut as_migrator(&database).await)
    .await
    .expect("seed a failed row");
    let before = history_rows(&database).await;
    let rules = policy(&[(&head, "sqlx_9")]);
    let outcome = run_to_target(
        &mut as_migrator(&database).await,
        &second,
        &rules,
        &head,
        "sqlx_9",
        true,
    )
    .await;
    assert_eq!(outcome, Err(RunRefusal::History));
    assert_eq!(history_rows(&database).await, before);
    assert!(!probe_exists(&database).await);
    drop_database(&database).await;
}

/// An Alembic literal is refused everywhere: the adoption finished and no
/// Python schema tool runs. Even a listed pair refuses when either literal is
/// not a sqlx revision, and an unlisted pair refuses before anything else.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL of run_postgres.sh"]
async fn alembic_literals_are_refused_everywhere() {
    let head = format!("sqlx_{}", MIGRATOR.iter().map(|m| m.version).max().unwrap());
    let database = migrated_database("alembic_refused").await;
    let before = history_rows(&database).await;
    for (current, target) in [
        ("ob_20261031_offboarding", "sqlx_1"),
        ("ob_20261031_offboarding", "ob_20261031_offboarding"),
        (&head, "ob_20261031_offboarding"),
    ] {
        assert_eq!(
            run_transition(
                &mut as_migrator(&database).await,
                &MIGRATOR,
                &policy(&[(current, target), ("ob_20261031_offboarding", "sqlx_1")]),
                current,
                target,
                true,
            )
            .await,
            Err(RunRefusal::Target),
            "{current} -> {target}"
        );
        // The compiled policy has no row between an Alembic and a sqlx
        // revision; only the retired chain's own history rows remain.
        if current.starts_with("sqlx_") != target.starts_with("sqlx_") {
            assert!(!TransitionPolicy::compiled().allows(current, target));
        }
        assert_eq!(
            history_rows(&database).await,
            before,
            "{current} -> {target}: history changed"
        );
    }
    // An unlisted Alembic pair is refused before anything else.
    assert_eq!(
        run_transition(
            &mut as_migrator(&database).await,
            &MIGRATOR,
            &policy(&[("sqlx_1", "sqlx_1")]),
            "p89_20261022_trade_freeze",
            "sqlx_2",
            true,
        )
        .await,
        Err(RunRefusal::Transition)
    );
    drop_database(&database).await;
}

#[test]
fn compiled_policy_parses_and_keeps_only_the_sqlx_rows() {
    let compiled = TransitionPolicy::compiled();
    // The release serves only sqlx_2: a host moves there from sqlx_1 and then
    // keeps it. No Alembic adoption pair and no sqlx_1 self row remain.
    assert!(compiled.allows("sqlx_1", "sqlx_2"));
    assert!(compiled.allows("sqlx_2", "sqlx_2"));
    assert!(!compiled.allows("sqlx_1", "sqlx_1"));
    assert!(!compiled.allows("ob_20261031_offboarding", "sqlx_1"));
    assert!(!compiled.allows("ob_20261031_offboarding", "sqlx_2"));
    assert!(!compiled.allows("sqlx_2", "head"));
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
    // A sqlx literal is in both or in neither, and names an embedded version.
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
    let head = format!("sqlx_{}", MIGRATOR.iter().map(|m| m.version).max().unwrap());
    let two_changes = "CREATE TABLE public.migrate_probe (id integer);\n\
                       ALTER TABLE public.products ADD COLUMN migrate_probe_flag boolean;\n";
    let second = migrator_with_second_migration(two_changes).await;
    let rules = policy(&[(&head, "sqlx_9")]);
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
    let database = migrated_database("blocked").await;
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
    let blocked = run_to_target(&mut migrator, &second, &rules, &head, "sqlx_9", true).await;
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
    assert_eq!(report.revision(), Some(head.clone()));
    assert_eq!(report.pending, vec![9]);
    assert!(report.foreign.is_empty(), "a failed run left a history row");

    // 2. Once released, a retry applies both changes, and an application that
    // reads throughout sees neither or both, never one.
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
        &head,
        "sqlx_9",
        true,
    )
    .await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = watcher.await.expect("the application watcher");
    assert_eq!(applied, Ok(RunOutcome::Applied(vec![9])));
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
    for (label, request, expected) in [
        (
            "status with --apply",
            args(MigrationAction::Status, true, None, None),
            Exit::Usage,
        ),
        (
            "run without a target",
            args(MigrationAction::Run, false, Some("sqlx_1"), None),
            Exit::Usage,
        ),
        (
            "run --apply without deploy state",
            args(MigrationAction::Run, true, Some("sqlx_1"), Some("sqlx_2")),
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
                ..args(MigrationAction::Run, false, Some("sqlx_1"), Some("sqlx_2"))
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

/// Initialize a disposable runner database with the compiled sqlx migrations:
/// plain `MIGRATOR.run`, the same embedded migrations and checksums the
/// runtime applies. This helper accepts only numeric loopback endpoints and
/// test database names. Run with `cargo test ... --test group_phase10
/// phase10_migrate_postgres::runner_database_applies_the_baseline -- --ignored`.
#[tokio::test]
#[ignore = "run by the component runners through their migrator URL"]
async fn runner_database_applies_the_baseline() {
    let raw = std::env::var("RUST_RUNNER_MIGRATOR_DATABASE_URL")
        .expect("RUST_RUNNER_MIGRATOR_DATABASE_URL must name the runner's migrator URL");
    let url = url::Url::parse(&raw).expect("parse the runner URL");
    assert!(matches!(
        url.host_str(),
        Some("127.0.0.1" | "[::1]" | "::1")
    ));
    let database = url.path().trim_start_matches('/');
    assert!(
        database.ends_with("_test") || database.ends_with("_template"),
        "runner database must have a disposable test name"
    );
    let options = PgConnectOptions::from_str(&raw).expect("parse the runner migrator URL");
    let mut connection = PgConnection::connect_with(&options)
        .await
        .expect("connect as the runner migrator");
    let initialized: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut connection)
            .await
            .expect("inspect runner sqlx history");
    if !initialized {
        let application_objects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
             WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S','f') \
             AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass \
                 AND d.objid=c.oid AND d.deptype='e')",
        )
        .fetch_one(&mut connection)
        .await
        .expect("inspect empty runner schema");
        assert_eq!(
            application_objects, 0,
            "refuse to initialize a populated schema"
        );
    }
    MIGRATOR
        .run(&mut connection)
        .await
        .expect("apply the runner sqlx baseline");
    let current = status_with(&mut connection, &MIGRATOR)
        .await
        .expect("read runner sqlx status");
    assert!(current.foreign.is_empty());
    assert!(current.pending.is_empty());
    let head = format!("sqlx_{}", MIGRATOR.iter().map(|m| m.version).max().unwrap());
    assert_eq!(current.revision().as_deref(), Some(head.as_str()));
    // No Alembic history remains: the runner databases are sqlx-only.
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT to_regclass('public.alembic_version') IS NOT NULL")
            .fetch_one(&mut connection)
            .await
            .expect("inspect runner schema history"),
        "the runner database still carries an alembic_version table"
    );
}
