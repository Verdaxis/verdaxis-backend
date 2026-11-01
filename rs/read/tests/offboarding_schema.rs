//! Offline checks for the individual-offboarding schema revision: the exact
//! checkpoint transition, the reviewed revision allowlists, the runtime ACL
//! policy text and the shape of the migration file.
//!
//! The database behavior of the same revision is covered by
//! `offboarding_schema_postgres.rs`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use verdaxis_read::{Config, compiled_sha, runtime::artifact::ALLOWED_REVISIONS, validate_config};

/// The revision that precedes the offboarding revision on the shared chain.
const PARENT: &str = "p5_20261020_read_indexes";
/// The offboarding revision. `alembic_version.version_num` is `VARCHAR(32)`.
const REVISION: &str = "ob_20261031_offboarding";
const MAX_REVISION_LENGTH: usize = 32;
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

/// Parse the checkpoint policy with the rules of `scripts/apply_migration_checkpoint.py`.
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

/// Every `revision` and `down_revision` pair declared under `alembic/versions`.
fn migration_graph() -> Vec<(String, Vec<String>)> {
    let directory = repo_root().join("alembic/versions");
    let mut graph = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let mut revision = None;
        let mut parents = Vec::new();
        for line in source.lines() {
            if let Some(rest) = line.strip_prefix("revision") {
                if let Some(value) = quoted_values(rest).into_iter().next() {
                    revision.get_or_insert(value);
                }
            } else if let Some(rest) = line.strip_prefix("down_revision")
                && parents.is_empty()
            {
                parents = quoted_values(rest);
            }
        }
        if let Some(revision) = revision {
            graph.push((revision, parents));
        }
    }
    graph
}

