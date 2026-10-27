//! The two session cookies: `refresh_token` and `device_session`.
//!
//! Both cookies share one attribute set, which is part of the browser
//! contract with the Python stack (coexistence C6): `HttpOnly`,
//! `SameSite=Lax`, `Path=/api/auth`, `Max-Age` of one refresh-token life, no
//! `Domain`, and `Secure` in every environment except `development`. A cleared
//! cookie repeats the same attributes with an empty value and `Max-Age=0`.

use std::fmt;

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use cookie::time::Duration;

use super::issue::REFRESH_TOKEN_DAYS;
use super::token_bytes::is_token_shaped;

pub const REFRESH_COOKIE: &str = "refresh_token";
pub const DEVICE_COOKIE: &str = "device_session";
pub const COOKIE_PATH: &str = "/api/auth";
/// Both cookies live as long as a refresh token: 604,800 seconds.
pub const COOKIE_MAX_AGE_SECONDS: i64 = REFRESH_TOKEN_DAYS * 24 * 60 * 60;

/// The cookie values a request presented. `Debug` never prints a value.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Presented {
    /// A non-empty `refresh_token` value.
    pub refresh: Option<String>,
    /// A non-empty `device_session` value, valid or not.
    pub device: Option<String>,
}

impl Presented {
    /// Whether the request carries a refresh cookie; the trusted-origin rule
    /// refuses an absent `Origin` only then.
    pub fn cookie_authenticated(&self) -> bool {
        self.refresh.is_some()
    }

    /// The device cookie, only when it has the generated 43-character shape.
    pub fn valid_device(&self) -> Option<&str> {
        self.device
            .as_deref()
            .filter(|device| is_token_shaped(device))
    }
}

impl fmt::Debug for Presented {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "[REDACTED]");
        formatter
            .debug_struct("Presented")
            .field("refresh", &redacted(&self.refresh))
            .field("device", &redacted(&self.device))
            .finish()
    }
}

/// Writes and clears the session cookies for one environment.
///
/// `set` and `clear` take and return the jar, the way `CookieJar` itself adds
/// cookies; the route returns the jar as part of its response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthCookies {
    secure: bool,
}

impl AuthCookies {
    /// Only an explicit `development` environment drops `Secure`; every other
    /// name, including an unknown one, keeps it.
    pub fn for_environment(environment: &str) -> Self {
        Self {
            secure: environment != "development",
        }
    }

    /// The session cookie values a request presented. An empty value counts
    /// as absent.
    pub fn read(jar: &CookieJar) -> Presented {
        let value = |name: &str| {
            jar.get(name)
                .map(|cookie| cookie.value().to_owned())
                .filter(|value| !value.is_empty())
        };
        Presented {
            refresh: value(REFRESH_COOKIE),
            device: value(DEVICE_COOKIE),
        }
    }

    /// Set both cookies for a new or rotated session.
    pub fn set(&self, jar: CookieJar, refresh: &str, device: &str) -> CookieJar {
        jar.add(self.cookie(REFRESH_COOKIE, refresh.to_owned()))
            .add(self.cookie(DEVICE_COOKIE, device.to_owned()))
    }

    /// Clear both cookies. The removal is always sent, whether or not the
    /// request presented the cookies.
    pub fn clear(&self, jar: CookieJar) -> CookieJar {
        jar.add(self.removal(REFRESH_COOKIE))
            .add(self.removal(DEVICE_COOKIE))
    }

    fn cookie(&self, name: &'static str, value: String) -> Cookie<'static> {
        Cookie::build((name, value))
            .http_only(true)
            .secure(self.secure)
            .same_site(SameSite::Lax)
            .path(COOKIE_PATH)
            .max_age(Duration::seconds(COOKIE_MAX_AGE_SECONDS))
            .build()
    }

    /// The same attributes with an empty value, `Max-Age=0` and an expiry in
    /// the past, so the browser drops the cookie at once.
    fn removal(&self, name: &'static str) -> Cookie<'static> {
        let mut cookie = self.cookie(name, String::new());
        cookie.make_removal();
        cookie
    }
}
