//! `verdaxis-read monitor health`: loopback readiness and local filesystem checks.
//!
//! The monitor reads the runtime identity files, asks each service for its
//! readiness over numeric loopback HTTP, checks free disk space and, when
//! configured, the size of one directory. It then publishes one private status
//! document (see [`super::status`]).
//!
//! It needs no credentials and no external service, so it works while the
//! services it watches are down. It reads exactly the files named in its
//! configuration and makes only loopback connections.
use super::status::{
    Admission, AdmitAll, Category, CheckResult, Fault, MAX_STATUS_BYTES, Previous, Status,
    build_status, exit_for, failure_status, guarded, load_monitor_status, parse_strict_json,
    publish, quarantine, report, report_refused,
};
use crate::cli::Exit;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use url::Url;

/// Largest runtime identity file, in bytes.
pub const MAX_IDENTITY_BYTES: usize = 256;
/// Largest response header block, in bytes.
const MAX_HEADER_BYTES: usize = 16_384;
const READ_CHUNK_BYTES: usize = 4096;
/// The name given to a status that has no single check to name.
const MONITOR_NAME: &str = "monitor";
const COMMAND: &str = "monitor health";
const READINESS_KEYS: [&str; 4] = ["status", "db", "environment", "release_sha"];

// ---- runtime identity --------------------------------------------------------

/// The environment and release a service is expected to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub environment: String,
    pub release_sha: String,
}

/// An identity file that failed a rule. It carries no detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityRefused;

/// A full, lowercase, nonzero release SHA.
fn is_release_sha(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && text.bytes().any(|byte| byte != b'0')
}

/// Read one identity file: a bounded regular file (not a link) with exactly the
/// two lines `ENVIRONMENT=<expected>` and `RELEASE_SHA=<sha>`, in either order.
///
/// Comments, blank lines, repeated or extra keys, non-ASCII bytes, a missing
/// final newline, a zero or short SHA, and a different environment are refused.
pub fn read_identity(
    path: &Path,
    expected_environment: &str,
) -> Result<RuntimeIdentity, IdentityRefused> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| IdentityRefused)?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(IdentityRefused);
    }
    let mut raw = Vec::new();
    file.take(MAX_IDENTITY_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| IdentityRefused)?;
    if raw.len() > MAX_IDENTITY_BYTES || !raw.ends_with(b"\n") {
        return Err(IdentityRefused);
    }
    let text = std::str::from_utf8(&raw)
        .ok()
        .filter(|text| text.is_ascii())
        .ok_or(IdentityRefused)?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != 2 {
        return Err(IdentityRefused);
    }
    let (mut environment, mut release_sha) = (None, None);
    for line in lines {
        if line.matches('=').count() != 1 {
            return Err(IdentityRefused);
        }
        let (key, value) = line.split_once('=').ok_or(IdentityRefused)?;
        let slot = match key {
            "ENVIRONMENT" => &mut environment,
            "RELEASE_SHA" => &mut release_sha,
            _ => return Err(IdentityRefused),
        };
        if slot.is_some() || value.is_empty() {
            return Err(IdentityRefused);
        }
        *slot = Some(value);
    }
    match (environment, release_sha) {
        (Some(environment), Some(sha))
            if environment == expected_environment && is_release_sha(sha) =>
        {
            Ok(RuntimeIdentity {
                environment: environment.to_owned(),
                release_sha: sha.to_owned(),
            })
        }
        _ => Err(IdentityRefused),
    }
}

// ---- readiness payload --------------------------------------------------------

/// Why a readiness payload was refused. Each class has its own status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessRefusal {
    /// The identity the caller expects is itself invalid.
    ExpectedIdentity,
    /// Not exactly the four string-valued keys.
    Schema,
    /// The service reports itself or its database as not ok.
    Unhealthy,
    /// The service answered for another environment or release.
    Identity,
}

