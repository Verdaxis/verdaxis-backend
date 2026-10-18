//! Load the same source-attested policy in the serving and contract binaries.

use crate::MarketPolicy;
use sha2::Digest;

const POLICY_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.json"
));
const MANIFEST_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/rust/market-policy.manifest.json"
));

pub fn committed_policy() -> Result<MarketPolicy, String> {
    attested_policy(POLICY_BYTES, MANIFEST_BYTES)
}

fn attested_policy(bytes: &[u8], manifest: &[u8]) -> Result<MarketPolicy, String> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        source_sha: String,
        policy_sha256: String,
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest).map_err(|_| "invalid policy manifest".to_owned())?;
    let digest: String = sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != manifest.policy_sha256 {
        return Err("market policy digest mismatch".to_owned());
    }
    crate::market::policy::load_policy(bytes, &manifest.source_sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_policy_from_another_oracle() {
        let mut policy: serde_json::Value = serde_json::from_slice(POLICY_BYTES).unwrap();
        policy["source_sha"] = serde_json::json!("a".repeat(40));
        let bytes = serde_json::to_vec(&policy).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(MANIFEST_BYTES).unwrap();
        manifest["policy_sha256"] = serde_json::json!(
            sha2::Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(
            attested_policy(&bytes, &serde_json::to_vec(&manifest).unwrap())
                .err()
                .unwrap()
                .contains("source SHA mismatch")
        );
    }

    #[test]
    fn rejects_policy_digest_drift() {
        let mut bytes = POLICY_BYTES.to_vec();
        bytes.push(b' ');
        assert!(
            attested_policy(&bytes, MANIFEST_BYTES)
                .err()
                .unwrap()
                .contains("digest mismatch")
        );
    }

    #[test]
    fn accepts_committed_policy_manifest() {
        assert!(committed_policy().is_ok());
    }
}
