//! Offline checks for the individual-offboarding schema revision: the exact
//! checkpoint transition, the reviewed revision allowlists, the runtime ACL
//! policy text and the shape of the migration file.
//!
//! The database behavior of the same revision is covered by
//! `offboarding_schema_postgres.rs`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use verdaxis_read::{compiled_sha, runtime::artifact::ALLOWED_REVISIONS, validate_config, Config};

/// The revision that precedes the offboarding revision on the shared chain.
const PARENT: &str = "p89_20261022_trade_freeze";
/// The offboarding revision on the retired Alembic chain.
const REVISION: &str = "ob_20261031_offboarding";
/// The runtime revision the binary serves: the head sqlx revision and the
/// only startup admission.
const HEAD: &str = ALLOWED_REVISIONS[0];
/// The first sqlx revision, which both hosts were adopted to.
const SQLX_1: &str = "sqlx_1";
const AUDIT_TABLE: &str = "individual_offboardings";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// A revision identifier the checkpoint tool accepts: a literal ID, never an alias.
fn is_literal_revision(value: &str) -> bool {
    let mut chars = value.chars();
    let first_is_valid = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    first_is_valid
        && value.len() <= 128
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "base" | "head" | "heads"
        )
}

/// Parse the checkpoint policy: literal revision pairs, no aliases, no duplicates.
fn parse_checkpoints(text: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            fields.len(),
            2,
            "policy line {} must have two fields",
            index + 1
        );
        let pair = (fields[0].trim().to_owned(), fields[1].trim().to_owned());
        assert!(
            is_literal_revision(&pair.0) && is_literal_revision(&pair.1),
            "policy line {} must name literal revisions",
            index + 1
        );
        assert!(
            !pairs.contains(&pair),
            "duplicate transition on line {}",
            index + 1
        );
        pairs.push(pair);
    }
    pairs
}

#[test]
fn checkpoint_requires_exact_transition() {
    let pairs = parse_checkpoints(&read("deploy/migration-checkpoints.tsv"));

    // Exactly two rows lead to the offboarding revision: the parent step and
    // the self row.
    let mut to_revision: Vec<_> = pairs
        .iter()
        .filter(|(_, target)| target == REVISION)
        .collect();
    to_revision.sort();
    // Sorted order: the self row sorts before the row from the parent.
    let expected = [
        (REVISION.to_owned(), REVISION.to_owned()),
        (PARENT.to_owned(), REVISION.to_owned()),
    ];
    assert_eq!(to_revision, expected.iter().collect::<Vec<_>>());
    // The retired Alembic chain ends at the offboarding revision with its self
    // row only. No row leaves it for a sqlx revision: the adoption finished on
    // both hosts and its row is gone.
    let from_revision: Vec<_> = pairs
        .iter()
        .filter(|(current, _)| current == REVISION)
        .map(|(_, target)| target.as_str())
        .collect();
    assert_eq!(from_revision, [REVISION]);
    // The runtime revision admits no transition except its own no-change row,
    // and only a sqlx revision moves to it.
    let runtime_rows: Vec<_> = pairs
        .iter()
        .filter(|(current, _)| current == HEAD)
        .map(|(current, target)| (current.as_str(), target.as_str()))
        .collect();
    assert_eq!(runtime_rows, [(HEAD, HEAD)]);
    assert!(pairs
        .iter()
        .filter(|(_, target)| target == HEAD)
        .all(|(current, _)| current.starts_with("sqlx_")));
    let policy = verdaxis_read::cli::migrate::TransitionPolicy::compiled();
    assert!(
        policy.allows("sqlx_2", HEAD),
        "the move to the runtime revision"
    );
    assert!(policy.allows(SQLX_1, "sqlx_2"), "the historical sqlx step");
    assert!(policy.allows(HEAD, HEAD), "the runtime no-change row");
    // The offboarding self row stays as history; `migrate run` refuses every
    // Alembic literal, so the row never runs.
    assert!(policy.allows(REVISION, REVISION), "the retained self row");
    for (current, target) in [
        (REVISION, SQLX_1),
        (REVISION, HEAD),
        (PARENT, SQLX_1),
        (PARENT, HEAD),
        (HEAD, REVISION),
        (HEAD, PARENT),
    ] {
        assert!(
            !policy.allows(current, target),
            "{current} -> {target} must refuse"
        );
    }
}

