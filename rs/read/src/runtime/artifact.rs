//! Compiled artifact identity.
//!
//! The deploy transaction and the CI verifier must refuse a binary that does
//! not belong to the release it claims. This module owns that comparison and
//! the version document the binary prints before it serves, so the operator
//! CLI and the transaction read one definition instead of a copy each.

use crate::compiled_sha;

/// The release metadata schema version this binary emits and accepts.
pub const ARTIFACT_VERSION: u32 = 1;

/// The glibc floor the published artifact is built against. The staging host
/// runs glibc 2.35 and the production host 2.39, so a `ubuntu-22.04` build
/// (glibc 2.35) runs on both; CI proves the built binary needs no newer symbol.
pub const LIBC_MIN: &str = "2.35";

/// The compiled migration-revision allowlist, embedded at build time.
///
/// This is the same reviewed file `runtime::config::validate_config` reads; the
/// unit tests below assert the two definitions cannot drift.
pub const ALLOWED_REVISIONS: &[&str] = &[
    "kycg_20261014_staging",
    "p0_20261016_union",
    "prc_20261018_cache",
    "prc_20261026_changes",
    "p3_20261020_auth_tokens",
    "sse_20261021_replay_floor",
];

/// The reviewed allowlist file, read by the drift test against `ALLOWED_REVISIONS`.
#[cfg(test)]
const ALLOWED_REVISIONS_FILE: &str = include_str!("../../../../deploy/rust/allowed-revisions.json");

/// Compare the source identity compiled into the binary with the identity the
/// runtime was told to serve. Both are full lowercase 40-hex release SHAs.
pub fn verify_artifact_identity(compiled: &str, runtime: &str) -> Result<(), String> {
    if compiled.len() != 40 || !compiled.bytes().all(is_lower_hex) {
        return Err("compiled identity is not a full lowercase SHA".into());
    }
    if runtime.len() != 40 || !runtime.bytes().all(is_lower_hex) {
        return Err("runtime identity is not a full lowercase SHA".into());
    }
    if compiled == runtime {
        Ok(())
    } else {
        Err("binary release mismatch".into())
    }
}

/// Refuse when the running binary's compiled identity differs from the release
/// the caller names. `runtime::config::validate_config` already compares the
/// configured release SHA with the compiled one; this helper exists so the
/// deploy transaction can compare an artifact's recorded compiled identity with
/// a runtime identity without compiling a second definition of the rule.
pub fn verify_compiled_identity(runtime_release_sha: &str) -> Result<(), String> {
    verify_artifact_identity(compiled_sha(), runtime_release_sha)
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// The document `verdaxis-read --version-json` prints before it serves.
///
/// `target` is not compiled into the crate (Cargo does not export `TARGET` to
/// the crate source), so the build target string is recorded by CI in the
/// artifact metadata and the verifier binds it there. This document carries
/// only what the binary itself can prove: its release SHA, its compiled
/// revision allowlist, its glibc floor, and the host architecture/OS it runs on.
pub fn version_json() -> serde_json::Value {
    serde_json::json!({
        "binary": "verdaxis-read",
        "artifact_version": ARTIFACT_VERSION,
        "release_sha": compiled_sha(),
        "migration_revision_allowlist": ALLOWED_REVISIONS,
        "libc_min": LIBC_MIN,
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_binary_refuses_new_release() {
        assert!(verify_artifact_identity(&"a".repeat(40), &"b".repeat(40)).is_err());
    }

    #[test]
    fn matching_identity_is_accepted() {
        assert!(verify_artifact_identity(&"a".repeat(40), &"a".repeat(40)).is_ok());
    }

    #[test]
    fn malformed_identity_is_refused() {
        let cases = [
            String::new(),
            "a".to_owned(),
            "a".repeat(39),
            "a".repeat(41),
            "A".repeat(40),
            "g".repeat(40),
        ];
        let matching = "a".repeat(40);
        for bad in &cases {
            assert!(
                verify_artifact_identity(bad, &matching).is_err(),
                "compiled `{bad}` must be refused"
            );
            assert!(
                verify_artifact_identity(&matching, bad).is_err(),
                "runtime `{bad}` must be refused"
            );
        }
    }

    #[test]
    fn version_document_reports_the_compiled_identity() {
        let document = version_json();
        assert_eq!(document["release_sha"], compiled_sha());
        assert_eq!(document["artifact_version"], ARTIFACT_VERSION);
        assert_eq!(document["libc_min"], LIBC_MIN);
        assert_eq!(document["arch"], std::env::consts::ARCH);
        assert_eq!(document["os"], std::env::consts::OS);
        assert_eq!(
            document["migration_revision_allowlist"],
            serde_json::json!([
                "kycg_20261014_staging",
                "p0_20261016_union",
                "prc_20261018_cache",
                "prc_20261026_changes"
            ])
        );
    }

    #[test]
    fn enumerated_allowlist_matches_the_reviewed_file() {
        let reviewed: Vec<String> = serde_json::from_str(ALLOWED_REVISIONS_FILE).unwrap();
        assert_eq!(
            reviewed,
            ALLOWED_REVISIONS
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>(),
            "artifact.rs must not drift from deploy/rust/allowed-revisions.json"
        );
    }

    #[test]
    fn compiled_identity_matches_this_build() {
        assert!(verify_compiled_identity(compiled_sha()).is_ok());
        assert!(verify_compiled_identity(&"b".repeat(40)).is_err());
    }
}
