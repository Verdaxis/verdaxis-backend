use std::fmt;

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde_json::Value;
use uuid::Uuid;

use super::{AuthError, VerifiedAccess};

pub struct VerificationSettings {
    current: DecodingKey,
    previous: Option<DecodingKey>,
    issuer: String,
    audience: String,
}

impl VerificationSettings {
    pub fn new(
        current: String,
        previous: Option<String>,
        issuer: String,
        audience: String,
    ) -> Self {
        Self {
            current: DecodingKey::from_secret(current.as_bytes()),
            previous: previous.map(|key| DecodingKey::from_secret(key.as_bytes())),
            issuer,
            audience,
        }
    }
}

impl fmt::Debug for VerificationSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerificationSettings")
            .field("keys", &"[REDACTED]")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .finish()
    }
}

pub fn verify_authorization(
    authorization: Option<&str>,
    settings: &VerificationSettings,
    now: DateTime<Utc>,
) -> Result<VerifiedAccess, AuthError> {
    let (scheme, token) = authorization
        .unwrap_or("")
        .split_once(' ')
        .unwrap_or((authorization.unwrap_or(""), ""));
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::MissingBearer);
    }
    verify_access(token.trim_start_matches(' '), settings, now)
}

pub fn verify_access(
    token: &str,
    settings: &VerificationSettings,
    now: DateTime<Utc>,
) -> Result<VerifiedAccess, AuthError> {
    // Cryptographic validation is always enabled and restricted to HS256.
    // Registered claims are checked below against the supplied action clock:
    // jsonwebtoken's u64 numeric dates and required-exp defaults are not PyJWT.
    let mut validation = Validation::new(Algorithm::HS256);
    validation.required_spec_claims.clear();
    validation.leeway = 0;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    let claims = std::iter::once(&settings.current)
        .chain(settings.previous.iter())
        .find_map(|key| {
            decode::<Value>(token, key, &validation)
                .ok()
                .map(|data| data.claims)
        })
        .ok_or(AuthError::Credentials)?;
    validate_claims(&claims, settings, now)?;
    let subject = claims
        .get("sub")
        .and_then(Value::as_str)
        .ok_or(AuthError::Credentials)?;
    let subject = python_uuid(subject).ok_or(AuthError::Credentials)?;
    let issuance = issued_at(&claims);
    Ok(VerifiedAccess {
        subject,
        issued_at: issuance.as_ref().ok().copied().flatten(),
        issuance_out_of_range: issuance.is_err(),
    })
}

fn validate_claims(
    claims: &Value,
    settings: &VerificationSettings,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    if !claims.is_object() {
        return Err(AuthError::Credentials);
    }
    let seconds =
        now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1_000_000_000.0;
    for name in ["iat", "nbf", "exp"] {
        if let Some(value) = claims.get(name) {
            let number = python_integer(value).ok_or(AuthError::Credentials)?;
            if (name == "exp" && number <= seconds) || (name != "exp" && number > seconds) {
                return Err(AuthError::Credentials);
            }
        }
    }
    if claims.get("iss").and_then(Value::as_str) != Some(settings.issuer.as_str()) {
        return Err(AuthError::Credentials);
    }
    let audience_matches = match claims.get("aud") {
        Some(Value::String(value)) => value == &settings.audience,
        Some(Value::Array(values)) => {
            values.iter().all(Value::is_string)
                && values
                    .iter()
                    .any(|value| value.as_str() == Some(settings.audience.as_str()))
        }
        _ => false,
    };
    if !audience_matches
        || claims.get("jti").is_some_and(|value| !value.is_string())
        || claims.get("type").and_then(Value::as_str) != Some("access")
    {
        return Err(AuthError::Credentials);
    }
    Ok(())
}

/// Python int(value) accepts booleans, truncates finite floats, and accepts
/// signed, spaced, underscored Unicode decimal strings for NumericDate claims.
fn python_integer(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64().filter(|n| n.is_finite()).map(f64::trunc),
        Value::String(text) => {
            let text = text.trim();
            let (sign, digits) = if let Some(rest) = text.strip_prefix('-') {
                (-1.0, rest)
            } else {
                (1.0, text.strip_prefix('+').unwrap_or(text))
            };
            let mut result = 0.0;
            let mut previous_digit = false;
            for character in digits.chars() {
                if character == '_' {
                    if !previous_digit {
                        return None;
                    }
                    previous_digit = false;
                } else {
                    let digit = unicode_decimal(character)?;
                    result = result * 10.0 + f64::from(digit);
                    previous_digit = true;
                }
            }
            previous_digit.then_some(sign * result)
        }
        _ => None,
    }
}

fn unicode_decimal(character: char) -> Option<u32> {
    let value = character as u32;
    // Unicode Nd blocks are contiguous groups of ten; mathematical digits
    // contain five adjacent groups. Keep this compatibility table explicit.
    const STARTS: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
        0x11650, 0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x16a60, 0x16ac0,
        0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950,
        0x1fbf0,
    ];
    STARTS
        .iter()
        .find_map(|start| (value >= *start && value < *start + 10).then(|| value - *start))
}

fn python_uuid(value: &str) -> Option<Uuid> {
    let value = value.replace("urn:", "").replace("uuid:", "");
    let value = value.trim_matches(['{', '}']).replace('-', "");
    (value.len() == 32)
        .then(|| Uuid::parse_str(&value).ok())
        .flatten()
}

fn issued_at(claims: &Value) -> Result<Option<DateTime<Utc>>, ()> {
    if let Some(value) = claims.get("iat_us") {
        let integer = match value {
            Value::Bool(value) => Some(if *value { 1 } else { 0 }),
            Value::Number(number) if !number.is_f64() => {
                if number.as_i64().is_some_and(|n| n < 0) {
                    None
                } else {
                    Some(number.as_i64().ok_or(())?)
                }
            }
            _ => None,
        };
        if let Some(micros) = integer {
            return DateTime::from_timestamp_micros(micros)
                .filter(|value| (1..=9999).contains(&chrono::Datelike::year(value)))
                .map(Some)
                .ok_or(());
        }
    }
    match claims.get("iat") {
        Some(Value::Number(number)) => {
            let seconds = number.as_f64().ok_or(())?;
            let micros = (seconds * 1_000_000.0).round_ties_even();
            if !micros.is_finite() || micros < i64::MIN as f64 || micros > i64::MAX as f64 {
                return Err(());
            }
            DateTime::from_timestamp_micros(micros as i64)
                .filter(|value| (1..=9999).contains(&chrono::Datelike::year(value)))
                .map(Some)
                .ok_or(())
        }
        _ => Ok(None),
    }
}
