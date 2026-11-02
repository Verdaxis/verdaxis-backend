use std::ffi::OsString;
use std::process::{Command as Process, Output};
use verdaxis_read::cli::{Command, Exit, parse_arguments};

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn binary(values: &[&str]) -> Output {
    Process::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .args(values)
        .env("RUST_CONFIG", "not-json")
        .env("DATABASE_URL", "not-a-database")
        .output()
        .expect("start disposable command")
}

#[test]
fn legacy_bind_form_still_serves() {
    let legacy = parse_arguments(args(&[
        "--bind",
        "127.0.0.1:0",
        "--budget-file",
        "/tmp/budget.json",
    ]));
    let explicit = parse_arguments(args(&[
        "serve",
        "--bind",
        "127.0.0.1:0",
        "--budget-file",
        "/tmp/budget.json",
    ]));
    assert!(
        matches!(legacy, Ok(Command::Serve(_))),
        "legacy form must select serve"
    );
    assert_eq!(legacy.unwrap(), explicit.unwrap());
    assert!(matches!(parse_arguments(vec![]), Ok(Command::Serve(_))));
    assert_eq!(binary(&["serve"]).status.code(), Some(4));
}

#[test]
fn subcommand_dispatch_is_exact() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let identity = ["--environment", "staging", "--release-sha", sha];
    for prefix in [
        vec!["migrate", "status"],
        vec!["acl", "converge", "--env-file", "/tmp/job.env"],
        vec!["job", "news", "--apply"],
        vec!["job", "auth-maintenance", "--apply"],
        vec!["job", "prune-analytics", "--apply"],
        vec!["job", "kyc-notices", "--apply"],
        vec!["job", "demo", "--apply"],
        vec!["monitor", "external"],
        vec!["monitor", "external-checks"],
        vec!["recover", "--dry-run"],
        vec!["ops", "preflight"],
        vec!["ops", "prepare-budget"],
        vec!["ops", "prepare-runtime"],
        vec!["ops", "inspect", "users"],
        vec!["ops", "inspect", "blockers"],
        vec!["ops", "ingest", "signals", "--file", "/tmp/input.csv"],
        vec!["ops", "import", "gena", "--file", "/tmp/input.csv"],
        vec!["ops", "cache-authority", "status"],
        vec!["ops", "fleet"],
        vec![
            "canary",
            "cleanup",
            "--email",
            "synthetic@example.invalid",
            "--apply",
        ],
    ] {
        let mut values = prefix.clone();
        values.extend(identity);
        assert!(
            parse_arguments(args(&values)).is_ok(),
            "valid command: {prefix:?}"
        );
        // A parsed future command must refuse; it must not fall through to serving
        // or claim to have performed a job before that command's task lands.
        assert_eq!(
            binary(&values).status.code(),
            Some(3),
            "fail closed: {prefix:?}"
        );
    }
    // The onboarding monitor is production-only: another environment or a
    // missing identity is a usage refusal before any pool; production fails
    // closed until the deployed identity and context exist.
    for (extra, code) in [
        (vec![], 2),
        (vec!["--environment", "staging", "--release-sha", sha], 2),
        (vec!["--environment", "production", "--release-sha", sha], 3),
    ] {
        let mut values = vec!["monitor", "onboarding"];
        values.extend(extra);
        assert_eq!(binary(&values).status.code(), Some(code), "{values:?}");
    }
    // Flags that do not belong to a monitor kind are refused, not ignored.
    let production = ["--environment", "production", "--release-sha", sha];
    for flags in [
        vec!["--dry-run", "--bootstrap"],
        vec!["--state-file", "relative.json"],
        vec!["--show-emails"],
        vec!["--event", "health"],
        vec!["--remind", "all"],
    ] {
        let mut values = vec!["monitor", "onboarding"];
        values.extend(production);
        values.extend(flags);
        assert_eq!(binary(&values).status.code(), Some(2), "{values:?}");
    }
    for kind in ["health", "backup-verify", "outbox"] {
        for flag in [
            vec!["--dry-run"],
            vec!["--bootstrap"],
            vec!["--show-emails"],
            vec!["--state-file", "/tmp/state.json"],
        ] {
            let mut values = vec!["monitor", kind];
            values.extend(flag);
            assert_eq!(binary(&values).status.code(), Some(2), "{values:?}");
        }
    }
    for values in [
        vec!["monitor", "health"],
        vec!["monitor", "backup-verify"],
        vec!["monitor", "alert", "--remind", "all"],
        vec!["monitor", "alert", "--remind", "backup"],
        vec!["monitor", "alert", "--event", "health"],
        vec!["monitor", "alert", "--event", "backup-failure"],
        vec![
            "monitor",
            "alert",
            "--reconcile",
            "health:7:telegram:delivered",
        ],
        vec!["monitor", "alert", "--rearm", "backup:7"],
        vec!["monitor", "alert", "--release", "health"],
        vec![
            "monitor",
            "alert",
            "--repair",
            "health:7",
            "--reason",
            "entry removed",
        ],
        vec![
            "monitor",
            "outbox",
            "--dsn",
            "dbname=x",
            "--max-pending",
            "-1",
        ],
        vec!["guard", "health"],
        vec!["guard", "rust-start"],
        vec!["guard", "deploy-start"],
    ] {
        assert!(
            parse_arguments(args(&values)).is_ok(),
            "valid local command: {values:?}"
        );
    }
    // The reminder job runs in production only: staging is refused before any
    // pool (exit 2); production waits for its deployed context (exit 3).
    for (environment, code) in [("staging", 2), ("production", 3)] {
        let values = [
            "job",
            "reminders",
            "--apply",
            "--environment",
            environment,
            "--release-sha",
            sha,
        ];
        assert_eq!(
            binary(&values).status.code(),
            Some(code),
            "reminders: {environment}"
        );
    }
    for values in [
        vec!["unknown"],
        vec!["job"],
        vec!["job", "unknown"],
        vec!["job", "news"],
        vec!["ops", "inspect", "unknown"],
        vec!["monitor", "alert", "--event", "health", "--remind", "all"],
        // The alert and outbox monitors take no identity and only their own flags.
        vec![
            "monitor",
            "alert",
            "--event",
            "health",
            "--environment",
            "staging",
            "--release-sha",
            sha,
        ],
        vec!["monitor", "alert", "--event", "health-unknown"],
        vec![
            "monitor", "alert", "--remind", "health", "--dsn", "dbname=x",
        ],
        vec!["monitor", "alert"],
        vec![
            "monitor",
            "outbox",
            "--environment",
            "staging",
            "--release-sha",
            sha,
        ],
        vec!["monitor", "outbox", "--dsn", "dbname=x", "--remind", "all"],
        vec!["monitor", "onboarding", "--dsn", "dbname=x"],
        vec!["monitor", "health", "--rearm", "health:1"],
        vec!["monitor", "alert", "--repair", "health:7"],
        vec!["monitor", "alert", "--event", "health", "--reason", "x"],
        vec!["monitor", "alert", "--repair", "health:7", "--reason", " "],
        vec!["guard", "health", "--hidden"],
        vec!["job", "news", "--environment", "test", "--release-sha", sha],
        vec![
            "job",
            "news",
            "--environment",
            "staging",
            "--release-sha",
            "short",
        ],
    ] {
        assert_eq!(
            binary(&values).status.code(),
            Some(2),
            "usage refusal: {values:?}"
        );
    }
    let output = binary(&[
        "canary",
        "cleanup",
        "--email",
        "private@example.invalid",
        "--environment",
        "staging",
        "--release-sha",
        "secret",
    ]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private@example.invalid"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
}

