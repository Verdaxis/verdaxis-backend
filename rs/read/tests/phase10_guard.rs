//! Deploy-start and start-guard corpus.
//!
//! The deploy-start guard is the shell helper that runs before the binary
//! exists, so these cases execute that file directly. The start guard and the
//! readiness guard are compiled from `cli/guard.rs` until the integration owner
//! registers the module in `cli/mod.rs`.

#[path = "../src/cli/guard.rs"]
mod guard;

use std::fs;
use std::io::Cursor;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SHA_A: &str = "1111111111111111111111111111111111111111";
const SHA_B: &str = "2222222222222222222222222222222222222222";
const DENIAL: &str = "Backend start denied by deployment guard";

fn denial(reason: &str) -> String {
    format!("{DENIAL}: {reason}\n")
}

struct Fixture {
    root: PathBuf,
    holders: Vec<Child>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "verdaxis-guard-{name}-{}-{stamp}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("scripts")).expect("fixture scripts");
        let root = root.canonicalize().expect("fixture root");
        fs::copy(
            guard_source(),
            root.join("scripts/verify-deploy-start-guard.sh"),
        )
        .expect("fixture guard");
        Self {
            root,
            holders: Vec::new(),
        }
    }

    fn guard(&self) -> PathBuf {
        self.root.join("scripts/verify-deploy-start-guard.sh")
    }

    fn deploy_dir(&self) -> PathBuf {
        self.root.join(".runtime-deploy")
    }

    fn state(&self, environment: &str) -> PathBuf {
        self.deploy_dir().join(format!("{environment}.state"))
    }

    fn lock(&self, environment: &str) -> PathBuf {
        self.deploy_dir().join(format!("{environment}.lock"))
    }

    fn restart_lock(&self, environment: &str) -> PathBuf {
        self.deploy_dir()
            .join(format!("{environment}.restart.lock"))
    }

    fn write_state(&self, environment: &str, sha: &str) {
        self.write_phase(environment, sha, "restart-authorized");
    }

    fn write_phase(&self, environment: &str, sha: &str, phase: &str) {
        fs::create_dir_all(self.deploy_dir()).expect("deploy directory");
        fs::set_permissions(self.deploy_dir(), fs::Permissions::from_mode(0o755))
            .expect("deploy directory mode");
        fs::write(
            self.state(environment),
            format!(
                "DEPLOYMENT_STATE={phase}\nENVIRONMENT={environment}\nRELEASE_SHA={sha}\n"
            ),
        )
        .expect("state");
        set_mode(&self.state(environment), 0o644);
    }

    fn create_lock(&self, path: &Path) {
        fs::create_dir_all(self.deploy_dir()).expect("deploy directory");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .expect("lock file");
    }

    fn hold(&mut self, path: &Path) {
        self.create_lock(path);
        // `-o` closes the descriptor before the command runs, so killing the
        // holder releases the lock even though its sleep child keeps running.
        let child = Command::new("flock")
            .args(["-o", path.to_str().expect("lock path")])
            .args(["sleep", "60"])
            .spawn()
            .expect("hold lock");
        self.holders.push(child);
        wait_until_busy(path);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for holder in &mut self.holders {
            let _ = holder.kill();
            let _ = holder.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode");
}

fn write_shim(directory: &Path, name: &str, body: &str) {
    fs::create_dir_all(directory).expect("shim directory");
    let path = directory.join(name);
    fs::write(&path, body).expect("shim");
    set_mode(&path, 0o755);
}

/// Reports a different inode for the state path, simulating a swap between the
/// open and the read. Bumping the path identity differs from the descriptor
/// identity on every platform.
const STAT_SHIM: &str = r#"#!/usr/bin/env bash
real=${REAL_STAT:?}
for argument in "$@"; do
    case $argument in
        *.state)
            output=$("$real" "$@") || exit $?
            printf '%s\n' "$output" | awk -F: 'BEGIN { OFS = ":" } { $2 = $2 + 1; print }'
            exit 0
            ;;
    esac
done
exec "$real" "$@"
"#;

/// Fails the way flock does on a kernel error, never with its conflict code.
const FLOCK_SHIM: &str = "#!/usr/bin/env bash\nexit 1\n";