/// The single- or double-quoted string literals in `text`, in order.
fn quoted_values(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['"', '\'']) {
        let quote = rest[start..].chars().next().unwrap();
        let after = &rest[start + 1..];
        let Some(end) = after.find(quote) else { break };
        values.push(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    values
}

#[test]
fn checkpoint_requires_exact_transition() {
    let pairs = parse_checkpoints(&read("deploy/migration-checkpoints.tsv"));

    // Exactly two rows lead to the new revision: the parent step and the self row.
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
    // The new head moves nowhere else, and no earlier revision skips to it.
    assert!(
        pairs
            .iter()
            .filter(|(current, _)| current == REVISION)
            .all(|(_, target)| target == REVISION)
    );
    // Every row names revisions that exist in the migration chain.
    let known: Vec<String> = migration_graph()
        .into_iter()
        .map(|(revision, _)| revision)
        .collect();
    for (current, target) in &pairs {
        assert!(known.contains(current), "unknown revision {current}");
        assert!(known.contains(target), "unknown revision {target}");
    }

    // The real validator, which the deploy uses, accepts and refuses the same pairs.
    let upper_parent = PARENT.to_ascii_uppercase();
    let upper_revision = REVISION.to_ascii_uppercase();
    let padded_parent = format!("{PARENT} ");
    let cases = serde_json::json!([
        [PARENT, REVISION, null],
        [REVISION, REVISION, null],
        ["sse_20261021_replay_floor", REVISION, null],
        ["p3_20261020_auth_tokens", REVISION, null],
        [PARENT, "head", null],
        [PARENT, "heads", null],
        ["base", REVISION, null],
        [upper_parent, REVISION, null],
        [PARENT, upper_revision, null],
        [padded_parent, REVISION, null],
        [REVISION, PARENT, null],
        [PARENT, REVISION, [REVISION]],
        [PARENT, REVISION, [PARENT, REVISION]],
    ]);
    let verdicts = checkpoint_verdicts(&cases);
    let expected_verdicts = [
        true, true, false, false, false, false, false, false, false, false, false, false, false,
    ];
    assert_eq!(verdicts, expected_verdicts);
}

/// Ask the deploy's own checkpoint validator about each `[current, target, heads]` case.
fn checkpoint_verdicts(cases: &serde_json::Value) -> Vec<bool> {
    const SCRIPT: &str = r#"
import importlib.util, json, sys
from pathlib import Path
from alembic.config import Config
from alembic.script import ScriptDirectory

root = Path(sys.argv[1])
sys.path.insert(0, str(root))
spec = importlib.util.spec_from_file_location(
    "apply_migration_checkpoint", root / "scripts/apply_migration_checkpoint.py"
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
policy = module.parse_checkpoint_policy((root / "deploy/migration-checkpoints.tsv").read_text())
scripts = ScriptDirectory.from_config(Config(str(root / "alembic.ini")))
sha = "a" * 40

def verdict(current, target, heads):
    try:
        module.validate_checkpoint_request(
            policy=policy,
            source_sha=sha,
            approved_source_sha=sha,
            expected_current=current,
            target=target,
            current_heads=(current,) if heads is None else tuple(heads),
            script_directory=scripts,
        )
    except module.MigrationCheckpointError:
        return False
    return True

print(json.dumps([verdict(*case) for case in json.loads(sys.argv[2])]))
"#;
    let python =
        std::env::var("RUST_TEST_PYTHON").expect("explicit project Python interpreter required");
    let root = repo_root().canonicalize().unwrap();
    let output = Command::new(python)
        .args(["-c", SCRIPT])
        .arg(&root)
        .arg(cases.to_string())
        .current_dir(&root)
        .env_clear()
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
        .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
        .output()
        .expect("run the checkpoint validator");
    assert!(
        output.status.success(),
        "validator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("validator prints a JSON list")
}

#[test]
fn migration_is_the_single_head_and_fits_the_version_column() {
    assert!(REVISION.len() <= MAX_REVISION_LENGTH);
    let graph = migration_graph();
    let own: Vec<_> = graph
        .iter()
        .filter(|(revision, _)| revision == REVISION)
        .collect();
    assert_eq!(own.len(), 1, "exactly one migration declares the revision");
    assert_eq!(own[0].1, [PARENT.to_owned()]);
    // The migration is the only head: nothing names it as a parent, and every
    // other revision is the parent of some revision.
    let parents: Vec<&String> = graph.iter().flat_map(|(_, parents)| parents).collect();
    let heads: Vec<&String> = graph
        .iter()
        .map(|(revision, _)| revision)
        .filter(|revision| !parents.contains(revision))
        .collect();
    assert_eq!(heads, [&REVISION.to_owned()]);
    assert!(
        repo_root()
            .join("alembic/versions/ob_20261031_individual_offboarding.py")
            .is_file()
    );
}

#[test]
fn revision_allowlists_end_with_the_offboarding_revision() {
    let reviewed: Vec<String> =
        serde_json::from_str(&read("deploy/rust/allowed-revisions.json")).unwrap();
    assert_eq!(reviewed.last().map(String::as_str), Some(REVISION));
    assert_eq!(
        reviewed.iter().rev().nth(1).map(String::as_str),
        Some(PARENT),
        "the parent checkpoint stays accepted for a one-change rollback"
    );
    assert_eq!(ALLOWED_REVISIONS.last().copied(), Some(REVISION));
    assert_eq!(ALLOWED_REVISIONS.len(), reviewed.len());

    // The deploy enables the stream listener at every checkpoint that keeps the stream tables.
    let deploy = read("scripts/deploy.sh");
    let line = deploy
        .lines()
        .find(|line| line.contains(r#"RUST_STREAM_LISTENER="1" ;;"#))
        .expect("stream listener checkpoint list");
    let listed: Vec<&str> = line.trim().split(')').next().unwrap().split('|').collect();
    assert_eq!(listed.last().copied(), Some(REVISION));
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
fn staging_runtime_accepts_the_offboarding_revision() {
    assert!(validate_config(&runtime_configuration("staging", REVISION)).is_ok());
    assert!(validate_config(&runtime_configuration("staging", PARENT)).is_ok());
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
    assert!(
        read("deploy/postgres/converge_runtime_object_acls.sql")
            .contains(r"\ir app_acl_policy.sql")
    );
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