#[test]
fn allowlists_admit_only_the_head_sqlx_revision() {
    // The binary serves only the head sqlx revision; the retained Alembic
    // chain ends at the offboarding revision, which is not a startup admission.
    let reviewed: Vec<String> =
        serde_json::from_str(&read("deploy/rust/allowed-revisions.json")).unwrap();
    assert_eq!(reviewed, [HEAD]);
    assert_eq!(ALLOWED_REVISIONS, [HEAD]);

    // The deploy enables the stream listener at every checkpoint that keeps
    // the stream tables, including the runtime revision.
    let deploy = read("scripts/deploy.sh");
    let line = deploy
        .lines()
        .find(|line| line.contains(r#"RUST_STREAM_LISTENER="1" ;;"#))
        .expect("stream listener checkpoint list");
    let listed: Vec<&str> = line.trim().split(')').next().unwrap().split('|').collect();
    assert_eq!(listed.last().copied(), Some(HEAD));
    assert!(listed.contains(&REVISION));
    assert!(listed.contains(&PARENT));
}

fn runtime_configuration(environment: &str, revision: &str) -> Config {
    let production = environment == "production";
    Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: environment.into(),
        release_sha: compiled_sha().into(),
        migration_revision: revision.into(),
        database_url: if production {
            "postgres://verdaxis_app:local-secret@127.0.0.1:5432/verdaxis_prod".into()
        } else {
            "postgres://verdaxis_app_staging:local-secret@127.0.0.1:5432/verdaxis_staging".into()
        },
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    }
}

#[test]
fn staging_runtime_accepts_only_the_head_sqlx_revision() {
    assert!(validate_config(&runtime_configuration("staging", HEAD)).is_ok());
    // The runtime refuses the offboarding revision on both environments: a
    // database still at the Alembic head is not a sqlx database.
    assert!(validate_config(&runtime_configuration("staging", REVISION)).is_err());
    assert!(validate_config(&runtime_configuration("production", REVISION)).is_err());
    assert!(validate_config(&runtime_configuration("staging", "ob_20261031_unlisted")).is_err());
}

/// The statements of the `app_table_policy` insert: table name to its privileges.
fn table_policy() -> BTreeMap<String, Vec<String>> {
    let policy = read("deploy/postgres/app_acl_policy.sql");
    let block = policy
        .split("INSERT INTO app_table_policy")
        .nth(1)
        .and_then(|rest| rest.split("CREATE TEMP TABLE app_column_policy").next())
        .expect("table policy block");
    let mut rows = BTreeMap::new();
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("('") else {
            continue;
        };
        let (name, privileges) = rest.split_once("', ARRAY[").expect("policy row");
        let privileges = privileges
            .split(']')
            .next()
            .unwrap()
            .split(',')
            .map(|privilege| privilege.trim().trim_matches('\'').to_owned())
            .collect();
        assert!(
            rows.insert(name.to_owned(), privileges).is_none(),
            "duplicate row {name}"
        );
    }
    rows
}

#[test]
fn runtime_policy_grants_the_audit_table_only_read_and_append() {
    let rows = table_policy();
    assert_eq!(
        rows.get(AUDIT_TABLE).map(Vec::as_slice),
        Some(&["SELECT".to_owned(), "INSERT".to_owned()][..])
    );
    let policy = read("deploy/postgres/app_acl_policy.sql");
    assert!(
        !policy.contains(&format!("('{AUDIT_TABLE}', '")),
        "the audit table has no column-level grants"
    );
    // The convergence bundle applies this one policy file.
    assert!(read("deploy/postgres/converge_runtime_object_acls.sql")
        .contains(r"\ir app_acl_policy.sql"));
    // The role validator treats the table as append-only for the application and read-only for backup.
    assert!(read("deploy/postgres/validate_roles.sql").contains(AUDIT_TABLE));
}

#[test]
fn runtime_policy_leaves_market_and_account_privileges_unchanged() {
    let rows = table_policy();
    let all = ["SELECT", "INSERT", "UPDATE", "DELETE"];
    let expected: [(&str, &[&str]); 16] = [
        ("users", &all),
        ("audit_logs", &["SELECT", "INSERT"]),
        ("user_status_transitions", &["SELECT", "INSERT"]),
        ("refresh_sessions", &all),
        ("password_reset_tokens", &["SELECT", "DELETE"]),
        ("orderbook_orders", &all),
        ("trades", &all),
        ("market_event_outbox", &["SELECT", "INSERT", "UPDATE"]),
        ("market_command_results", &["SELECT", "INSERT"]),
        ("market_support_contexts", &["SELECT", "INSERT"]),
        ("market_support_authorizations", &["SELECT", "INSERT"]),
        ("negotiations", &["SELECT", "INSERT", "DELETE"]),
        ("inventory_items", &all),
        ("organizations", &["SELECT", "DELETE"]),
        (
            "organization_join_requests",
            &["SELECT", "INSERT", "UPDATE"],
        ),
        ("kyc_documents", &["SELECT", "INSERT"]),
    ];
    for (table, privileges) in expected {
        let actual: Vec<&str> = rows[table].iter().map(String::as_str).collect();
        assert_eq!(actual, privileges, "{table}");
    }
}
