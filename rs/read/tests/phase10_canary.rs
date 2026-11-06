//! Phase 10 Task 19: the signup-canary cleanup route and command, offline.
//!
//! Every case serves the full router on lazy pools aimed at a closed port, so
//! a request that reached the database would fail; the pool size proves it
//! never opened a connection. The database cases are in
//! `phase10_canary_postgres.rs`.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, Response},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use std::process::{Command, Output};
use std::time::Duration;
use tower::ServiceExt;
use verdaxis_read::{AppState, compiled_sha, Config, MarketPolicy, runtime::artifact::ALLOWED_REVISIONS};

const PATH: &str = "/api/monitor/signup-canary-cleanup";
const CLOSED_PORT_URL: &str = "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test";
const TOKEN: &str = "monitor-SENTINEL-7f3a";
const CANARY: &str = "canary+staging-20261104@staging-20261104.canary.verdaxis.exchange";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn config(token: Option<&str>) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
        database_url: CLOSED_PORT_URL.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: verdaxis_read::Phase2Options {
            monitor_token: token.map(str::to_owned),
            ..Default::default()
        },
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    }
}

/// The serving router and its read pool, which the route deletes through.
fn app(token: Option<&str>) -> (Router, sqlx::PgPool) {
    let read = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(2))
        .connect_lazy(CLOSED_PORT_URL)
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(CLOSED_PORT_URL)
        .unwrap();
    let pools = verdaxis_read::runtime::db::Pools {
        read: read.clone(),
        health,
    };
    let policy = MarketPolicy {
        source_sha: compiled_sha().into(),
        products: vec![],
        points: vec![],
    };
    let state = AppState::new(config(token), pools, policy).unwrap();
    (verdaxis_read::router(state), read)
}

async fn post(app: &Router, token: Option<&str>, body: &str) -> Response<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("x-monitor-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

/// Status, the raw response text (headers and body) and the parsed body.
async fn answer(response: Response<Body>) -> (u16, String, Value) {
    let status = response.status().as_u16();
    let headers = format!("{:?}", response.headers());
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, format!("{headers}\n{text}"), body)
}

fn body(email: &str) -> String {
    json!({ "email": email }).to_string()
}

#[tokio::test]
async fn canary_token_unset_returns_404_without_opening_a_transaction() {
    for token in [None, Some("")] {
        let (app, pool) = app(token);
        for (presented, request) in [
            (None, body(CANARY)),
            (Some(TOKEN), body(CANARY)),
            // The token is checked before the body model, as FastAPI does.
            (Some(TOKEN), body("not-an-email")),
            (None, "[]".to_owned()),
        ] {
            let (status, _, detail) = answer(post(&app, presented, &request).await).await;
            assert_eq!((status, detail), (404, json!({"detail": "Not found"})));
        }
        // Malformed JSON is refused before any dependency runs.
        let (status, _, detail) = answer(post(&app, None, "{").await).await;
        assert_eq!(status, 422);
        assert_eq!(detail["detail"][0]["type"], "json_invalid");
        assert_eq!(pool.size(), 0, "no connection was opened");
    }
}

#[tokio::test]
async fn canary_wrong_token_returns_401_and_the_token_never_appears_in_output() {
    let (app, pool) = app(Some(TOKEN));
    let wrong = "monitor-WRONG-presented";
    for presented in [None, Some(wrong), Some(""), Some("monitor-SENTINEL-7f3")] {
        let response = post(&app, presented, &body(CANARY)).await;
        assert!(response.headers().get("www-authenticate").is_none());
        let (status, text, detail) = answer(response).await;
        assert_eq!((status, detail), (401, json!({"detail": "Unauthorized"})));
        assert!(!text.contains(TOKEN) && !text.contains(wrong), "{text}");
    }
    assert_eq!(pool.size(), 0);
    // The route never logs the token: the source names it only as the
    // configured value and the presented header.
    let source = include_str!("../src/routes/monitor_canary.rs");
    for line in source.lines() {
        let logs = [
            "tracing::",
            "println!",
            "eprintln!",
            "info!",
            "warn!",
            "error!",
        ];
        assert!(!logs.iter().any(|log| line.contains(log)), "{line}");
    }
}

