//! The trusted-origin rule for browser cookie transitions.
//!
//! A route that sets, rotates or clears the session cookies (login, refresh,
//! logout, password change, invitation acceptance) calls
//! [`require_trusted_origin`] before it reads a credential. The rule:
//!
//! - each environment has a closed set of origins; an unknown environment has
//!   none;
//! - a present `Origin` header must equal one entry exactly, byte for byte.
//!   Browsers send the serialized origin (lowercase scheme and host, no path),
//!   so a path, query, fragment, user information, other port, other scheme or
//!   other case is refused without parsing;
//! - more than one `Origin` header is refused;
//! - an absent `Origin` is refused when the request carries a refresh cookie,
//!   and allowed otherwise (a client without session cookies).

use axum::http::{HeaderMap, header::ORIGIN};

use super::error::{AuthCode, AuthFailure};

const PRODUCTION: &[&str] = &[
    "https://app.verdaxis.exchange",
    "https://verdaxis.exchange",
    "https://canary.verdaxis.exchange",
];
const STAGING: &[&str] = &["https://staging.verdaxis.exchange"];
const DEVELOPMENT: &[&str] = &[
    "http://localhost:5173",
    "http://localhost:5174",
    "http://127.0.0.1:5173",
    "http://127.0.0.1:5174",
];
const TEST: &[&str] = &["https://test"];

/// The closed set of browser origins trusted for cookie transitions in one
/// environment. The runtime configuration accepts only the four names below.
pub fn trusted_origins(environment: &str) -> &'static [&'static str] {
    match environment {
        "production" => PRODUCTION,
        "staging" => STAGING,
        "development" => DEVELOPMENT,
        "test" => TEST,
        _ => &[],
    }
}

/// Refuse a cookie transition from an untrusted browser origin with 403
/// `BROWSER_ORIGIN_REJECTED`. `cookie_authenticated` is true when the request
/// carries a non-empty refresh cookie.
/// The refusal uses the route error envelope, separate from token verification.
pub fn require_trusted_origin(
    headers: &HeaderMap,
    environment: &str,
    cookie_authenticated: bool,
) -> Result<(), AuthFailure> {
    let rejected = || Err(AuthFailure::new(AuthCode::BrowserOriginRejected));
    let mut origins = headers.get_all(ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return if cookie_authenticated {
            rejected()
        } else {
            Ok(())
        };
    };
    if origins.next().is_some() {
        return rejected();
    }
    match origin.to_str() {
        Ok(origin) if trusted_origins(environment).contains(&origin) => Ok(()),
        _ => rejected(),
    }
}