fn guard_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/scripts/verify-deploy-start-guard.sh")
}

fn run_guard(fixture: &Fixture, arguments: &[&str]) -> Output {
    run_guard_with_path(fixture, None, arguments)
}

/// Run the guard with an optional PATH prefix that supplies a shim.
fn run_guard_with_path(fixture: &Fixture, shim: Option<&Path>, arguments: &[&str]) -> Output {
    let base = std::env::var("PATH").unwrap_or_default();
    let path = match shim {
        Some(directory) => format!("{}:{base}", directory.display()),
        None => base,
    };
    Command::new("bash")
        .arg(fixture.guard())
        .args(arguments)
        .env_clear()
        .env("PATH", path)
        .env("REAL_STAT", real_stat())
        .output()
        .expect("run deploy-start guard")
}

/// The first `stat` on PATH, which the shims delegate to.
fn real_stat() -> PathBuf {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join("stat");
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("stat is not on PATH");
}

fn run_for(fixture: &Fixture, environment: &str, sha: &str) -> Output {
    run_guard(
        fixture,
        &["--environment", environment, "--expected-release-sha", sha],
    )
}

fn assert_allowed(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label}: guard denied a permitted start: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{label}: stdout must stay empty");
    assert!(output.stderr.is_empty(), "{label}: stderr must stay empty");
}

fn assert_denied(output: &Output, label: &str, reason: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{label}: guard must deny with exit 1"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        denial(reason),
        "{label}: denial reason"
    );
    assert!(output.stdout.is_empty(), "{label}: denial wrote to stdout");
}

fn wait_until_busy(path: &Path) {
    for _ in 0..200 {
        let status = Command::new("flock")
            .arg("-n")
            .arg(path)
            .arg("true")
            .status()
            .expect("probe lock");
        if !status.success() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("lock holder did not acquire {}", path.display());
}

#[test]
fn deploy_guard_allows_absent_state() {
    let fixture = Fixture::new("absent");
    assert_allowed(&run_for(&fixture, "staging", SHA_A), "no state directory");

    fs::create_dir_all(fixture.deploy_dir()).expect("deploy directory");
    fs::set_permissions(fixture.deploy_dir(), fs::Permissions::from_mode(0o755))
        .expect("deploy directory mode");
    assert_allowed(
        &run_for(&fixture, "production", SHA_B),
        "empty state directory",
    );

    // A helper outside the fixed scripts directory has no authority.
    let elsewhere = fixture.root.join("bin");
    fs::create_dir_all(&elsewhere).expect("bin");
    fs::copy(
        fixture.guard(),
        elsewhere.join("verify-deploy-start-guard.sh"),
    )
    .expect("copy");
    let output = Command::new("bash")
        .arg(elsewhere.join("verify-deploy-start-guard.sh"))
        .args(["--environment", "staging", "--expected-release-sha", SHA_A])
        .output()
        .expect("run misplaced guard");
    assert_denied(&output, "misplaced helper", "misplaced helper");

    for arguments in [
        vec![],
        vec!["--environment", "staging"],
        vec![
            "--environment",
            "staging",
            "--expected-release-sha",
            SHA_A,
            "extra",
        ],
        vec!["--expected-release-sha", SHA_A, "--environment", "staging"],
        vec!["--environment", "test", "--expected-release-sha", SHA_A],
        vec![
            "--environment",
            "staging",
            "--expected-release-sha",
            "short",
        ],
        vec![
            "--environment",
            "staging",
            "--expected-release-sha",
            "111111111111111111111111111111111111111A",
        ],
        vec![
            "--environment",
            "staging",
            "--expected-release-sha",
            SHA_A,
            "--apply",
        ],
    ] {
        assert_denied(
            &run_guard(&fixture, &arguments),
            "usage refusal",
            "invalid arguments",
        );
    }
}

#[test]
fn deploy_guard_requires_both_locks_for_restart_authorization() {
    let mut fixture = Fixture::new("locks");
    let environment = "staging";
    fixture.write_state(environment, SHA_A);
    let main = fixture.lock(environment);
    let restart = fixture.restart_lock(environment);

    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "stale authorization",
        "restart authorization is not held",
    );

    fixture.hold(&main);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "main lock only",
        "restart authorization is not held",
    );

    fixture.hold(&restart);
    assert_allowed(&run_for(&fixture, environment, SHA_A), "both locks held");

    // Release the main lock while the restart window lock stays busy.
    let mut main_holder = fixture.holders.remove(0);
    main_holder.kill().expect("stop main holder");
    main_holder.wait().expect("reap main holder");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "restart lock only",
        "restart authorization is not held",
    );

    for holder in &mut fixture.holders {
        holder.kill().expect("stop holder");
        holder.wait().expect("reap holder");
    }
    fixture.holders.clear();
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "no locks",
        "restart authorization is not held",
    );
}