/// Check one readiness payload against the identity the monitor expects.
///
/// The payload must be an object with exactly `status`, `db`, `environment` and
/// `release_sha`, all strings, with `status` and `db` equal to `ok`. Extra keys
/// of any type are refused, so a service cannot leak more than the contract.
pub fn validate_readiness_payload(
    payload: &Value,
    expected_environment: &str,
    expected_release_sha: &str,
) -> Result<(), ReadinessRefusal> {
    if !matches!(expected_environment, "production" | "staging")
        || !is_release_sha(expected_release_sha)
    {
        return Err(ReadinessRefusal::ExpectedIdentity);
    }
    let object = payload.as_object().ok_or(ReadinessRefusal::Schema)?;
    if object.len() != READINESS_KEYS.len() {
        return Err(ReadinessRefusal::Schema);
    }
    let mut values = [""; 4];
    for (slot, key) in values.iter_mut().zip(READINESS_KEYS) {
        *slot = object
            .get(key)
            .and_then(Value::as_str)
            .ok_or(ReadinessRefusal::Schema)?;
    }
    let [status, database, environment, release_sha] = values;
    if status != "ok" || database != "ok" {
        return Err(ReadinessRefusal::Unhealthy);
    }
    if environment != expected_environment || release_sha != expected_release_sha {
        return Err(ReadinessRefusal::Identity);
    }
    Ok(())
}

// ---- loopback HTTP ------------------------------------------------------------

/// Why a response could not be read. Each maps to one fixed status code.
enum HttpFault {
    Timeout,
    Class(&'static str),
    Connection,
}

/// Time left until `deadline`; none left is a timeout.
fn remaining(deadline: Instant) -> Result<Duration, HttpFault> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(HttpFault::Timeout)
    } else {
        Ok(left)
    }
}

fn classify(error: &io::Error) -> HttpFault {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HttpFault::Timeout,
        _ => HttpFault::Connection,
    }
}

/// One parsed response head.
struct Head {
    status: u16,
    content_length: Option<usize>,
}

/// Parse the status line and headers. Repeated headers, a header without a
/// colon, a chunked body or an oversize `Content-Length` are refused.
fn parse_head(block: &[u8], max_body_bytes: usize) -> Result<Head, HttpFault> {
    let invalid = || HttpFault::Class("connection_error");
    let mut lines = block
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let status_line = lines.next().ok_or_else(invalid)?;
    let mut parts = status_line.splitn(3, |byte| *byte == b' ');
    let (Some(_protocol), Some(status), Some(_reason)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    let status: u16 = std::str::from_utf8(status)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(invalid)?;
    let mut headers: Vec<(Vec<u8>, &[u8])> = Vec::new();
    for line in lines {
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or_else(invalid)?;
        let name = line[..colon].trim_ascii().to_ascii_lowercase();
        if headers.iter().any(|(seen, _)| *seen == name) {
            return Err(invalid());
        }
        headers.push((name, line[colon + 1..].trim_ascii()));
    }
    let header = |wanted: &[u8]| {
        headers
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| *value)
    };
    if header(b"transfer-encoding").is_some() {
        return Err(HttpFault::Class("unsupported_transfer_encoding"));
    }
    let content_length = match header(b"content-length") {
        None => None,
        Some(value) => {
            let length: usize = std::str::from_utf8(value)
                .ok()
                .and_then(|text| text.parse().ok())
                .ok_or_else(invalid)?;
            if length > max_body_bytes {
                return Err(HttpFault::Class("body_too_large"));
            }
            Some(length)
        }
    };
    Ok(Head {
        status,
        content_length,
    })
}

