//! Phase 10 Task 10: the reviewed unit bytes launch the release binary.
//!
//! These cells read the committed unit files and run their exact commands,
//! with only the fixed checkout path relocated into a scratch directory and
//! systemd's `${NAME}` variables substituted. They replace these Python tests,
//! which ran the retired Python prestarts:
//!
//! | Retired Python test | Cell here |
//! |---|---|
//! | `test_rust_units::test_exact_rust_prestarts_need_no_app_environment` | `serve_prestarts_run_exactly_with_no_environment` |
//! | `test_rust_production::test_production_exact_prestarts_run_with_no_environment_or_app` | `serve_prestarts_run_exactly_with_no_environment` |
//! | `test_rust_units::test_rust_start_preserves_environment_arguments_and_main_pid` | `serve_launcher_preserves_pid_and_environment` |
//! | `test_rust_production_bootstrap::test_production_env_launcher_preserves_pid_and_release` | `serve_launcher_preserves_pid_and_environment` |
//! | `test_rust_units::test_rust_unit_runs_the_start_guard_and_the_revision_check` | `every_unit_exec_start_is_the_release_binary_or_a_shell_guard` |
//! | `test_rust_units::test_rust_unit_budget_check_uses_the_prepared_artifact_and_real_flags` | `serve_unit_paths_are_the_files_the_deploy_transaction_writes` |
//! | `test_rust_units::test_rust_unit_keeps_runtime_credentials_separate` | `serve_unit_paths_are_the_files_the_deploy_transaction_writes` |
//! | `test_rust_units::test_rust_unit_paths_are_the_files_the_deploy_transaction_writes` | `serve_unit_paths_are_the_files_the_deploy_transaction_writes` |
//!
//! Scope (Task 10a): the 16 canonical units and the five Task 27/28 local
//! monitor units. The demo, onboarding and external units follow with their
//! commands (Task 10b).
use std::collections::BTreeMap;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const REVISION: &str = "p89_20261022_trade_freeze";
const PRODUCTION_ROOT: &str = "/home/verdaxis-prod/verdaxis/prod/be";
const STAGING_ROOT: &str = "/home/verdaxis-prod/verdaxis/staging/be";
const HOST_COPY: &str = "/usr/local/libexec/verdaxis/verdaxis-read";
/// The manifest's canonical Rust units, in manifest order.
const CANONICAL: [(&str, &str); 16] = [
    ("production", "verdaxis-rust.service"),
    ("production", "verdaxis-news-refresh.service"),
    ("production", "verdaxis-news-refresh.timer"),
    ("production", "verdaxis-auth-maintenance.service"),
    ("production", "verdaxis-auth-maintenance.timer"),
    ("production", "verdaxis-product-analytics-prune.service"),
    ("production", "verdaxis-product-analytics-prune.timer"),
    ("staging", "verdaxis-rust-staging.service"),
    ("staging", "verdaxis-news-refresh-staging.service"),
    ("staging", "verdaxis-news-refresh-staging.timer"),
    ("staging", "verdaxis-auth-maintenance-staging.service"),
    ("staging", "verdaxis-auth-maintenance-staging.timer"),
    (
        "staging",
        "verdaxis-product-analytics-prune-staging.service",
    ),
    ("staging", "verdaxis-product-analytics-prune-staging.timer"),
    ("production", "verdaxis-order-expiry-reminders.service"),
    ("production", "verdaxis-order-expiry-reminders.timer"),
];
/// Job service stem and its `job` kind; the env file has the kind's name.
const JOBS: [(&str, &str); 4] = [
    ("verdaxis-news-refresh", "news"),
    ("verdaxis-auth-maintenance", "auth-maintenance"),
    ("verdaxis-product-analytics-prune", "prune-analytics"),
    ("verdaxis-order-expiry-reminders", "reminders"),
];
/// The Task 27/28 local monitor units and the command each runs.
const MONITORS: [(&str, &str); 4] = [
    ("verdaxis-health.service", "monitor health"),
    ("verdaxis-backup-verify.service", "monitor backup-verify"),
    (
        "verdaxis-monitor-alert@.service",
        "monitor alert --event %i",
    ),
    (
        "verdaxis-monitor-alert-reminder.service",
        "monitor alert --remind all",
    ),
];
const LOCAL_TIMERS: [&str; 3] = [
    "verdaxis-health.timer",
    "verdaxis-backup-verify.timer",
    "verdaxis-monitor-alert-reminder.timer",
];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn text(relative: &str) -> String {
    std::fs::read_to_string(repository().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn canonical(name: &str) -> String {
    text(&format!("deploy/systemd/{name}"))
}

fn root_of(environment: &str) -> &'static str {
    match environment {
        "production" => PRODUCTION_ROOT,
        _ => STAGING_ROOT,
    }
}

/// Every value of `key=` in the unit, in order.
fn directives<'a>(unit: &'a str, key: &str) -> Vec<&'a str> {
    let prefix = format!("{key}=");
    unit.lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()))
        .collect()
}

