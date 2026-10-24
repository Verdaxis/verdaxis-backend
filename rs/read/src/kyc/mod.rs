pub mod documents;
pub mod grace;
pub mod keyring;
pub mod policy;
pub mod submission;
pub mod vault;

impl From<&crate::Config> for policy::KycPolicy {
    fn from(config: &crate::Config) -> Self {
        Self {
            execution_gate_enabled: config.kyc_execution_gate_enabled,
        }
    }
}