/// `GET` one numeric-loopback HTTP URL and return the status and the body.
///
/// One deadline covers connecting, sending, the headers and the body, so a
/// service that drips bytes cannot hold the monitor. Header and body sizes are
/// bounded, redirects are not followed and only `http://127.0.0.1:<port>` is
/// accepted.
fn read_response(
    url: &str,
    deadline: Instant,
    max_body_bytes: usize,
) -> Result<(u16, Vec<u8>), HttpFault> {
    let url = Url::parse(url).map_err(|_| HttpFault::Connection)?;
    let port = url.port().ok_or(HttpFault::Connection)?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
        return Err(HttpFault::Connection);
    }
    let mut target = url.path().to_owned();
    if let Some(query) = url.query() {
        target.push('?');
        target.push_str(query);
    }
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?).map_err(|e| classify(&e))?;
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|e| classify(&e))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| classify(&e))?;

    let mut data = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    let mut head: Option<(usize, Head)> = None;
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| classify(&e))?;
        let read = stream.read(&mut chunk).map_err(|e| classify(&e))?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..read]);
        if head.is_none() {
            let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                if data.len() > MAX_HEADER_BYTES {
                    return Err(HttpFault::Class("headers_too_large"));
                }
                continue;
            };
            if end > MAX_HEADER_BYTES {
                return Err(HttpFault::Class("headers_too_large"));
            }
            head = Some((end, parse_head(&data[..end], max_body_bytes)?));
        }
        if let Some((end, parsed)) = &head {
            let body_length = data.len() - end - 4;
            if body_length > max_body_bytes {
                return Err(HttpFault::Class("body_too_large"));
            }
            if parsed
                .content_length
                .is_some_and(|length| body_length >= length)
            {
                break;
            }
        }
    }
    let Some((end, parsed)) = head else {
        return Err(HttpFault::Connection);
    };
    let body = data[end + 4..].to_vec();
    if parsed
        .content_length
        .is_some_and(|length| body.len() != length)
    {
        return Err(HttpFault::Connection);
    }
    Ok((parsed.status, body))
}

/// One readiness check of one service.
///
/// The result code is one of `ok`, `timeout`, `connection_error`,
/// `body_too_large`, `headers_too_large`, `unsupported_transfer_encoding`,
/// `redirect_rejected`, `http_status`, `invalid_json`, `unexpected_json` or
/// `identity_mismatch`. A body is never copied into the result.
pub fn check_readiness(
    name: &str,
    url: &str,
    expected_environment: &str,
    expected_release_sha: &str,
    timeout: Duration,
    max_body_bytes: usize,
) -> CheckResult {
    let started = Instant::now();
    let result = |category: Category, code: &str| CheckResult::new(name, category, code, started);
    let failure = |code: &str| result(Category::Failure, code);
    let (status, body) = match read_response(url, started + timeout, max_body_bytes) {
        Ok(response) => response,
        Err(HttpFault::Timeout) => return failure("timeout"),
        Err(HttpFault::Class(code)) => return failure(code),
        Err(HttpFault::Connection) => return failure("connection_error"),
    };
    if (300..400).contains(&status) {
        return failure("redirect_rejected");
    }
    if status != 200 {
        return failure("http_status");
    }
    let Some(payload) = parse_strict_json(&body) else {
        return failure("invalid_json");
    };
    match validate_readiness_payload(&payload, expected_environment, expected_release_sha) {
        Ok(()) => result(Category::Healthy, "ok"),
        Err(ReadinessRefusal::Identity) => failure("identity_mismatch"),
        Err(_) => failure("unexpected_json"),
    }
}

// ---- filesystem checks ----------------------------------------------------------

/// Bytes on one volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub total: u64,
    /// Space available to an unprivileged process.
    pub free: u64,
}

/// Space of the volume that holds `path`.
pub fn system_disk_usage(path: &Path) -> io::Result<DiskUsage> {
    let stat = rustix::fs::statvfs(path)?;
    Ok(DiskUsage {
        total: stat.f_blocks.saturating_mul(stat.f_frsize),
        free: stat.f_bavail.saturating_mul(stat.f_frsize),
    })
}