#[tokio::test]
async fn canary_rejects_a_malformed_or_non_canary_email_with_422() {
    let (app, pool) = app(Some(TOKEN));
    let not_canary = json!({"detail": "Only monitor canary emails can be cleaned up"});
    for email in [
        "user@example.com",
        "canary+x@example.com",
        "monitor@staging-1.canary.verdaxis.exchange",
        "canary+x@canary.verdaxis.exchange.evil.example",
    ] {
        let (status, _, detail) = answer(post(&app, Some(TOKEN), &body(email)).await).await;
        assert_eq!((status, &detail), (422, &not_canary), "{email}");
    }
    // Typed validation keeps pydantic's list envelope.
    for (request, kind, location) in [
        (
            body("not-an-email"),
            "value_error",
            json!(["body", "email"]),
        ),
        (
            json!({"email": 7}).to_string(),
            "string_type",
            json!(["body", "email"]),
        ),
        ("{}".to_owned(), "missing", json!(["body", "email"])),
        (String::new(), "missing", json!(["body"])),
        ("[1]".to_owned(), "model_attributes_type", json!(["body"])),
    ] {
        let (status, _, detail) = answer(post(&app, Some(TOKEN), &request).await).await;
        assert_eq!(status, 422, "{request}");
        assert_eq!(detail["detail"][0]["type"], kind, "{request}");
        assert_eq!(detail["detail"][0]["loc"], location, "{request}");
    }
    assert_eq!(pool.size(), 0, "a refused email never reaches the database");
}

fn canary_command(extra: &[&str], state_dir: Option<&std::path::Path>, email: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("ENVIRONMENT", "staging")
        .env("RELEASE_SHA", SHA)
        .env(
            "DATABASE_URL",
            "postgresql://verdaxis_app_staging:pw@127.0.0.1:1/verdaxis_staging",
        )
        .args(["canary", "cleanup", "--email", email])
        .args(["--environment", "staging", "--release-sha", SHA])
        .args(extra);
    if let Some(directory) = state_dir {
        command.env("DEPLOY_STATE_DIR", directory);
    }
    command.output().unwrap()
}

fn outcome(output: &Output) -> (Option<i32>, String) {
    let line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "one JSON line: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(line["command"], "canary cleanup");
    (
        output.status.code(),
        line["outcome"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn canary_cli_requires_apply_and_the_route_uses_the_same_function() {
    let state = std::env::temp_dir().join(format!("verdaxis-p10-canary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir(&state).unwrap();
    assert_eq!(
        outcome(&canary_command(&[], Some(&state), CANARY)),
        (Some(2), "usage_refused".to_owned())
    );
    assert_eq!(
        outcome(&canary_command(&["--apply"], None, CANARY)),
        (Some(3), "deploy_state_refused".to_owned())
    );
    assert_eq!(
        outcome(&canary_command(
            &["--apply"],
            Some(&state),
            "user@example.com"
        )),
        (Some(2), "not_a_canary_email".to_owned())
    );
    assert_eq!(
        outcome(&canary_command(&["--apply"], Some(&state), "not-an-email")),
        (Some(2), "invalid_email".to_owned())
    );
    // Every gate passes; the closed port is the first failure.
    assert_eq!(
        outcome(&canary_command(&["--apply"], Some(&state), CANARY)),
        (Some(4), "dependency_unavailable".to_owned())
    );
    std::fs::write(state.join("staging.state"), "blocked\n").unwrap();
    assert_eq!(
        outcome(&canary_command(&["--apply"], Some(&state), CANARY)),
        (Some(3), "deploy_state_refused".to_owned())
    );
    std::fs::remove_dir_all(&state).unwrap();
    // One cleanup function: the route calls the command's.
    let route = include_str!("../src/routes/monitor_canary.rs");
    assert!(route.contains("use crate::cli::canary::{"));
    assert!(route.contains("match cleanup(&route.pool, &email, domain).await"));
    let command = include_str!("../src/cli/canary.rs");
    assert!(command.contains("cleanup_on(&mut connection, &email, domain)"));
}

#[test]
fn canary_domain_is_the_python_predicate() {
    use verdaxis_read::cli::canary::canary_domain;
    assert_eq!(
        canary_domain(CANARY),
        Some("staging-20261104.canary.verdaxis.exchange")
    );
    assert_eq!(
        canary_domain("canary+@a.canary.verdaxis.exchange"),
        Some("a.canary.verdaxis.exchange")
    );
    for email in [
        "canary@a.canary.verdaxis.exchange",
        "x+canary@a.canary.verdaxis.exchange",
        "canary+a@verdaxis.exchange",
    ] {
        assert_eq!(canary_domain(email), None, "{email}");
    }
}

fn snippet_lines(relative: &str) -> Vec<String> {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    )
    .unwrap();
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn canary_caddy_snippet_has_one_argument_and_the_exact_path() {
    let lines = snippet_lines("deploy/caddy/phase10-monitor-canary.caddy");
    assert_eq!(lines[0], "(verdaxis_rust_monitor_canary) {");
    assert_eq!(
        lines[1],
        format!("@monitor_canary path {PATH}"),
        "one exact path, no method matcher"
    );
    let text = lines.join("\n");
    assert_eq!(text.matches("{args[").count(), 1);
    assert!(text.contains("reverse_proxy {args[0]} {"));
    assert_eq!(
        text.matches("reverse_proxy").count(),
        1,
        "no Python fallback"
    );
    for absent in ["fail_duration", "max_fails", "*", "path_regexp"] {
        assert!(!text.contains(absent), "{absent}");
    }
}
