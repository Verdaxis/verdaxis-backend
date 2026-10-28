//! In-app validation of the request `Host` (authority), behind Caddy.
//!
//! Caddy already selects the API site by host name. This layer repeats the
//! check inside the process so a request that reaches the loopback listener
//! with a name nobody configured (a mis-routed proxy, a DNS-rebinding page, a
//! forged `Host` through a future route change) is refused before any route,
//! quota or redirect builder reads that name.
//!
//! Rules:
//! - The allowlist is the comma-separated `ALLOWED_HOSTS` setting. An entry is
//!   a DNS name, an IPv4 address or a bracketed IPv6 address, with an optional
//!   port. Wildcards are not supported.
//! - Names compare case-insensitively (RFC 9110 section 4.2.3).
//! - An entry without a port matches that host on any port, and a request with
//!   no port. The port carries no information about which site was asked for;
//!   the proxy and the health checks use several ports (443, 8010, 8011, test
//!   ports). An entry with a port matches only that port, as sent.
//! - Loopback authorities are always allowed, on any port: `localhost`, any
//!   `127.0.0.0/8` address and `[::1]`. The deploy readiness gate, the
//!   monitors, the shadow harness and the operator runbooks reach the service
//!   at `127.0.0.1:<port>` (or `127.0.0.1` without a port). A loopback name
//!   cannot be chosen by a remote attacker to redirect a link to their site,
//!   so allowing it does not weaken the check.
//! - A request has to carry exactly one `Host` header. If the request target is
//!   in absolute form, its authority is checked too. A request with no
//!   authority at all is refused.
//! - A trailing dot (`api.example.`), user information and non-ASCII names do
//!   not match; clients send the plain ASCII name.
//! - An unset or blank `ALLOWED_HOSTS` turns the check off, so a missing key
//!   cannot stop a deploy. The deploy preparer sets it for production and
//!   staging, and the binary logs which mode it started in. A present but
//!   malformed value refuses the start, like every other runtime setting.

use axum::{
    extract::{Request, State},
    http::{StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
    sync::Arc,
};

/// Error code of the refusal, in the standard `{"detail": {"code", "message"}}` shape.
pub const INVALID_HOST_CODE: &str = "INVALID_HOST";

const MAX_NAME_BYTES: usize = 253;
const MAX_LABEL_BYTES: usize = 63;

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostName {
    /// A DNS name in lower case.
    Name(String),
    Address(IpAddr),
}

impl HostName {
    fn is_loopback(&self) -> bool {
        match self {
            HostName::Name(name) => name == "localhost",
            HostName::Address(address) => address.is_loopback(),
        }
    }
}

/// A parsed `host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostAuthority {
    host: HostName,
    port: Option<u16>,
}

impl HostAuthority {
    /// Parse the text of a `Host` header or of an allowlist entry. Returns
    /// `None` for anything that is not a plain `host[:port]`.
    fn parse(text: &str) -> Option<Self> {
        // `Authority` accepts user information; a Host never carries it. It
        // also leaves a non-numeric port as part of the host, so the port is
        // split off here instead of read back from it.
        if text.contains('@') {
            return None;
        }
        Authority::from_str(text).ok()?;
        let (host, port) = if text.starts_with('[') {
            let (host, rest) = text.split_at(text.find(']')? + 1);
            match rest {
                "" => (host, None),
                _ => (host, Some(rest.strip_prefix(':')?)),
            }
        } else {
            match text.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (text, None),
            }
        };
        if host.is_empty() {
            return None;
        }
        let port = match port {
            None => None,
            Some(digits) => {
                if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                Some(digits.parse::<u16>().ok()?)
            }
        };
        let host = if let Some(inner) = host.strip_prefix('[') {
            let inner = inner.strip_suffix(']')?;
            HostName::Address(IpAddr::V6(Ipv6Addr::from_str(inner).ok()?))
        } else if let Ok(address) = Ipv4Addr::from_str(host) {
            HostName::Address(IpAddr::V4(address))
        } else {
            HostName::Name(host.to_ascii_lowercase())
        };
        Some(Self { host, port })
    }

    /// Check the shape of an allowlist entry, which is stricter than the shape
    /// a request may have: no wildcard, no empty or over-long label.
    fn parse_entry(text: &str) -> Option<Self> {
        let parsed = Self::parse(text)?;
        if let HostName::Name(name) = &parsed.host {
            let valid_label = |label: &str| {
                !label.is_empty()
                    && label.len() <= MAX_LABEL_BYTES
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            };
            if name.len() > MAX_NAME_BYTES || !name.split('.').all(valid_label) {
                return None;
            }
        }
        Some(parsed)
    }
}

