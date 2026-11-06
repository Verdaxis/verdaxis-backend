//! The market write operations in the serving composition.
//!
//! The 43 operations come from the switch ledger
//! (`tests/rust_contract/phase89/switch-routes.json`), which also pins each
//! operation's answer to an invalid bearer token. The offline cases build the
//! served router (`verdaxis_read::router`) with pools that are never dialled.
//! The cases marked `ignore` serve a fresh database cloned from the migrated
//! template (`RUST_SERVING_*`, `rs/read/tests/run_postgres.sh`).
#[path = "support/market_database.rs"]
mod market_database;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::runtime::config::{bootstrap_arguments, config_from_deployed_environment};
use verdaxis_read::runtime::db::Pools;
use verdaxis_read::runtime::market_write::MarketWriteOptions;
use verdaxis_read::{
    AppState, Config, MarketPolicy, compiled_sha, runtime::artifact::ALLOWED_REVISIONS,
};

const LEDGER: &str = include_str!("../../../tests/rust_contract/phase89/switch-routes.json");
const MARKERS: &str = include_str!("fixtures/market_write_markers.sql");
const SECRET: &str = "serving-tests-signing-key-at-least-32-chars";
const ISSUER: &str = "serving-tests";
const AUDIENCE: &str = "serving-web";
/// Offline pools name a Unix socket in a directory that does not exist.
const NO_DATABASE: &str =
    "postgres://fixture@localhost/verdaxis_test?host=/nonexistent-verdaxis-serving-tests";
/// The validated configuration must name a loopback database; offline cases
/// never dial it.
const OFFLINE_CONFIG_URL: &str =
    "postgresql://verdaxis_app_test:fixture@127.0.0.1:1/verdaxis_serving_test";
const CONTEXT_HEADER: &str = "x-verdaxis-market-support-context";
const SAMPLE_ID: &str = "00000000-0000-4000-8000-000000000001";
const BUYER: u128 = 0x1001;
const SELLER: u128 = 0x2001;
const ADMIN: u128 = 0x3001;
const SUPPORT_ADMIN: u128 = 0x3002;
const BUYER_ORG: u128 = 0x1000;
const ITEM: u128 = 0x4000;
const PRODUCT: &str = "c4cecebc-3d2d-5840-8021-57a9a11bc673";

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

struct Operation {
    id: String,
    method: String,
    pattern: String,
    support_enabled: u16,
    support_disabled: u16,
}

impl Operation {
    /// The pattern with every identifier segment replaced by one sample UUID.
    fn path(&self) -> String {
        self.pattern
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    SAMPLE_ID
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    }
    fn writes(&self) -> bool {
        self.method != "GET"
    }
}

fn ledger() -> Vec<Operation> {
    let ledger: Value = serde_json::from_str(LEDGER).unwrap();
    let operations: Vec<Operation> = ledger["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| Operation {
            id: row["id"].as_str().unwrap().into(),
            method: row["method"].as_str().unwrap().into(),
            pattern: row["pattern"].as_str().unwrap().into(),
            support_enabled: row["invalid_bearer_status"]["support_enabled"]
                .as_u64()
                .unwrap() as u16,
            support_disabled: row["invalid_bearer_status"]["support_disabled"]
                .as_u64()
                .unwrap() as u16,
        })
        .collect();
    assert_eq!(operations.len(), 43);
    assert_eq!(ledger["count"], 43);
    operations
}

fn options(support_enabled: bool) -> Value {
    json!({
        "auto_matching_enabled": true,
        "support_enabled": support_enabled,
        "support_bootstrap_admin_user_ids": id(ADMIN).to_string(),
    })
}

fn config(database_url: &str, market: Option<Value>) -> Config {
    let mut document = json!({
        "environment": "test",
        "release_sha": compiled_sha(),
        "migration_revision": ALLOWED_REVISIONS[0],
        "database_url": database_url,
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 8,
        "pool_max": 9,
        "python_budget": 0,
        "configured_max_connections": 100,
        "kyc_execution_gate_enabled": true,
    });
    if let Some(market) = market {
        document["phase2"] = json!({
            "jwt_secret": SECRET,
            "jwt_issuer": ISSUER,
            "jwt_audience": AUDIENCE,
            "frontend_url": "https://app.example.test",
            "email_from": "Verdaxis <noreply@example.test>",
            "resend_api_key": "re_disposable_key",
            "market_write": market,
        });
    }
    serde_json::from_value(document).unwrap()
}

fn policy() -> MarketPolicy {
    serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap()
}

fn offline_state(market: Option<Value>) -> AppState {
    let lazy = |connections| {
        PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy(NO_DATABASE)
            .unwrap()
    };
    let pools = Pools {
        read: lazy(8),
        health: lazy(1),
    };
    AppState::new(config(OFFLINE_CONFIG_URL, market), pools, policy()).unwrap()
}

/// Capture every log line of the requests sent through `run`.
fn captured<F: std::future::Future<Output = ()>>(run: F) -> String {
    #[derive(Clone, Default)]
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let writer = Writer::default();
    let sink = writer.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || sink.clone())
        .finish();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tracing::subscriber::with_default(subscriber, || runtime.block_on(run));
    String::from_utf8(writer.0.lock().unwrap().clone()).unwrap()
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn invalid_bearer(method: &str, path: &str, request_id: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", "Bearer not-a-valid-token")
        .header("content-type", "application/json")
        .header("x-request-id", request_id)
}

