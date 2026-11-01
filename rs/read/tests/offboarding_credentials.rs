//! Offline checks of the account credential cutoff primitive.
//!
//! The Python half runs the project's interpreter (`RUST_TEST_PYTHON`, default
//! `python3`) on the unchanged Python password verifier.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use verdaxis_read::auth::{
    offboarding::{CUTOFF_SKEW_MARGIN_SECONDS, unusable_password_hash},
    password,
};

const HARNESS: &str = "rs/read/tests/fixtures/offboarding_python_harness.py";
const RESULT_PREFIX: &str = "OFFBOARDING_RESULT ";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn python_password_check(hash: &str, candidates: &[&str]) -> Value {
    let root = repository_root();
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut child = Command::new(python)
        .current_dir(&root)
        .env("PYTHONPATH", &root)
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env(
            "JWT_SECRET",
            "synthetic-offboarding-key-at-least-32-characters",
        )
        .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
        .arg(root.join(HARNESS))
        .arg("password")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Python interpreter unavailable: set RUST_TEST_PYTHON or put python3 on PATH");
    let request = json!({"hash": hash, "passwords": candidates});
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(request.to_string().as_bytes())
        .expect("write request");
    let output = child.wait_with_output().expect("Python check finished");
    assert!(
        output.status.success(),
        "Python password check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix(RESULT_PREFIX))
        .expect("one result line");
    serde_json::from_str(line).expect("JSON result")
}

#[test]
fn skew_margin_is_a_few_seconds() {
    assert!(
        (1..=10).contains(&CUTOFF_SKEW_MARGIN_SECONDS),
        "the documented margin covers host clock differences without a long lockout window"
    );
}

#[tokio::test]
async fn unusable_hash_rejected_by_both_stacks() {
    let first = unusable_password_hash().expect("hash");
    let second = unusable_password_hash().expect("hash");
    assert_ne!(first, second, "every account gets its own unusable hash");
    for hash in [&first, &second] {
        assert!(
            hash.starts_with(&format!("$2b${}$", password::BCRYPT_COST)),
            "a standard bcrypt hash both stacks parse, at the real password cost"
        );
        assert_eq!(hash.len(), 60);
    }
    let long = "x".repeat(200);
    let candidates = [
        "",
        "password123",
        "SyntheticPassword123",
        first.as_str(),
        second.as_str(),
        long.as_str(),
    ];
    for candidate in candidates {
        assert!(
            !password::verify(candidate, &first).await,
            "the Rust verifier must reject every presented password"
        );
    }
    let python = python_password_check(&first, &candidates);
    for candidate in candidates {
        assert_eq!(
            python["passwords"][candidate], "rejected",
            "the Python verifier must parse the hash and reject {candidate:?}"
        );
    }
}
