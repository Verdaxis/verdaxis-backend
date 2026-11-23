//! Environment credentials are private and have no Debug or Serialize implementation.
use super::ErrorCode;
use serde_json::Value;
pub const VARIABLES: [&str; 1] = ["MISTRAL_API_KEY"];

pub(super) struct Credential {
    pub key: String,
}

pub(super) fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Credential, ErrorCode> {
    let key = lookup(VARIABLES[0])
        .filter(|key| !key.is_empty())
        .ok_or(ErrorCode::NoCredentials)?;
    if key.len() > 512 || !key.is_ascii() || key.chars().any(char::is_control) {
        return Err(ErrorCode::CredentialRejected);
    }
    Ok(Credential { key })
}

pub(super) fn contains_key(bytes: &[u8], credential: &Credential) -> bool {
    let key = credential.key.as_bytes();
    let escaped = serde_json::to_string(&credential.key).expect("string serialization");
    bytes.windows(key.len()).any(|part| part == key)
        || bytes
            .windows(escaped.len() - 2)
            .any(|part| part == &escaped.as_bytes()[1..escaped.len() - 1])
}

pub(super) fn value_contains_key(value: &Value, credential: &Credential) -> bool {
    match value {
        Value::String(text) => contains_key(text.as_bytes(), credential),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_key(value, credential)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            contains_key(key.as_bytes(), credential) || value_contains_key(value, credential)
        }),
        _ => false,
    }
}