/// The request id the log line of operation `index` carries.
fn request_id(index: usize) -> String {
    format!("serving-{index:02}")
}

fn logged_route(log: &str, request_id: &str) -> Vec<String> {
    log.lines()
        .filter(|line| line.contains("\"http_request\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["fields"]["request_id"] == request_id)
        .map(|line| {
            line["fields"]["route"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn served_router_mounts_every_market_operation() {
    for support in [true, false] {
        let operations = ledger();
        let mut observed = Vec::new();
        let log = captured(async {
            let router = verdaxis_read::router(offline_state(Some(options(support))));
            for (index, operation) in operations.iter().enumerate() {
                let request =
                    invalid_bearer(&operation.method, &operation.path(), &request_id(index))
                        .body(Body::from("{}"))
                        .unwrap();
                observed.push(send(&router, request).await.0.as_u16());
            }
        });
        for (index, operation) in operations.iter().enumerate() {
            let expected = if support {
                operation.support_enabled
            } else {
                operation.support_disabled
            };
            assert_eq!(
                observed[index], expected,
                "{} with support {support}",
                operation.id
            );
            assert_eq!(
                logged_route(&log, &request_id(index)),
                vec![operation.pattern.clone()],
                "{} logs one request line with its route template",
                operation.id
            );
        }
    }
}

#[test]
fn served_router_takes_both_trade_creation_paths() {
    let log = captured(async {
        let router = verdaxis_read::router(offline_state(Some(options(true))));
        for (index, path) in ["/api/trades/", "/api/trades"].into_iter().enumerate() {
            let request = invalid_bearer("POST", path, &request_id(index))
                .body(Body::from("{}"))
                .unwrap();
            assert_eq!(send(&router, request).await.0, StatusCode::UNAUTHORIZED);
        }
    });
    assert_eq!(logged_route(&log, &request_id(0)), vec!["/api/trades/"]);
    assert_eq!(logged_route(&log, &request_id(1)), vec!["/api/trades"]);
}

#[test]
fn served_router_composes_put_and_delete_on_one_order_path() {
    let log = captured(async {
        // Building the router merges the amend and cancel components on one
        // path; a conflicting merge would panic here.
        let router = verdaxis_read::router(offline_state(Some(options(true))));
        let path = format!("/api/orderbook/{SAMPLE_ID}");
        for (index, method) in ["PUT", "DELETE", "PATCH"].into_iter().enumerate() {
            let request = invalid_bearer(method, &path, &request_id(index))
                .body(Body::from("{}"))
                .unwrap();
            let expected = if method == "PATCH" {
                StatusCode::METHOD_NOT_ALLOWED
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(send(&router, request).await.0, expected, "{method}");
        }
    });
    for index in 0..2 {
        assert_eq!(
            logged_route(&log, &request_id(index)),
            vec!["/api/orderbook/{id}"]
        );
    }
}

#[test]
fn served_router_without_phase2_settings_mounts_no_market_operation() {
    let operations = ledger();
    let log = captured(async {
        let router = verdaxis_read::router(offline_state(None));
        for (index, operation) in operations.iter().enumerate() {
            let request = invalid_bearer(&operation.method, &operation.path(), &request_id(index))
                .body(Body::from("{}"))
                .unwrap();
            let (status, body) = send(&router, request).await;
            if operation.id == "order.create" {
                // The read service's unauthenticated answer, not order entry.
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                assert_eq!(body, json!({"detail": "Could not validate credentials"}));
            } else {
                assert!(
                    matches!(status.as_u16(), 404 | 405),
                    "{} answered {status}",
                    operation.id
                );
            }
        }
    });
    for (index, operation) in operations.iter().enumerate() {
        let routes = logged_route(&log, &request_id(index));
        assert_eq!(routes.len(), 1, "{}", operation.id);
        if operation.id != "order.create" {
            assert_ne!(routes[0], operation.pattern, "{}", operation.id);
        }
    }
    // The health and readiness routes stay mounted either way.
    let _ = captured(async {
        for state in [offline_state(None), offline_state(Some(options(true)))] {
            let router = verdaxis_read::router(state);
            let request = Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap();
            assert_eq!(send(&router, request).await.0, StatusCode::OK);
        }
    });
}

/// The public reads on the order-entry path and its neighbours route to the
/// same read handlers whether or not the market writes are mounted.
#[test]
fn public_reads_still_serve_with_the_market_routes_mounted() {
    const READS: [(&str, &str); 4] = [
        ("/api/orderbook", "/api/orderbook"),
        ("/api/orderbook/bids", "/api/orderbook/bids"),
        ("/api/orderbook/asks", "/api/orderbook/asks"),
        ("/api/listings", "/api/listings"),
    ];
    let mut answers = Vec::new();
    let log = captured(async {
        for (round, market) in [None, Some(options(true))].into_iter().enumerate() {
            let router = verdaxis_read::router(offline_state(market));
            for (index, (path, _)) in READS.iter().enumerate() {
                let request = Request::builder()
                    .uri(*path)
                    .header("x-request-id", request_id(round * 10 + index))
                    .body(Body::empty())
                    .unwrap();
                answers.push(send(&router, request).await);
            }
        }
    });
    let (without, with) = answers.split_at(READS.len());
    for (index, (path, template)) in READS.iter().enumerate() {
        assert_eq!(with[index], without[index], "{path}");
        assert_ne!(with[index].0, StatusCode::METHOD_NOT_ALLOWED, "{path}");
        for round in [0, 1] {
            assert_eq!(
                logged_route(&log, &request_id(round * 10 + index)),
                vec![(*template).to_owned()],
                "{path}"
            );
        }
    }
}

/// Python's support-scope middleware: a request that carries a support
/// context is refused with 403 before authentication unless it is a read or
/// one of the three classified mutations (order entry, order cancellation by
/// POST and context exit). Every other request answers as without the header.
#[test]
fn support_scope_applies_to_market_writes_as_python() {
    const CLASSIFIED: [&str; 3] = ["order.create", "order.cancel", "support.context_exit"];
    let operations = ledger();
    let _ = captured(async {
        let router = verdaxis_read::router(offline_state(Some(options(true))));
        for (index, operation) in operations.iter().enumerate() {
            let request = invalid_bearer(&operation.method, &operation.path(), &request_id(index))
                .header(CONTEXT_HEADER, Uuid::new_v4().to_string())
                .body(Body::from("{}"))
                .unwrap();
            let (status, body) = send(&router, request).await;
            if operation.writes() && !CLASSIFIED.contains(&operation.id.as_str()) {
                assert_eq!(status, StatusCode::FORBIDDEN, "{}", operation.id);
                assert_eq!(
                    body["detail"]["code"], "MARKET_SUPPORT_MUTATION_NOT_ALLOWED",
                    "{}",
                    operation.id
                );
            } else {
                assert_eq!(
                    status.as_u16(),
                    operation.support_enabled,
                    "{}",
                    operation.id
                );
            }
        }
    });
}

fn environment(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    let mut environment: HashMap<String, String> = [
        ("RELEASE_SHA", "a".repeat(40)),
        ("ENVIRONMENT", "staging".into()),
        ("MIGRATION_REVISION", "p89_20261022_trade_freeze".into()),
        (
            "DATABASE_URL",
            "postgresql://verdaxis_app_staging:fixture-secret@127.0.0.1:5432/verdaxis_staging"
                .into(),
        ),
        ("DB_SESSION_TIME_ZONE", "UTC".into()),
        ("JWT_SECRET", SECRET.into()),
        ("JWT_ISSUER", ISSUER.into()),
        ("JWT_AUDIENCE", AUDIENCE.into()),
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
    environment
}

fn deployed(pairs: &[(&str, &str)]) -> Result<MarketWriteOptions, String> {
    let budget =
        json!({"version":1,"release_sha":"a".repeat(40),"environment_digest":"b".repeat(64),
        "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":17,
        "python_budget":52,"total":69,"configured_max_connections":100})
        .to_string();
    config_from_deployed_environment(
        &environment(pairs),
        "127.0.0.1:8011".parse().unwrap(),
        &budget,
    )
    .map(|config| config.phase2.market_write)
}

#[test]
fn deployed_market_keys_follow_python() {
    let defaults = deployed(&[]).unwrap();
    assert!(defaults.auto_matching_enabled, "absent means enabled");
    assert!(!defaults.support_enabled);
    assert_eq!(defaults.support_max_ttl_hours, 168);
    assert_eq!(defaults.support_context_ttl_minutes, 60);
    assert_eq!(defaults.support_consent_version, "v1");
    assert_eq!(defaults.support_consent_reference, "market-support/v1");

    let disabled = deployed(&[("AUTO_MATCHING_ENABLED", "Off")]).unwrap();
    assert!(!disabled.auto_matching_enabled);
    let enabled = deployed(&[
        ("MARKET_SUPPORT_ENABLED", "yes"),
        ("MARKET_SUPPORT_CONSENT_VERSION", " 2026-10-01 "),
        ("MARKET_SUPPORT_CONSENT_REFERENCE", "Staging support terms"),
        ("MARKET_SUPPORT_MAX_TTL_HOURS", "720"),
        ("MARKET_SUPPORT_CONTEXT_TTL_MINUTES", " 1 "),
    ])
    .unwrap();
    assert!(enabled.support_enabled);
    assert_eq!(enabled.support_consent_version, "2026-10-01");
    assert_eq!(enabled.support_max_ttl_hours, 720);
    assert_eq!(enabled.support_context_ttl_minutes, 1);

    for (pairs, reason) in [
        (
            &[("AUTO_MATCHING_ENABLED", "maybe")][..],
            "AUTO_MATCHING_ENABLED",
        ),
        (
            &[("MARKET_SUPPORT_ENABLED", "2")][..],
            "MARKET_SUPPORT_ENABLED",
        ),
        (
            &[("MARKET_SUPPORT_MAX_TTL_HOURS", "0")][..],
            "MARKET_SUPPORT_MAX_TTL_HOURS",
        ),
        (
            &[("MARKET_SUPPORT_MAX_TTL_HOURS", "721")][..],
            "MARKET_SUPPORT_MAX_TTL_HOURS",
        ),
        (
            &[("MARKET_SUPPORT_CONTEXT_TTL_MINUTES", "1441")][..],
            "MARKET_SUPPORT_CONTEXT_TTL_MINUTES",
        ),
        (
            &[("MARKET_SUPPORT_CONTEXT_TTL_MINUTES", "ten")][..],
            "MARKET_SUPPORT_CONTEXT_TTL_MINUTES",
        ),
        (
            &[("MARKET_SUPPORT_ENABLED", "true")][..],
            "explicitly configured",
        ),
        (
            &[
                ("MARKET_SUPPORT_ENABLED", "true"),
                ("MARKET_SUPPORT_CONSENT_VERSION", " "),
                ("MARKET_SUPPORT_CONSENT_REFERENCE", "terms"),
            ][..],
            "required when enabled",
        ),
    ] {
        let refusal = deployed(pairs).err().unwrap_or_default();
        assert!(refusal.contains(reason), "{pairs:?}: {refusal}");
    }
}

#[tokio::test]
async fn malformed_market_settings_refuse_startup() {
    let lazy = |connections| {
        PgPoolOptions::new()
            .max_connections(connections)
            .connect_lazy(NO_DATABASE)
            .unwrap()
    };
    for market in [
        json!({"support_context_ttl_minutes": 0}),
        json!({"support_max_ttl_hours": 721}),
        json!({"analytics_enabled": true, "analytics_website_id": "site", "analytics_base_url": "ftp://collector.example.test"}),
    ] {
        let pools = Pools {
            read: lazy(8),
            health: lazy(1),
        };
        assert!(
            AppState::new(
                config(OFFLINE_CONFIG_URL, Some(market.clone())),
                pools,
                policy()
            )
            .is_err(),
            "{market}"
        );
    }
}

#[test]
fn bootstrap_arguments_take_only_the_flag_and_an_absolute_budget_file() {
    let arguments = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        bootstrap_arguments(&arguments(&[
            "--bootstrap-watchlist-producer",
            "--budget-file",
            "/srv/budget.json"
        ]))
        .unwrap(),
        std::path::PathBuf::from("/srv/budget.json")
    );
    for refused in [
        &["--bootstrap-watchlist-producer"][..],
        &[
            "--bootstrap-watchlist-producer",
            "--budget-file",
            "budget.json",
        ][..],
        &["--bootstrap-watchlist-producer", "--bind", "127.0.0.1:1"][..],
        &[
            "--bootstrap-watchlist-producer",
            "--budget-file",
            "/srv/budget.json",
            "--bind",
            "127.0.0.1:1",
        ][..],
    ] {
        assert!(
            bootstrap_arguments(&arguments(refused)).is_err(),
            "{refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Database cases.

struct Fixture {
    _database: market_database::MarketDatabase,
    admin: PgConnection,
    runtime_url: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = market_database::MarketDatabase::new("RUST_SERVING").await;
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        for source in [
            include_str!("fixtures/market_write_matching.sql"),
            include_str!("fixtures/market_write_publish.sql"),
            include_str!("fixtures/market_write_create.sql"),
            include_str!("fixtures/phase9_races.sql"),
            include_str!("fixtures/phase8_commission.sql"),
            include_str!("fixtures/phase89_serving.sql"),
        ] {
            sqlx::raw_sql(source).execute(&mut admin).await.unwrap();
        }
        sqlx::query("SELECT serving_seed()")
            .execute(&mut admin)
            .await
            .unwrap();
        let runtime_url = database.runtime_url.clone();
        Self {
            _database: database,
            admin,
            runtime_url,
        }
    }

    async fn state(&self, market: Value) -> (AppState, PgPool) {
        let config = config(&self.runtime_url, Some(market));
        let mut pools = verdaxis_read::connect(&config).await.unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&self.runtime_url)
            .await
            .unwrap();
        pools.read = pool.clone();
        (AppState::new(config, pools, policy()).unwrap(), pool)
    }

    async fn router(&self, market: Value) -> (Router, PgPool) {
        let (state, pool) = self.state(market).await;
        (verdaxis_read::router(state), pool)
    }

    async fn markers(&mut self) -> Value {
        sqlx::query_scalar(MARKERS)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    async fn scalar<T>(&mut self, sql: &'static str) -> T
    where
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
    {
        sqlx::query_scalar(sql)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }
}

fn token(actor: u128) -> String {
    let now = chrono::Utc::now().timestamp();
    encode(
        &Header::default(),
        &json!({"sub":id(actor),"type":"access","iat":now,"exp":now+600,"iss":ISSUER,"aud":AUDIENCE}),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

async fn call(
    router: &Router,
    actor: u128,
    method: &str,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(actor)))
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    send(router, request.body(body).unwrap()).await
}

fn bid(price: &str, point: Uuid) -> Value {
    json!({"side":"BID","product_id":PRODUCT,"delivery_point_id":point,"quantity_mt":"10","price_per_mt_usd":price})
}

async fn rotterdam(f: &mut Fixture) -> Uuid {
    f.scalar("SELECT id FROM delivery_points WHERE name='Rotterdam'")
        .await
}

fn uuid_of(body: &Value, field: &str) -> Uuid {
    body[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} in {body}"))
        .parse()
        .unwrap()
}

/// The durable marker each market write must raise. Audit-only writes name
/// `audit_actions`, so a renamed audit action fails the marker test.
fn markers_raised_by(operation: &str) -> &'static [&'static str] {
    match operation {
        "freeze.freeze" => &["trade_freezes", "audit_actions"],
        "freeze.unfreeze" | "commission.update" => &["audit_actions"],
        "suggestion.dismiss" => &["dismissed_suggestions"],
        "support.capability_grant" => &["capability_assignments", "audit_actions"],
        "support.context_start" => &["support_contexts", "audit_actions"],
        "support.context_exit" | "support.capability_revoke" => &["audit_actions"],
        operation
            if ["inventory.", "order.", "trade."]
                .iter()
                .any(|prefix| operation.starts_with(prefix)) =>
        {
            &["outbox_evidence"]
        }
        other => panic!("no marker listed for {other}"),
    }
}

/// Every market write that succeeds through the served router raises at least
/// one durable marker of `fixtures/market_write_markers.sql` (the marker set
/// the switch pack counts against its baseline).
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn every_market_write_moves_a_marker() {
    let mut f = Fixture::new().await;
    let point = rotterdam(&mut f).await;
    let (router, pool) = f.router(options(true)).await;
    let product: Value = serde_json::to_value(&policy().products)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == PRODUCT)
        .cloned()
        .unwrap();
    let mut covered = Vec::new();
    let mut step =
        |name: &'static str, before: &Value, after: &Value, result: &(StatusCode, Value)| {
            assert!(result.0.is_success(), "{name}: {result:?}");
            for marker in markers_raised_by(name) {
                assert!(
                    after[*marker].as_i64() > before[*marker].as_i64(),
                    "{name} did not raise {marker}: {before} -> {after}"
                );
            }
            covered.push(name);
        };

    // Inventory create, update and delete.
    let before = f.markers().await;
    let created = call(&router, SELLER, "POST", "/api/inventory", json!({"port_id":"pub-port","product_name":product["name"],"fuel_type":product["fuel_type"],"current_stock_mt":"0","incoming_stock_mt":"5","price_per_mt_usd":"120"}), None).await;
    step("inventory.create", &before, &f.markers().await, &created);
    let item = uuid_of(&created.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "PATCH",
        &format!("/api/inventory/{item}"),
        json!({"current_stock_mt":"3"}),
        None,
    )
    .await;
    step("inventory.update", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "DELETE",
        &format!("/api/inventory/{item}"),
        Value::Null,
        None,
    )
    .await;
    step("inventory.delete", &before, &f.markers().await, &result);

    // Publication, order entry, amendment and both cancellation forms.
    let before = f.markers().await;
    let published = call(
        &router,
        SELLER,
        "POST",
        &format!("/api/inventory/{}/publish", id(ITEM)),
        Value::Null,
        None,
    )
    .await;
    step("inventory.publish", &before, &f.markers().await, &published);
    let listing = uuid_of(&published.1, "listing_id");
    let before = f.markers().await;
    let created = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("50", point),
        Some("serving-create-1"),
    )
    .await;
    step("order.create", &before, &f.markers().await, &created);
    let order = uuid_of(&created.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        BUYER,
        "PUT",
        &format!("/api/orderbook/{order}"),
        json!({"price_per_mt_usd":"55"}),
        None,
    )
    .await;
    step("order.amend", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        BUYER,
        "POST",
        &format!("/api/orderbook/{order}/cancel"),
        json!({"reason":"No longer needed"}),
        None,
    )
    .await;
    step("order.cancel", &before, &f.markers().await, &result);
    let second = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("51", point),
        Some("serving-create-2"),
    )
    .await;
    let second = uuid_of(&second.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        BUYER,
        "DELETE",
        &format!("/api/orderbook/{second}"),
        Value::Null,
        None,
    )
    .await;
    step("order.delete", &before, &f.markers().await, &result);

    // Order taking and the trade lifecycle.
    let take = |quantity: &'static str| {
        let pool = pool.clone();
        async move {
            let row: Value =
                sqlx::query_scalar("SELECT to_jsonb(o) FROM orderbook_orders o WHERE id=$1")
                    .bind(listing)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let digest = verdaxis_read::canonical_terms_from_row(&row)
                .and_then(|v| verdaxis_read::digest_terms(&v))
                .unwrap();
            json!({"order_id":listing,"quantity_mt":quantity,"expected_terms_digest":digest})
        }
    };
    let before = f.markers().await;
    let traded = call(
        &router,
        BUYER,
        "POST",
        "/api/trades/",
        take("10").await,
        Some("serving-take-1"),
    )
    .await;
    step("trade.create", &before, &f.markers().await, &traded);
    let trade = uuid_of(&traded.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "PUT",
        &format!("/api/trades/{trade}/confirm"),
        Value::Null,
        None,
    )
    .await;
    step("trade.confirm", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "PUT",
        &format!("/api/trades/{trade}/deliver"),
        json!({"final_quantity_mt":"10","final_price_per_mt":"100"}),
        None,
    )
    .await;
    step("trade.deliver", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "POST",
        &format!("/api/trades/{trade}/pay"),
        Value::Null,
        None,
    )
    .await;
    step("trade.pay", &before, &f.markers().await, &result);
    let declined = call(
        &router,
        BUYER,
        "POST",
        "/api/trades",
        take("5").await,
        Some("serving-take-2"),
    )
    .await;
    assert!(declined.0.is_success(), "{declined:?}");
    let declined = uuid_of(&declined.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        SELLER,
        "PUT",
        &format!("/api/trades/{declined}/decline"),
        Value::Null,
        None,
    )
    .await;
    step("trade.decline", &before, &f.markers().await, &result);

    // Freeze administration on a fresh pending trade.
    let frozen = call(
        &router,
        BUYER,
        "POST",
        "/api/trades/",
        take("5").await,
        Some("serving-take-3"),
    )
    .await;
    let frozen = uuid_of(&frozen.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        ADMIN,
        "POST",
        &format!("/api/admin/trades/{frozen}/freeze"),
        json!({"reason_code":"OTHER","note":"review"}),
        None,
    )
    .await;
    step("freeze.freeze", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        ADMIN,
        "POST",
        &format!("/api/admin/trades/{frozen}/unfreeze"),
        json!({"note":"complete"}),
        None,
    )
    .await;
    step("freeze.unfreeze", &before, &f.markers().await, &result);

    // Commission administration.
    let commission: Uuid = f
        .scalar("SELECT id FROM commissions ORDER BY created_at LIMIT 1")
        .await;
    let before = f.markers().await;
    let result = call(
        &router,
        ADMIN,
        "PUT",
        &format!("/api/orders/admin/commissions/{commission}"),
        json!({"status":"INVOICED"}),
        None,
    )
    .await;
    step("commission.update", &before, &f.markers().await, &result);

    // Suggestion dismissal.
    let before = f.markers().await;
    let result = call(
        &router,
        BUYER,
        "PATCH",
        &format!("/api/matchmaking/suggestions/{listing}/dismiss"),
        Value::Null,
        None,
    )
    .await;
    step("suggestion.dismiss", &before, &f.markers().await, &result);

    // Market support: capability grant, context start and exit, revoke.
    let root = "/api/admin/market-support";
    let before = f.markers().await;
    let granted = call(&router, ADMIN, "POST", &format!("{root}/capability-assignments"), json!({"user_id":id(SUPPORT_ADMIN),"capability":"MARKET_SUPPORT_LISTINGS","reason":"Support request"}), None).await;
    step(
        "support.capability_grant",
        &before,
        &f.markers().await,
        &granted,
    );
    let assignment = uuid_of(&granted.1, "id");
    // Starting a context takes both support capabilities.
    let second = call(&router, ADMIN, "POST", &format!("{root}/capability-assignments"), json!({"user_id":id(SUPPORT_ADMIN),"capability":"MARKET_SUPPORT_AUTHORIZATIONS","reason":"Support request"}), None).await;
    assert_eq!(second.0, StatusCode::CREATED, "{second:?}");
    let before = f.markers().await;
    let started = call(&router, SUPPORT_ADMIN, "POST", &format!("{root}/contexts"), json!({"organization_id":id(BUYER_ORG),"support_reference":"Serving case","confirm_replacement":false}), None).await;
    step(
        "support.context_start",
        &before,
        &f.markers().await,
        &started,
    );
    let context = uuid_of(&started.1, "id");
    let before = f.markers().await;
    let result = call(
        &router,
        SUPPORT_ADMIN,
        "POST",
        &format!("{root}/contexts/{context}/exit"),
        Value::Null,
        None,
    )
    .await;
    step("support.context_exit", &before, &f.markers().await, &result);
    let before = f.markers().await;
    let result = call(
        &router,
        ADMIN,
        "POST",
        &format!("{root}/capability-assignments/{assignment}/revoke"),
        json!({"reason":"Request complete"}),
        None,
    )
    .await;
    step(
        "support.capability_revoke",
        &before,
        &f.markers().await,
        &result,
    );

    // Every write operation in the ledger is covered, except the four retired
    // legacy support writes, which never succeed.
    let writes: Vec<String> = ledger()
        .into_iter()
        .filter(|operation| operation.writes() && !operation.id.ends_with("_retired"))
        .map(|operation| operation.id)
        .collect();
    let mut expected = writes.clone();
    expected.sort();
    let mut seen: Vec<String> = covered.iter().map(|name| (*name).to_owned()).collect();
    seen.sort();
    assert_eq!(seen, expected);
    pool.close().await;
}

/// `AUTO_MATCHING_ENABLED=false` reaches order entry, amendment and
/// publication: a crossing order rests. The absent key matches at once.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn auto_matching_and_support_follow_the_deployed_keys() {
    let mut f = Fixture::new().await;
    let point = rotterdam(&mut f).await;
    let mut disabled = options(false);
    disabled["auto_matching_enabled"] = json!(false);
    let (router, pool) = f.router(disabled).await;
    let resting = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("90", point),
        Some("auto-1"),
    )
    .await;
    assert_eq!(resting.0, StatusCode::CREATED, "{resting:?}");
    let resting = uuid_of(&resting.1, "id");
    let published = call(
        &router,
        SELLER,
        "POST",
        &format!("/api/inventory/{}/publish", id(ITEM)),
        Value::Null,
        None,
    )
    .await;
    assert!(published.0.is_success(), "{published:?}");
    let crossing = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("120", point),
        Some("auto-2"),
    )
    .await;
    assert_eq!(crossing.0, StatusCode::CREATED, "{crossing:?}");
    let amended = call(
        &router,
        BUYER,
        "PUT",
        &format!("/api/orderbook/{resting}"),
        json!({"price_per_mt_usd":"130"}),
        None,
    )
    .await;
    assert!(amended.0.is_success(), "{amended:?}");
    assert_eq!(f.scalar::<i64>("SELECT count(*) FROM trades").await, 0);
    // Support disabled: the support routes answer as Python's disabled gate.
    let support = call(
        &router,
        ADMIN,
        "GET",
        "/api/admin/market-support/capabilities",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(support.0, StatusCode::NOT_FOUND, "{support:?}");
    pool.close().await;

    let (router, pool) = f.router(json!({})).await;
    let matched = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("125", point),
        Some("auto-3"),
    )
    .await;
    assert_eq!(matched.0, StatusCode::CREATED, "{matched:?}");
    assert!(f.scalar::<i64>("SELECT count(*) FROM trades").await >= 1);
    pool.close().await;
}

/// The conversion event leaves only after the order commits, and a replay of
/// the same idempotency key sends none.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn served_create_sends_analytics_after_commit() {
    use axum::{extract::State, routing::post};
    #[derive(Clone, Default)]
    struct Collector {
        rows: Arc<tokio::sync::Mutex<Vec<Value>>>,
        arrived: Arc<tokio::sync::Notify>,
    }
    async fn receive(
        State(c): State<Collector>,
        axum::Json(v): axum::Json<Value>,
    ) -> axum::Json<Value> {
        c.rows.lock().await.push(v);
        c.arrived.notify_one();
        axum::Json(json!({"ok": true}))
    }
    let collector = Collector::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/api/send", post(receive))
        .with_state(collector.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut f = Fixture::new().await;
    let point = rotterdam(&mut f).await;
    let mut market = options(false);
    market["analytics_enabled"] = json!(true);
    market["analytics_base_url"] = json!(format!("http://{address}"));
    market["analytics_website_id"] = json!("serving-site");
    let (router, pool) = f.router(market).await;
    let created = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("60", point),
        Some("analytics-1"),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{created:?}");
    let order = uuid_of(&created.1, "id");
    tokio::time::timeout(Duration::from_secs(5), collector.arrived.notified())
        .await
        .expect("one conversion event");
    let rows = collector.rows.lock().await.clone();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["payload"]["website"], "serving-site");
    // The committed order exists by the time the event is delivered.
    let committed: i64 = sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE id=$1")
        .bind(order)
        .fetch_one(&mut f.admin)
        .await
        .unwrap();
    assert_eq!(committed, 1);
    let replay = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("60", point),
        Some("analytics-1"),
    )
    .await;
    assert_eq!(uuid_of(&replay.1, "id"), order, "{replay:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        collector.rows.lock().await.len(),
        1,
        "a replay sends no event"
    );
    pool.close().await;
}

