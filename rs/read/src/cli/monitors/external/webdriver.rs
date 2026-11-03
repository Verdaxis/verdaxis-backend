//! The rendered login page check: ChromeDriver through its W3C HTTP interface.
//!
//! Each attempt starts ChromeDriver on a free loopback port in its own
//! process group, with a fixed environment and a private profile directory,
//! opens one headless session, loads the page without waiting for it, and
//! polls one fixed script until the login controls and the required texts
//! are visible, a forbidden text appears, a Vercel Security Checkpoint is
//! shown, or the attempt's deadline passes. The session, the driver's whole
//! process group and the profile directory are always removed.
//!
//! The control channel is loopback only and never uses a proxy. No page
//! content leaves the check: failures name the missing control or text.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{ExternalConfig, PageCheck, log};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_TIMEOUT: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const RENDER_POLL_INTERVAL: Duration = Duration::from_millis(250);
const STOP_WAIT: Duration = Duration::from_secs(2);

/// The fixed script the page runs. It reports only booleans.
pub const RENDER_STATE_SCRIPT: &str = r#"
const isVisible = (element) => {
  if (!element) return false;
  const style = window.getComputedStyle(element);
  const bounds = element.getBoundingClientRect();
  return style.display !== "none" &&
    style.visibility !== "hidden" &&
    Number(style.opacity) !== 0 &&
    bounds.width > 0 && bounds.height > 0;
};
const email = document.querySelector(
  'input[type="email"], input[autocomplete="username"]'
);
const password = document.querySelector('input[type="password"]');
const loginForm = password ? password.form : null;
const submitSelector = 'button[type="submit"], input[type="submit"], button:not([type])';
const submit = loginForm ? loginForm.querySelector(submitSelector) : null;
const markup = document.documentElement
  ? document.documentElement.outerHTML.toLowerCase()
  : "";
const contains = (needle) => markup.includes(String(needle).toLowerCase());
return {
  emailVisible: isVisible(email),
  passwordVisible: isVisible(password),
  submitVisible: isVisible(submit),
  requiredMatches: arguments[0].map(contains),
  forbiddenMatches: arguments[1].map(contains),
  securityCheckpoint: [
    "vercel security checkpoint",
    "we're verifying your browser",
    "we are verifying your browser"
  ].some(contains) || markup.includes("x-vercel-challenge-token")
};
"#;

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub chromedriver: Option<PathBuf>,
    pub profile_parent: PathBuf,
    pub home: PathBuf,
    pub lang: Option<String>,
    pub lc_all: Option<String>,
    pub timeout_seconds: u64,
    pub retries: u32,
    pub retry_delay: Duration,
}

/// Why an attempt ended without a page verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptError {
    TimedOut,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    SecurityCheckpoint,
    Failed(Vec<String>),
}

fn failure(message: &str) -> AttemptError {
    AttemptError::Failed(message.to_owned())
}

/// `CHROMEDRIVER_BIN`, else the snap driver, else the first
/// `chromium.chromedriver` or `chromedriver` on `PATH`.
pub fn find_chromedriver(settings: &RenderSettings, path: Option<&str>) -> Option<PathBuf> {
    if let Some(configured) = &settings.chromedriver {
        return Some(configured.clone());
    }
    let snap = Path::new("/snap/bin/chromium.chromedriver");
    if snap.is_file() {
        return Some(snap.to_path_buf());
    }
    use std::os::unix::fs::PermissionsExt;
    for name in ["chromium.chromedriver", "chromedriver"] {
        for directory in path
            .unwrap_or("")
            .split(':')
            .filter(|part| !part.is_empty())
        {
            let candidate = Path::new(directory).join(name);
            let executable = std::fs::metadata(&candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            });
            if executable {
                return Some(candidate);
            }
        }
    }
    None
}

fn control_client(timeout: Duration) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .ok()
}

/// One W3C request. Errors carry ChromeDriver's status or error name only.
pub async fn webdriver_request(
    port: u16,
    method: reqwest::Method,
    path: &str,
    payload: Option<&Value>,
    timeout: Duration,
) -> Result<Value, AttemptError> {
    let control_failed = || failure("ChromeDriver control request failed");
    let client = control_client(timeout).ok_or_else(control_failed)?;
    let mut request = client
        .request(method, format!("http://127.0.0.1:{port}{path}"))
        .header("content-type", "application/json");
    if let Some(payload) = payload {
        request = request.body(payload.to_string());
    }
    let response = request.send().await.map_err(|_| control_failed())?;
    let status = response.status();
    if !status.is_success() {
        return Err(AttemptError::Failed(format!(
            "ChromeDriver returned HTTP {}",
            status.as_u16()
        )));
    }
    let bytes = response.bytes().await.map_err(|_| control_failed())?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| control_failed())?;
    let Some(value) = document.as_object().and_then(|object| object.get("value")) else {
        return Err(failure("ChromeDriver returned an invalid response"));
    };
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(AttemptError::Failed(format!(
            "ChromeDriver reported {error}"
        )));
    }
    Ok(value.clone())
}