/// The set of authorities this process serves.
#[derive(Debug, Clone, Default)]
pub struct HostPolicy {
    /// `None` means the check is off (`ALLOWED_HOSTS` unset or blank).
    allowed: Option<Vec<HostAuthority>>,
}

impl HostPolicy {
    /// Build the policy from the raw `ALLOWED_HOSTS` text.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let mut allowed = Vec::new();
        for entry in raw.unwrap_or("").split(',').map(str::trim) {
            if entry.is_empty() {
                continue;
            }
            let parsed = HostAuthority::parse_entry(entry).ok_or_else(|| {
                "ALLOWED_HOSTS entries must be plain host names, IP addresses or \
                 bracketed IPv6 addresses with an optional port"
                    .to_owned()
            })?;
            allowed.push(parsed);
        }
        Ok(Self {
            allowed: (!allowed.is_empty()).then_some(allowed),
        })
    }

    /// Whether requests are checked at all.
    pub fn is_enforced(&self) -> bool {
        self.allowed.is_some()
    }

    /// The configured entries for the startup log line, never request data.
    pub fn configured_hosts(&self) -> Vec<String> {
        self.allowed
            .iter()
            .flatten()
            .map(|entry| {
                let host = match &entry.host {
                    HostName::Name(name) => name.clone(),
                    HostName::Address(IpAddr::V4(address)) => address.to_string(),
                    HostName::Address(IpAddr::V6(address)) => format!("[{address}]"),
                };
                match entry.port {
                    Some(port) => format!("{host}:{port}"),
                    None => host,
                }
            })
            .collect()
    }

    fn permits_text(&self, text: &str) -> bool {
        let Some(allowed) = &self.allowed else {
            return true;
        };
        let Some(requested) = HostAuthority::parse(text) else {
            return false;
        };
        requested.host.is_loopback()
            || allowed.iter().any(|entry| {
                entry.host == requested.host
                    && entry.port.is_none_or(|port| Some(port) == requested.port)
            })
    }

    /// Decide for one request: its single `Host` header and, for an
    /// absolute-form target, the target's authority must both be allowed.
    pub fn permits(&self, request: &Request) -> bool {
        if !self.is_enforced() {
            return true;
        }
        let mut hosts = request.headers().get_all(header::HOST).iter();
        let header_host = match (hosts.next(), hosts.next()) {
            (Some(value), None) => match value.to_str() {
                Ok(text) => Some(text),
                Err(_) => return false,
            },
            (None, _) => None,
            (Some(_), Some(_)) => return false,
        };
        let target_host = request.uri().authority().map(Authority::as_str);
        if header_host.is_none() && target_host.is_none() {
            return false;
        }
        header_host
            .into_iter()
            .chain(target_host)
            .all(|text| self.permits_text(text))
    }
}

/// The 400 for a refused request. The message never echoes the offered host.
fn invalid_host() -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({"detail": {
            "code": INVALID_HOST_CODE,
            "message": "The Host header is missing or is not served by this API.",
        }})),
    )
        .into_response()
}