fn job_services() -> Vec<(&'static str, String, &'static str)> {
    CANONICAL
        .iter()
        .filter(|(_, name)| name.ends_with(".service") && !name.starts_with("verdaxis-rust"))
        .map(|(environment, name)| {
            let stem = name
                .trim_end_matches(".service")
                .trim_end_matches("-staging");
            let kind = JOBS
                .iter()
                .find(|(job_stem, _)| *job_stem == stem)
                .map(|(_, kind)| *kind)
                .unwrap_or_else(|| panic!("{name} is not a known job"));
            (*environment, (*name).to_owned(), kind)
        })
        .collect()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-units-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join(".runtime-rust/jobs")).unwrap();
        std::fs::create_dir_all(path.join("deploy/scripts")).unwrap();
        symlink(
            env!("CARGO_BIN_EXE_verdaxis-read"),
            path.join(".runtime-rust/verdaxis-read"),
        )
        .unwrap();
        Self(path)
    }

    fn private(&self, relative: &str, contents: &str) {
        let path = self.0.join(relative);
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// The unit command with the checkout relocated and systemd's variables
    /// substituted, split on whitespace (no unit command quotes anything).
    fn command(&self, line: &str, root: &str, variables: &BTreeMap<String, String>) -> Vec<String> {
        let mut command = line.replace(root, self.0.to_str().unwrap());
        for (key, value) in variables {
            command = command.replace(&format!("${{{key}}}"), value);
        }
        assert!(!command.contains("${"), "unsubstituted variable: {command}");
        command.split_whitespace().map(str::to_owned).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(arguments: &[String], environment: &BTreeMap<String, String>, directory: &Path) -> Output {
    Command::new(&arguments[0])
        .args(&arguments[1..])
        .env_clear()
        .envs(environment)
        .current_dir(directory)
        .output()
        .unwrap()
}

#[test]
fn manifest_selects_exactly_the_canonical_rust_units() {
    let manifest = text("deploy/systemd/runtime-units.manifest");
    let rows: Vec<(&str, &str)> = manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_once('\t').expect("environment<TAB>unit"))
        .collect();
    assert_eq!(rows, CANONICAL.to_vec());
    for (_, name) in CANONICAL {
        assert!(
            repository().join("deploy/systemd").join(name).is_file(),
            "{name}"
        );
    }
    // The backend pair stays installed from its current file until Task 32;
    // the staging reminder pair is not installed (D1).
    for absent in ["verdaxis-backend", "order-expiry-reminders-staging"] {
        assert!(!manifest.contains(absent), "{absent}");
    }
}

#[test]
fn no_unit_contains_python_or_a_venv_path() {
    let mut units: Vec<(String, String)> = CANONICAL
        .iter()
        .map(|(_, name)| ((*name).to_owned(), canonical(name)))
        .collect();
    for name in MONITORS.iter().map(|(name, _)| *name).chain(LOCAL_TIMERS) {
        units.push((name.to_owned(), text(&format!("deploy/monitor/{name}"))));
    }
    for (name, unit) in units {
        let lower = unit.to_ascii_lowercase();
        // `/be/scripts/` is the Python script directory; the shell guard lives
        // in `deploy/scripts/`.
        for needle in ["python", "venv", ".py ", ".py\n", "uvicorn", "/be/scripts/"] {
            assert!(!lower.contains(needle), "{name} contains {needle:?}");
        }
    }
    assert!(!text("scripts/install_systemd_units.sh").contains("python"));
}

#[test]
fn every_unit_exec_start_is_the_release_binary_or_a_shell_guard() {
    for (environment, name) in CANONICAL.iter().filter(|(_, n)| n.ends_with(".service")) {
        let unit = canonical(name);
        let root = root_of(environment);
        let binary = format!("{root}/.runtime-rust/verdaxis-read");
        assert!(directives(&unit, "ExecStartPost").is_empty(), "{name}");
        assert!(directives(&unit, "ExecStop").is_empty(), "{name}");
        let starts = directives(&unit, "ExecStart");
        assert_eq!(starts.len(), 1, "{name}");
        assert!(
            starts[0].starts_with(&format!("/usr/bin/env {binary} ")),
            "{name}: {}",
            starts[0]
        );
        let prestarts = directives(&unit, "ExecStartPre");
        if name.starts_with("verdaxis-rust") {
            assert_eq!(
                prestarts,
                [
                    format!(
                        "/usr/bin/env -i PATH=/usr/bin:/bin /bin/bash {root}/deploy/scripts/verify-deploy-start-guard.sh --environment {environment} --expected-release-sha ${{RELEASE_SHA}}"
                    ),
                    format!(
                        "/usr/bin/env -i {binary} guard rust-start --expected-release-sha ${{RELEASE_SHA}} --budget-file {root}/.runtime-rust/budget.json --expected-revision ${{MIGRATION_REVISION}} --revision-file {root}/.runtime-rust/revision.json"
                    ),
                ],
                "{name}"
            );
        } else {
            assert!(
                prestarts.is_empty(),
                "{name}: the binary runs the job gates"
            );
        }
    }
}

#[test]
fn job_units_load_only_their_purpose_env() {
    let policy = text("deploy/rust/runtime-policy.env");
    for (environment, name, kind) in job_services() {
        let unit = canonical(&name);
        let root = root_of(environment);
        let delivery = format!("jobs/{kind}.env");
        assert!(
            policy
                .lines()
                .any(|row| row.starts_with(&format!("job.{kind}\t")) && row.ends_with(&delivery)),
            "{kind} has a policy file"
        );
        assert_eq!(
            directives(&unit, "EnvironmentFile"),
            [format!("{root}/.runtime-rust/{delivery}")],
            "{name}"
        );
        assert_eq!(
            directives(&unit, "Environment"),
            [format!("DEPLOY_STATE_DIR={root}/.runtime-deploy")],
            "{name}"
        );
        assert_eq!(
            directives(&unit, "ExecStart"),
            [format!(
                "/usr/bin/env {root}/.runtime-rust/verdaxis-read job {kind} --environment {environment} --release-sha ${{RELEASE_SHA}} --apply"
            )],
            "{name}"
        );
        let state = format!("{root}/.runtime-deploy/{environment}.state");
        assert!(
            directives(&unit, "ConditionPathExists").contains(&format!("!{state}").as_str()),
            "{name}: a live deploy skips the job"
        );
        assert_eq!(directives(&unit, "User"), ["verdaxis-prod"], "{name}");
    }
}

#[test]
fn demo_units_run_the_rust_job_with_their_own_env_file() {
    for (environment, name) in [
        ("production", "verdaxis-demo-activity.service"),
        ("staging", "verdaxis-demo-activity-staging.service"),
    ] {
        let unit = text(&format!("deploy/monitor/{name}"));
        let root = root_of(environment);
        assert_eq!(
            directives(&unit, "EnvironmentFile"),
            [format!("{root}/.runtime-rust/jobs/demo.env")],
            "{name}"
        );
        assert_eq!(
            directives(&unit, "Environment"),
            [format!("DEPLOY_STATE_DIR={root}/.runtime-deploy")],
            "{name}"
        );
        assert_eq!(
            directives(&unit, "ExecStart"),
            [format!(
                "/usr/bin/env {root}/.runtime-rust/verdaxis-read job demo --environment {environment} --release-sha ${{RELEASE_SHA}} --apply"
            )],
            "{name}"
        );
        assert_eq!(directives(&unit, "User"), ["verdaxis-prod"], "{name}");
        let state = format!("!{root}/.runtime-deploy/{environment}.state");
        assert!(
            directives(&unit, "ConditionPathExists").contains(&state.as_str()),
            "{name}"
        );
        for forbidden in [
            "python",
            "venv",
            "verify_runtime_identity",
            "/etc/verdaxis-monitor/demo/",
        ] {
            assert!(
                !unit.to_ascii_lowercase().contains(forbidden),
                "{name}: {forbidden}"
            );
        }
    }
}

#[test]
fn job_unit_timeouts_cover_the_job_deadline() {
    for (_, name, kind) in job_services() {
        let unit = canonical(&name);
        let timeout: u64 = directives(&unit, "TimeoutStartSec")[0].parse().unwrap();
        // News: about 810 s worst case (Task 11 L3). Prune: 240 s deadline.
        let floor = match kind {
            "news" => 900,
            _ => 300,
        };
        assert!(timeout >= floor, "{name}: {timeout} < {floor}");
        if kind == "prune-analytics" {
            assert_eq!(directives(&unit, "Restart"), ["on-failure"]);
            assert_eq!(directives(&unit, "RestartPreventExitStatus"), ["2 3"]);
        }
    }
}

#[test]
fn job_unit_commands_reach_the_database_through_every_gate() {
    // Each job's own keys with placeholder values; the app URL is the
    // environment's role on a closed port. Exit 4 proves the flags parsed, the
    // job dispatched, and the identity and deploy-state gates passed.
    for (environment, name, kind) in job_services() {
        let scratch = Scratch::new(kind);
        let unit = canonical(&name);
        let root = root_of(environment);
        let (role, database) = match environment {
            "production" => ("verdaxis_app", "verdaxis_prod"),
            _ => ("verdaxis_app_staging", "verdaxis_staging"),
        };
        let mut variables: BTreeMap<String, String> = [
            ("ENVIRONMENT", environment.to_owned()),
            ("RELEASE_SHA", SHA.to_owned()),
            (
                "DATABASE_URL",
                format!("postgresql://{role}:pw@127.0.0.1:1/{database}"),
            ),
            ("RESEND_API_KEY", "re_placeholder".to_owned()),
            ("EMAIL_FROM", "Verdaxis <ops@example.invalid>".to_owned()),
            ("FRONTEND_URL", "https://app.example.invalid".to_owned()),
            ("PATH", "/usr/bin:/bin".to_owned()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
        let relocated = scratch.0.to_str().unwrap().to_owned();
        for assignment in directives(&unit, "Environment") {
            let (key, value) = assignment.split_once('=').unwrap();
            variables.insert(key.to_owned(), value.replace(root, &relocated));
        }
        std::fs::create_dir(scratch.0.join(".runtime-deploy")).unwrap();
        let command = scratch.command(directives(&unit, "ExecStart")[0], root, &variables);
        let output = run(&command, &variables, &scratch.0);
        assert_eq!(
            output.status.code(),
            Some(4),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // A live deploy state refuses before the database.
        std::fs::write(
            scratch
                .0
                .join(format!(".runtime-deploy/{environment}.state")),
            "blocked\n",
        )
        .unwrap();
        assert_eq!(
            run(&command, &variables, &scratch.0).status.code(),
            Some(3),
            "{name}"
        );
    }
}

#[test]
fn serve_prestarts_run_exactly_with_no_environment() {
    for (environment, name) in [
        ("production", "verdaxis-rust.service"),
        ("staging", "verdaxis-rust-staging.service"),
    ] {
        let scratch = Scratch::new(environment);
        std::fs::copy(
            repository().join("deploy/scripts/verify-deploy-start-guard.sh"),
            scratch
                .0
                .join("deploy/scripts/verify-deploy-start-guard.sh"),
        )
        .unwrap();
        scratch.private(
            ".runtime-rust/budget.json",
            &format!("{{\"version\":1,\"release_sha\":\"{SHA}\",\"read_slots\":16}}"),
        );
        scratch.private(
            ".runtime-rust/revision.json",
            &format!("{{\"release_sha\":\"{SHA}\",\"revision\":\"{REVISION}\"}}"),
        );
        let unit = canonical(name);
        let variables: BTreeMap<String, String> = [
            ("RELEASE_SHA".to_owned(), SHA.to_owned()),
            ("MIGRATION_REVISION".to_owned(), REVISION.to_owned()),
        ]
        .into();
        // systemd passes the unit environment; `env -i` must drop it, so a
        // database URL in it cannot reach either prestart.
        let unit_environment: BTreeMap<String, String> = [(
            "DATABASE_URL".to_owned(),
            "postgresql://must-not-be-used".to_owned(),
        )]
        .into();
        let prestarts: Vec<Vec<String>> = directives(&unit, "ExecStartPre")
            .into_iter()
            .map(|line| scratch.command(line, root_of(environment), &variables))
            .collect();
        for prestart in &prestarts {
            let output = run(prestart, &unit_environment, &scratch.0);
            assert!(
                output.status.success(),
                "{name}: {prestart:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(!scratch.0.join(".env").exists());
        // Tampered checkpoint evidence is refused by the same unit command.
        scratch.private(
            ".runtime-rust/revision.json",
            &format!("{{\"release_sha\":\"{SHA}\",\"revision\":\"wrong\"}}"),
        );
        assert!(
            !run(&prestarts[1], &unit_environment, &scratch.0)
                .status
                .success()
        );
        // A live deploy without the restart window denies the start.
        std::fs::create_dir(scratch.0.join(".runtime-deploy")).unwrap();
        scratch.private(&format!(".runtime-deploy/{environment}.state"), "blocked\n");
        assert!(
            !run(&prestarts[0], &unit_environment, &scratch.0)
                .status
                .success()
        );
    }
}

#[test]
fn serve_launcher_preserves_pid_and_environment() {
    for (environment, name, bind) in [
        ("production", "verdaxis-rust.service", "127.0.0.1:8010"),
        ("staging", "verdaxis-rust-staging.service", "127.0.0.1:8011"),
    ] {
        let root = root_of(environment);
        let unit = canonical(name);
        let start: Vec<&str> = directives(&unit, "ExecStart")[0]
            .split_whitespace()
            .collect();
        assert_eq!(
            start,
            [
                "/usr/bin/env",
                &format!("{root}/.runtime-rust/verdaxis-read"),
                "serve",
                "--bind",
                bind,
                "--budget-file",
                &format!("{root}/.runtime-rust/budget.json"),
            ]
        );
        // The launcher execs in the same PID and keeps the unit environment.
        let child = Command::new(start[0])
            .args(["/bin/sh", "-c", "echo $$ $RELEASE_SHA"])
            .env_clear()
            .env("RELEASE_SHA", SHA)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("{pid} {SHA}")
        );
    }
}

#[test]
fn serve_unit_paths_are_the_files_the_deploy_transaction_writes() {
    let deploy = text("scripts/deploy.sh");
    for line in [
        "RUST_RUNTIME_DIR=\"$BACKEND_DIR/.runtime-rust\"",
        "ACTIVE_BINARY=\"$RUST_RUNTIME_DIR/verdaxis-read\"",
        "--output \"$RUST_RUNTIME_DIR/budget.json\"",
    ] {
        assert!(deploy.contains(line), "{line}");
    }
    for (environment, name) in [
        ("production", "verdaxis-rust.service"),
        ("staging", "verdaxis-rust-staging.service"),
    ] {
        let root = root_of(environment);
        let unit = canonical(name);
        assert_eq!(directives(&unit, "WorkingDirectory"), [root]);
        // Deploy-written files only; the secret-rich `.env` never reaches serve.
        assert_eq!(
            directives(&unit, "EnvironmentFile"),
            [
                format!("{root}/.runtime-release.env"),
                format!("{root}/.runtime-rust/runtime.env"),
            ]
        );
        assert_eq!(directives(&unit, "User"), ["verdaxis-prod"]);
        assert_eq!(directives(&unit, "MemoryMax"), ["256M"]);
        let credential = match environment {
            "production" => "kyc-document-keys:/etc/verdaxis/kyc-document-keys.json",
            _ => "kyc-document-keys:/etc/verdaxis/kyc-document-keys-staging.json",
        };
        assert_eq!(directives(&unit, "LoadCredential"), [credential]);
        let pre = unit.find("guard rust-start").unwrap();
        assert!(pre < unit.find("ExecStart=").unwrap());
    }
}

#[test]
fn monitor_units_exec_the_digest_pinned_host_copy() {
    let manifest: serde_json::Value =
        serde_json::from_str(&text("deploy/monitor/artifact-manifest.json")).unwrap();
    let listed: Vec<&str> = manifest["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|artifact| artifact["source"].as_str().unwrap())
        .collect();
    for (name, command) in MONITORS {
        let unit = text(&format!("deploy/monitor/{name}"));
        assert_eq!(
            directives(&unit, "ExecStart"),
            [format!("{HOST_COPY} {command}")],
            "{name}"
        );
        assert!(directives(&unit, "ExecStartPre").is_empty(), "{name}");
        assert!(
            listed.contains(&name),
            "{name} is digest-pinned in the artifact manifest"
        );
    }
    for name in LOCAL_TIMERS {
        assert!(listed.contains(&name), "{name}");
    }
    for (name, suffix) in [
        ("verdaxis-monitor.service", ""),
        ("verdaxis-monitor-verify.service", " --telegram-disabled"),
    ] {
        let source = format!("../external_monitor/systemd/{name}");
        let unit = text(&format!("deploy/external_monitor/systemd/{name}"));
        assert_eq!(
            directives(&unit, "ExecStart"),
            [format!("{HOST_COPY} monitor external{suffix}")]
        );
        assert!(listed.contains(&source.as_str()), "{name}");
        assert_eq!(directives(&unit, "User"), ["root"]);
        assert!(directives(&unit, "OnFailure").is_empty());
        assert!(directives(&unit, "OnFailureJobMode").is_empty());
    }
    for name in ["verdaxis-health.service", "verdaxis-backup-verify.service"] {
        assert_eq!(
            directives(
                &text(&format!("deploy/monitor/{name}")),
                "StateDirectoryMode"
            ),
            ["0710"]
        );
    }
}

/// Decision 203: Caddy and the PostgreSQL cluster restart after an abnormal
/// exit on the production host (their distribution units have Restart=no).
#[test]
fn host_service_overrides_restart_on_failure() {
    for unit in ["caddy.service", "postgresql@17-main.service"] {
        let text = std::fs::read_to_string(repository().join(format!(
            "deploy/systemd/overrides/{unit}.d/10-verdaxis-restart.conf"
        )))
        .unwrap();
        let settings: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect();
        assert_eq!(
            settings,
            ["[Service]", "Restart=on-failure", "RestartSec=5s"],
            "{unit}"
        );
    }
}

#[test]
fn onboarding_unit_runs_the_rust_monitor_with_its_own_env_file() {
    let unit = text("deploy/external_monitor/systemd/verdaxis-onboarding-attention.service");
    // Items 1-4 of the lane brief: the reviewed unit bytes.
    // 1. ExecStart is the Rust `monitor onboarding` command.
    assert_eq!(
        directives(&unit, "ExecStart"),
        [format!(
            "/usr/bin/env {PRODUCTION_ROOT}/.runtime-rust/verdaxis-read monitor onboarding --environment production --release-sha ${{RELEASE_SHA}} --state-file /var/lib/verdaxis-onboarding-attention/state.json"
        )]
    );
    // 2. Only the purpose env, the release env and the optional monitor env;
    // the full backend .env is removed and made inaccessible.
    assert_eq!(
        directives(&unit, "EnvironmentFile"),
        [
            format!("{PRODUCTION_ROOT}/.runtime-rust/jobs/monitor-onboarding.env"),
            format!("{PRODUCTION_ROOT}/.runtime-release.env"),
            "-/etc/verdaxis-monitor.env".to_owned(),
        ]
    );
    assert_eq!(
        directives(&unit, "Environment"),
        [format!(
            "DEPLOY_STATE_DIR={PRODUCTION_ROOT}/.runtime-deploy"
        )]
    );
    assert!(
        directives(&unit, "ConditionPathExists")
            .iter()
            .all(|v| !v.contains("/be/.env"))
    );
    assert!(
        directives(&unit, "InaccessiblePaths")
            .contains(&format!("{PRODUCTION_ROOT}/.env").as_str())
    );
    // 3. No python, no venv anywhere in the unit.
    let lower = unit.to_ascii_lowercase();
    for needle in ["python", "venv", ".py ", ".py\n", "uvicorn", "/be/scripts/"] {
        assert!(!lower.contains(needle), "{needle:?}");
    }
    // 4. Identity, state and hardening are the reviewed existing values,
    // and Telegram needs the network and PostgreSQL units to be present.
    for (key, expected) in [
        ("User", "verdaxis-prod"),
        ("StateDirectory", "verdaxis-onboarding-attention"),
        ("StateDirectoryMode", "0700"),
        ("ReadWritePaths", "/var/lib/verdaxis-onboarding-attention"),
        ("TimeoutStartSec", "5min"),
        ("UMask", "0077"),
        ("Requires", "postgresql.service"),
        ("After", "network-online.target postgresql.service"),
    ] {
        assert_eq!(directives(&unit, key), [expected], "{key}");
    }
    for key in [
        "NoNewPrivileges",
        "PrivateDevices",
        "PrivateTmp",
        "ProtectSystem",
        "ProtectHome",
        "ProtectControlGroups",
        "ProtectKernelModules",
        "ProtectKernelTunables",
        "ProtectClock",
        "ProtectHostname",
        "RestrictRealtime",
        "RestrictSUIDSGID",
        "LockPersonality",
    ] {
        assert!(!directives(&unit, key).is_empty(), "{key} is kept");
    }
}

#[test]
fn manifest_counts_are_16_canonical_11_local_5_external() {
    let inventory: serde_json::Value =
        serde_json::from_str(&text("tests/rust_contract/phase10/units.json")).unwrap();
    for (folder, expected) in [
        ("deploy/systemd/", 16),
        ("deploy/monitor/", 11),
        ("deploy/external_monitor/systemd/", 5),
    ] {
        let rows: Vec<_> = inventory["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["end_state"] == "rust" && row["source"].as_str().unwrap().starts_with(folder)
            })
            .collect();
        assert_eq!(rows.len(), expected, "{folder}");
        for row in rows {
            let source = row["source"].as_str().unwrap();
            assert!(repository().join(source).is_file(), "{source}");
        }
        let actual = std::fs::read_dir(repository().join(folder))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                matches!(
                    entry.path().extension().and_then(|s| s.to_str()),
                    Some("service" | "timer")
                )
            })
            .filter(|entry| {
                inventory["artifacts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| {
                        row["end_state"] == "rust"
                            && row["source"]
                                == format!("{folder}{}", entry.file_name().to_str().unwrap())
                    })
            })
            .count();
        assert_eq!(actual, expected, "{folder}");
    }
}

#[test]
fn timer_catch_up_after_restart_runs_once() {
    let inventory: serde_json::Value =
        serde_json::from_str(&text("tests/rust_contract/phase10/units.json")).unwrap();
    let mut persistent = 0;
    for row in inventory["artifacts"].as_array().unwrap() {
        let source = row["source"].as_str().unwrap();
        if row["end_state"] != "rust" || !source.ends_with(".timer") {
            continue;
        }
        let timer = text(source);
        if !directives(&timer, "Persistent").contains(&"true") {
            continue;
        }
        persistent += 1;
        let service_name = directives(&timer, "Unit")
            .first()
            .copied()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                source
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .replace(".timer", ".service")
            });
        let service_path = Path::new(source).parent().unwrap().join(service_name);
        let service = text(service_path.to_str().unwrap());
        assert_eq!(directives(&service, "Type"), ["oneshot"], "{source}");
        assert!(directives(&service, "Restart").is_empty(), "{source}");
    }
    assert!(persistent > 0);
}

#[test]
fn old_unit_can_start_the_new_binary() {
    let fixture = repository().join("rs/read/tests/fixtures/phase10/old-units");
    let mut commands = 0;
    for folder in ["systemd", "monitor"] {
        for entry in std::fs::read_dir(fixture.join(folder)).unwrap() {
            let path = entry.unwrap().path();
            let unit = std::fs::read_to_string(&path).unwrap();
            for line in directives(&unit, "ExecStart") {
                let words: Vec<String> = line
                    .replace("${RELEASE_SHA}", SHA)
                    .replace("%i", "health")
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                let Some(index) = words
                    .iter()
                    .position(|word| word.ends_with("/verdaxis-read"))
                else {
                    panic!(
                        "old ExecStart is not the release binary: {}",
                        path.display()
                    );
                };
                // --help parses the complete old argv without running a job,
                // reading credentials, connecting to a DB or contacting a host.
                let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
                    .args(&words[index + 1..])
                    .arg("--help")
                    .env_clear()
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}: {}",
                    path.display(),
                    String::from_utf8_lossy(&output.stderr)
                );
                commands += 1;
            }
        }
    }
    assert_eq!(commands, 13);
}