/// A Rust market write wakes a live subscriber of the market event channel
/// when it commits, with Python's empty payload, so delivery never waits for
/// the dispatchers' two-second poll; the committed rows are visible by then.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_market_write_wakes_live_subscribers_on_commit() {
    let mut f = Fixture::new().await;
    let point = rotterdam(&mut f).await;
    let (router, pool) = f.router(options(false)).await;
    let mut listener = sqlx::postgres::PgListener::connect(&f.runtime_url)
        .await
        .unwrap();
    listener
        .listen(verdaxis_read::streams::election::WAKE_CHANNEL)
        .await
        .unwrap();
    let before: i64 = f.scalar("SELECT count(*) FROM market_event_outbox").await;
    let started = std::time::Instant::now();
    let created = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("70", point),
        Some("wake-1"),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{created:?}");
    let notification = tokio::time::timeout(Duration::from_secs(1), listener.recv())
        .await
        .expect("a wake before the next poll")
        .unwrap();
    assert!(
        started.elapsed() < verdaxis_read::streams::election::POLL_INTERVAL,
        "the wake arrived within one poll interval"
    );
    assert_eq!(notification.channel(), "verdaxis_market_events");
    assert_eq!(notification.payload(), "");
    let after: i64 = f.scalar("SELECT count(*) FROM market_event_outbox").await;
    assert!(
        after > before,
        "the woken subscriber sees the committed rows"
    );
    // One commit, one wake: repeated signals of one transaction fold together.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err(),
        "no second wake for the same commit"
    );
    pool.close().await;
}