#[test]
fn deploy_guard_rejects_dangling_state_and_wrong_sha() {
    let mut fixture = Fixture::new("state");
    let environment = "staging";
    fixture.write_state(environment, SHA_A);
    let main = fixture.lock(environment);
    let restart = fixture.restart_lock(environment);
    fixture.hold(&main);
    fixture.hold(&restart);
    let state = fixture.state(environment);
    assert_allowed(&run_for(&fixture, environment, SHA_A), "exact state");
    assert_denied(
        &run_for(&fixture, environment, SHA_B),
        "wrong release SHA",
        "deployment state does not authorize a restart",
    );

    let valid = fixture.root.join("valid.state");
    fs::rename(&state, &valid).expect("move state");
    std::os::unix::fs::symlink(fixture.root.join("missing.state"), &state).expect("dangling");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "dangling state",
        "deployment state is not a regular file",
    );
    fs::remove_file(&state).expect("remove symlink");
    std::os::unix::fs::symlink(&valid, &state).expect("state symlink");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "state symlink",
        "deployment state is not a regular file",
    );
    fs::remove_file(&state).expect("remove symlink");
    fs::rename(&valid, &state).expect("restore state");

    fs::write(
        &state,
        format!(
            "DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT={environment}\nRELEASE_SHA={SHA_A}\nEXTRA=denied\n"
        ),
    )
    .expect("extra line");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "extra state line",
        "deployment state does not authorize a restart",
    );

    fs::write(
        &state,
        format!(
            "DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT={environment}\nRELEASE_SHA={SHA_A}\n\n"
        ),
    )
    .expect("extra newline");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "trailing newline",
        "deployment state does not authorize a restart",
    );

    fs::write(&state, "x".repeat(257)).expect("oversize state");
    set_mode(&state, 0o644);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "oversize state",
        "deployment state is too large",
    );

    fixture.write_state(environment, SHA_A);
    set_mode(&state, 0o666);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "writable state",
        "deployment state is not owner-controlled",
    );

    fixture.write_state(environment, SHA_A);
    let state_link = fixture.root.join("state-link");
    fs::hard_link(&state, &state_link).expect("state hard link");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "hard-linked state",
        "deployment state is not owner-controlled",
    );
    fs::remove_file(&state_link).expect("remove state hard link");

    fixture.write_state(environment, SHA_A);
    fs::remove_file(&state).expect("remove state");
    fs::create_dir(&state).expect("state directory");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "state directory",
        "deployment state is not a regular file",
    );
    fs::remove_dir(&state).expect("remove state directory");

    fixture.write_state(environment, SHA_A);
    fs::remove_file(&main).expect("remove main lock");
    fs::create_dir(&main).expect("lock directory");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "lock directory",
        "main lock is not a regular file",
    );
    fs::remove_dir(&main).expect("remove lock directory");
    fixture.create_lock(&main);
    set_mode(&main, 0o666);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "writable lock",
        "main lock is not owner-controlled",
    );
    set_mode(&main, 0o600);
    // Hold the main lock so these cases reach the restart-lock checks.
    fixture.hold(&main);
    fs::remove_file(&restart).expect("remove restart lock");
    fs::create_dir(&restart).expect("restart lock directory");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "replaced restart lock",
        "restart lock is not a regular file",
    );
    fs::remove_dir(&restart).expect("remove restart lock directory");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "missing restart lock",
        "restart lock is not a regular file",
    );

    // A linked or writable deploy directory is not deploy authority.
    let real_dir = fixture.root.join("real-deploy");
    fs::rename(fixture.deploy_dir(), &real_dir).expect("move deploy directory");
    std::os::unix::fs::symlink(&real_dir, fixture.deploy_dir()).expect("deploy symlink");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "linked deploy directory",
        "deploy state is not a private directory",
    );
    fs::remove_file(fixture.deploy_dir()).expect("remove deploy symlink");
    fs::rename(&real_dir, fixture.deploy_dir()).expect("restore deploy directory");
    fs::set_permissions(fixture.deploy_dir(), fs::Permissions::from_mode(0o777))
        .expect("writable deploy directory");
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "writable deploy directory",
        "deploy state is not a private directory",
    );
}