/// Free-space check: at or below `critical_free_percent` is a failure, at or
/// below `warning_free_percent` a warning. The warning level must lie above the
/// critical level. An empty volume counts as 0 percent free.
pub fn check_filesystem(
    name: &str,
    path: &Path,
    warning_free_percent: f64,
    critical_free_percent: f64,
    disk_usage: &dyn Fn(&Path) -> io::Result<DiskUsage>,
) -> CheckResult {
    let started = Instant::now();
    let result = |category: Category, code: &str| CheckResult::new(name, category, code, started);
    if warning_free_percent <= critical_free_percent {
        return result(Category::Failure, "invalid_thresholds");
    }
    let Ok(usage) = disk_usage(path) else {
        return result(Category::Failure, "filesystem_unreadable");
    };
    let free_percent = if usage.total == 0 {
        0.0
    } else {
        usage.free as f64 / usage.total as f64 * 100.0
    };
    if free_percent <= critical_free_percent {
        result(Category::Failure, "disk_free_critical")
    } else if free_percent <= warning_free_percent {
        result(Category::Warning, "disk_free_warning")
    } else {
        result(Category::Healthy, "ok")
    }
}

/// Size check of one directory tree. Links are counted as entries and never
/// followed. The walk stops at `deadline`, after `max_entries` entries, or when
/// the file bytes pass `max_bytes`.
pub fn check_directory_size(
    name: &str,
    path: &Path,
    max_bytes: u64,
    deadline: Instant,
    max_entries: usize,
) -> CheckResult {
    let started = Instant::now();
    let result = |code: &str| CheckResult::new(name, Category::Failure, code, started);
    let (mut total, mut entries) = (0_u64, 0_usize);
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        if Instant::now() >= deadline {
            return result("total_deadline_exceeded");
        }
        let Ok(listing) = std::fs::read_dir(&current) else {
            return result("directory_unreadable");
        };
        for entry in listing {
            let Ok(entry) = entry else {
                return result("directory_unreadable");
            };
            entries += 1;
            if entries > max_entries {
                return result("directory_entry_limit");
            }
            let Ok(kind) = entry.file_type() else {
                return result("directory_unreadable");
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let Ok(metadata) = entry.metadata() else {
                    return result("directory_unreadable");
                };
                total = total.saturating_add(metadata.len());
                if total > max_bytes {
                    return result("directory_size_exceeded");
                }
            }
        }
    }
    CheckResult::new(name, Category::Healthy, "ok", started)
}

// ---- running the checks -------------------------------------------------------------

/// One check, given the time left in the total budget.
pub type Check<'a> = Box<dyn FnOnce(Duration) -> CheckResult + 'a>;

/// Run the checks in order inside one total budget.
///
/// `clock` returns the time since a fixed start. When the budget is spent before
/// a check starts, that check and every later one is replaced by one
/// `total_deadline_exceeded` result. A check that panics becomes an
/// `unexpected_error` result; the panic text is dropped. Results are named
/// `check_<position>` when the check could not name itself.
pub fn execute_checks(
    checks: Vec<Check<'_>>,
    total: Duration,
    clock: &dyn Fn() -> Duration,
) -> Vec<CheckResult> {
    let deadline = clock() + total;
    let mut results = Vec::new();
    for (index, check) in checks.into_iter().enumerate() {
        let position = index + 1;
        let unnamed = |code: &str| CheckResult {
            name: format!("check_{position}"),
            category: Category::Failure,
            code: code.to_owned(),
            duration_ms: 0,
        };
        let left = match deadline.checked_sub(clock()) {
            Some(left) if !left.is_zero() => left,
            _ => {
                results.push(unnamed("total_deadline_exceeded"));
                break;
            }
        };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(left))) {
            Ok(result) => results.push(result),
            Err(_) => results.push(unnamed("unexpected_error")),
        }
    }
    results
}

// ---- configuration and run ------------------------------------------------------------

/// One service to probe and the identity file that says what it must report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: &'static str,
    pub url: String,
    pub expected_environment: &'static str,
    pub identity_file: PathBuf,
}

