//! Environment credentials are private and have no Debug or Serialize implementation.
use super::ErrorCode;
use serde_json::Value;
use tokio::time::Instant;

pub const VARIABLES: [&str; 3] = [
    "OPENCODE_API_KEY",
    "OPENCODE_API_KEY_2",
    "OPENCODE_API_KEY_3",
];

pub(super) struct Credential {
    pub slot: u8,
    pub key: String,
    pub disabled: bool,
    pub usage: Option<(Instant, Option<f64>)>,
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
            disabled: false,
            usage: None,
        });
    }
    if credentials.is_empty() {
        return Err(ErrorCode::NoCredentials);
    }
    Ok(credentials)
}

pub(super) fn highest_usage(body: &Value) -> Option<f64> {
    let mut highest: f64 = 0.0;
    for window in ["rolling", "weekly", "monthly"] {
        let meter = &body["usage"][window];
        let percent = meter["percent"].as_f64()?;
        if meter["status"] != "ok" || !percent.is_finite() || !(0.0..100.0).contains(&percent) {
            return None;
        }
        highest = highest.max(percent);
    }
    Some(highest)
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