/// Middleware: refuse a request whose authority is not allowed.
pub async fn guard(
    State(policy): State<Arc<HostPolicy>>,
    request: Request,
    next: Next,
) -> Response {
    if policy.permits(&request) {
        next.run(request).await
    } else {
        invalid_host()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(raw: &str) -> HostPolicy {
        HostPolicy::parse(Some(raw)).unwrap()
    }

    #[test]
    fn unset_or_blank_setting_is_not_enforced() {
        for raw in [None, Some(""), Some("  "), Some(" , ,")] {
            let parsed = HostPolicy::parse(raw).unwrap();
            assert!(!parsed.is_enforced(), "{raw:?}");
            assert!(parsed.permits_text("anything.example"));
        }
    }

    #[test]
    fn names_compare_case_insensitively_and_ignore_the_port() {
        let policy = policy(" API.Verdaxis.Exchange , api-staging.verdaxis.exchange ");
        assert!(policy.is_enforced());
        for host in [
            "api.verdaxis.exchange",
            "Api.VERDAXIS.exchange",
            "api.verdaxis.exchange:443",
            "api-staging.verdaxis.exchange:8443",
        ] {
            assert!(policy.permits_text(host), "{host}");
        }
        for host in [
            "evil.example",
            "api.verdaxis.exchange.evil.example",
            "xapi.verdaxis.exchange",
            "verdaxis.exchange",
            "api.verdaxis.exchange.",
            "api.verdaxis.exchange:abc",
            "api.verdaxis.exchange:99999",
            "user@api.verdaxis.exchange",
            "api.verdaxis.exchange@evil.example",
            "api.verdaxis.exchange evil.example",
            "",
            ":443",
        ] {
            assert!(!policy.permits_text(host), "{host:?}");
        }
    }

    #[test]
    fn an_entry_with_a_port_matches_only_that_port() {
        let policy = policy("api.example:8443");
        assert!(policy.permits_text("api.example:8443"));
        assert!(!policy.permits_text("api.example"));
        assert!(!policy.permits_text("api.example:443"));
    }

    #[test]
    fn loopback_is_always_allowed_on_any_port() {
        let policy = policy("api.example");
        for host in [
            "127.0.0.1",
            "127.0.0.1:8010",
            "127.0.0.1:8011",
            "localhost",
            "LOCALHOST:49152",
            "[::1]",
            "[::1]:8010",
            "127.1.2.3:80",
        ] {
            assert!(policy.permits_text(host), "{host}");
        }
        for host in [
            "128.0.0.1",
            "0.0.0.0",
            "localhost.evil.example",
            "[::2]",
            "[::1",
            "[::1]x",
            "[::1]:",
            "[::1]:8o",
            "127.0.0.1:",
            "::1",
        ] {
            assert!(!policy.permits_text(host), "{host}");
        }
    }

    #[test]
    fn ip_entries_compare_by_address() {
        let policy = policy("203.0.113.7, [2001:DB8::1]:8443");
        assert!(policy.permits_text("203.0.113.7:80"));
        assert!(policy.permits_text("[2001:db8:0:0:0:0:0:1]:8443"));
        assert!(!policy.permits_text("[2001:db8::1]:80"));
        assert!(!policy.permits_text("203.0.113.8"));
    }

    #[test]
    fn malformed_entries_refuse_the_setting() {
        for raw in [
            "*.example",
            "https://api.example",
            "api.example/path",
            "api.example:notaport",
            "user@api.example",
            "api example",
            "api.example.",
            ".example",
            "api..example",
            "-api.example",
            "api-.example",
            "münchen.example",
            "[::1",
            "api_host.example",
            "api.example,*",
        ] {
            assert!(HostPolicy::parse(Some(raw)).is_err(), "{raw}");
        }
        let long_label = format!("{}.example", "a".repeat(64));
        assert!(HostPolicy::parse(Some(&long_label)).is_err());
        let long_name = vec!["a".repeat(60); 5].join(".");
        assert!(HostPolicy::parse(Some(&long_name)).is_err());
    }

    #[test]
    fn configured_hosts_list_the_normalized_entries() {
        let policy = policy("API.example, [2001:DB8::1]:8443, 203.0.113.7");
        assert_eq!(
            policy.configured_hosts(),
            ["api.example", "[2001:db8::1]:8443", "203.0.113.7"]
        );
        assert!(HostPolicy::default().configured_hosts().is_empty());
    }
}
