pub mod grace;
pub mod policy;

impl From<&crate::Config> for policy::KycPolicy {
    fn from(config: &crate::Config) -> Self {
        Self {
            execution_gate_enabled: config.kyc_execution_gate_enabled,
        }
    }
}
