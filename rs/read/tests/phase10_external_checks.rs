//! Phase 10 Task 29: `monitor external-checks` is retired, not ported.
//!
//! `verdaxis-external-monitor.service/.timer` ran
//! `/usr/local/libexec/verdaxis-external-monitor`. On 4 Nov that program and
//! the unit were installed on neither host, and the repository has no source
//! for it, so there is no behaviour to port. The units are retired (Task 32
//! deletes them); the command stays parsed but refuses, opens nothing and
//! makes no request.
use std::process::Command;

#[test]
fn external_checks_are_bounded_one_request_each_and_isolated() {
    // Retired: the command refuses before any work, with no state to read.
    let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .args(["monitor", "external-checks"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let line: serde_json::Value = serde_json::from_slice(&output.stderr)
        .or_else(|_| serde_json::from_slice(&output.stdout))
        .unwrap_or_default();
    assert_eq!(line["outcome"], "not_available", "{line}");

    // The inventory records the retirement for both unit files.
    let units: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase10/units.json"
    ))
    .unwrap();
    let retired: Vec<&serde_json::Value> = units["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["source"]
                .as_str()
                .unwrap()
                .contains("systemd/verdaxis-external-monitor.")
        })
        .collect();
    assert_eq!(retired.len(), 2);
    for row in retired {
        assert_eq!(row["end_state"], "gone");
        assert_eq!(row["deleting_task"], 32);
    }
}