#[test]
fn deploy_guard_denies_unsearchable_state_and_any_other_phase() {
    let mut fixture = Fixture::new("unsearchable");
    let environment = "staging";
    fixture.write_state(environment, SHA_A);
    let main = fixture.lock(environment);
    let restart = fixture.restart_lock(environment);
    fixture.hold(&main);
    fixture.hold(&restart);
    assert_allowed(
        &run_for(&fixture, environment, SHA_A),
        "private searchable state",
    );

    for phase in [
        "deploying",
        "failed",
        "blocked",
        "rust-failed",
        "readiness-pending",
        "unknown",
    ] {
        fixture.write_phase(environment, SHA_A, phase);
        assert_denied(
            &run_for(&fixture, environment, SHA_A),
            phase,
            "deployment state does not authorize a restart",
        );
    }
    fixture.write_state(environment, SHA_A);

    // An unsearchable directory must deny, never read as an absent state.
    set_mode(&fixture.deploy_dir(), 0o000);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "unsearchable deploy directory",
        "deploy state is unsearchable",
    );
    set_mode(&fixture.deploy_dir(), 0o755);
    assert_allowed(
        &run_for(&fixture, environment, SHA_A),
        "restored deploy directory",
    );
    set_mode(&fixture.deploy_dir(), 0o644);
    assert_denied(
        &run_for(&fixture, environment, SHA_A),
        "unreadable deploy directory",
        "deploy state is unsearchable",
    );
    set_mode(&fixture.deploy_dir(), 0o755);
}

#[test]
fn deploy_guard_denies_flock_and_descriptor_identity_errors() {
    let mut fixture = Fixture::new("shims");
    let environment = "staging";
    fixture.write_state(environment, SHA_A);
    let main = fixture.lock(environment);
    let restart = fixture.restart_lock(environment);
    fixture.hold(&main);
    fixture.hold(&restart);
    let arguments = [
        "--environment",
        environment,
        "--expected-release-sha",
        SHA_A,
    ];

    let stat_shim = fixture.root.join("stat-shim");
    write_shim(&stat_shim, "stat", STAT_SHIM);
    assert_denied(
        &run_guard_with_path(&fixture, Some(&stat_shim), &arguments),
        "descriptor identity mismatch",
        "deployment state changed during validation",
    );

    let flock_shim = fixture.root.join("flock-shim");
    write_shim(&flock_shim, "flock", FLOCK_SHIM);
    assert_denied(
        &run_guard_with_path(&fixture, Some(&flock_shim), &arguments),
        "flock error denies",
        "main lock check failed",
    );

    // Without the shims the same fixture still authorizes the restart.
    assert_allowed(&run_guard(&fixture, &arguments), "unshimmed restart");
}

