//! Phase 10 Task 22: the offline gates of `ops import gena`.
//!
//! The operator URL points at a closed loopback port: a file that passes every
//! offline check reaches the connection and fails there (exit 4), so exit 2 or
//! 3 proves the refusal came before any database.
use std::collections::BTreeMap;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use verdaxis_read::cli::ops::csv::Record;
use verdaxis_read::cli::ops::gena::{COLUMNS, validate};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const PRECONDITION: i32 = 3;
const DEPENDENCY: i32 = 4;
const OPERATOR_URL: &str = "postgres://verdaxis_operator_staging:pw@127.0.0.1:1/verdaxis_staging";
const HEADER: &str =
    "project_name,country,region,capacity_kt,cod_year,status,feedstock,technology,lat,lng,gena_id";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-gena-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn csv(&self, name: &str, rows: &[&str]) -> PathBuf {
        let path = self.0.join(name);
        let mut text = format!("{HEADER}\n");
        for row in rows {
            text.push_str(row);
            text.push('\n');
        }
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn import(file: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", SHA)
        .env("OPERATOR_DATABASE_URL", OPERATOR_URL)
        .args(["ops", "import", "gena", "--fuel-type", "Methanol"])
        .arg("--file")
        .arg(file)
        .args(["--environment", "staging", "--release-sha", SHA])
        .args(extra)
        .output()
        .unwrap()
}

/// One record by column, for the validator.
fn record(values: &[(&str, &str)]) -> Record {
    let mut fields: BTreeMap<String, String> = COLUMNS
        .iter()
        .map(|column| ((*column).to_owned(), String::new()))
        .collect();
    for (column, value) in values {
        fields.insert((*column).to_owned(), (*value).to_owned());
    }
    Record {
        row_number: 1,
        fields,
    }
}

const VALID: [(&str, &str); 4] = [
    ("project_name", "Green Methanol One"),
    ("country", "Denmark"),
    ("gena_id", "GENA-1"),
    ("status", "Operational"),
];

#[test]
fn gena_zero_coordinate_is_preserved() {
    let mut values = VALID.to_vec();
    values.extend([("lat", "0"), ("lng", "0.0")]);
    assert_eq!(
        validate(&record(&values), false).unwrap().point,
        Some((0.0, 0.0))
    );
    let mut equator = VALID.to_vec();
    equator.extend([("lat", "0"), ("lng", "103.8")]);
    assert_eq!(
        validate(&record(&equator), false).unwrap().point,
        Some((0.0, 103.8))
    );
    assert_eq!(validate(&record(&VALID), false).unwrap().point, None);
}

#[test]
fn gena_unknown_status_defaults_to_announced() {
    for (raw, stored) in [
        ("Operational", "OPERATIONAL"),
        ("  under construction ", "UNDER_CONSTRUCTION"),
        ("CANCELLED", "CANCELLED"),
        ("announced", "ANNOUNCED"),
        ("feasibility study", "ANNOUNCED"),
        ("", "ANNOUNCED"),
    ] {
        let mut values = VALID.to_vec();
        values.retain(|(column, _)| *column != "status");
        values.push(("status", raw));
        assert_eq!(
            validate(&record(&values), false).unwrap().status,
            stored,
            "{raw:?}"
        );
    }
}

#[test]
fn gena_blank_id_is_refused_by_default() {
    let mut values = VALID.to_vec();
    values.retain(|(column, _)| *column != "gena_id");
    assert!(validate(&record(&values), false).is_err());
    assert_eq!(validate(&record(&values), true).unwrap().gena_id, None);
    let scratch = Scratch::new();
    let file = scratch.csv("blank-id.csv", &["Plant,Denmark,,,,,,,,,"]);
    let output = import(&file, &["--apply"]);
    assert_eq!(
        output.status.code(),
        Some(PRECONDITION),
        "refused before any database"
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["errors"][0]["row"], 1);
    assert_eq!(
        import(&file, &["--apply", "--allow-blank-id"])
            .status
            .code(),
        Some(DEPENDENCY),
        "--allow-blank-id reaches the connection"
    );
}

#[test]
fn gena_invalid_coordinate_or_missing_required_text_fails_before_any_write() {
    let scratch = Scratch::new();
    let valid = "Plant,Denmark,Europe,100.5,2027,operational,biomass,gasification,55.6,12.5,GENA-1";
    for (label, bad) in [
        (
            "latitude out of range",
            "Plant,Denmark,,,,,,,91,12.5,GENA-2",
        ),
        (
            "longitude out of range",
            "Plant,Denmark,,,,,,,55.6,-181,GENA-2",
        ),
        ("only one coordinate", "Plant,Denmark,,,,,,,55.6,,GENA-2"),
        ("not a number", "Plant,Denmark,,,,,,,north,12.5,GENA-2"),
        ("infinite", "Plant,Denmark,,,,,,,inf,12.5,GENA-2"),
        ("missing name", " ,Denmark,,,,,,,,,GENA-2"),
        ("missing country", "Plant,,,,,,,,,,GENA-2"),
        ("bad capacity", "Plant,Denmark,,-1,,,,,,,GENA-2"),
        ("bad year", "Plant,Denmark,,,20x7,,,,,,GENA-2"),
        (
            "long country",
            &format!("Plant,{},,,,,,,,,GENA-2", "c".repeat(101)),
        ),
        ("short row", "Plant,Denmark"),
    ] {
        let file = scratch.csv(&format!("{}.csv", label.replace(' ', "-")), &[valid, bad]);
        let output = import(&file, &["--apply"]);
        assert_eq!(
            output.status.code(),
            Some(PRECONDITION),
            "{label}: the whole file is refused"
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["errors"][0]["row"], 2, "{label}");
        assert_eq!(report["created"], 0, "{label}");
    }
}

#[test]
fn gena_file_and_row_bounds_are_enforced() {
    let scratch = Scratch::new();
    let row = "Plant,Denmark,,,,,,,,,GENA-1";
    let many = vec![row; 10_001];
    let too_many = scratch.csv("rows.csv", &many);
    let large = scratch.0.join("large.csv");
    std::fs::write(
        &large,
        format!(
            "{HEADER}\n{}",
            format!("{row}\n").repeat(4 * 1024 * 1024 / row.len() + 1)
        ),
    )
    .unwrap();
    let link = scratch.0.join("link.csv");
    symlink(scratch.csv("ok.csv", &[row]), &link).unwrap();
    let extra_column = scratch.0.join("extra.csv");
    std::fs::write(&extra_column, format!("{HEADER},comment\n{row},x\n")).unwrap();
    for (label, path) in [
        ("rows", too_many),
        ("size", large),
        ("symlink", link),
        ("header", extra_column),
    ] {
        assert_eq!(
            import(&path, &[]).status.code(),
            Some(PRECONDITION),
            "{label}"
        );
    }
    let blank_fuel = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .env_clear()
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", SHA)
        .args(["ops", "import", "gena", "--fuel-type", " ", "--file"])
        .arg(scratch.csv("fuel.csv", &[row]))
        .args(["--environment", "staging", "--release-sha", SHA])
        .output()
        .unwrap();
    assert_eq!(
        blank_fuel.status.code(),
        Some(2),
        "a blank fuel type is a usage refusal"
    );
}