/// Everything the monitor reads. The default is the installed layout; tests
/// build their own values.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    pub status_file: PathBuf,
    pub endpoints: Vec<Endpoint>,
    pub filesystem: PathBuf,
    pub warning_free_percent: f64,
    pub critical_free_percent: f64,
    pub analytics_directory: Option<PathBuf>,
    pub analytics_max_bytes: u64,
    pub analytics_max_entries: usize,
    pub request_timeout: Duration,
    pub total_timeout: Duration,
    pub max_body_bytes: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        let identity = |environment: &str| {
            PathBuf::from(format!(
                "/etc/verdaxis-monitor/runtime-identities/{environment}.identity"
            ))
        };
        Self {
            status_file: PathBuf::from("/var/lib/verdaxis-health/status.json"),
            endpoints: vec![
                Endpoint {
                    name: "prod_ready",
                    url: "http://127.0.0.1:8000/health/ready".to_owned(),
                    expected_environment: "production",
                    identity_file: identity("production"),
                },
                Endpoint {
                    name: "staging_ready",
                    url: "http://127.0.0.1:8001/health/ready".to_owned(),
                    expected_environment: "staging",
                    identity_file: identity("staging"),
                },
            ],
            filesystem: PathBuf::from("/"),
            warning_free_percent: 20.0,
            critical_free_percent: 15.0,
            analytics_directory: None,
            analytics_max_bytes: 5 * 1024 * 1024 * 1024,
            analytics_max_entries: 100_000,
            request_timeout: Duration::from_secs(5),
            total_timeout: Duration::from_secs(20),
            max_body_bytes: 1024,
        }
    }
}

/// Run every check and build the new status.
///
/// An unsafe previous document is quarantined first and reported as one
/// `previous_status_invalid` failure with no check run. A bad identity file
/// refuses the whole run before any request is sent.
pub fn collect(config: &HealthConfig, now: DateTime<Utc>) -> Result<Status, Fault> {
    let previous = load_monitor_status(&config.status_file, MAX_STATUS_BYTES);
    if previous == Previous::Malformed {
        quarantine(&config.status_file).map_err(|_| Fault)?;
        return Ok(failure_status("previous_status_invalid", now, MONITOR_NAME));
    }
    let identities: Vec<RuntimeIdentity> = config
        .endpoints
        .iter()
        .map(|endpoint| read_identity(&endpoint.identity_file, endpoint.expected_environment))
        .collect::<Result<_, _>>()
        .map_err(|_| Fault)?;

    let started = Instant::now();
    let mut checks: Vec<Check<'_>> = Vec::new();
    for (endpoint, identity) in config.endpoints.iter().zip(identities) {
        checks.push(Box::new(move |left| {
            check_readiness(
                endpoint.name,
                &endpoint.url,
                endpoint.expected_environment,
                &identity.release_sha,
                config.request_timeout.min(left),
                config.max_body_bytes,
            )
        }));
    }
    checks.push(Box::new(|_left| {
        check_filesystem(
            "root_disk",
            &config.filesystem,
            config.warning_free_percent,
            config.critical_free_percent,
            &system_disk_usage,
        )
    }));
    if let Some(directory) = &config.analytics_directory {
        let deadline = started + config.total_timeout;
        checks.push(Box::new(move |_left| {
            check_directory_size(
                "analytics_directory",
                directory,
                config.analytics_max_bytes,
                deadline,
                config.analytics_max_entries,
            )
        }));
    }
    let budget = config.total_timeout.max(Duration::from_millis(1));
    let results = execute_checks(checks, budget, &|| started.elapsed());
    Ok(build_status(results, &previous, now))
}

/// Collect, admit and publish one status. Returns 0 for a healthy status, 2 for a
/// warning or failure, and the admission or write error when the status could
/// not be published.
pub fn run(config: &HealthConfig, admission: &dyn Admission) -> Exit {
    run_at(config, admission, Utc::now())
}

/// [`run`] with the observation time given, for tests that use a fixed clock.
pub fn run_at(config: &HealthConfig, admission: &dyn Admission, now: DateTime<Utc>) -> Exit {
    let status = guarded(MONITOR_NAME, now, || collect(config, now));
    match publish(&config.status_file, &status, admission) {
        Ok(()) => {
            report(COMMAND, &status);
            exit_for(status.category)
        }
        Err(exit) => {
            report_refused(COMMAND);
            exit
        }
    }
}

/// The installed command: default layout, no journal yet.
pub fn run_default() -> Exit {
    run(&HealthConfig::default(), &AdmitAll)
}
