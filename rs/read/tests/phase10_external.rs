//! Phase 10 Task 29: `monitor external`, against loopback fakes.
//!
//! Every public target is replaced by a fake HTTP server on 127.0.0.1, the
//! Caddy and systemctl programs by recording scripts, and ChromeDriver by a
//! small W3C fake (Python, from `RUST_TEST_PYTHON` or `python3`). Nothing
//! leaves the machine.
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::monitors::external::{
    self, Endpoint, ExternalConfig, PageCheck, check_backup_status, check_caddyfile,
    check_endpoint, check_restore_status, check_units, demo_trade_canary, directory_file_bytes,
    http_client, load_cached_expensive_checks, python_fromisoformat, run_with, signup_canary,
};

const SECRET: &str = "SECRET-sentinel-4b1d";

// ---- fixtures -----------------------------------------------------------------

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "verdaxis-p10-external-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// An executable script with this body.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A recorded request: method, path with query, headers and body.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: HeaderMap,
    body: String,
}

type Responder = Arc<dyn Fn(&Seen) -> Response + Send + Sync>;

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    respond: Responder,
}

/// A loopback HTTP server that records every request and answers with
/// `respond`. Returns its base URL and the record.
async fn fake(
    respond: impl Fn(&Seen) -> Response + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let state = Fake {
        seen: seen.clone(),
        respond: Arc::new(respond),
    };
    async fn handle(State(state): State<Fake>, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        let seen = Seen {
            path: parts
                .uri
                .path_and_query()
                .map_or("/".into(), |p| p.to_string()),
            headers: parts.headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        };
        state.seen.lock().unwrap().push(seen.clone());
        (state.respond)(&seen)
    }
    let app = Router::new().fallback(handle).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen)
}

fn ok(body: &str) -> Response {
    (StatusCode::OK, body.to_owned()).into_response()
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 4, 1, 0, 0).unwrap()
}

/// A configuration whose every external piece is disabled or local: the
/// caller turns on what its case needs.
fn base_config(scratch: &Scratch) -> ExternalConfig {
    let vars: HashMap<String, String> = [
        ("STATE_FILE", scratch.path("state/state.json")),
        ("STATUS_FILE", scratch.path("state/status.json")),
        ("CADDYFILE", scratch.path("Caddyfile")),
        ("BACKUP_STATUS_FILE", scratch.path("backups/status.json")),
        ("ANALYTICS_DATA_DIR", scratch.path("analytics")),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_string_lossy().into_owned()))
    .chain(
        [
            ("HTTP_RETRY_DELAY_SECONDS", "0"),
            ("HTTP_TIMEOUT_SECONDS", "5"),
            ("RENDER_RETRY_DELAY_SECONDS", "0"),
            ("MIN_DISK_FREE_PERCENT", "0"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned())),
    )
    .collect();
    let mut config = ExternalConfig::from_environment(&vars, &scratch.0, false).unwrap();
    config.endpoints = Vec::new();
    config.bundle_checks = Vec::new();
    config.rendered_checks_enabled = false;
    config.signup_canary_enabled = false;
    config.demo_trade_canary_enabled = false;
    config.watched_units = Vec::new();
    config.telegram_disabled = true;
    // A Caddy that validates and names every required host.
    let hosts: Vec<Value> = external::REQUIRED_HOSTS
        .iter()
        .map(|host| json!(host))
        .collect();
    let adapted = json!({"apps": {"http": {"servers": {"srv0": {"routes": [{"match": [{"host": hosts}]}]}}}}});
    let caddy = scratch.script(
        "caddy",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\ncat <<'EOF'\n{adapted}\nEOF\n",
            scratch.path("caddy.argv").display()
        ),
    );
    config.caddy_command = vec![caddy.to_string_lossy().into_owned()];
    std::fs::write(scratch.path("Caddyfile"), "import sites/*\n").unwrap();
    std::fs::create_dir_all(scratch.path("analytics")).unwrap();
    write_backups(scratch, now(), true);
    config
}

