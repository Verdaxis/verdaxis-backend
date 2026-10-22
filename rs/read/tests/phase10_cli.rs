use std::ffi::OsString;
use std::process::{Command as Process, Output};
use verdaxis_read::cli::{parse_arguments, Command, Exit};

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
        vec!["job", "reminders", "--apply"],
        vec!["job", "kyc-notices", "--apply"],
        vec!["job", "demo", "--apply"],
        vec!["monitor", "outbox"],
        vec!["monitor", "external"],
        vec!["monitor", "external-checks"],
        vec!["monitor", "onboarding"],
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
    for values in [
        vec!["monitor", "health"],
        vec!["monitor", "backup-verify"],
        vec!["monitor", "alert", "--remind", "all"],
        vec!["monitor", "alert", "--event", "health"],
        vec!["guard", "health"],
        vec!["guard", "rust-start"],
        vec!["guard", "deploy-start"],
    ] {
        assert!(
            parse_arguments(args(&values)).is_ok(),
            "valid local command: {values:?}"
        );
    }
    for values in [
        vec!["unknown"],
        vec!["job"],
        vec!["job", "unknown"],
        vec!["job", "news"],
        vec!["ops", "inspect", "unknown"],
        vec!["monitor", "alert", "--event", "health", "--remind", "all"],
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