fn unused_loopback_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .ok()
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// The driver's process group: SIGTERM, a short wait, then SIGKILL for any
/// browser that outlived it.
async fn stop_driver(child: &mut tokio::process::Child) {
    let Some(pid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: killpg only sends a signal to the group this attempt created.
    unsafe { libc::killpg(pid, libc::SIGTERM) };
    let _ = tokio::time::timeout(STOP_WAIT, child.wait()).await;
    // SAFETY: as above; a group that is already gone answers ESRCH.
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    let _ = tokio::time::timeout(STOP_WAIT, child.wait()).await;
}

/// A private profile directory under the configured parent, removed on drop.
struct Profile(PathBuf);

impl Profile {
    fn create(parent: &Path) -> Option<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = parent.join(format!(
            "verdaxis-monitor-chrome-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&path).ok()?;
        Some(Self(path))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session_capabilities(user_data: &Path, timeout_seconds: u64) -> Value {
    json!({"capabilities": {"alwaysMatch": {
        "browserName": "chrome",
        "pageLoadStrategy": "none",
        "timeouts": {"implicit": 0, "pageLoad": timeout_seconds * 1_000, "script": 5_000},
        "goog:chromeOptions": {"args": [
            "--headless",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--disable-extensions",
            "--disable-sync",
            "--run-all-compositor-stages-before-draw",
            format!("--user-data-dir={}", user_data.display()),
        ]},
    }}})
}

/// The verdict for one render state. A state of the wrong shape is a failure.
pub fn rendered_page_violations(
    check: &PageCheck,
    state: &Value,
) -> Result<Vec<String>, AttemptError> {
    let invalid = || failure("ChromeDriver returned an invalid render state");
    let flag = |key: &str| state.get(key).and_then(Value::as_bool).ok_or_else(invalid);
    let flags = |key: &str, length: usize| -> Result<Vec<bool>, AttemptError> {
        let items = state
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        if items.len() != length {
            return Err(invalid());
        }
        items
            .iter()
            .map(|item| item.as_bool().ok_or_else(invalid))
            .collect()
    };
    let (email, password, submit) = (
        flag("emailVisible")?,
        flag("passwordVisible")?,
        flag("submitVisible")?,
    );
    let required = flags("requiredMatches", check.must_contain.len())?;
    let forbidden = flags("forbiddenMatches", check.must_not_contain.len())?;
    let mut violations = Vec::new();
    if !email {
        violations.push("missing visible email input".to_owned());
    }
    if !password {
        violations.push("missing visible password input".to_owned());
    }
    if !submit {
        violations.push("missing visible submit control".to_owned());
    }
    for (needle, matched) in check.must_contain.iter().zip(required) {
        if !matched {
            violations.push(format!("missing required text {}", super::quoted(needle)));
        }
    }
    for (needle, matched) in check.must_not_contain.iter().zip(forbidden) {
        if matched {
            violations.push(format!("contains forbidden text {}", super::quoted(needle)));
        }
    }
    Ok(violations)
}

pub async fn render_attempt(
    check: &PageCheck,
    chromedriver: &Path,
    settings: &RenderSettings,
) -> Result<Outcome, AttemptError> {
    let deadline = Instant::now() + Duration::from_secs(settings.timeout_seconds);
    let port = unused_loopback_port().ok_or_else(|| failure("ChromeDriver could not start"))?;
    let profile = Profile::create(&settings.profile_parent)
        .ok_or_else(|| failure("ChromeDriver could not start"))?;
    let mut child = tokio::process::Command::new(chromedriver)
        .arg(format!("--port={port}"))
        .arg("--allowed-ips=127.0.0.1")
        .env_clear()
        .env("HOME", &settings.home)
        .env("LANG", settings.lang.as_deref().unwrap_or("C.UTF-8"))
        .env("LC_ALL", settings.lc_all.as_deref().unwrap_or("C.UTF-8"))
        .env("PATH", "/bin:/usr/bin")
        .env("TMPDIR", &profile.0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| failure("ChromeDriver could not start"))?;
    let mut session: Option<String> = None;
    let result = drive(
        check,
        port,
        settings,
        deadline,
        &profile.0,
        &mut child,
        &mut session,
    )
    .await;
    if let Some(id) = session {
        let path = format!("/session/{}", urlencoding_path(&id));
        let _ = webdriver_request(port, reqwest::Method::DELETE, &path, None, STOP_WAIT).await;
    }
    stop_driver(&mut child).await;
    drop(profile);
    result
}

/// A session id as one path segment (Python's `quote(id, safe="")`).
fn urlencoding_path(id: &str) -> String {
    id.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"_.-~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn drive(
    check: &PageCheck,
    port: u16,
    settings: &RenderSettings,
    deadline: Instant,
    profile: &Path,
    child: &mut tokio::process::Child,
    session: &mut Option<String>,
) -> Result<Outcome, AttemptError> {
    // Wait for the driver to report ready.
    loop {
        if Instant::now() >= deadline {
            return Err(AttemptError::TimedOut);
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Err(failure("ChromeDriver exited during startup"));
        }
        if let Ok(status) =
            webdriver_request(port, reqwest::Method::GET, "/status", None, STATUS_TIMEOUT).await
            && status.get("ready") == Some(&Value::Bool(true))
        {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let created = webdriver_request(
        port,
        reqwest::Method::POST,
        "/session",
        Some(&session_capabilities(
            &profile.join("user-data"),
            settings.timeout_seconds,
        )),
        remaining(deadline).max(Duration::from_millis(100)),
    )
    .await?;
    let id = created
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| failure("ChromeDriver did not create a session"))?;
    *session = Some(id.to_owned());
    let session_path = format!("/session/{}", urlencoding_path(id));
    webdriver_request(
        port,
        reqwest::Method::POST,
        &format!("{session_path}/url"),
        Some(&json!({"url": check.url})),
        remaining(deadline).clamp(Duration::from_millis(100), Duration::from_secs(10)),
    )
    .await?;
    let script = json!({
        "script": RENDER_STATE_SCRIPT,
        "args": [check.must_contain, check.must_not_contain],
    });
    let mut last: Option<Vec<String>> = None;
    while Instant::now() < deadline {
        let state = webdriver_request(
            port,
            reqwest::Method::POST,
            &format!("{session_path}/execute/sync"),
            Some(&script),
            remaining(deadline).clamp(Duration::from_millis(100), REQUEST_TIMEOUT),
        )
        .await?;
        if !state.is_object() {
            return Err(failure("ChromeDriver returned an invalid render state"));
        }
        if state.get("securityCheckpoint") == Some(&Value::Bool(true)) {
            return Ok(Outcome::SecurityCheckpoint);
        }
        let violations = rendered_page_violations(check, &state)?;
        if violations.is_empty() {
            return Ok(Outcome::Ok);
        }
        if violations
            .iter()
            .any(|violation| violation.starts_with("contains forbidden text"))
        {
            return Ok(Outcome::Failed(violations));
        }
        last = Some(violations);
        tokio::time::sleep(remaining(deadline).min(RENDER_POLL_INTERVAL)).await;
    }
    match last {
        Some(violations) => Ok(Outcome::Failed(violations)),
        None => Err(AttemptError::TimedOut),
    }
}

pub async fn check_rendered_pages(config: &ExternalConfig) -> Vec<String> {
    if !config.rendered_checks_enabled {
        return Vec::new();
    }
    let settings = &config.render;
    let path = std::env::var("PATH").ok();
    let Some(chromedriver) = find_chromedriver(settings, path.as_deref()) else {
        return vec!["rendered page checks enabled but no ChromeDriver binary was found".into()];
    };
    if std::fs::create_dir_all(&settings.profile_parent).is_err() {
        return vec!["rendered page checks could not create the browser profile directory".into()];
    }
    let mut errors = Vec::new();
    for check in &config.rendered_pages {
        let mut final_error: Option<String> = None;
        for attempt in 1..=settings.retries {
            final_error = match render_attempt(check, &chromedriver, settings).await {
                Err(AttemptError::TimedOut) => Some(format!(
                    "{} render timed out after {}s",
                    check.name, settings.timeout_seconds
                )),
                Err(AttemptError::Failed(reason)) => {
                    Some(format!("{} render failed: {reason}", check.name))
                }
                Ok(Outcome::SecurityCheckpoint) => {
                    log(&format!(
                        "{} skipped: Vercel returned Security Checkpoint to monitor client",
                        check.name
                    ));
                    None
                }
                Ok(Outcome::Ok) => None,
                Ok(Outcome::Failed(violations)) => Some(format!(
                    "{} rendered page {}",
                    check.name,
                    violations.join("; ")
                )),
            };
            if final_error.is_none() {
                break;
            }
            if attempt < settings.retries {
                log(&format!(
                    "{} render attempt {attempt}/{} failed; retrying: {}",
                    check.name,
                    settings.retries,
                    final_error.as_deref().unwrap_or("")
                ));
                tokio::time::sleep(settings.retry_delay).await;
            }
        }
        errors.extend(final_error);
    }
    errors
}