/// A Rust market write reaches a live stream subscriber (the Rust sequencer
/// election, dispatcher and hub) without waiting for the poll.
///
/// The election waits up to one poll interval for a wake after each turn. The
/// next write is sent right after the previous delivery, so without the
/// commit-time wake its delivery would wait for that poll, one interval after
/// the previous turn, less the time the write itself took. Each delivery must
/// come within a quarter of an interval after the write returns.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn rust_market_writes_reach_a_live_stream_subscriber_without_waiting_for_the_poll() {
    use verdaxis_read::streams::election::POLL_INTERVAL;
    use verdaxis_read::streams::runtime::{CapacityConfig, start_streams};
    let mut f = Fixture::new().await;
    let point = rotterdam(&mut f).await;
    let (router, pool) = f.router(options(false)).await;
    let mut config = config(&f.runtime_url, Some(options(false)));
    // Two services of four workers, each with pool 2, overflow 1 and one
    // listener, plus the reserve: the Python budget of 52 connections.
    config.python_budget = 52;
    config.prepared_budget = Some(
        json!({"version":1,"release_sha":config.release_sha,"environment_digest":"a".repeat(64),
            "services":2,"workers":4,"pool":2,"overflow":1,"reserve":20,"rust_pool":config.pool_max,
            "python_budget":52,"cache_listener":0,"stream_listener":1,"total":52+config.pool_max+1,
            "configured_max_connections":config.configured_max_connections})
        .to_string(),
    );
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let mut streams = AppState::new(config, pools, policy()).unwrap();
    let runtime = start_streams(&mut streams, CapacityConfig::default())
        .await
        .expect("the stream runtime starts");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !verdaxis_read::streams::runtime::ready(&streams) {
        assert!(
            std::time::Instant::now() < deadline,
            "the stream runtime is not ready"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let hub = streams.streams.as_ref().unwrap().hub.clone();
    let mut buyer = hub.subscribe_org(id(BUYER_ORG)).unwrap();
    let mut latencies = Vec::new();
    for round in 0..4 {
        let price = format!("{}", 40 + round);
        let key = format!("live-{round}");
        let started = std::time::Instant::now();
        let created = call(
            &router,
            BUYER,
            "POST",
            "/api/orderbook",
            bid(&price, point),
            Some(&key),
        )
        .await;
        assert_eq!(created.0, StatusCode::CREATED, "{created:?}");
        let committed = std::time::Instant::now();
        println!("LIVE_WRITE_MS {}", started.elapsed().as_millis());
        tokio::time::timeout(Duration::from_secs(5), buyer.recv())
            .await
            .expect("the write is delivered")
            .expect("an open subscription");
        latencies.push(committed.elapsed());
    }
    println!(
        "LIVE_DELIVERY_MS {:?}",
        latencies
            .iter()
            .map(Duration::as_millis)
            .collect::<Vec<_>>()
    );
    // The first round may meet a turn already under way; the later ones each
    // start right after a turn.
    for latency in &latencies[1..] {
        assert!(
            *latency < POLL_INTERVAL / 4,
            "a delivery waited for the poll: {latencies:?}"
        );
    }
    runtime.shutdown().await;
    pool.close().await;
}

/// The operator bootstrap creates the watermark once, reports it unchanged
/// after that, refuses once trusted evidence exists without a watermark, and
/// refuses before the producer checkpoint without touching the database.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn bootstrap_mode_creates_once_and_refuses_trusted_evidence() {
    use verdaxis_read::watchlist_producer::{BootstrapOutcome, bootstrap_command};
    let mut f = Fixture::new().await;
    let config = config(&f.runtime_url, Some(options(false)));
    let created = bootstrap_command(&config).await.unwrap();
    let BootstrapOutcome::Created { watermark } = created else {
        panic!("{created:?}");
    };
    assert_eq!(
        created.report(),
        format!("outcome=created watermark={watermark}")
    );
    let again = bootstrap_command(&config).await.unwrap();
    assert_eq!(again, BootstrapOutcome::AlreadyPresent { watermark });
    assert_eq!(
        again.report(),
        format!("outcome=already_present watermark={watermark}")
    );

    let mut early = config.clone();
    early.migration_revision = "p5_20261020_read_indexes".into();
    early.database_url = "postgresql://unused:unused-secret@127.0.0.1:1/unused_test".into();
    assert_eq!(
        bootstrap_command(&early).await,
        Err("producer_schema_absent")
    );

    // A second database where a Rust write left evidence before any bootstrap.
    let mut g = Fixture::new().await;
    let point = rotterdam(&mut g).await;
    let (router, pool) = g.router(options(false)).await;
    let created = call(
        &router,
        BUYER,
        "POST",
        "/api/orderbook",
        bid("80", point),
        Some("evidence-1"),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED, "{created:?}");
    pool.close().await;
    let evidence_config = crate::config(&g.runtime_url, Some(options(false)));
    assert_eq!(
        bootstrap_command(&evidence_config).await,
        Err("trusted_evidence_exists")
    );
    let watermarks: i64 = g
        .scalar("SELECT count(*) FROM watchlist_producer_state")
        .await;
    assert_eq!(watermarks, 0);
    let first: i64 = f
        .scalar("SELECT count(*) FROM watchlist_producer_state")
        .await;
    assert_eq!(first, 1);
}