#[test]
fn version_json_is_still_first() {
    let output = binary(&["--version-json", "--unknown"]);
    assert!(output.status.success());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["binary"], "verdaxis-read");
    assert!(document["release_sha"].is_string());
    assert!(output.stderr.is_empty());
}

#[test]
fn every_command_has_help_and_no_hidden_flag() {
    for command in [
        "serve", "migrate", "acl", "job", "monitor", "recover", "ops", "guard", "canary",
    ] {
        let output = binary(&[command, "--help"]);
        assert!(
            output.status.success(),
            "{command} help must not load config"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        assert_eq!(binary(&[command, "--hidden"]).status.code(), Some(2));
    }
    assert_eq!(Exit::Ok as i32, 0);
    assert_eq!(Exit::Usage as i32, 2);
    assert_eq!(Exit::Precondition as i32, 3);
    assert_eq!(Exit::Dependency as i32, 4);
    assert_eq!(Exit::Ambiguous as i32, 5);
}

#[cfg(unix)]
#[test]
fn dispatch_never_panics_on_bad_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = OsString::from_vec(vec![0xff]);
    let output = Process::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .arg(&invalid)
        .env("RUST_CONFIG", "not-json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(parse_arguments(vec![invalid.clone()]), Err(Exit::Usage));
    let output = Process::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .arg("--version-json")
        .arg(invalid)
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn local_monitors_refuse_flags_they_do_not_take() {
    // The refusal happens before the monitor reads or writes anything.
    for values in [
        vec!["monitor", "health", "--event", "health"],
        vec!["monitor", "health", "--remind", "all"],
        vec!["monitor", "health", "--telegram-disabled"],
        vec!["monitor", "backup-verify", "--event", "backup"],
        vec!["monitor", "backup-verify", "--telegram-disabled"],
        vec![
            "monitor",
            "health",
            "--environment",
            "staging",
            "--release-sha",
            "0123456789abcdef0123456789abcdef01234567",
        ],
        vec![
            "monitor",
            "backup-verify",
            "--environment",
            "production",
            "--release-sha",
            "0123456789abcdef0123456789abcdef01234567",
        ],
    ] {
        assert!(parse_arguments(args(&values)).is_ok(), "parses: {values:?}");
        assert_eq!(
            binary(&values).status.code(),
            Some(2),
            "usage refusal: {values:?}"
        );
    }
}

#[test]
fn local_monitors_run_on_the_calling_thread() {
    // A blocking-pool thread can wait forever when a unit limits its tasks, so
    // the dispatch must not move a local monitor onto the blocking pool.
    let source = include_str!("../src/cli/mod.rs");
    assert!(!source.contains("spawn_blocking"));
    assert!(source.contains("fn run_local_monitor(args: &MonitorArgs, run: fn() -> Exit) -> Exit"));
}