/// A complete backup set and restore record at `at`.
fn write_backups(scratch: &Scratch, at: DateTime<Utc>, valid_gzip: bool) {
    use std::io::Write;
    let directory = scratch.path("backups");
    std::fs::create_dir_all(&directory).unwrap();
    let id = "20261104-005500";
    std::fs::write(
        directory.join("status.json"),
        json!({
            "ok": true,
            "databases": ["verdaxis", "verdaxis_staging", "umami"],
            "completed_at": at.format("%Y-%m-%dT%H:%M:%S+00:00").to_string(),
            "backup_id": id,
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        directory.join("restore-status.json"),
        json!({
            "ok": true,
            "completed_at": at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "backup_id": id,
            "databases": ["verdaxis", "verdaxis_staging", "umami"],
        })
        .to_string(),
    )
    .unwrap();
    for prefix in ["verdaxis", "verdaxis-staging", "umami"] {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"-- dump\n").unwrap();
        let mut bytes = encoder.finish().unwrap();
        if !valid_gzip {
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
        }
        std::fs::write(directory.join(format!("{prefix}-{id}.sql.gz")), bytes).unwrap();
    }
}

fn endpoint(name: &str, url: String, contains: Option<&str>, vercel: bool) -> Endpoint {
    Endpoint {
        name: name.into(),
        url,
        status: 200,
        contains: contains.map(Into::into),
        allow_vercel_challenge: vercel,
    }
}

fn status_file(config: &ExternalConfig) -> Value {
    serde_json::from_str(&std::fs::read_to_string(&config.status_file).unwrap()).unwrap()
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

// ---- named cases --------------------------------------------------------------

#[tokio::test]
async fn external_monitor_checks_route_uptime_database_outbox_and_backup() {
    let scratch = Scratch::new();
    let (api, _) = fake(|seen| match seen.path.as_str() {
        "/health" => ok(r#"{"status":"ok","db":"ok"}"#),
        "/down" => (StatusCode::SERVICE_UNAVAILABLE, "maintenance").into_response(),
        _ => ok("<html></html>"),
    })
    .await;
    let mut config = base_config(&scratch);
    config.endpoints = vec![
        endpoint(
            "verdaxis api",
            format!("{api}/health"),
            Some("\"status\":\"ok\""),
            false,
        ),
        endpoint("verdaxis landing", format!("{api}/"), None, true),
    ];
    let clock = || now();
    assert_eq!(run_with(&config, &clock).await, Exit::Ok);
    let status = status_file(&config);
    assert_eq!(status["ok"], true);
    assert_eq!(status["endpoints"][0]["http_status"], 200);
    assert_eq!(status["expensive_checks"]["errors"], json!([]));

    // A route down and stale backup evidence both fail the run (exit 2).
    let scratch = Scratch::new();
    let mut config = base_config(&scratch);
    config.http_retries = 2;
    config.endpoints = vec![endpoint(
        "verdaxis staging api",
        format!("{api}/down"),
        None,
        false,
    )];
    write_backups(&scratch, now() - chrono::Duration::days(9), true);
    assert_eq!(run_with(&config, &clock).await, Exit::Usage, "exit 2");
    let errors: Vec<String> =
        serde_json::from_value(status_file(&config)["errors"].clone()).unwrap();
    assert_eq!(
        errors,
        [
            "verdaxis staging api returned HTTP 503, expected 200",
            "database backup is stale (216 hours old)",
            "restore verification is stale",
        ]
    );
    assert_eq!(
        status_file(&config)["endpoints"][0]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // The installed monitor has no outbox probe and no database connection:
    // its configuration names neither.
    let source = include_str!("../src/cli/monitors/external.rs");
    for absent in ["outbox", "PgPool", "sqlx"] {
        assert!(!source.contains(absent), "{absent}");
    }
}

#[tokio::test]
async fn external_monitor_signup_canary_creates_then_cleans_up() {
    let scratch = Scratch::new();
    let mut config = base_config(&scratch);
    config.monitor_token = Some(SECRET.into());
    let client = http_client().unwrap();
    // Healthy: 202 check_email, then a cleanup that deletes nothing.
    let (api, seen) = fake(|seen| match seen.path.as_str() {
        "/auth/register" => (StatusCode::ACCEPTED, r#"{"status":"check_email"}"#).into_response(),
        _ => ok(r#"{"deleted_users":0,"deleted_orgs":0}"#),
    })
    .await;
    assert_eq!(
        signup_canary(&client, &config, "staging", &api, now()).await,
        None
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        ["/auth/register", "/monitor/signup-canary-cleanup"]
    );
    let stamp = now().timestamp();
    let email = format!("canary+staging-{stamp}@staging-{stamp}.canary.verdaxis.exchange");
    let register: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(register["email"], email);
    assert_eq!(register["role"], "BUYER");
    assert_eq!(
        serde_json::from_str::<Value>(&seen[1].body).unwrap(),
        json!({"email": email})
    );
    assert!(
        seen.iter()
            .all(|request| request.headers["x-monitor-token"] == SECRET)
    );

    // A failed registration still cleans up; a leftover identity is a failure.
    let (api, seen) = fake(|seen| match seen.path.as_str() {
        "/auth/register" => (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response(),
        _ => ok(r#"{"deleted_users":1,"deleted_orgs":1}"#),
    })
    .await;
    assert_eq!(
        signup_canary(&client, &config, "prod", &api, now())
            .await
            .as_deref(),
        Some("prod signup canary register returned HTTP 500")
    );
    assert_eq!(seen.lock().unwrap().len(), 2, "cleanup ran");
    let (api, _) = fake(|seen| match seen.path.as_str() {
        "/auth/register" => (StatusCode::ACCEPTED, r#"{"status":"check_email"}"#).into_response(),
        _ => ok(r#"{"deleted_users":1,"deleted_orgs":0}"#),
    })
    .await;
    assert_eq!(
        signup_canary(&client, &config, "prod", &api, now())
            .await
            .as_deref(),
        Some("prod signup canary cleanup reported an undeleted identity")
    );
    for body in [
        r#"{"deleted_users":true,"deleted_orgs":0}"#,
        r#"{"deleted_users":2,"deleted_orgs":0}"#,
        "[]",
    ] {
        let body = body.to_owned();
        let (api, _) = fake(move |seen| match seen.path.as_str() {
            "/auth/register" => {
                (StatusCode::ACCEPTED, r#"{"status":"check_email"}"#).into_response()
            }
            _ => ok(&body),
        })
        .await;
        assert_eq!(
            signup_canary(&client, &config, "prod", &api, now())
                .await
                .as_deref(),
            Some("prod signup canary cleanup returned invalid deletion counts")
        );
    }
    config.monitor_token = None;
    assert_eq!(
        signup_canary(&client, &config, "prod", &api, now())
            .await
            .as_deref(),
        Some("prod signup canary missing MONITOR_TOKEN")
    );
}

#[tokio::test]
async fn external_monitor_telegram_disabled_suppresses_alerts() {
    let (telegram, sent) = fake(|_| ok(r#"{"ok":true}"#)).await;
    let (api, _) = fake(|_| (StatusCode::BAD_GATEWAY, "x").into_response()).await;
    let run = |disabled: bool, scratch: &Scratch| {
        let mut config = base_config(scratch);
        config.http_retries = 1;
        config.endpoints = vec![endpoint("verdaxis app", format!("{api}/app"), None, false)];
        config.telegram_disabled = disabled;
        config.telegram_token = Some(SECRET.into());
        config.telegram_chat_id = Some("42".into());
        config.telegram_base = telegram.clone();
        config
    };
    let clock = || now();
    let scratch = Scratch::new();
    assert_eq!(run_with(&run(true, &scratch), &clock).await, Exit::Usage);
    assert!(sent.lock().unwrap().is_empty(), "verify mode sends nothing");
    // The state still records the failure.
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(scratch.path("state/state.json")).unwrap())
            .unwrap();
    assert_eq!(state["ok"], false);

    let scratch = Scratch::new();
    let config = run(false, &scratch);
    assert_eq!(run_with(&config, &clock).await, Exit::Usage);
    assert_eq!(sent.lock().unwrap().len(), 1, "a new failure alerts once");
    let message = sent.lock().unwrap()[0].clone();
    assert_eq!(message.path, format!("/bot{SECRET}/sendMessage"));
    assert!(message.body.starts_with(
        "chat_id=42&text=Verdaxis+monitor+failure%3A%0A-+verdaxis+app+returned+HTTP+502"
    ));
    // Within the cooldown a repeated failure does not alert again.
    let later = || now() + chrono::Duration::minutes(10);
    assert_eq!(run_with(&config, &later).await, Exit::Usage);
    assert_eq!(sent.lock().unwrap().len(), 1);
    // After the cooldown it does.
    let after = || now() + chrono::Duration::hours(2);
    run_with(&config, &after).await;
    assert_eq!(sent.lock().unwrap().len(), 2);
    // The command-line flag disables it as TELEGRAM_DISABLED=1 does.
    let vars: HashMap<String, String> = HashMap::new();
    assert!(
        ExternalConfig::from_environment(&vars, Path::new("/tmp"), true)
            .unwrap()
            .telegram_disabled
    );
    let vars: HashMap<String, String> = [("TELEGRAM_DISABLED".to_owned(), "Yes".to_owned())].into();
    assert!(
        ExternalConfig::from_environment(&vars, Path::new("/tmp"), false)
            .unwrap()
            .telegram_disabled
    );
}

#[tokio::test]
async fn external_monitor_expensive_checks_are_cached_for_thirty_minutes() {
    let scratch = Scratch::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let (api, _) = fake(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        (StatusCode::NOT_FOUND, "x").into_response()
    })
    .await;
    let mut config = base_config(&scratch);
    config.bundle_checks = vec![PageCheck {
        name: "verdaxis app production bundle".into(),
        url: format!("{api}/login?lang=en"),
        must_contain: vec![],
        must_not_contain: vec![],
    }];
    let first = || now();
    run_with(&config, &first).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let cached = status_file(&config)["expensive_checks"].clone();
    assert_eq!(cached["checked_at"], now().timestamp());
    assert_eq!(
        cached["errors"][0],
        "verdaxis app production bundle page returned HTTP 404, expected 200"
    );
    // 29 minutes later the cached result is reused, errors included.
    let inside = || now() + chrono::Duration::minutes(29);
    assert_eq!(run_with(&config, &inside).await, Exit::Usage);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(status_file(&config)["expensive_checks"], cached);
    // At 30 minutes the checks run again.
    let due = || now() + chrono::Duration::minutes(30);
    run_with(&config, &due).await;
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    // A future, boolean or malformed record is not a cache.
    for record in [
        json!({"checked_at": now().timestamp() + 60, "errors": []}),
        json!({"checked_at": true, "errors": []}),
        json!({"checked_at": now().timestamp(), "errors": [1]}),
    ] {
        external::save_state(&config.status_file, &json!({"expensive_checks": record})).unwrap();
        assert!(
            load_cached_expensive_checks(&config, now().timestamp()).is_none(),
            "{record}"
        );
    }
    // The interval has a 300 s floor and an unparsable value disables the cache.
    let vars: HashMap<String, String> = [(
        "EXPENSIVE_CHECK_INTERVAL_SECONDS".to_owned(),
        "10".to_owned(),
    )]
    .into();
    assert_eq!(
        ExternalConfig::from_environment(&vars, &scratch.0, false)
            .unwrap()
            .expensive_interval_seconds,
        Some(300)
    );
    let vars: HashMap<String, String> = [(
        "EXPENSIVE_CHECK_INTERVAL_SECONDS".to_owned(),
        "soon".to_owned(),
    )]
    .into();
    assert_eq!(
        ExternalConfig::from_environment(&vars, &scratch.0, false)
            .unwrap()
            .expensive_interval_seconds,
        None
    );
}

#[tokio::test]
async fn external_monitor_classifies_vercel_challenges_without_an_alert() {
    let scratch = Scratch::new();
    let (edge, _) = fake(|_| {
        (
            StatusCode::FORBIDDEN,
            [("server", "Vercel"), ("x-vercel-mitigated", "challenge")],
            "<title>Vercel Security Checkpoint</title>",
        )
            .into_response()
    })
    .await;
    let mut config = base_config(&scratch);
    config.http_retries = 1;
    let client = http_client().unwrap();
    let allowed = endpoint("verdaxis landing", format!("{edge}/"), None, true);
    let (error, status) = check_endpoint(&client, &config, &allowed).await;
    assert_eq!(error, None);
    assert_eq!(status["ok"], true);
    assert_eq!(status["attempts"][0]["note"], "vercel_security_challenge");
    assert_eq!(
        status["note"],
        "Vercel edge is reachable but returned Security Checkpoint to the monitor client"
    );
    let strict = endpoint("verdaxis staging", format!("{edge}/"), None, false);
    assert_eq!(
        check_endpoint(&client, &config, &strict).await.0.as_deref(),
        Some("verdaxis staging returned HTTP 403, expected 200")
    );
    // The bundle check skips a checkpoint page; the run stays healthy.
    config.endpoints = vec![allowed];
    config.bundle_checks = vec![PageCheck {
        name: "verdaxis app production bundle".into(),
        url: format!("{edge}/login"),
        must_contain: vec![],
        must_not_contain: vec![],
    }];
    let clock = || now();
    assert_eq!(run_with(&config, &clock).await, Exit::Ok);
}

#[tokio::test]
async fn external_monitor_state_and_status_keep_existing_keys() {
    let scratch = Scratch::new();
    let (api, _) = fake(|seen| match seen.path.as_str() {
        "/fail" => (StatusCode::BAD_GATEWAY, "x").into_response(),
        _ => ok("ok"),
    })
    .await;
    let mut config = base_config(&scratch);
    config.http_retries = 1;
    config.endpoints = vec![endpoint("verdaxis app", format!("{api}/app"), None, false)];
    let clock = || now();
    assert_eq!(run_with(&config, &clock).await, Exit::Ok);
    let status = status_file(&config);
    assert_eq!(
        keys(&status),
        [
            "alert_cooldown_seconds",
            "checked_at",
            "checked_at_utc",
            "endpoints",
            "errors",
            "expensive_checks",
            "ok"
        ]
    );
    assert_eq!(status["checked_at_utc"], "2026-11-04T01:00:00Z");
    assert_eq!(status["alert_cooldown_seconds"], 3600);
    assert_eq!(
        keys(&status["endpoints"][0]),
        [
            "attempts",
            "expected_status",
            "http_status",
            "name",
            "ok",
            "url"
        ]
    );
    assert_eq!(
        keys(&status["expensive_checks"]),
        ["checked_at", "checked_at_utc", "errors", "ok"]
    );
    let raw = std::fs::read_to_string(&config.status_file).unwrap();
    assert!(raw.starts_with("{\n  \"alert_cooldown_seconds\": 3600,\n") && raw.ends_with("}\n"));
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(&config.state_file).unwrap()).unwrap();
    assert_eq!(keys(&state), ["last_alert_at", "last_checked_at", "ok"]);
    config.endpoints = vec![endpoint("verdaxis app", format!("{api}/fail"), None, false)];
    run_with(&config, &clock).await;
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(&config.state_file).unwrap()).unwrap();
    assert_eq!(
        keys(&state),
        ["errors", "last_alert_at", "last_checked_at", "ok"]
    );
    assert_eq!(state["last_alert_at"], now().timestamp());
    assert_eq!(
        keys(&status_file(&config)["endpoints"][0]),
        [
            "attempts",
            "error",
            "expected_status",
            "http_status",
            "name",
            "ok",
            "url"
        ]
    );
    // A Python-written state is read: its failure and alert time carry over.
    std::fs::write(
        &config.state_file,
        r#"{"ok": false, "last_alert_at": 1762217000.0, "errors": []}"#,
    )
    .unwrap();
    let later = || now() + chrono::Duration::minutes(1);
    run_with(&config, &later).await;
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(&config.state_file).unwrap()).unwrap();
    assert_eq!(
        state["last_alert_at"],
        now().timestamp() + 60,
        "1762217000 is older than the cooldown"
    );
}

#[tokio::test]
async fn external_monitor_never_shells_out_with_interpolation() {
    let scratch = Scratch::new();
    let mut config = base_config(&scratch);
    // A Caddyfile path with shell syntax reaches Caddy as one argument.
    let hostile = scratch.path("Caddy file; touch pwned $(id)");
    std::fs::write(&hostile, "import x\n").unwrap();
    config.caddyfile = hostile.clone();
    assert_eq!(check_caddyfile(&config).await, Vec::<String>::new());
    let argv = std::fs::read_to_string(scratch.path("caddy.argv")).unwrap();
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        ["adapt", "--validate", "--config", hostile.to_str().unwrap()]
    );
    assert!(!scratch.path("pwned").exists());
    // The source starts programs only from argument vectors.
    for source in [
        include_str!("../src/cli/monitors/external.rs"),
        include_str!("../src/cli/monitors/external/webdriver.rs"),
    ] {
        for forbidden in ["\"sh\"", "/bin/sh", "bash", "\"-c\"", "shell(", "docker"] {
            assert!(!source.contains(forbidden), "{forbidden}");
        }
    }
}

#[tokio::test]
async fn external_monitor_redacts_bodies_secrets_and_headers() {
    let scratch = Scratch::new();
    let (api, _) = fake(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("set-cookie", format!("session={SECRET}"))],
            format!("traceback with {SECRET}"),
        )
            .into_response()
    })
    .await;
    let (telegram, sent) =
        fake(|_| (StatusCode::BAD_REQUEST, format!("echo {SECRET}")).into_response()).await;
    let mut config = base_config(&scratch);
    config.http_retries = 1;
    config.endpoints = vec![endpoint(
        "verdaxis api",
        format!("{api}/health"),
        Some("\"status\":\"ok\""),
        false,
    )];
    config.monitor_token = Some(SECRET.into());
    config.signup_canary_enabled = true;
    config.canary_targets = vec![("staging".into(), api.clone())];
    config.telegram_disabled = false;
    config.telegram_token = Some(SECRET.into());
    config.telegram_chat_id = Some("42".into());
    config.telegram_base = telegram;
    let clock = || now();
    assert_eq!(run_with(&config, &clock).await, Exit::Usage);
    let status = std::fs::read_to_string(&config.status_file).unwrap();
    let state = std::fs::read_to_string(&config.state_file).unwrap();
    assert!(!status.contains(SECRET) && !state.contains(SECRET));
    let alert = &sent.lock().unwrap()[0];
    let text = alert.body.split("&text=").nth(1).unwrap();
    assert!(!text.contains("SECRET"), "{text}");
    assert!(text.contains("signup+canary+register+returned+HTTP+500"));
}

// ---- the other checks ---------------------------------------------------------

#[tokio::test]
async fn caddy_check_reports_size_validation_and_missing_hosts() {
    let scratch = Scratch::new();
    let mut config = base_config(&scratch);
    std::fs::write(scratch.path("Caddyfile"), "small\n").unwrap();
    config.min_caddyfile_bytes = 100;
    let partial = json!({"apps": {"http": {"servers": {"a": {"routes": [{"match": [{"host": ["verdaxis.exchange"]}]}]}}}}});
    let caddy = scratch.script(
        "partial",
        &format!("#!/bin/sh\ncat <<'EOF'\n{partial}\nEOF\n"),
    );
    config.caddy_command = vec![caddy.to_string_lossy().into_owned()];
    let errors = check_caddyfile(&config).await;
    assert_eq!(
        errors[0],
        format!(
            "{} is only 6 bytes; expected at least 100",
            scratch.path("Caddyfile").display()
        )
    );
    assert!(
        errors[1]
            .starts_with("missing Caddy host blocks: www.verdaxis.exchange, app.verdaxis.exchange")
    );
    let failing = scratch.script(
        "failing",
        "#!/bin/sh\necho 'line one' >&2\necho 'Error: bad directive' >&2\nexit 1\n",
    );
    config.caddy_command = vec![failing.to_string_lossy().into_owned()];
    assert_eq!(
        check_caddyfile(&config).await[1],
        "caddy validate failed: Error: bad directive"
    );
    config.caddyfile = scratch.path("absent");
    assert_eq!(
        check_caddyfile(&config).await,
        [format!(
            "{} does not exist",
            scratch.path("absent").display()
        )]
    );
}

#[tokio::test]
async fn backup_and_restore_evidence_rules() {
    let scratch = Scratch::new();
    let config = base_config(&scratch);
    assert!(check_backup_status(&config, now()).is_empty());
    assert!(check_restore_status(&config, now()).is_empty());
    write_backups(&scratch, now(), false);
    assert_eq!(
        check_backup_status(&config, now()),
        [
            "backup artifact failed gzip validation: verdaxis-20261104-005500.sql.gz",
            "backup artifact failed gzip validation: verdaxis-staging-20261104-005500.sql.gz",
            "backup artifact failed gzip validation: umami-20261104-005500.sql.gz",
        ]
    );
    write_backups(&scratch, now(), true);
    std::fs::write(scratch.path("backups/umami-20261104-005500.sql.gz"), b"").unwrap();
    std::fs::remove_file(scratch.path("backups/verdaxis-20261104-005500.sql.gz")).unwrap();
    assert_eq!(
        check_backup_status(&config, now()),
        [
            "backup artifact is missing: verdaxis-20261104-005500.sql.gz",
            "backup artifact is empty: umami-20261104-005500.sql.gz"
        ]
    );
    // Python 3.10 refuses a `Z` suffix in the backup record.
    let record = scratch.path("backups/status.json");
    let mut payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    payload["completed_at"] = json!("2026-11-04T00:55:00Z");
    std::fs::write(&record, payload.to_string()).unwrap();
    assert_eq!(
        check_backup_status(&config, now()),
        ["backup status has an invalid completion timestamp: invalid isoformat string"]
    );
    payload["completed_at"] = json!("2026-11-04T01:10:00+00:00");
    payload["databases"] = json!(["verdaxis", "umami"]);
    std::fs::write(&record, payload.to_string()).unwrap();
    assert_eq!(
        check_backup_status(&config, now()),
        ["last database backup did not include prod, staging, and analytics"]
    );
    std::fs::remove_file(&record).unwrap();
    assert_eq!(
        check_backup_status(&config, now()),
        [format!("backup status is missing: {}", record.display())]
    );
    // Restore record: exact keys, booleans and bounds.
    let restore = scratch.path("backups/restore-status.json");
    for (document, message) in [
        (
            json!({"ok": true}),
            "restore verification status has an invalid schema",
        ),
        (
            json!({"ok": 1, "completed_at": "", "backup_id": "", "databases": []}),
            "restore verification status has an invalid schema",
        ),
        (
            json!({"ok": false, "completed_at": "", "backup_id": "", "databases": []}),
            "last restore verification did not complete successfully",
        ),
        (
            json!({"ok": true, "completed_at": "2026-11-04T00:00:00Z", "backup_id": "20261104-005500", "databases": ["verdaxis", "verdaxis", "umami"]}),
            "restore verification status has an invalid database inventory",
        ),
        (
            json!({"ok": true, "completed_at": "2026-11-04T00:00:00Z", "backup_id": "x", "databases": ["verdaxis", "verdaxis_staging", "umami"]}),
            "restore verification status has an invalid backup ID",
        ),
        (
            json!({"ok": true, "completed_at": "2026-11-04T00:00:00", "backup_id": "20261104-005500", "databases": ["verdaxis", "verdaxis_staging", "umami"]}),
            "restore verification status has an invalid completion timestamp",
        ),
        (
            json!({"ok": true, "completed_at": "2026-11-04T01:10:00Z", "backup_id": "20261104-005500", "databases": ["verdaxis", "verdaxis_staging", "umami"]}),
            "restore verification completion timestamp is in the future",
        ),
    ] {
        std::fs::write(&restore, document.to_string()).unwrap();
        assert_eq!(
            check_restore_status(&config, now()),
            [message],
            "{document}"
        );
    }
    std::fs::write(&restore, vec![b' '; 5_001]).unwrap();
    assert_eq!(
        check_restore_status(&config, now()),
        ["restore verification status is unreadable"]
    );
}

#[test]
fn python_fromisoformat_matches_python_310() {
    for (text, offset) in [
        ("2026-11-04", Some(false)),
        ("2026-11-04T01", Some(false)),
        ("2026-11-04 01:02:03.123", Some(false)),
        ("2026-11-04T01:02:03.123456+08:00", Some(true)),
        ("2026-11-04T01:02-05:30", Some(true)),
        ("2026-11-04T01:02:03Z", None),
        ("2026-11-04T01:02:03.1234", None),
        ("2026-13-04", None),
        ("20261104", None),
    ] {
        assert_eq!(
            python_fromisoformat(text).map(|(_, has)| has),
            offset,
            "{text}"
        );
    }
}

#[tokio::test]
async fn failed_local_monitor_units_are_reported_from_unit_state_only() {
    let scratch = Scratch::new();
    let mut config = base_config(&scratch);
    let systemctl = scratch.script(
        "systemctl",
        "#!/bin/sh\ncase \"$4\" in\n  verdaxis-health.service) printf 'LoadState=loaded\\nActiveState=failed\\nResult=exit-code\\n' ;;\n  verdaxis-backup-verify.service) printf 'LoadState=loaded\\nActiveState=inactive\\nResult=success\\n' ;;\n  *) printf 'LoadState=not-found\\nActiveState=inactive\\nResult=success\\n' ;;\nesac\n",
    );
    config.systemctl = systemctl;
    config.watched_units = external::WATCHED_UNITS.map(String::from).to_vec();
    // Plan R4-3: a producer that refused admission (exit 4) is failed while
    // its status file still says healthy; the unit state alone raises it.
    assert_eq!(
        check_units(&config).await,
        ["verdaxis-health.service is failed (result exit-code)"]
    );
    config.systemctl = scratch.path("absent-systemctl");
    assert_eq!(
        check_units(&config).await.len(),
        5,
        "an unreadable state is an error"
    );
}

#[tokio::test]
async fn demo_trade_canary_requires_a_fresh_utc_demo_trade() {
    let client = http_client().unwrap();
    let trade = |confirmed: &str| {
        json!({"items": [{"is_demo_trade": true, "demo_status": "DEMO_ONLY", "source_kind": "DEMO_SEED", "provenance_kind": "DEMO_SEED", "confirmed_at": confirmed}]}).to_string()
    };
    for (body, expected) in [
        (trade("2026-11-04T00:40:00Z"), None),
        (
            trade("2026-11-04T00:10:00Z"),
            Some("staging demo trade canary is stale"),
        ),
        (
            trade("2026-11-04T01:10:00+00:00"),
            Some("staging demo trade canary timestamp is too far in the future"),
        ),
        (
            trade("2026-11-04T08:40:00+08:00"),
            Some("staging demo trade canary timestamp is not UTC"),
        ),
        (
            trade("yesterday"),
            Some("staging demo trade canary returned an invalid timestamp"),
        ),
        (
            json!({"items": []}).to_string(),
            Some("staging demo trade canary returned an invalid item list"),
        ),
        (
            "not json".to_owned(),
            Some("staging demo trade canary returned invalid JSON"),
        ),
    ] {
        let served = body.clone();
        let (api, seen) = fake(move |_| ok(&served)).await;
        assert_eq!(
            demo_trade_canary(&client, "staging", &api, now())
                .await
                .as_deref(),
            expected,
            "{body}"
        );
        assert_eq!(
            seen.lock().unwrap()[0].path,
            "/trade-tape?demo_only=true&limit=1"
        );
    }
}

#[test]
fn analytics_storage_counts_regular_files_without_following_directories() {
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.path("data/nested")).unwrap();
    std::fs::write(scratch.path("data/a"), vec![0_u8; 10]).unwrap();
    std::fs::write(scratch.path("data/nested/b"), vec![0_u8; 5]).unwrap();
    std::fs::create_dir_all(scratch.path("elsewhere")).unwrap();
    std::fs::write(scratch.path("elsewhere/c"), vec![0_u8; 1000]).unwrap();
    std::os::unix::fs::symlink(scratch.path("elsewhere"), scratch.path("data/link-dir")).unwrap();
    std::os::unix::fs::symlink(scratch.path("elsewhere/c"), scratch.path("data/link-file"))
        .unwrap();
    // A link to a directory is not followed; a link to a file counts its target.
    assert_eq!(directory_file_bytes(&scratch.path("data")), 1015);
    assert_eq!(directory_file_bytes(&scratch.path("missing")), 0);
}

// ---- rendered page (fake ChromeDriver) -------------------------------------------

fn python() -> String {
    std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| {
        [
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/homebrew/bin/python3",
        ]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .expect("a Python interpreter for the fake ChromeDriver")
        .to_owned()
    })
}

/// A fake ChromeDriver: serves the W3C calls the check makes, answers
/// `execute/sync` with `states` in order (the last one repeats), records
/// every call and its environment, and starts a child that stands in for the
/// browser (same process group).
fn fake_chromedriver(scratch: &Scratch, behaviour: Value) -> (PathBuf, PathBuf) {
    let record = scratch.path("driver.jsonl");
    let script = format!(
        r#"#!{python} -I
import json, os, subprocess, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
BEHAVIOUR = json.loads({behaviour:?})
RECORD = {record:?}
def note(entry):
    with open(RECORD, "a") as handle:
        handle.write(json.dumps(entry) + "\n")
port = int([a for a in sys.argv if a.startswith("--port=")][0].split("=", 1)[1])
browser = subprocess.Popen(["/bin/sleep", "60"])
note({{"argv": sys.argv[1:], "env": sorted(os.environ), "browser": browser.pid}})
if BEHAVIOUR.get("exit_at_start"):
    sys.exit(1)
index = [0]
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def reply(self, code, value):
        body = json.dumps({{"value": value}}).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self.reply(200, {{"ready": True}}) if self.path == "/status" else self.reply(404, {{"error": "unknown command"}})
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        payload = json.loads(self.rfile.read(length) or b"{{}}")
        note({{"path": self.path, "payload": payload}})
        if self.path == "/session":
            if BEHAVIOUR.get("session_error"):
                return self.reply(500, {{"error": "session not created"}})
            return self.reply(200, {{"sessionId": "abc 1", "capabilities": {{}}}})
        if self.path.endswith("/url"):
            return self.reply(200, None)
        states = BEHAVIOUR["states"]
        state = states[min(index[0], len(states) - 1)]
        index[0] += 1
        self.reply(200, state)
    def do_DELETE(self):
        note({{"path": self.path, "method": "DELETE"}})
        self.reply(200, None)
HTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#,
        python = python(),
        behaviour = behaviour.to_string(),
        record = record.to_string_lossy(),
    );
    (scratch.script("fake-chromedriver", &script), record)
}

fn render_state(controls: bool, required: bool, forbidden: [bool; 2], checkpoint: bool) -> Value {
    json!({
        "emailVisible": controls, "passwordVisible": controls, "submitVisible": controls,
        "requiredMatches": [required], "forbiddenMatches": forbidden, "securityCheckpoint": checkpoint,
    })
}

fn render_config(scratch: &Scratch, driver: &Path, timeout: u64) -> ExternalConfig {
    let mut config = base_config(scratch);
    config.rendered_checks_enabled = true;
    config.rendered_pages = external::default_rendered_pages();
    config.render.chromedriver = Some(driver.to_path_buf());
    config.render.profile_parent = scratch.path("profiles");
    config.render.timeout_seconds = timeout;
    config.render.retries = 1;
    config
}

fn records(record: &Path) -> Vec<Value> {
    std::fs::read_to_string(record)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[tokio::test]
async fn rendered_login_page_is_checked_through_chromedriver_and_cleaned_up() {
    use verdaxis_read::cli::monitors::external::webdriver::check_rendered_pages;
    let scratch = Scratch::new();
    // Loading first, then the full login form.
    let (driver, record) = fake_chromedriver(
        &scratch,
        json!({"states": [render_state(false, false, [false, false], false), render_state(true, true, [false, false], false)]}),
    );
    let config = render_config(&scratch, &driver, 20);
    assert_eq!(check_rendered_pages(&config).await, Vec::<String>::new());
    let calls = records(&record);
    let start = &calls[0];
    assert_eq!(start["argv"][1], "--allowed-ips=127.0.0.1");
    assert!(start["argv"][0].as_str().unwrap().starts_with("--port="));
    // macOS adds its own __CF_USER_TEXT_ENCODING to every process.
    let environment: Vec<&str> = start["env"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|key| !key.starts_with("__CF"))
        .collect();
    assert_eq!(environment, ["HOME", "LANG", "LC_ALL", "PATH", "TMPDIR"]);
    let session = &calls[1];
    assert_eq!(session["path"], "/session");
    assert_eq!(
        session["payload"]["capabilities"]["alwaysMatch"]["pageLoadStrategy"],
        "none"
    );
    assert_eq!(calls[2]["path"], "/session/abc%201/url");
    assert_eq!(
        calls[2]["payload"]["url"],
        "https://app.verdaxis.exchange/login?lang=en"
    );
    assert_eq!(
        calls[3]["payload"]["args"],
        json!([
            ["Sign In"],
            [
                "Verdaxis is temporarily under maintenance",
                "temporarily under maintenance"
            ]
        ])
    );
    assert_eq!(calls.last().unwrap()["method"], "DELETE");
    // The driver's group, browser included, is gone; so is the profile.
    let browser = start["browser"].as_i64().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !alive(browser),
        "the browser child was killed with its group"
    );
    assert_eq!(
        std::fs::read_dir(scratch.path("profiles")).unwrap().count(),
        0
    );
}

#[tokio::test]
async fn rendered_page_outcomes_match_the_python_monitor() {
    use verdaxis_read::cli::monitors::external::webdriver::check_rendered_pages;
    let name = "verdaxis app login rendered page";
    for (behaviour, timeout, expected) in [
        (
            json!({"states": [render_state(true, true, [true, false], false)]}),
            20,
            Some(format!(
                "{name} rendered page contains forbidden text 'Verdaxis is temporarily under maintenance'"
            )),
        ),
        (
            json!({"states": [render_state(false, true, [false, false], false)]}),
            2,
            Some(format!(
                "{name} rendered page missing visible email input; missing visible password input; missing visible submit control"
            )),
        ),
        (
            json!({"states": [render_state(false, false, [false, false], true)]}),
            20,
            None,
        ),
        (
            json!({"states": [{"emailVisible": true}]}),
            20,
            Some(format!(
                "{name} render failed: ChromeDriver returned an invalid render state"
            )),
        ),
        (
            json!({"session_error": true, "states": []}),
            20,
            Some(format!(
                "{name} render failed: ChromeDriver returned HTTP 500"
            )),
        ),
        (
            json!({"exit_at_start": true, "states": []}),
            20,
            Some(format!(
                "{name} render failed: ChromeDriver exited during startup"
            )),
        ),
    ] {
        let scratch = Scratch::new();
        let (driver, _) = fake_chromedriver(&scratch, behaviour.clone());
        let config = render_config(&scratch, &driver, timeout);
        assert_eq!(
            check_rendered_pages(&config).await,
            expected.into_iter().collect::<Vec<_>>(),
            "{behaviour}"
        );
    }
    // No configured driver, no snap driver and an empty PATH: none is found.
    if !Path::new("/snap/bin/chromium.chromedriver").exists() {
        let scratch = Scratch::new();
        let mut config = render_config(&scratch, Path::new("/nonexistent"), 2);
        config.render.chromedriver = None;
        let empty = scratch.path("empty");
        assert_eq!(
            verdaxis_read::cli::monitors::external::webdriver::find_chromedriver(
                &config.render,
                Some(empty.to_str().unwrap()),
            ),
            None
        );
    }
}
