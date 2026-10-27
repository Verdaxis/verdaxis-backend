//! Opaque token bytes: 256 bits from the operating system, URL-safe base64
//! without padding (43 characters). The same shape is used for refresh-token
//! identifiers and for the device-session cookie.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Entropy per token, in bytes.
pub const TOKEN_BYTES: usize = 32;
/// The encoded length of [`TOKEN_BYTES`] as unpadded URL-safe base64.
pub const TOKEN_LENGTH: usize = 43;

/// A new 256-bit token, matching `^[A-Za-z0-9_-]{43}$`.
///
/// A failure to read OS entropy is unrecoverable; the process cannot issue a
/// session safely without it.
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system entropy source is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The lowercase SHA-256 hex digest of a token, the stored `jti_hash` form.
pub fn token_digest(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write;
        write!(&mut hex, "{byte:02x}").expect("writing to a String does not fail");
    }
    hex
}

/// Whether a value has the exact shape of a generated token.
pub fn is_token_shaped(token: &str) -> bool {
    token.len() == TOKEN_LENGTH
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
