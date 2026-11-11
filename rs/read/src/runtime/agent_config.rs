use crate::agent::llm::models;

pub const MAX_RATE_LIMIT_MESSAGES: u32 = 10_000;
pub const MAX_RATE_LIMIT_WINDOW_SECONDS: u64 = 86_400;
pub const MAX_DAILY_TOKEN_BUDGET: u64 = 1_000_000_000;

/// Agent admission settings. Missing settings leave the agent disabled.
#[derive(Clone, serde::Deserialize)]
#[serde(default)]
pub struct AgentOptions {
    pub enabled: bool,
    pub model: String,
    pub rate_limit_messages: u32,
    pub rate_limit_window_seconds: u64,
    pub daily_token_budget: u64,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "deepseek-v4.1-flash".to_owned(),
            rate_limit_messages: 20,
            rate_limit_window_seconds: 600,
            daily_token_budget: 200_000,
        }
    }
}

impl AgentOptions {
    pub fn validate(&self) -> Result<(), String> {
        models::settings(&self.model).map_err(|_| "AGENT_MODEL is not supported".to_owned())?;
        for (key, value, maximum) in [
            (
                "AGENT_RATE_LIMIT_MESSAGES",
                u64::from(self.rate_limit_messages),
                u64::from(MAX_RATE_LIMIT_MESSAGES),
            ),
            (
                "AGENT_RATE_LIMIT_WINDOW_SECONDS",
                self.rate_limit_window_seconds,
                MAX_RATE_LIMIT_WINDOW_SECONDS,
            ),
            (
                "AGENT_DAILY_TOKEN_BUDGET",
                self.daily_token_budget,
                MAX_DAILY_TOKEN_BUDGET,
            ),
        ] {
            if !(1..=maximum).contains(&value) {
                return Err(format!("{key} must be between 1 and {maximum}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_use_disabled_defaults() {
        let options: AgentOptions = serde_json::from_str("{}").unwrap();
        assert!(!options.enabled);
        assert_eq!(options.model, "deepseek-v4.1-flash");
        assert_eq!(options.rate_limit_messages, 20);
        assert_eq!(options.rate_limit_window_seconds, 600);
        assert_eq!(options.daily_token_budget, 200_000);
        assert!(options.validate().is_ok());
        let phase2: crate::Phase2Options = serde_json::from_str("{}").unwrap();
        assert!(!phase2.agent.enabled);
        assert_eq!(phase2.agent.daily_token_budget, 200_000);
    }

    #[test]
    fn limits_accept_endpoints_and_refuse_outside_values() {
        for value in [1, MAX_RATE_LIMIT_MESSAGES] {
            assert!(
                AgentOptions {
                    rate_limit_messages: value,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for value in [0, MAX_RATE_LIMIT_MESSAGES + 1] {
            assert!(
                AgentOptions {
                    rate_limit_messages: value,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        for value in [1, MAX_RATE_LIMIT_WINDOW_SECONDS] {
            assert!(
                AgentOptions {
                    rate_limit_window_seconds: value,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for value in [0, MAX_RATE_LIMIT_WINDOW_SECONDS + 1] {
            assert!(
                AgentOptions {
                    rate_limit_window_seconds: value,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        for value in [1, MAX_DAILY_TOKEN_BUDGET] {
            assert!(
                AgentOptions {
                    daily_token_budget: value,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for value in [0, MAX_DAILY_TOKEN_BUDGET + 1] {
            assert!(
                AgentOptions {
                    daily_token_budget: value,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn model_allowlist_applies_when_disabled_too() {
        for model in ["", "unknown", " deepseek-v4.1-flash "] {
            assert!(
                AgentOptions {
                    model: model.to_owned(),
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
}
