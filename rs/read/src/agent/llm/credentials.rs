//! Environment credentials are private and have no Debug or Serialize implementation.
use super::ErrorCode;
use serde_json::Value;
use std::sync::Mutex;
use tokio::time::Instant;

pub const VARIABLES: [&str; 3] = [
    "OPENCODE_API_KEY",
    "OPENCODE_API_KEY_2",
    "OPENCODE_API_KEY_3",
];

pub(super) struct Credential {
    pub slot: u8,
    pub key: String,
    pub state: Mutex<SlotState>,
}

#[derive(Clone, Copy, Default)]
pub(super) enum Headroom {
    Available(f64),
    Exhausted,
    #[default]
    Unknown,
}

#[derive(Default)]
pub(super) struct SlotState {
    pub disabled: bool,
    pub usage: Option<(Instant, Headroom)>,
}

pub(super) fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Vec<Credential>, ErrorCode> {
    let mut credentials: Vec<Credential> = Vec::new();
    for (index, name) in VARIABLES.iter().enumerate() {
        let Some(key) = lookup(name).filter(|key| !key.is_empty()) else {
            continue;
        };
        if key.len() > 512
            || !key.is_ascii()
            || key.chars().any(char::is_control)
            || credentials.iter().any(|credential| credential.key == key)
        {
            return Err(ErrorCode::CredentialRejected);
        }
        credentials.push(Credential {
            slot: index as u8 + 1,
            key,
            state: Mutex::new(SlotState::default()),
        });
    }
    if credentials.is_empty() {
        return Err(ErrorCode::NoCredentials);
    }
    Ok(credentials)
}

/// A known unavailable meter wins over missing fields in another window.
pub(super) fn headroom(body: &Value) -> Headroom {
    let mut highest: f64 = 0.0;
    let mut complete = true;
    for window in ["rolling", "weekly", "monthly"] {
        let meter = &body["usage"][window];
        if meter["status"]
            .as_str()
            .is_some_and(|status| status != "ok")
        {
            return Headroom::Exhausted;
        }
        match meter["percent"].as_f64() {
            Some(percent) if percent.is_finite() && percent >= 100.0 => return Headroom::Exhausted,
            Some(percent) if percent.is_finite() && percent >= 0.0 && meter["status"] == "ok" => {
                highest = highest.max(percent);
            }
            _ => complete = false,
        }
    }
    if complete {
        Headroom::Available(highest)
    } else {
        Headroom::Unknown
    }
}

pub(super) fn contains_key(bytes: &[u8], credentials: &[Credential]) -> bool {
    credentials.iter().any(|credential| {
        let key = credential.key.as_bytes();
        let escaped = serde_json::to_string(&credential.key).expect("string serialization");
        bytes.windows(key.len()).any(|part| part == key)
            || bytes
                .windows(escaped.len() - 2)
                .any(|part| part == &escaped.as_bytes()[1..escaped.len() - 1])
    })
}

pub(super) fn value_contains_key(value: &Value, credentials: &[Credential]) -> bool {
    match value {
        Value::String(text) => contains_key(text.as_bytes(), credentials),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_key(value, credentials)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            contains_key(key.as_bytes(), credentials) || value_contains_key(value, credentials)
        }),
        _ => false,
    }
}