#[test]
fn rust_start_requires_regular_budget_and_revision_files() {
    let fixture = Fixture::new("rust-start");
    let budget = fixture.root.join("budget.json");
    let revision = fixture.root.join("revision.json");
    let budget_text = format!("{{\"version\":1,\"release_sha\":\"{SHA_A}\",\"read_slots\":16}}");
    let revision_text = format!("{{\"release_sha\":\"{SHA_A}\",\"revision\":\"rev_20261014\"}}");
    fs::write(&budget, &budget_text).expect("budget");
    fs::write(&revision, &revision_text).expect("revision");
    set_mode(&budget, 0o600);
    set_mode(&revision, 0o600);

    assert_eq!(
        guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision),
        Ok(())
    );

    assert!(guard::rust_start(None, None, None, None).is_err());
    assert!(
        guard::rust_start(Some(SHA_A), Some(&budget), None, Some(&revision)).is_err(),
        "revision literal is required"
    );
    assert_eq!(
        guard::rust_start(
            Some(SHA_A),
            Some(&budget),
            Some("rev_20261014"),
            Some(&revision)
        ),
        Ok(())
    );

    assert!(guard::check_rust_start(SHA_B, &budget, "rev_20261014", &revision).is_err());
    assert!(guard::check_rust_start(SHA_A, &budget, "rev_other", &revision).is_err());
    assert!(guard::check_rust_start("short", &budget, "rev_20261014", &revision).is_err());
    assert!(guard::check_rust_start(SHA_A, &budget, "", &revision).is_err());

    set_mode(&budget, 0o640);
    assert!(guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err());
    set_mode(&budget, 0o600);

    set_mode(&revision, 0o604);
    assert!(guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err());
    set_mode(&revision, 0o600);

    let linked = fixture.root.join("linked-budget.json");
    std::os::unix::fs::symlink(&budget, &linked).expect("budget symlink");
    assert!(guard::check_rust_start(SHA_A, &linked, "rev_20261014", &revision).is_err());

    let hard_link = fixture.root.join("hard-budget.json");
    fs::hard_link(&budget, &hard_link).expect("budget hard link");
    assert!(guard::check_rust_start(SHA_A, &hard_link, "rev_20261014", &revision).is_err());
    fs::remove_file(&hard_link).expect("remove hard link");

    assert!(guard::check_rust_start(SHA_A, &fixture.root, "rev_20261014", &revision).is_err());

    for (label, text) in [
        ("not JSON", "not-json".to_owned()),
        ("array", "[]".to_owned()),
        (
            "missing version",
            format!("{{\"release_sha\":\"{SHA_A}\"}}"),
        ),
        (
            "string version",
            format!("{{\"version\":\"1\",\"release_sha\":\"{SHA_A}\"}}"),
        ),
        (
            "wrong version",
            format!("{{\"version\":2,\"release_sha\":\"{SHA_A}\"}}"),
        ),
        (
            "wrong budget SHA",
            format!("{{\"version\":1,\"release_sha\":\"{SHA_B}\"}}"),
        ),
    ] {
        fs::write(&budget, text).expect("budget variant");
        set_mode(&budget, 0o600);
        assert!(
            guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err(),
            "budget: {label}"
        );
    }
    fs::write(&budget, budget_text).expect("restore budget");
    set_mode(&budget, 0o600);

    for (label, text) in [
        ("not JSON", "not-json".to_owned()),
        (
            "extra key",
            format!("{{\"release_sha\":\"{SHA_A}\",\"revision\":\"rev_20261014\",\"extra\":1}}"),
        ),
        (
            "wrong revision",
            format!("{{\"release_sha\":\"{SHA_A}\",\"revision\":\"rev_other\"}}"),
        ),
        (
            "wrong revision SHA",
            format!("{{\"release_sha\":\"{SHA_B}\",\"revision\":\"rev_20261014\"}}"),
        ),
        (
            "non-string revision",
            format!("{{\"release_sha\":\"{SHA_A}\",\"revision\":1}}"),
        ),
    ] {
        fs::write(&revision, text).expect("revision variant");
        set_mode(&revision, 0o600);
        assert!(
            guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err(),
            "revision: {label}"
        );
    }
    fs::write(&revision, revision_text).expect("restore revision");
    set_mode(&revision, 0o600);

    fs::write(&budget, " ".repeat(guard::MAX_BUDGET_BYTES as usize + 1)).expect("oversize budget");
    set_mode(&budget, 0o600);
    assert!(guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err());

    fs::remove_file(&budget).expect("remove budget");
    assert!(guard::check_rust_start(SHA_A, &budget, "rev_20261014", &revision).is_err());
}

