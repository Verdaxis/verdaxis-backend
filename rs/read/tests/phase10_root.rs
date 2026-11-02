//! Phase 10 Task 18: the root route and the root/health Caddy group.
//!
//! Every case builds the serving router on a lazy pool aimed at a closed
//! port, so a handler that reached for the database would fail or hang. The
//! one case that needs a real database (the 200 readiness body) is compiled
//! only with `contract-tests` and reads the disposable test URL.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, Response},
};
use sqlx::postgres::PgPoolOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, MarketPolicy, compiled_sha};

const ROOT_BODY: &str = r#"{"message":"Verdaxis API is running","version":"1.0.0"}"#;
const READ_SLOTS: u32 = 4;
const CLOSED_PORT_URL: &str = "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test";

fn repository_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn config(database_url: &str) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: database_url.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: READ_SLOTS,
        pool_max: READ_SLOTS + 1,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    }
}

/// Application state on lazy pools. The returned pools are clones of the
/// state's, so a test can see whether a request opened a connection.
fn state_for(database_url: &str) -> (AppState, sqlx::PgPool, sqlx::PgPool) {
    state_with(config(database_url))
}

fn state_with(config: Config) -> (AppState, sqlx::PgPool, sqlx::PgPool) {
    let read = PgPoolOptions::new()
        .max_connections(READ_SLOTS)
        .acquire_timeout(Duration::from_secs(30))
        .connect_lazy(&config.database_url)
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&config.database_url)
        .unwrap();
    let pools = verdaxis_read::runtime::db::Pools {
        read: read.clone(),
        health: health.clone(),
    };
    let policy = MarketPolicy {
        source_sha: compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    (AppState::new(config, pools, policy).unwrap(), read, health)
}

fn closed_port_app() -> Router {
    verdaxis_read::router(state_for(CLOSED_PORT_URL).0)
}

