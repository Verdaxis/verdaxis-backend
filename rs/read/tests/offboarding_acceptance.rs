//! Integrated acceptance checks of individual offboarding that need no
//! database: the release switch follows the deployed `.env` only, and only
//! organization rejection and offboarding leave a member without an
//! organization.
//!
//! `switch_follows_the_deployed_environment` runs the deploy projection
//! (`scripts/prepare_rust_runtime.py`) with the project's interpreter
//! (`RUST_TEST_PYTHON`, default `python3`).
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::json;
use verdaxis_read::runtime::config::config_from_deployed_environment;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Every file under `folder` with one of `extensions`, recursively.
fn files(folder: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repository_root())
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// One `KEY=value` or `KEY="escaped value"` line of a systemd environment
/// file, as `write_env_file` renders it.
fn environment_line(line: &str) -> (String, String) {
    let (key, value) = line.split_once('=').expect("KEY=value");
    let value = match value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(quoted) => quoted.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => value.to_owned(),
    };
    (key.to_owned(), value)
}

/// The deployed `.env` alone decides the switch. A fresh deploy whose `.env`
/// does not set it projects nothing, and the Rust process reads an absent
/// switch as off; an `.env` that sets it reaches the Rust process in normal
/// form. The projection refuses `true` in production (unit tests in
/// `tests/unit/test_rust_release.py`). The Caddy snippet that routes the group
/// to Rust is imported only on the host's site file, never by a deploy source,
/// and no unit or deploy script other than the projection names the switch.
#[test]
fn switch_follows_the_deployed_environment() {
    let root = repository_root();
    let release = "a".repeat(40);
    let budget = json!({"version":1,"release_sha":release,"environment_digest":"b".repeat(64),
        "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":17,
        "python_budget":52,"total":69,"configured_max_connections":100})
    .to_string();
    let bind = "127.0.0.1:8011".parse().unwrap();
    let switch = |environment: &HashMap<String, String>| {
        config_from_deployed_environment(environment, bind, &budget)
            .map(|config| config.phase2.offboarding_enabled)
    };

    let (fresh, fresh_folder) = project_deployed_environment(&root, &release, None);
    assert!(
        !fresh.contains_key("OFFBOARDING_ENABLED"),
        "a fresh deploy projects the switch: {fresh:?}"
    );
    assert_eq!(switch(&fresh), Ok(false), "a fresh deploy is off");

    let (enabled, enabled_folder) =
        project_deployed_environment(&root, &release, Some("OFFBOARDING_ENABLED=yes"));
    assert_eq!(
        enabled.get("OFFBOARDING_ENABLED").map(String::as_str),
        Some("true")
    );
    assert_eq!(switch(&enabled), Ok(true), "the .env turns it on");
    // The runtime file names data files in its folder; remove them only now.
    std::fs::remove_dir_all(&fresh_folder).unwrap();
    std::fs::remove_dir_all(&enabled_folder).unwrap();

    // No deploy source imports the routing snippet; only the projection names
    // the switch.
    let snippet = root.join("deploy/caddy/rust-offboarding.caddy");
    let projection = root.join("scripts/prepare_rust_runtime.py");
    let mut deploy_sources = files(
        &root.join("deploy"),
        &[
            "caddy", "service", "timer", "json", "tsv", "sql", "manifest",
        ],
    );
    deploy_sources.extend(files(&root.join("scripts"), &["sh", "py"]));
    for path in deploy_sources {
        if path == snippet {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for needle in ["verdaxis_rust_offboarding", "rust-offboarding.caddy"] {
            assert!(!text.contains(needle), "{} names {needle}", relative(&path));
        }
        if path != projection {
            assert!(
                !text.contains("OFFBOARDING_ENABLED"),
                "{} names OFFBOARDING_ENABLED",
                relative(&path)
            );
        }
    }
}

/// Run the deploy projection (`scripts/prepare_rust_runtime.py`) over a staging
/// `.env`, with one extra line when given. Return the Rust runtime file and
/// the temporary folder, which the caller removes after its last read.
fn project_deployed_environment(
    root: &Path,
    release: &str,
    extra_line: Option<&str>,
) -> (HashMap<String, String>, PathBuf) {
    let folder = std::env::temp_dir().join(format!("offboarding-deploy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let source = folder.join("deployed.env");
    let mut lines = vec![
        "ENVIRONMENT=staging".to_owned(),
        format!("RELEASE_SHA={release}"),
        "DATABASE_URL=postgresql://verdaxis_app_staging:disposable@127.0.0.1:5432/verdaxis_staging"
            .into(),
        "JWT_SECRET=runtime-test-authority-0000000000000000".into(),
        "JWT_ISSUER=test-api".into(),
        "JWT_AUDIENCE=test-web".into(),
        "FRONTEND_URL=https://staging.verdaxis.exchange".into(),
        "EMAIL_FROM=noreply@example.test".into(),
        "RESEND_API_KEY=disposable-key".into(),
    ];
    lines.extend(extra_line.map(str::to_owned));
    std::fs::write(&source, lines.join("\n") + "\n").unwrap();
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let output = Command::new(python)
        .current_dir(root)
        .env("PYTHONPATH", root)
        .arg(root.join("scripts/prepare_rust_runtime.py"))
        .arg("--environment-file")
        .arg(&source)
        .args(["--release-sha", release])
        .arg("--output-dir")
        .arg(folder.join("prepared"))
        .arg("--source-root")
        .arg(root)
        .args(["--migration-revision", "ob_20261031_offboarding"])
        .args(["--environment", "staging"])
        .output()
        .expect("Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH");
    assert!(
        output.status.success(),
        "the projection refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let runtime = std::fs::read_to_string(folder.join("prepared/runtime.env")).unwrap();
    let environment = runtime
        .lines()
        .filter(|line| !line.is_empty())
        .map(environment_line)
        .collect();
    (environment, folder)
}

/// A source statement that sets a member's organization to none, in Rust SQL
/// text (`organization_id=NULL`, spaces allowed) or a Python attribute
/// assignment (`.organization_id = None`).
fn detaches(line: &str) -> bool {
    // Only the spaces around `=` are removed, so a word before the column
    // name stays separated from it.
    let mut compact = line.to_owned();
    while compact.contains(" =") || compact.contains("= ") {
        compact = compact.replace(" =", "=").replace("= ", "=");
    }
    let sql = compact
        .match_indices("organization_id=NULL")
        .any(|(at, _)| {
            compact[..at]
                .chars()
                .last()
                .is_none_or(|before| !(before.is_ascii_alphanumeric() || before == '_'))
        });
    sql || compact.contains(".organization_id=None")
}

/// The organization-less branch of the recorded-member rule (and the trade
/// read) admits a member of a rejected organization because only two flows
/// leave a member without an organization: rejection of the organization,
/// in both stacks, and offboarding, which also sets the marker those paths
/// refuse. The scan covers the application sources of both stacks; the two
/// other statements it finds are test fixtures compiled only into tests.
#[test]
fn only_rejection_and_offboarding_detach_a_user() {
    let root = repository_root();
    let mut found: Vec<String> = Vec::new();
    for (folder, extension) in [("rs/read/src", "rs"), ("app", "py")] {
        for path in files(&root.join(folder), &[extension]) {
            let text = std::fs::read_to_string(&path).unwrap();
            for line in text.lines() {
                if detaches(line) {
                    found.push(relative(&path));
                }
            }
        }
    }
    found.sort();
    assert_eq!(
        found,
        [
            // Python organization rejection.
            "app/routers/auth_simple.py",
            // The offboarding command, which sets the marker in the same statement.
            "rs/read/src/business/offboarding.rs",
            // Rust organization rejection.
            "rs/read/src/business/organizations.rs",
            // Test fixture in a `cfg(any(test, feature = "contract-tests"))` module.
            "rs/read/src/order_write/admission.rs",
            // Test fixture module compiled only under `cfg(all(test, feature = "contract-tests"))`.
            "rs/read/src/order_write/ordered_contract.rs",
        ]
    );
    let python = std::fs::read_to_string(root.join("app/routers/auth_simple.py")).unwrap();
    let assignment = python.lines().position(detaches).unwrap();
    let function = python
        .lines()
        .take(assignment)
        .filter(|line| line.starts_with("async def "))
        .last();
    assert_eq!(
        function,
        Some("async def reject_organization("),
        "the Python statement belongs to organization rejection"
    );
    let rust = std::fs::read_to_string(root.join("rs/read/src/business/organizations.rs")).unwrap();
    let statement = rust.lines().position(detaches).unwrap();
    assert!(
        rust.lines()
            .take(statement)
            .any(|line| line.contains("\"organization_rejected\"")),
        "the Rust statement belongs to organization rejection"
    );
    let admission =
        std::fs::read_to_string(root.join("rs/read/src/order_write/admission.rs")).unwrap();
    let fixture = admission.lines().position(detaches).unwrap();
    let gate = admission
        .lines()
        .position(|line| line.contains("#[cfg(any(test, feature = \"contract-tests\"))]"))
        .unwrap();
    assert!(gate < fixture, "the admission statement is a test fixture");
    let owner =
        std::fs::read_to_string(root.join("rs/read/src/order_write/transaction.rs")).unwrap();
    assert!(
        owner.contains(
            "#[cfg(all(test, feature = \"contract-tests\"))]\n#[path = \"ordered_contract.rs\"]"
        ),
        "the ordered contract module is compiled only into tests"
    );
}