#[test]
fn health_guard_accepts_exactly_four_keys() {
    let exact = format!(
        "{{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"
    );
    assert_eq!(
        guard::check_health(&mut Cursor::new(exact.as_bytes()), "staging", SHA_A),
        Ok(())
    );
    assert_eq!(
        guard::health(
            Some("staging"),
            Some(SHA_A),
            &mut Cursor::new(exact.as_bytes())
        ),
        Ok(())
    );
    assert!(guard::health(None, Some(SHA_A), &mut Cursor::new(exact.as_bytes())).is_err());
    assert!(guard::health(Some("staging"), None, &mut Cursor::new(exact.as_bytes())).is_err());
    let reordered = format!(
        "{{\"release_sha\":\"{SHA_A}\",\"db\":\"ok\",\"environment\":\"staging\",\"status\":\"ok\"}}"
    );
    assert_eq!(
        guard::check_health(&mut Cursor::new(reordered.as_bytes()), "staging", SHA_A),
        Ok(())
    );

    for (label, body) in [
        (
            "extra key",
            format!(
                "{{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\",\"build\":\"x\"}}"
            ),
        ),
        (
            "extra nested key",
            format!(
                "{{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\",\"detail\":{{}}}}"
            ),
        ),
        (
            "missing status",
            format!("{{\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"),
        ),
        (
            "missing db",
            format!(
                "{{\"status\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"
            ),
        ),
        (
            "missing environment",
            format!("{{\"status\":\"ok\",\"db\":\"ok\",\"release_sha\":\"{SHA_A}\"}}"),
        ),
        (
            "missing release",
            "{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"staging\"}".to_owned(),
        ),
        (
            "wrong status",
            format!(
                "{{\"status\":\"degraded\",\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"
            ),
        ),
        (
            "wrong db",
            format!(
                "{{\"status\":\"ok\",\"db\":\"down\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"
            ),
        ),
        (
            "wrong environment",
            format!(
                "{{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"production\",\"release_sha\":\"{SHA_A}\"}}"
            ),
        ),
        (
            "wrong release",
            format!(
                "{{\"status\":\"ok\",\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_B}\"}}"
            ),
        ),
        (
            "non-string status",
            format!(
                "{{\"status\":null,\"db\":\"ok\",\"environment\":\"staging\",\"release_sha\":\"{SHA_A}\"}}"
            ),
        ),
        ("array", "[]".to_owned()),
        ("string", "\"ok\"".to_owned()),
        ("not JSON", "status=ok".to_owned()),
        ("empty", String::new()),
    ] {
        assert!(
            guard::check_health(&mut Cursor::new(body.as_bytes()), "staging", SHA_A).is_err(),
            "health: {label}"
        );
    }

    let oversize = format!("{}{}", " ".repeat(64 * 1024 + 1), exact);
    assert!(
        guard::check_health(&mut Cursor::new(oversize.as_bytes()), "staging", SHA_A).is_err(),
        "health: oversize body"
    );

    assert!(guard::check_health(&mut Cursor::new(exact.as_bytes()), "test", SHA_A).is_err());
    assert!(guard::check_health(&mut Cursor::new(exact.as_bytes()), "staging", "short").is_err());
}

#[test]
fn guard_scripts_contain_no_python() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let reviewed = fs::read_to_string(root.join("deploy/tests/shellcheck-files.txt"))
        .expect("reviewed shell list");
    let forbidden = ["py", "thon"].concat();
    for relative in [
        "deploy/scripts/verify-deploy-start-guard.sh",
        "deploy/tests/guard-contract.sh",
    ] {
        let path = root.join(relative);
        let text = fs::read_to_string(&path).expect("guard script");
        assert!(
            !text.to_ascii_lowercase().contains(&forbidden),
            "{relative}: must not depend on an interpreter"
        );
        assert!(
            text.starts_with("#!/usr/bin/env bash\n"),
            "{relative}: bash shebang"
        );
        assert!(
            text.contains("set -euo pipefail"),
            "{relative}: strict mode"
        );
        assert!(
            reviewed.lines().any(|line| line == relative),
            "{relative}: listed for the reviewed shell gate"
        );
    }
    let shell = fs::read_to_string(root.join("deploy/scripts/verify-deploy-start-guard.sh"))
        .expect("shell guard");
    for flag in [
        "--environment",
        "--expected-release-sha",
        "restart-authorized",
    ] {
        assert!(shell.contains(flag), "shell guard ABI: {flag}");
    }
}