async fn send(app: &Router, method: &str, path: &str) -> Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body_text(response: Response<Body>) -> String {
    String::from_utf8(to_bytes(response.into_body(), 4096).await.unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn root_returns_the_exact_python_body() {
    let response = send(&closed_port_app(), "GET", "/").await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    let text = body_text(response).await;
    assert_eq!(text, ROOT_BODY);
    let keys: Vec<String> =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
            .unwrap()
            .keys()
            .cloned()
            .collect();
    assert_eq!(keys, ["message", "version"]);

    // While the Python application is in the tree, its literal is the source
    // of the two values.
    let python = repository_file("app/main.py");
    if python.exists() {
        let source = std::fs::read_to_string(python).unwrap();
        assert!(
            source.contains(r#"return {"message": "Verdaxis API is running", "version": "1.0.0"}"#),
            "app/main.py no longer returns the pinned root body"
        );
    }

    // The handler takes no argument: no state, no identity, no configuration.
    let verdaxis_read::routes::root::RootBody { message, version } =
        verdaxis_read::routes::root::root().await.0;
    assert_eq!((message, version), ("Verdaxis API is running", "1.0.0"));
}

#[tokio::test]
async fn root_does_not_touch_the_database() {
    let (state, read, health) = state_for(CLOSED_PORT_URL);
    // Every read-admission slot is taken, so a handler that asked for a
    // database permit would answer 503 instead of 200.
    let _held = state
        .reads
        .clone()
        .try_acquire_many_owned(READ_SLOTS)
        .unwrap();
    let app = verdaxis_read::router(state);
    let response = tokio::time::timeout(Duration::from_secs(1), send(&app, "GET", "/"))
        .await
        .expect("root must answer without waiting for a connection");
    assert_eq!(response.status(), 200);
    assert_eq!(body_text(response).await, ROOT_BODY);
    assert_eq!(read.size(), 0, "root opened a read connection");
    assert_eq!(health.size(), 0, "root opened a health connection");

    // Closed pools refuse every acquire at once; root still answers.
    read.close().await;
    health.close().await;
    let response = send(&app, "GET", "/").await;
    assert_eq!(response.status(), 200);
    assert_eq!(body_text(response).await, ROOT_BODY);
}

#[tokio::test]
async fn root_is_get_only() {
    let app = closed_port_app();
    for method in ["POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        let root = send(&app, method, "/").await;
        // The shared fallback answers a wrong method on any mounted path.
        let shared = send(&app, method, "/health/live").await;
        assert_eq!(root.status(), 405, "{method} /");
        assert_eq!(root.status(), shared.status(), "{method}");
        assert_eq!(root.headers()["allow"], "GET", "{method} /");
        assert_eq!(root.headers()["allow"], shared.headers()["allow"]);
        assert_eq!(
            root.headers()["content-type"],
            shared.headers()["content-type"]
        );
        let (root_body, shared_body) = (body_text(root).await, body_text(shared).await);
        assert_eq!(root_body, shared_body, "{method}");
        if method == "HEAD" {
            assert!(root_body.is_empty());
        } else {
            assert_eq!(root_body, r#"{"detail":"Method Not Allowed"}"#, "{method}");
        }
    }
}

/// The unreachable-database readiness answer: status, content type, exact text.
async fn readiness_failure(app: &Router, path: &str) -> (u16, String, String) {
    let response = send(app, "GET", path).await;
    let status = response.status().as_u16();
    let content_type = response.headers()["content-type"]
        .to_str()
        .unwrap()
        .to_owned();
    (status, content_type, body_text(response).await)
}

#[tokio::test]
async fn health_live_answers_status_ok_alone() {
    let response = send(&closed_port_app(), "GET", "/health/live").await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(body_text(response).await, r#"{"status":"ok"}"#);
}

/// A real database answers the readiness probe: both ready routes carry the
/// four keys, in Python's order, with this process's identity.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn health_routes_keep_the_exact_four_key_body() {
    let url =
        std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable test URL required");
    let (state, _read, _health) = state_for(&url);
    let app = verdaxis_read::router(state);
    let expected = format!(
        r#"{{"status":"ok","db":"ok","environment":"test","release_sha":"{}"}}"#,
        compiled_sha()
    );
    for path in ["/health", "/health/ready"] {
        let response = send(&app, "GET", path).await;
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(body_text(response).await, expected, "{path}");
    }
    let live = send(&app, "GET", "/health/live").await;
    assert_eq!(live.status(), 200);
    assert_eq!(body_text(live).await, r#"{"status":"ok"}"#);
}

/// A stream subsystem that is down while the database answers: the 503 keeps
/// `db` truthful and, like every failure body, carries no provenance.
#[cfg(feature = "contract-tests")]
#[tokio::test]
async fn stream_down_503_has_no_provenance() {
    let url =
        std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable test URL required");
    let mut config = config(&url);
    config.prepared_budget = Some(
        serde_json::json!({"version":1,"release_sha":config.release_sha,
            "environment_digest":"a".repeat(64), "services":2,"workers":4,
            "pool":2,"overflow":1,"reserve":20,"rust_pool":config.pool_max,
            "python_budget":52,"cache_listener":0,"stream_listener":1,
            "total":52+config.pool_max+1,
            "configured_max_connections":config.configured_max_connections})
        .to_string(),
    );
    let (state, _read, _health) = state_with(config);
    assert!(!verdaxis_read::streams::runtime::ready(&state));
    let app = verdaxis_read::router(state);
    for path in ["/health", "/health/ready"] {
        let response = send(&app, "GET", path).await;
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(
            body_text(response).await,
            r#"{"status":"error","db":"ok"}"#,
            "{path}"
        );
    }
}

#[tokio::test]
async fn health_503_body_has_no_provenance() {
    let app = closed_port_app();
    for path in ["/health", "/health/ready"] {
        let (status, content_type, body) = readiness_failure(&app, path).await;
        assert_eq!(status, 503, "{path}");
        assert_eq!(content_type, "application/json", "{path}");
        assert_eq!(body, r#"{"status":"error","db":"unavailable"}"#, "{path}");
        assert!(!body.contains(compiled_sha()), "{path}");
        assert!(!body.contains("environment"), "{path}");
    }
    // The probe function gives the same answer to a direct caller.
    let (_, _, health) = state_for(CLOSED_PORT_URL);
    let (status, body) =
        verdaxis_read::runtime::health::probe(&health, "production", &"a".repeat(40)).await;
    assert_eq!(status, 503);
    assert_eq!(
        body,
        serde_json::json!({"status": "error", "db": "unavailable"})
    );
}

/// The snippet text without comment lines.
fn snippet_lines(relative: &str) -> Vec<String> {
    std::fs::read_to_string(repository_file(relative))
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// The transport lines of a `reverse_proxy` block: everything after its
/// opening line, up to the line that closes the block.
fn transport_lines(lines: &[String], opening: &str) -> Vec<String> {
    let start = lines
        .iter()
        .position(|line| line.starts_with(opening))
        .unwrap_or_else(|| panic!("no reverse_proxy block opens with {opening}"));
    let mut depth = 0_i32;
    let mut block = Vec::new();
    for line in &lines[start..] {
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if depth == 0 {
            break;
        }
        if block.len() + 1 < lines[start..].len() && line != &lines[start] {
            block.push(line.clone());
        }
    }
    block
}

#[tokio::test]
async fn root_health_snippet_has_one_argument_and_exact_paths() {
    let lines = snippet_lines("deploy/caddy/phase10-root-health.caddy");
    let text = lines.join("\n");

    // One named snippet, one upstream argument, no second upstream.
    let definitions: Vec<&String> = lines.iter().filter(|l| l.starts_with('(')).collect();
    assert_eq!(definitions, ["(verdaxis_rust_root_health) {"]);
    assert_eq!(text.matches("{args[").count(), 1, "{text}");
    assert_eq!(text.matches("{args[0]}").count(), 1, "{text}");
    let proxies: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("reverse_proxy"))
        .collect();
    assert_eq!(proxies, ["reverse_proxy {args[0]} {"]);
    for forbidden in [
        "fallback",
        "127.0.0.1",
        "localhost",
        "http://",
        "https://",
        "respond",
        "redir",
        "rewrite",
        "method",
        "path_regexp",
        "expression",
    ] {
        assert!(!text.contains(forbidden), "snippet contains {forbidden}");
    }
    // The only handle is the one named matcher: no catch-all.
    let handles: Vec<&String> = lines.iter().filter(|l| l.starts_with("handle")).collect();
    assert_eq!(handles.len(), 1, "{handles:?}");
    assert_eq!(handles[0], "handle @root_health {");

    // Exactly the four paths, each once.
    let matchers: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with('@') && l.contains(" path "))
        .collect();
    assert_eq!(matchers.len(), 1, "{matchers:?}");
    let mut paths: Vec<&str> = matchers[0].split_whitespace().skip(2).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["/", "/health", "/health/live", "/health/ready"]);

    // One upstream and no fallback: a remembered dial failure would leave the
    // group with nothing to select, so there is no passive health check. The
    // bounded retry bridges a restart; the other lines match the read groups.
    assert!(!text.contains("fail_duration"), "{text}");
    assert!(!text.contains("max_fails"), "{text}");
    let ours = transport_lines(&lines, "reverse_proxy {args[0]} {");
    assert_eq!(
        ours,
        [
            "lb_policy first",
            "lb_try_duration 10s",
            "lb_try_interval 250ms",
            "flush_interval -1",
            "transport http {",
            "keepalive off",
            "}",
        ]
    );
    let existing = snippet_lines("deploy/caddy/rust-public-staging.caddy");
    let reference = transport_lines(&existing, "reverse_proxy {args[0]} {args[5]} {");
    assert!(ours.iter().all(|line| reference.contains(line)));

    // Each named path is served by this binary: mounted, never the 404 that
    // would send a caller to a fallback owner.
    let app = closed_port_app();
    for path in paths {
        assert!(
            verdaxis_read::runtime::http::MOUNTED_GET_PATHS.contains(&path),
            "{path} is not in the mounted GET inventory"
        );
        let status = send(&app, "GET", path).await.status();
        assert!(
            status == 200 || status == 503,
            "GET {path} answered {status}"
        );
    }
}
