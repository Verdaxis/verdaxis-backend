//! Phase 10 Task 21: the offline gates of `ops ingest signals`.
//!
//! Every case runs the binary with an operator URL on a closed loopback port:
//! a file that passes every offline check reaches the connection and fails
//! there (exit 4), so exit 2 or 3 proves the refusal came before any database.
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const USAGE: i32 = 2;
const PRECONDITION: i32 = 3;
const DEPENDENCY: i32 = 4;
const OPERATOR_URL: &str = "postgres://verdaxis_operator_staging:pw@127.0.0.1:1/verdaxis_staging";

const INDICATIONS: &str = "source_event_id,market_product,delivery_point,availability_window,side,price_per_mt_usd,quantity_mt,observed_at";
const FAIR_BANDS: &str = "source_event_id,market_product,delivery_point,availability_window,low_price_per_mt_usd,mid_price_per_mt_usd,high_price_per_mt_usd,model_name,model_version,observed_at";
const STEMS: &str = "source_event_id,stem_uid,market_product,delivery_point,availability_window,status,quantity_mt,stem_start,stem_end,observed_at";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-ingest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `ops ingest signals` with the deployed identity and an operator URL.
fn ingest(environment: &str, family: &str, source: &str, file: &Path, url: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ENVIRONMENT", environment)
        .env("RELEASE_SHA", SHA)
        .env("OPERATOR_DATABASE_URL", url)
        .args([
            "ops", "ingest", "signals", "--family", family, "--source", source,
        ])
        .arg("--file")
        .arg(file)
        .args(["--environment", environment, "--release-sha", SHA])
        .output()
        .unwrap()
}

fn code(output: &Output) -> Option<i32> {
    output.status.code()
}

/// The header with its columns in reverse order (order does not matter).
fn reversed(header: &str) -> String {
    header.split(',').rev().collect::<Vec<_>>().join(",")
}

#[test]
fn signal_csv_headers_are_exact_per_family() {
    let scratch = Scratch::new();
    for (family, header) in [
        ("indications", INDICATIONS),
        ("fair_bands", FAIR_BANDS),
        ("stems", STEMS),
    ] {
        let exact = scratch.file(
            &format!("{family}-exact.csv"),
            &format!("{}\n", reversed(header)),
        );
        assert_eq!(
            code(&ingest(
                "staging",
                family,
                "desk-marks",
                &exact,
                OPERATOR_URL
            )),
            Some(DEPENDENCY),
            "{family}: the exact header reaches the connection"
        );
        let missing_column = header.rsplit_once(',').unwrap().0;
        let extra_column = format!("{header},comment");
        let other_family = if family == "indications" {
            STEMS
        } else {
            INDICATIONS
        };
        for (label, text) in [
            ("missing", missing_column),
            ("extra", extra_column.as_str()),
            ("other family", other_family),
            ("empty", ""),
        ] {
            let file = scratch.file(&format!("{family}-{label}.csv"), &format!("{text}\n"));
            let output = ingest("staging", family, "desk-marks", &file, OPERATOR_URL);
            assert_eq!(
                code(&output),
                Some(PRECONDITION),
                "{family}: {label} header"
            );
        }
    }
}

#[test]
fn signal_source_pattern_and_file_size_are_bounded() {
    let scratch = Scratch::new();
    let file = scratch.file("ok.csv", &format!("{INDICATIONS}\n"));
    for source in [
        "",
        "a",
        "Desk",
        "-desk",
        "_desk",
        "desk marks",
        "desk/marks",
        &"a".repeat(65),
    ] {
        assert_eq!(
            code(&ingest(
                "staging",
                "indications",
                source,
                &file,
                OPERATOR_URL
            )),
            Some(USAGE),
            "source {source:?}"
        );
    }
    for source in ["desk-marks", "broker_sheet_a", "0x", &"a".repeat(64)] {
        assert_eq!(
            code(&ingest(
                "staging",
                "indications",
                source,
                &file,
                OPERATOR_URL
            )),
            Some(DEPENDENCY),
            "source {source:?}"
        );
    }
    // Bigger than 4 MiB, a symlink, and a directory are all refused.
    let row = "e1,B30,Singapore,SPOT,BID,700.00,100,2026-11-01T00:00:00Z\n";
    let large = scratch.file(
        "large.csv",
        &format!(
            "{INDICATIONS}\n{}",
            row.repeat(4 * 1024 * 1024 / row.len() + 1)
        ),
    );
    let link = scratch.0.join("link.csv");
    symlink(&file, &link).unwrap();
    for (label, path) in [
        ("large", large),
        ("symlink", link),
        ("directory", scratch.0.clone()),
    ] {
        assert_eq!(
            code(&ingest(
                "staging",
                "indications",
                "desk-marks",
                &path,
                OPERATOR_URL
            )),
            Some(PRECONDITION),
            "{label}"
        );
    }
}

#[test]
fn signal_ingest_refuses_production_and_any_unattested_target() {
    let scratch = Scratch::new();
    let file = scratch.file("ok.csv", &format!("{INDICATIONS}\n"));
    let production = ingest(
        "production",
        "indications",
        "desk-marks",
        &file,
        "postgres://verdaxis_operator:pw@127.0.0.1:1/verdaxis_prod",
    );
    assert_eq!(
        code(&production),
        Some(USAGE),
        "production exits 2 before a pool"
    );
    for (label, url) in [
        (
            "app role",
            "postgres://verdaxis_app_staging:pw@127.0.0.1:1/verdaxis_staging",
        ),
        (
            "migrator role",
            "postgres://verdaxis_migrator_staging:pw@127.0.0.1:1/verdaxis_staging",
        ),
        (
            "production database",
            "postgres://verdaxis_operator_staging:pw@127.0.0.1:1/verdaxis_prod",
        ),
        ("no URL", ""),
    ] {
        let output = ingest("staging", "indications", "desk-marks", &file, url);
        assert_eq!(code(&output), Some(PRECONDITION), "{label}");
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(!text.contains(":pw@"), "{label}: the URL is never printed");
    }
    // The deployed identity must name this release.
    let mismatch = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", "f".repeat(40))
        .env("OPERATOR_DATABASE_URL", OPERATOR_URL)
        .args([
            "ops",
            "ingest",
            "signals",
            "--family",
            "indications",
            "--source",
            "desk-marks",
        ])
        .arg("--file")
        .arg(&file)
        .args(["--environment", "staging", "--release-sha", SHA])
        .output()
        .unwrap();
    assert_eq!(code(&mismatch), Some(PRECONDITION), "another release");
}
