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
use std::os::unix::fs::{symlink, PermissionsExt};
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
        assert!(!run(&prestarts[1], &unit_environment, &scratch.0)
            .status
            .success());
        // A live deploy without the restart window denies the start.
        std::fs::create_dir(scratch.0.join(".runtime-deploy")).unwrap();
        scratch.private(&format!(".runtime-deploy/{environment}.state"), "blocked\n");
        assert!(!run(&prestarts[0], &unit_environment, &scratch.0)
            .status
            .success());
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
        assert_eq!(directives(&unit, "MemoryMax"), ["768M"]);
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
            [format!(
                "/usr/local/libexec/verdaxis/verdaxis-read-monitor monitor external{suffix}"
            )]
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
    assert!(directives(&unit, "ConditionPathExists")
        .iter()
        .all(|v| !v.contains("/be/.env")));
    assert!(directives(&unit, "InaccessiblePaths")
        .contains(&format!("{PRODUCTION_ROOT}/.env").as_str()));
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
        serde_json::from_str(r####"{"schema_version":1,"source_commit":"2e1ccad2dc1a996bec629dff4c892a32ac3da576","source_checkpoint":"sha256 of each source file; no branch-name identity","artifacts":[{"id":"unit.deploy/systemd/verdaxis-auth-maintenance-staging.service","source":"deploy/systemd/verdaxis-auth-maintenance-staging.service","kind":"unit","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"17fcaadaf5477c509826051d345d49f3ed3e7bf9efb94933faf47ffaacf3d418","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance-staging.service"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance-staging.timer","source":"deploy/systemd/verdaxis-auth-maintenance-staging.timer","kind":"timer","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"7290696b5cfb94e3d6a2b83fa6646e021948e6b13feb335304fb8a09508c2049","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance.service","source":"deploy/systemd/verdaxis-auth-maintenance.service","kind":"unit","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"31f2e5fd3840542a795ebee6e0bfd776b915e8ea742cad9dc3a571618cea586b","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance.service"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance.timer","source":"deploy/systemd/verdaxis-auth-maintenance.timer","kind":"timer","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"a30a00e0f22c43ddf9a8e0f5fcfa379a3f68c176a758ab18647ee7eeaeddd014","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance.timer"},{"id":"unit.deploy/systemd/verdaxis-backend-staging.service","source":"deploy/systemd/verdaxis-backend-staging.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"56f0fe79c9bd0363795209d34c73bb744e3fc1e38f16b84057745f41100820bb","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backend-staging.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-backend.service","source":"deploy/systemd/verdaxis-backend.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"99955437f20e0291faaacdb5ef4c446552aa091f3ab46d58579cbc91391dc9fa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backend.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-news-refresh-staging.service","source":"deploy/systemd/verdaxis-news-refresh-staging.service","kind":"unit","end_state":"rust","owner":"job news","task":11,"source_sha256":"e2f4456bd817a13adab3cb1095ddece50f2f12b5c0885cbc045ca2d833a2baf1","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh-staging.service"},{"id":"unit.deploy/systemd/verdaxis-news-refresh-staging.timer","source":"deploy/systemd/verdaxis-news-refresh-staging.timer","kind":"timer","end_state":"rust","owner":"job news","task":11,"source_sha256":"2d82ecf19866c655f47f74d0ec44f8ecab4e8b7dff28164555006e893b79200b","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-news-refresh.service","source":"deploy/systemd/verdaxis-news-refresh.service","kind":"unit","end_state":"rust","owner":"job news","task":11,"source_sha256":"3ff0d95162b70dceb42579d5586addd6271cef2b9d56ec230c67b25bae62aab9","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh.service"},{"id":"unit.deploy/systemd/verdaxis-news-refresh.timer","source":"deploy/systemd/verdaxis-news-refresh.timer","kind":"timer","end_state":"rust","owner":"job news","task":11,"source_sha256":"ee175cbcbd0bd59cc80a28d9217e0c500cd3cfaa171e00b0696e76fe0933a1c5","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh.timer"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders-staging.service","source":"deploy/systemd/verdaxis-order-expiry-reminders-staging.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"cd8e9e01f2d7a3e80537020b1ebbab0dd33353530c56a15c9a3ef1ed1d0f0069","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders-staging.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders-staging.timer","source":"deploy/systemd/verdaxis-order-expiry-reminders-staging.timer","kind":"timer","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"1e32fd6d19d06ea6bcdae356d28f177e1529aace567b5a382c20b9247e618e12","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders-staging.timer","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders.service","source":"deploy/systemd/verdaxis-order-expiry-reminders.service","kind":"unit","end_state":"rust","owner":"job reminders","task":14,"source_sha256":"193916c8fe4b6fd1ca4013deaec3254f1f1ba00944da29e809405de0b941f410","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders.service"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders.timer","source":"deploy/systemd/verdaxis-order-expiry-reminders.timer","kind":"timer","end_state":"rust","owner":"job reminders","task":14,"source_sha256":"9826106c408bb94af06cf2804f36b60e5f14187b33b0af742c0eadc4d1c54868","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders.timer"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune-staging.service","source":"deploy/systemd/verdaxis-product-analytics-prune-staging.service","kind":"unit","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"849edc45c1ec04e441a6f05667ea7941ead4a056bdb3fd054f481942a6ab9c19","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune-staging.service"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune-staging.timer","source":"deploy/systemd/verdaxis-product-analytics-prune-staging.timer","kind":"timer","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"f6eea37b9acc3736221cec11eda934c85d43b09a092edc58b00ae8502023f317","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune.service","source":"deploy/systemd/verdaxis-product-analytics-prune.service","kind":"unit","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"10cccc3706e01dc26def1aa6d24e5cf0b2f2af8f76105319082d03a9775865cc","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune.service"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune.timer","source":"deploy/systemd/verdaxis-product-analytics-prune.timer","kind":"timer","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"1765328b7ef948909b3b3fb2975cefdf63cd62e546ee40740bc463b2d2f5f5e3","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune.timer"},{"id":"unit.deploy/systemd/verdaxis-rust-staging.service","source":"deploy/systemd/verdaxis-rust-staging.service","kind":"unit","end_state":"rust","owner":"serve","task":10,"source_sha256":"73577839a5b0c3e476783cfea7e5252155494e28dd33e8f507b152e43b743ec5","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-rust-staging.service"},{"id":"unit.deploy/systemd/verdaxis-rust.service","source":"deploy/systemd/verdaxis-rust.service","kind":"unit","end_state":"rust","owner":"serve","task":10,"source_sha256":"bb00aa497ad497cab705f0e8209d54d1bfa5c179462c576b45c7b974457d76d6","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-rust.service"},{"id":"unit.deploy/monitor/verdaxis-backup-verify.service","source":"deploy/monitor/verdaxis-backup-verify.service","kind":"unit","end_state":"rust","owner":"monitor backup-verify","task":27,"source_sha256":"89f009dc127b15e29ae94d02718ee35442f625e67b9d9ff3fae07334cc62474e","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backup-verify.service"},{"id":"unit.deploy/monitor/verdaxis-backup-verify.timer","source":"deploy/monitor/verdaxis-backup-verify.timer","kind":"timer","end_state":"rust","owner":"monitor backup-verify","task":27,"source_sha256":"8fb9fa6da3b8502a9fee577a65438c442f2fdfe5f69fcc3f609d10003db4909c","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backup-verify.timer"},{"id":"unit.deploy/monitor/verdaxis-demo-activity-staging.service","source":"deploy/monitor/verdaxis-demo-activity-staging.service","kind":"unit","end_state":"rust","owner":"job demo","task":17,"source_sha256":"c4791cc13c91ffcfcd68ce4a81715244a17c09949796e911cdd8934dd84e5c6a","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity-staging.service"},{"id":"unit.deploy/monitor/verdaxis-demo-activity-staging.timer","source":"deploy/monitor/verdaxis-demo-activity-staging.timer","kind":"timer","end_state":"rust","owner":"job demo","task":17,"source_sha256":"4e25897e15afb5fc896c309d9aaa4f629f2a91205a30df63c544174eb3a8184e","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity-staging.timer"},{"id":"unit.deploy/monitor/verdaxis-demo-activity.service","source":"deploy/monitor/verdaxis-demo-activity.service","kind":"unit","end_state":"rust","owner":"job demo","task":17,"source_sha256":"42e2ed46153da9003ec2123eac49e62179d0628ea62039de773fea04629ff0d2","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity.service"},{"id":"unit.deploy/monitor/verdaxis-demo-activity.timer","source":"deploy/monitor/verdaxis-demo-activity.timer","kind":"timer","end_state":"rust","owner":"job demo","task":17,"source_sha256":"5f3fefba55a4c1a6760b2de4ab14bd7595fefc80fab0d5f700a04127afa77dfc","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity.timer"},{"id":"unit.deploy/monitor/verdaxis-health.service","source":"deploy/monitor/verdaxis-health.service","kind":"unit","end_state":"rust","owner":"monitor health","task":27,"source_sha256":"4bb7b661acf541aec94db8ea4b4b193bf9be1070d4bcd3da9873ec942bb62986","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-health.service"},{"id":"unit.deploy/monitor/verdaxis-health.timer","source":"deploy/monitor/verdaxis-health.timer","kind":"timer","end_state":"rust","owner":"monitor health","task":27,"source_sha256":"eb9871b8a10bab460788986279bf9b2e020732fccfd98155a8dbec3495cef6c7","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-health.timer"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert-reminder.service","source":"deploy/monitor/verdaxis-monitor-alert-reminder.service","kind":"unit","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"4b895eef3fb1b9905c958b18af51ae29497035db7171624490645f1433cfc6bf","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert-reminder.service"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert-reminder.timer","source":"deploy/monitor/verdaxis-monitor-alert-reminder.timer","kind":"timer","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"3eb076b7f4ed7502b53e75505910d6892947a5b5cac7f23d3fd1e15be7250269","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert-reminder.timer"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert@.service","source":"deploy/monitor/verdaxis-monitor-alert@.service","kind":"unit","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"9525ef17cc8795bc62d0bf7e1d5e48d0d8df88562d7f0f99c8e5d86fd4098cd7","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert@.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-codex-diagnose.service","source":"deploy/external_monitor/systemd/verdaxis-codex-diagnose.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"a65c08d0dc1f96fa0f7e38c880ea6689fd2bbae8d4b6157b9df97a80e25eb195","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-codex-diagnose.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-monitor.service","source":"deploy/external_monitor/systemd/verdaxis-external-monitor.service","kind":"unit","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"a2a3afa0e48b424d48f4f62539c59ee2578b75e8cbf23c367d2533ed8c53c9e1","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-monitor.service","last_reference_change":"Task 29: no installed unit on either host and no implementation in source or on a host (4 Nov read-only check); retired, not ported; Task 32 removes the unit files"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-monitor.timer","source":"deploy/external_monitor/systemd/verdaxis-external-monitor.timer","kind":"timer","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"f31a806543711dc7a7193bfad0b491cf018f679a6645cea6a48baa374d981f8c","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-monitor.timer","last_reference_change":"Task 29: no installed unit on either host and no implementation in source or on a host (4 Nov read-only check); retired, not ported; Task 32 removes the unit files"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-recover.service","source":"deploy/external_monitor/systemd/verdaxis-external-recover.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"2e60fb30b1c3bbb9ec114904338a13e5ffcc7ec35c26af1f3fcf848d6cd80708","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-recover.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor-verify.service","source":"deploy/external_monitor/systemd/verdaxis-monitor-verify.service","kind":"unit","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"b6d9ba54185794d270ed65823abb6cfe72603244748662620b5f8fe12406a81d","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-verify.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor.service","source":"deploy/external_monitor/systemd/verdaxis-monitor.service","kind":"unit","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"bc17eb6fbaf7d7dff1eed88758145abb1ff7117c12f129b67160048b4d9ffbaa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor.timer","source":"deploy/external_monitor/systemd/verdaxis-monitor.timer","kind":"timer","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"038ca76afa2bba9a5630bd2300079c6036c693e7338f108e75910a368bebeffa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor.timer"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-onboarding-attention.service","source":"deploy/external_monitor/systemd/verdaxis-onboarding-attention.service","kind":"unit","end_state":"rust","owner":"monitor onboarding","task":16,"source_sha256":"aec4b5dbfe7153eb04bddc1646963357d8ee5559dee84aee5ad38231ceaeb370","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-onboarding-attention.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer","source":"deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer","kind":"timer","end_state":"rust","owner":"monitor onboarding","task":16,"source_sha256":"0cbcf48a1388366371ab398cc95cab58ea2545830cac17aa22d15d861b7b6925","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-onboarding-attention.timer"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-recover.service","source":"deploy/external_monitor/systemd/verdaxis-recover.service","kind":"unit","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"f83b70150b94bbd38cbbf70749ced983344574d6331db67b27864d7137a5fca2","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-recover.service","last_reference_change":"Decision 203: automatic recovery retired (masked on the shared host, absent on EU production); crash gaps closed by systemd restart drop-ins and the serve watchdog; Task 32 removes the files"}]}"####).unwrap();
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
        serde_json::from_str(r####"{"schema_version":1,"source_commit":"2e1ccad2dc1a996bec629dff4c892a32ac3da576","source_checkpoint":"sha256 of each source file; no branch-name identity","artifacts":[{"id":"unit.deploy/systemd/verdaxis-auth-maintenance-staging.service","source":"deploy/systemd/verdaxis-auth-maintenance-staging.service","kind":"unit","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"17fcaadaf5477c509826051d345d49f3ed3e7bf9efb94933faf47ffaacf3d418","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance-staging.service"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance-staging.timer","source":"deploy/systemd/verdaxis-auth-maintenance-staging.timer","kind":"timer","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"7290696b5cfb94e3d6a2b83fa6646e021948e6b13feb335304fb8a09508c2049","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance.service","source":"deploy/systemd/verdaxis-auth-maintenance.service","kind":"unit","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"31f2e5fd3840542a795ebee6e0bfd776b915e8ea742cad9dc3a571618cea586b","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance.service"},{"id":"unit.deploy/systemd/verdaxis-auth-maintenance.timer","source":"deploy/systemd/verdaxis-auth-maintenance.timer","kind":"timer","end_state":"rust","owner":"job auth-maintenance","task":12,"source_sha256":"a30a00e0f22c43ddf9a8e0f5fcfa379a3f68c176a758ab18647ee7eeaeddd014","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-auth-maintenance.timer"},{"id":"unit.deploy/systemd/verdaxis-backend-staging.service","source":"deploy/systemd/verdaxis-backend-staging.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"56f0fe79c9bd0363795209d34c73bb744e3fc1e38f16b84057745f41100820bb","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backend-staging.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-backend.service","source":"deploy/systemd/verdaxis-backend.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"99955437f20e0291faaacdb5ef4c446552aa091f3ab46d58579cbc91391dc9fa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backend.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-news-refresh-staging.service","source":"deploy/systemd/verdaxis-news-refresh-staging.service","kind":"unit","end_state":"rust","owner":"job news","task":11,"source_sha256":"e2f4456bd817a13adab3cb1095ddece50f2f12b5c0885cbc045ca2d833a2baf1","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh-staging.service"},{"id":"unit.deploy/systemd/verdaxis-news-refresh-staging.timer","source":"deploy/systemd/verdaxis-news-refresh-staging.timer","kind":"timer","end_state":"rust","owner":"job news","task":11,"source_sha256":"2d82ecf19866c655f47f74d0ec44f8ecab4e8b7dff28164555006e893b79200b","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-news-refresh.service","source":"deploy/systemd/verdaxis-news-refresh.service","kind":"unit","end_state":"rust","owner":"job news","task":11,"source_sha256":"3ff0d95162b70dceb42579d5586addd6271cef2b9d56ec230c67b25bae62aab9","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh.service"},{"id":"unit.deploy/systemd/verdaxis-news-refresh.timer","source":"deploy/systemd/verdaxis-news-refresh.timer","kind":"timer","end_state":"rust","owner":"job news","task":11,"source_sha256":"ee175cbcbd0bd59cc80a28d9217e0c500cd3cfaa171e00b0696e76fe0933a1c5","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-news-refresh.timer"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders-staging.service","source":"deploy/systemd/verdaxis-order-expiry-reminders-staging.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"cd8e9e01f2d7a3e80537020b1ebbab0dd33353530c56a15c9a3ef1ed1d0f0069","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders-staging.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders-staging.timer","source":"deploy/systemd/verdaxis-order-expiry-reminders-staging.timer","kind":"timer","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"1e32fd6d19d06ea6bcdae356d28f177e1529aace567b5a382c20b9247e618e12","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders-staging.timer","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders.service","source":"deploy/systemd/verdaxis-order-expiry-reminders.service","kind":"unit","end_state":"rust","owner":"job reminders","task":14,"source_sha256":"193916c8fe4b6fd1ca4013deaec3254f1f1ba00944da29e809405de0b941f410","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders.service"},{"id":"unit.deploy/systemd/verdaxis-order-expiry-reminders.timer","source":"deploy/systemd/verdaxis-order-expiry-reminders.timer","kind":"timer","end_state":"rust","owner":"job reminders","task":14,"source_sha256":"9826106c408bb94af06cf2804f36b60e5f14187b33b0af742c0eadc4d1c54868","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-order-expiry-reminders.timer"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune-staging.service","source":"deploy/systemd/verdaxis-product-analytics-prune-staging.service","kind":"unit","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"849edc45c1ec04e441a6f05667ea7941ead4a056bdb3fd054f481942a6ab9c19","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune-staging.service"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune-staging.timer","source":"deploy/systemd/verdaxis-product-analytics-prune-staging.timer","kind":"timer","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"f6eea37b9acc3736221cec11eda934c85d43b09a092edc58b00ae8502023f317","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune-staging.timer"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune.service","source":"deploy/systemd/verdaxis-product-analytics-prune.service","kind":"unit","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"10cccc3706e01dc26def1aa6d24e5cf0b2f2af8f76105319082d03a9775865cc","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune.service"},{"id":"unit.deploy/systemd/verdaxis-product-analytics-prune.timer","source":"deploy/systemd/verdaxis-product-analytics-prune.timer","kind":"timer","end_state":"rust","owner":"job prune-analytics","task":13,"source_sha256":"1765328b7ef948909b3b3fb2975cefdf63cd62e546ee40740bc463b2d2f5f5e3","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-product-analytics-prune.timer"},{"id":"unit.deploy/systemd/verdaxis-rust-staging.service","source":"deploy/systemd/verdaxis-rust-staging.service","kind":"unit","end_state":"rust","owner":"serve","task":10,"source_sha256":"73577839a5b0c3e476783cfea7e5252155494e28dd33e8f507b152e43b743ec5","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-rust-staging.service"},{"id":"unit.deploy/systemd/verdaxis-rust.service","source":"deploy/systemd/verdaxis-rust.service","kind":"unit","end_state":"rust","owner":"serve","task":10,"source_sha256":"bb00aa497ad497cab705f0e8209d54d1bfa5c179462c576b45c7b974457d76d6","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-rust.service"},{"id":"unit.deploy/monitor/verdaxis-backup-verify.service","source":"deploy/monitor/verdaxis-backup-verify.service","kind":"unit","end_state":"rust","owner":"monitor backup-verify","task":27,"source_sha256":"89f009dc127b15e29ae94d02718ee35442f625e67b9d9ff3fae07334cc62474e","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backup-verify.service"},{"id":"unit.deploy/monitor/verdaxis-backup-verify.timer","source":"deploy/monitor/verdaxis-backup-verify.timer","kind":"timer","end_state":"rust","owner":"monitor backup-verify","task":27,"source_sha256":"8fb9fa6da3b8502a9fee577a65438c442f2fdfe5f69fcc3f609d10003db4909c","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-backup-verify.timer"},{"id":"unit.deploy/monitor/verdaxis-demo-activity-staging.service","source":"deploy/monitor/verdaxis-demo-activity-staging.service","kind":"unit","end_state":"rust","owner":"job demo","task":17,"source_sha256":"c4791cc13c91ffcfcd68ce4a81715244a17c09949796e911cdd8934dd84e5c6a","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity-staging.service"},{"id":"unit.deploy/monitor/verdaxis-demo-activity-staging.timer","source":"deploy/monitor/verdaxis-demo-activity-staging.timer","kind":"timer","end_state":"rust","owner":"job demo","task":17,"source_sha256":"4e25897e15afb5fc896c309d9aaa4f629f2a91205a30df63c544174eb3a8184e","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity-staging.timer"},{"id":"unit.deploy/monitor/verdaxis-demo-activity.service","source":"deploy/monitor/verdaxis-demo-activity.service","kind":"unit","end_state":"rust","owner":"job demo","task":17,"source_sha256":"42e2ed46153da9003ec2123eac49e62179d0628ea62039de773fea04629ff0d2","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity.service"},{"id":"unit.deploy/monitor/verdaxis-demo-activity.timer","source":"deploy/monitor/verdaxis-demo-activity.timer","kind":"timer","end_state":"rust","owner":"job demo","task":17,"source_sha256":"5f3fefba55a4c1a6760b2de4ab14bd7595fefc80fab0d5f700a04127afa77dfc","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-demo-activity.timer"},{"id":"unit.deploy/monitor/verdaxis-health.service","source":"deploy/monitor/verdaxis-health.service","kind":"unit","end_state":"rust","owner":"monitor health","task":27,"source_sha256":"4bb7b661acf541aec94db8ea4b4b193bf9be1070d4bcd3da9873ec942bb62986","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-health.service"},{"id":"unit.deploy/monitor/verdaxis-health.timer","source":"deploy/monitor/verdaxis-health.timer","kind":"timer","end_state":"rust","owner":"monitor health","task":27,"source_sha256":"eb9871b8a10bab460788986279bf9b2e020732fccfd98155a8dbec3495cef6c7","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-health.timer"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert-reminder.service","source":"deploy/monitor/verdaxis-monitor-alert-reminder.service","kind":"unit","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"4b895eef3fb1b9905c958b18af51ae29497035db7171624490645f1433cfc6bf","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert-reminder.service"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert-reminder.timer","source":"deploy/monitor/verdaxis-monitor-alert-reminder.timer","kind":"timer","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"3eb076b7f4ed7502b53e75505910d6892947a5b5cac7f23d3fd1e15be7250269","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert-reminder.timer"},{"id":"unit.deploy/monitor/verdaxis-monitor-alert@.service","source":"deploy/monitor/verdaxis-monitor-alert@.service","kind":"unit","end_state":"rust","owner":"monitor alert","task":28,"source_sha256":"9525ef17cc8795bc62d0bf7e1d5e48d0d8df88562d7f0f99c8e5d86fd4098cd7","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-alert@.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-codex-diagnose.service","source":"deploy/external_monitor/systemd/verdaxis-codex-diagnose.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"a65c08d0dc1f96fa0f7e38c880ea6689fd2bbae8d4b6157b9df97a80e25eb195","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-codex-diagnose.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-monitor.service","source":"deploy/external_monitor/systemd/verdaxis-external-monitor.service","kind":"unit","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"a2a3afa0e48b424d48f4f62539c59ee2578b75e8cbf23c367d2533ed8c53c9e1","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-monitor.service","last_reference_change":"Task 29: no installed unit on either host and no implementation in source or on a host (4 Nov read-only check); retired, not ported; Task 32 removes the unit files"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-monitor.timer","source":"deploy/external_monitor/systemd/verdaxis-external-monitor.timer","kind":"timer","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"f31a806543711dc7a7193bfad0b491cf018f679a6645cea6a48baa374d981f8c","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-monitor.timer","last_reference_change":"Task 29: no installed unit on either host and no implementation in source or on a host (4 Nov read-only check); retired, not ported; Task 32 removes the unit files"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-external-recover.service","source":"deploy/external_monitor/systemd/verdaxis-external-recover.service","kind":"unit","end_state":"gone","owner":"Task 32 removal owner","task":32,"source_sha256":"2e60fb30b1c3bbb9ec114904338a13e5ffcc7ec35c26af1f3fcf848d6cd80708","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-external-recover.service","last_reference_change":"Task 32 removes Python units, upstreams and runtime references"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor-verify.service","source":"deploy/external_monitor/systemd/verdaxis-monitor-verify.service","kind":"unit","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"b6d9ba54185794d270ed65823abb6cfe72603244748662620b5f8fe12406a81d","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor-verify.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor.service","source":"deploy/external_monitor/systemd/verdaxis-monitor.service","kind":"unit","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"bc17eb6fbaf7d7dff1eed88758145abb1ff7117c12f129b67160048b4d9ffbaa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-monitor.timer","source":"deploy/external_monitor/systemd/verdaxis-monitor.timer","kind":"timer","end_state":"rust","owner":"monitor external","task":29,"source_sha256":"038ca76afa2bba9a5630bd2300079c6036c693e7338f108e75910a368bebeffa","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-monitor.timer"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-onboarding-attention.service","source":"deploy/external_monitor/systemd/verdaxis-onboarding-attention.service","kind":"unit","end_state":"rust","owner":"monitor onboarding","task":16,"source_sha256":"aec4b5dbfe7153eb04bddc1646963357d8ee5559dee84aee5ad38231ceaeb370","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-onboarding-attention.service"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer","source":"deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer","kind":"timer","end_state":"rust","owner":"monitor onboarding","task":16,"source_sha256":"0cbcf48a1388366371ab398cc95cab58ea2545830cac17aa22d15d861b7b6925","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":null,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-onboarding-attention.timer"},{"id":"unit.deploy/external_monitor/systemd/verdaxis-recover.service","source":"deploy/external_monitor/systemd/verdaxis-recover.service","kind":"unit","end_state":"gone","owner":"Task 32 removal (decision 203 / Task 29 retirement)","task":32,"source_sha256":"f83b70150b94bbd38cbbf70749ced983344574d6331db67b27864d7137a5fca2","status":"planned","evidence":["test_phase10_units_are_exactly_41","no_unit_contains_python_or_a_venv_path"],"deleting_task":32,"rollback":"Restore the prior route or unit command before Task 32; no route rollback after deletion.","installed_check":"systemctl cat verdaxis-recover.service","last_reference_change":"Decision 203: automatic recovery retired (masked on the shared host, absent on EU production); crash gaps closed by systemd restart drop-ins and the serve watchdog; Task 32 removes the files"}]}"####).unwrap();
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
        // A persistent timer starts its service once for all missed elapses.
        // A restart may follow only a failed run, so the catch-up still ends
        // with one successful run; the retries must be bounded.
        match directives(&service, "Restart").as_slice() {
            [] => {}
            ["on-failure"] => {
                assert_eq!(directives(&service, "StartLimitBurst"), ["5"], "{source}");
                assert_eq!(
                    directives(&service, "StartLimitIntervalSec"),
                    ["15min"],
                    "{source}"
                );
                assert_eq!(
                    directives(&service, "RestartPreventExitStatus"),
                    ["2 3"],
                    "{source}"
                );
            }
            other => panic!("{source}: unbounded or unexpected Restart={other:?}"),
        }
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
