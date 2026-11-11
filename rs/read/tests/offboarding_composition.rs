//! The serving composition of individual offboarding: the release switch,
//! the administrator quota, the absence of a Python route, and the Caddy
//! routing of the group.
//!
//! No database is needed. The application pools are lazy, and every request
//! here stops before its first query: a route that is not mounted answers the
//! serving fallback, and a mounted route refuses a request without a bearer
//! token before admission.
//!
//! `caddy_routes_the_group_only_when_enabled` needs a Caddy 2 binary
//! (`CADDY_BINARY`, default `caddy` on `PATH`) and is ignored in a plain run.

use std::{collections::HashMap, net::SocketAddr, path::Path, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use verdaxis_read::{
    AppState, Config, compiled_sha, runtime::artifact::ALLOWED_REVISIONS,
    runtime::config::config_from_deployed_environment,
};

const SAMPLE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SECRET: &str = "synthetic-composition-key-at-least-32-characters";

/// The four operations behind the release switch.
const ROUTES: [(&str, &str); 4] = [
    ("POST", "/api/auth/admin/users/{id}/offboard"),
    ("GET", "/api/auth/admin/users/{id}/offboarding-preview"),
    ("GET", "/api/auth/admin/users/{id}/offboarding-trades"),
    ("POST", "/api/auth/admin/trades/{id}/reassign-responsible"),
];
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

fn concrete(path: &str) -> String {
    path.replace("{id}", SAMPLE_ID)
}

/// An application with the Phase 2 settings and lazy pools to a closed port.
fn state(switch: Option<bool>) -> AppState {
    let mut phase2 = json!({
        "jwt_secret": SECRET, "jwt_issuer": "composition-test", "jwt_audience": "composition-test",
        "frontend_url": "https://app.example.test", "email_from": "team@example.test",
        "resend_api_key": "synthetic", "auth_domain_datasets": "null"
    });
    if let Some(enabled) = switch {
        phase2["offboarding_enabled"] = json!(enabled);
    }
    let database_url = "postgres://composition:unused@127.0.0.1:9/composition_test";
    let config: Config = serde_json::from_value(json!({
        "kyc_execution_gate_enabled": false,
        "environment": "test",
        "release_sha": compiled_sha(),
        "migration_revision": ALLOWED_REVISIONS[0],
        "database_url": database_url,
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 4,
        "pool_max": 5,
        "python_budget": 52,
        "configured_max_connections": 100,
        "phase2": phase2,
    }))
    .unwrap();
    // The read pool matches the admission slots; readiness has one connection.
    let pool = |connections| {
        PgPoolOptions::new()
            .max_connections(connections)
            .connect_lazy(database_url)
            .unwrap()
    };
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    AppState::new(
        config,
        verdaxis_read::runtime::db::Pools {
            read: pool(4),
            health: pool(1),
        },
        policy,
    )
    .unwrap()
}

async fn send(router: &Router, method: &str, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .body(Body::from(if method == "POST" { "{}" } else { "" }))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn deployed(pairs: &[(&str, &str)]) -> Result<bool, String> {
    let mut environment: HashMap<String, String> = [
        ("RELEASE_SHA", "a".repeat(40)),
        ("ENVIRONMENT", "staging".into()),
        ("MIGRATION_REVISION", "ob_20261031_offboarding".into()),
        (
            "DATABASE_URL",
            "postgresql://verdaxis_app_staging:fixture-secret@127.0.0.1:5432/verdaxis_staging"
                .into(),
        ),
        ("DB_SESSION_TIME_ZONE", "UTC".into()),
        ("JWT_SECRET", SECRET.into()),
        ("JWT_ISSUER", "composition-test".into()),
        ("JWT_AUDIENCE", "composition-test".into()),
        ("FRONTEND_URL", "https://staging.example.test".into()),
        ("EMAIL_FROM", "Verdaxis <noreply@example.test>".into()),
        ("RESEND_API_KEY", "re_disposable_key".into()),
        (
            "AUTH_DOMAIN_DATASETS_FILE",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/auth-domains.json"
            )
            .into(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    for (key, value) in pairs {
        environment.insert((*key).into(), (*value).into());
    }
    let budget =
        json!({"version":1,"release_sha":"a".repeat(40),"environment_digest":"b".repeat(64),
        "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":17,
        "python_budget":52,"total":69,"configured_max_connections":100})
        .to_string();
    config_from_deployed_environment(&environment, "127.0.0.1:8011".parse().unwrap(), &budget)
        .map(|config| config.phase2.offboarding_enabled)
}

/// The switch is off by default: an absent key, a false value and a harness
/// configuration without the field mount nothing, and the served router
/// answers every method on the four paths with its fallback. Once on, the
/// same router reaches each route's own handler.
#[tokio::test]
async fn disabled_command_is_not_routed() {
    assert_eq!(
        verdaxis_read::runtime::offboarding::SWITCH,
        "OFFBOARDING_ENABLED"
    );
    assert_eq!(deployed(&[]), Ok(false), "absent means disabled");
    assert_eq!(deployed(&[("OFFBOARDING_ENABLED", "off")]), Ok(false));
    assert_eq!(deployed(&[("OFFBOARDING_ENABLED", " True ")]), Ok(true));
    assert!(deployed(&[("OFFBOARDING_ENABLED", "enabled")]).is_err());

    for switch in [None, Some(false)] {
        let app = state(switch);
        assert!(!verdaxis_read::runtime::offboarding::mounts(&app));
        let served = verdaxis_read::router(app);
        for (_, path) in ROUTES {
            for method in METHODS {
                let (status, body) = send(&served, method, &concrete(path)).await;
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{switch:?} {method} {path}: {body}"
                );
            }
        }
    }

    let app = state(Some(true));
    assert!(verdaxis_read::runtime::offboarding::mounts(&app));
    let served = verdaxis_read::router(app);
    for (method, path) in ROUTES {
        let (status, body) = send(&served, method, &concrete(path)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}: {body}");
        for other in METHODS.into_iter().filter(|other| *other != method) {
            let (status, _) = send(&served, other, &concrete(path)).await;
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{other} {path}");
        }
    }
}

/// The group draws from the one `/api/admin/` client quota that the
/// pre-authentication gate charges: after 299 gate admissions, one request to
/// the group is admitted (the 300th) and every further request is refused
/// with the shared 429 before its handler runs.
#[tokio::test]
async fn the_group_shares_the_administrator_quota() {
    let served = verdaxis_read::router(state(Some(true)));
    for _ in 0..299 {
        let (status, _) = send(&served, "GET", verdaxis_read::runtime::http::PREAUTH_PATH).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, _) = send(&served, "POST", &concrete(ROUTES[0].1)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the 300th request is admitted"
    );
    for (method, path) in ROUTES {
        let (status, body) = send(&served, method, &concrete(path)).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{method} {path}");
        assert_eq!(body["detail"]["code"], "RATE_LIMITED");
    }
    // The gate itself refuses the same client now: one counter.
    let (status, _) = send(&served, "GET", verdaxis_read::runtime::http::PREAUTH_PATH).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

// ---------------------------------------------------------------------------
// Caddy routing.
// ---------------------------------------------------------------------------

/// Serve `router` on a loopback port and return the port.
async fn serve(router: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    port
}

/// A Python stand-in: every request is answered 404 and labelled.
fn python_stub() -> Router {
    Router::new().fallback(|| async {
        (
            StatusCode::NOT_FOUND,
            [("x-owner", "python")],
            "{\"detail\":\"Not Found\"}",
        )
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One running Caddy, removed with its folder when dropped.
struct Caddy {
    child: std::process::Child,
    folder: std::path::PathBuf,
    port: u16,
}

impl Drop for Caddy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn caddy_binary() -> String {
    std::env::var("CADDY_BINARY").unwrap_or_else(|_| "caddy".into())
}

/// Start Caddy with the reviewed snippet and one site whose route block has
/// `import_line` (or none) before its Python fallback.
async fn caddy(import_line: Option<String>, python: u16) -> Caddy {
    let port = free_port();
    let folder = std::env::temp_dir().join(format!("offboarding-caddy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let snippet = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/caddy/rust-offboarding.caddy"),
    )
    .unwrap();
    let config = format!(
        "{{\n\tadmin off\n\tauto_https off\n\tpersist_config off\n}}\n{snippet}\nhttp://127.0.0.1:{port} {{\n\tbind 127.0.0.1\n\troute {{\n\t\t{}\n\t\thandle {{\n\t\t\treverse_proxy 127.0.0.1:{python}\n\t\t}}\n\t}}\n}}\n",
        import_line.unwrap_or_default()
    );
    let file = folder.join("Caddyfile");
    std::fs::write(&file, config).unwrap();
    let child = std::process::Command::new(caddy_binary())
        .args(["run", "--config"])
        .arg(&file)
        .args(["--adapter", "caddyfile"])
        .env("HOME", &folder)
        .env("XDG_DATA_HOME", folder.join("data"))
        .env("XDG_CONFIG_HOME", folder.join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(folder.join("caddy.log")).unwrap())
        .spawn()
        .expect("a Caddy binary (CADDY_BINARY or caddy on PATH)");
    let mut caddy = Caddy {
        child,
        folder,
        port,
    };
    let client = client();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if client
            .get(format!("http://127.0.0.1:{port}/ready-probe"))
            .send()
            .await
            .is_ok()
        {
            return caddy;
        }
        if let Some(status) = caddy.child.try_wait().unwrap() {
            let log = std::fs::read_to_string(caddy.folder.join("caddy.log")).unwrap_or_default();
            panic!("Caddy stopped with {status}: {log}");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Caddy did not start"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// The answering owner of one request: `python`, or the Rust status.
async fn owner(caddy: &Caddy, method: &str, path: &str) -> String {
    let response = client()
        .request(
            method.parse().unwrap(),
            format!("http://127.0.0.1:{}{path}", caddy.port),
        )
        .header("content-type", "application/json")
        .body(if method == "POST" { "{}" } else { "" })
        .send()
        .await
        .unwrap();
    if response
        .headers()
        .get("x-owner")
        .is_some_and(|value| value == "python")
    {
        "python".into()
    } else {
        format!("rust {}", response.status().as_u16())
    }
}

/// Requests near the four operations that must stay on Python in every mode.
fn neighbours() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (method, path) in ROUTES {
        let sample = concrete(path);
        for other in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
            if other != method {
                found.push((other.to_owned(), sample.clone()));
            }
        }
        found.push((method.to_owned(), format!("{sample}/")));
        found.push((method.to_owned(), format!("{sample}/extra")));
        found.push((method.to_owned(), sample.to_uppercase()));
        let (parent, _) = sample.rsplit_once('/').unwrap();
        found.push((method.to_owned(), parent.to_owned()));
    }
    found
}

/// The reviewed snippet in an isolated Caddy, in front of the real served
/// Rust router and a fallback stand-in. Disabled (the import absent) every
/// request reaches the fallback, which has no such route. With the snippet
/// enabled but the Rust switch off, the four operations reach Rust and find
/// no route. Enabled on both sides, exactly the four operations reach their
/// Rust handlers; their neighbours stay on the fallback in every mode.
#[tokio::test]
#[ignore = "requires a Caddy 2 binary (CADDY_BINARY or caddy on PATH)"]
async fn caddy_routes_the_group_only_when_enabled() {
    let version = std::process::Command::new(caddy_binary())
        .arg("version")
        .output()
        .expect("a Caddy binary");
    let version = String::from_utf8_lossy(&version.stdout).to_string();
    assert!(version.starts_with("v2."), "{version}");
    eprintln!("caddy {version}");
    let fallback = serve(python_stub()).await;
    let rust_on = serve(verdaxis_read::router(state(Some(true)))).await;
    let rust_off = serve(verdaxis_read::router(state(None))).await;
    let import = |rust: u16| Some(format!("import verdaxis_rust_offboarding 127.0.0.1:{rust}"));
    let modes = [
        ("import absent", None, "python"),
        ("Rust switch off", import(rust_off), "rust 404"),
        ("enabled", import(rust_on), "rust 401"),
    ];
    for (mode, line, expected) in modes {
        let caddy = caddy(line, fallback).await;
        for (method, path) in ROUTES {
            let path = concrete(path);
            assert_eq!(
                owner(&caddy, method, &path).await,
                expected,
                "{mode}: {method} {path}"
            );
        }
        for (method, path) in neighbours() {
            assert_eq!(
                owner(&caddy, &method, &path).await,
                "python",
                "{mode}: {method} {path}"
            );
        }
    }
}
