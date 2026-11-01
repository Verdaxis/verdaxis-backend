//! Phase 10 Task 27: the local health monitor (`monitor health`).
//!
//! Every case uses fixture files in a scratch directory and a fake loopback
//! server. Nothing here contacts a real service or writes outside the scratch
//! directory.
#[path = "support/monitor_fixture.rs"]
mod support;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{TempDir, contract_fixture, repository_root};
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::monitors::health::{
    DiskUsage, Endpoint, HealthConfig, ReadinessRefusal, check_directory_size, check_filesystem,
    check_readiness, collect, execute_checks, read_identity, run, validate_readiness_payload,
};
use verdaxis_read::cli::monitors::status::{
    Admission, AdmitAll, Category, CheckResult, Fault, Previous, Status, build_status,
    failure_status, guarded, load_monitor_status, valid_monitor_status,
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, 0, 0, 0).unwrap()
}

fn readiness(environment: &str, sha: &str) -> Value {
    json!({"status": "ok", "db": "ok", "environment": environment, "release_sha": sha})
}

// ---- fake loopback server ---------------------------------------------------

type Handler = dyn Fn(&str, &mut TcpStream) + Send + Sync;

struct FakeServer {
    port: u16,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeServer {
    fn start(handler: Arc<Handler>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let (stop_flag, request_count) = (stop.clone(), requests.clone());
        let thread = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let handler = handler.clone();
                        let request_count = request_count.clone();
                        std::thread::spawn(move || {
                            stream.set_nonblocking(false).unwrap();
                            if let Some(path) = read_request_path(&mut stream) {
                                request_count.fetch_add(1, Ordering::SeqCst);
                                handler(&path, &mut stream);
                            }
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Self {
            port,
            stop,
            requests,
            thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 512];
    while !data.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    }
    let line = String::from_utf8_lossy(&data).lines().next()?.to_owned();
    line.split(' ').nth(1).map(str::to_owned)
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn router() -> FakeServer {
    FakeServer::start(Arc::new(|path, stream| {
        let good = readiness("production", SHA).to_string();
        match path {
            "/ready" => respond(stream, "200 OK", &[], good.as_bytes()),
            "/extra" => {
                let mut body = readiness("production", SHA);
                body["build_number"] = json!(7);
                respond(stream, "200 OK", &[], body.to_string().as_bytes());
            }
            "/nested" => {
                let mut body = readiness("production", SHA);
                body["debug"] = json!({"secret": true});
                respond(stream, "200 OK", &[], body.to_string().as_bytes());
            }
            "/secret" => respond(stream, "200 OK", &[], br#"{"password":"do-not-copy"}"#),
            "/duplicate" => {
                let body = format!(
                    r#"{{"status":"ok","db":"ok","environment":"production","release_sha":"{SHA}","status":"ok"}}"#
                );
                respond(stream, "200 OK", &[], body.as_bytes());
            }
            "/wrong-environment" => respond(
                stream,
                "200 OK",
                &[],
                readiness("staging", SHA).to_string().as_bytes(),
            ),
            "/wrong-sha" => respond(
                stream,
                "200 OK",
                &[],
                readiness("production", OTHER_SHA).to_string().as_bytes(),
            ),
            "/not-ok" => respond(
                stream,
                "200 OK",
                &[],
                json!({"status":"ok","db":"connected","environment":"production","release_sha":SHA})
                    .to_string()
                    .as_bytes(),
            ),
            "/down" => respond(
                stream,
                "503 Service Unavailable",
                &[],
                json!({"status":"error","db":"unavailable","environment":"production","release_sha":SHA})
                    .to_string()
                    .as_bytes(),
            ),
            "/large" => respond(stream, "200 OK", &[], &[b'x'; 4096]),
            "/redirect" => respond(stream, "302 Found", &[("Location", "/ready")], b"redirect"),
            "/chunked" => {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n0\r\n\r\n",
                );
            }
            "/huge-headers" => {
                let filler = "y".repeat(20_000);
                respond(stream, "200 OK", &[("X-Filler", filler.as_str())], good.as_bytes());
            }
            "/duplicate-header" => respond(
                stream,
                "200 OK",
                &[("X-One", "1"), ("x-one", "2")],
                good.as_bytes(),
            ),
            "/unframed-flood" => {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                let block = [b'z'; 1024];
                for _ in 0..64 {
                    if stream.write_all(&block).is_err() {
                        break;
                    }
                }
            }
            "/short-body" => {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{}",
                );
            }
            "/garbage" => {
                let _ = stream.write_all(b"not http at all\r\n\r\n");
            }
            "/slow" => {
                std::thread::sleep(Duration::from_millis(400));
                respond(stream, "200 OK", &[], good.as_bytes());
            }
            "/slow-drip" => {
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        good.len()
                    )
                    .as_bytes(),
                );
                for byte in good.bytes() {
                    if stream.write_all(&[byte]).is_err() || stream.flush().is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            _ => respond(stream, "404 Not Found", &[], b"missing"),
        }
    }))
}

fn check(server: &FakeServer, path: &str, timeout_ms: u64, max_body: usize) -> CheckResult {
    check_readiness(
        "test",
        &server.url(path),
        "production",
        SHA,
        Duration::from_millis(timeout_ms),
        max_body,
    )
}

// ---- identity and readiness contract ----------------------------------------

#[test]
fn health_validates_the_exact_identity_and_readiness_keys() {
    // The shared readiness corpus decides every payload.
    let corpus: Value = serde_json::from_slice(
        &std::fs::read(repository_root().join("deploy/monitor/runtime-v2-readiness-corpus.json"))
            .expect("read the shared readiness corpus"),
    )
    .unwrap();
    let cases = corpus["cases"].as_array().expect("corpus cases");
    assert!(cases.len() >= 9);
    let environment = corpus["contract"]["environment"].as_str().unwrap();
    let sha = corpus["contract"]["release_sha"].as_str().unwrap();
    for case in cases {
        let accepted = validate_readiness_payload(&case["payload"], environment, sha).is_ok();
        assert_eq!(accepted, case["valid"], "corpus case {}", case["name"]);
    }
    // Each refusal class keeps its own name.
    let mut changed = readiness("production", SHA);
    changed["environment"] = json!("staging");
    assert_eq!(
        validate_readiness_payload(&changed, "production", SHA),
        Err(ReadinessRefusal::Identity)
    );
    assert_eq!(
        validate_readiness_payload(&json!({"status": "ok", "db": "down"}), "production", SHA),
        Err(ReadinessRefusal::Schema)
    );
    assert_eq!(
        validate_readiness_payload(
            &json!({"status":"error","db":"unavailable","environment":"production","release_sha":SHA}),
            "production",
            SHA
        ),
        Err(ReadinessRefusal::Unhealthy)
    );
    assert_eq!(
        validate_readiness_payload(&json!(["not", "an", "object"]), "production", SHA),
        Err(ReadinessRefusal::Schema)
    );

    // Identity files hold exactly two data lines.
    let scratch = TempDir::new();
    let file = scratch.join("production.identity");
    std::fs::write(
        &file,
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"),
    )
    .unwrap();
    let identity = read_identity(&file, "production").expect("valid identity file");
    assert_eq!(
        (identity.environment.as_str(), identity.release_sha.as_str()),
        ("production", SHA)
    );
    std::fs::write(
        &file,
        format!("RELEASE_SHA={SHA}\nENVIRONMENT=production\n"),
    )
    .unwrap();
    assert!(
        read_identity(&file, "production").is_ok(),
        "line order is free"
    );
    for raw in [
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}\nEXTRA=forbidden\n"),
        format!("ENVIRONMENT=production\nENVIRONMENT=production\nRELEASE_SHA={SHA}\n"),
        "ENVIRONMENT=production\n".to_owned(),
        format!("# comment\nENVIRONMENT=production\nRELEASE_SHA={SHA}\n"),
        format!("ENVIRONMENT=production\n\nRELEASE_SHA={SHA}\n"),
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}"),
        format!("ENVIRONMENT=\nRELEASE_SHA={SHA}\n"),
        format!("ENVIRONMENT=staging\nRELEASE_SHA={SHA}\n"),
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}=\n"),
        format!("ENVIRONMENT=production\nRELEASE_SHA={}\n", "é".repeat(20)),
        format!(
            "ENVIRONMENT=production\nRELEASE_SHA={SHA}\n{}\n",
            "x".repeat(300)
        ),
    ] {
        std::fs::write(&file, &raw).unwrap();
        assert!(
            read_identity(&file, "production").is_err(),
            "refused: {raw:?}"
        );
    }
    // A symlink or a directory is not an identity file, and neither is a missing one.
    let actual = scratch.join("actual.identity");
    std::fs::write(
        &actual,
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"),
    )
    .unwrap();
    let linked = scratch.join("linked.identity");
    symlink(&actual, &linked).unwrap();
    assert!(read_identity(&linked, "production").is_err());
    assert!(read_identity(scratch.path(), "production").is_err());
    assert!(read_identity(&scratch.join("absent.identity"), "production").is_err());
}

#[test]
fn health_readiness_requires_exact_sanitized_json_over_loopback() {
    let server = router();
    let healthy = check(&server, "/ready", 500, 1024);
    assert_eq!(
        (healthy.category, healthy.code.as_str()),
        (Category::Healthy, "ok")
    );
    assert_eq!(healthy.name, "test");
    for (path, code) in [
        ("/extra", "unexpected_json"),
        ("/nested", "unexpected_json"),
        ("/secret", "unexpected_json"),
        ("/not-ok", "unexpected_json"),
        ("/duplicate", "invalid_json"),
        ("/wrong-environment", "identity_mismatch"),
        ("/wrong-sha", "identity_mismatch"),
        ("/redirect", "redirect_rejected"),
        ("/missing", "http_status"),
        ("/down", "http_status"),
    ] {
        let result = check(&server, path, 500, 1024);
        assert_eq!(
            (result.category, result.code.as_str()),
            (Category::Failure, code),
            "{path}"
        );
        assert!(!format!("{result:?}").contains("do-not-copy"));
    }
}

#[test]
fn health_rejects_a_zero_or_short_release_sha() {
    let scratch = TempDir::new();
    let file = scratch.join("production.identity");
    for sha in [
        "0".repeat(40),
        "a".repeat(39),
        "a".repeat(41),
        "A".repeat(40),
        "g".repeat(40),
        String::new(),
    ] {
        std::fs::write(
            &file,
            format!("ENVIRONMENT=production\nRELEASE_SHA={sha}\n"),
        )
        .unwrap();
        assert!(read_identity(&file, "production").is_err(), "sha {sha:?}");
        assert_eq!(
            validate_readiness_payload(&readiness("production", &sha), "production", &sha),
            Err(ReadinessRefusal::ExpectedIdentity),
            "expected sha {sha:?}"
        );
    }
    assert_eq!(
        validate_readiness_payload(&readiness("test", SHA), "test", SHA),
        Err(ReadinessRefusal::ExpectedIdentity),
        "only the two deployed environments are valid"
    );

    // A bad identity file stops the run before any request, with one fixed code.
    let server = router();
    let mut config = config_for(&scratch, &server, "/ready");
    let zero = scratch.join("zero.identity");
    std::fs::write(
        &zero,
        format!("ENVIRONMENT=production\nRELEASE_SHA={}\n", "0".repeat(40)),
    )
    .unwrap();
    config.endpoints[0].identity_file = zero;
    assert_eq!(run(&config, &AdmitAll), Exit::Usage);
    let published = read_status(&config.status_file);
    assert_eq!(published["category"], "failure");
    assert_eq!(published["checks"][0]["code"], "fatal_error");
    assert_eq!(published["checks"][0]["name"], "monitor");
    assert_eq!(
        server.request_count(),
        0,
        "no readiness request after a bad identity"
    );
}

fn config_for(scratch: &TempDir, server: &FakeServer, path: &str) -> HealthConfig {
    let identity = scratch.join("production.identity");
    std::fs::write(
        &identity,
        format!("ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"),
    )
    .unwrap();
    HealthConfig {
        status_file: scratch.join("state/status.json"),
        endpoints: vec![Endpoint {
            name: "prod_ready",
            url: server.url(path),
            expected_environment: "production",
            identity_file: identity,
        }],
        filesystem: scratch.path().to_path_buf(),
        // Free space above 0.002 percent is healthy, so a tiny scratch volume still passes.
        warning_free_percent: 0.002,
        critical_free_percent: 0.001,
        analytics_directory: None,
        analytics_max_bytes: 5 * 1024 * 1024 * 1024,
        analytics_max_entries: 100_000,
        request_timeout: Duration::from_secs(2),
        total_timeout: Duration::from_secs(5),
        max_body_bytes: 1024,
    }
}

fn read_status(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("status file")).expect("status JSON")
}

#[test]
fn health_default_endpoints_are_the_fixed_numeric_loopback_pair() {
    let config = HealthConfig::default();
    let inventory: Vec<(&str, &str, &str)> = config
        .endpoints
        .iter()
        .map(|endpoint| {
            (
                endpoint.name,
                endpoint.url.as_str(),
                endpoint.expected_environment,
            )
        })
        .collect();
    assert_eq!(
        inventory,
        [
            (
                "prod_ready",
                "http://127.0.0.1:8000/health/ready",
                "production"
            ),
            (
                "staging_ready",
                "http://127.0.0.1:8001/health/ready",
                "staging"
            ),
        ]
    );
    assert_eq!(
        config.endpoints[0].identity_file,
        Path::new("/etc/verdaxis-monitor/runtime-identities/production.identity")
    );
    assert_eq!(
        config.endpoints[1].identity_file,
        Path::new("/etc/verdaxis-monitor/runtime-identities/staging.identity")
    );
    assert_eq!(
        config.status_file,
        Path::new("/var/lib/verdaxis-health/status.json")
    );
    assert_eq!(config.filesystem, Path::new("/"));
    assert_eq!(
        (config.warning_free_percent, config.critical_free_percent),
        (20.0, 15.0)
    );
    assert!(config.analytics_directory.is_none());
    assert_eq!(config.analytics_max_bytes, 5 * 1024 * 1024 * 1024);
    assert_eq!(config.analytics_max_entries, 100_000);
    assert_eq!(config.request_timeout, Duration::from_secs(5));
    assert_eq!(config.total_timeout, Duration::from_secs(20));
    assert_eq!(config.max_body_bytes, 1024);
}

#[test]
fn health_works_with_the_database_down() {
    // The monitor reads loopback readiness and local files only. The source names no
    // database, environment or process access.
    let source = include_str!("../src/cli/monitors/health.rs").to_lowercase();
    for forbidden in [
        "sqlx",
        "pgpool",
        "database_url",
        "std::env",
        "runcontext",
        "process::command",
        "reqwest",
        "telegram",
    ] {
        assert!(
            !source.contains(forbidden),
            "health.rs must not mention {forbidden}"
        );
    }
    // A service that reports its database as down is a failed check, never a crash,
    // and a healthy report needs nothing but the service's own answer.
    let server = router();
    let scratch = TempDir::new();
    let down = config_for(&scratch, &server, "/down");
    assert_eq!(run(&down, &AdmitAll), Exit::Usage);
    let published = read_status(&down.status_file);
    assert_eq!(published["checks"][0]["code"], "http_status");
    let up = config_for(&scratch, &server, "/ready");
    assert_eq!(run(&up, &AdmitAll), Exit::Ok);
    let published = read_status(&up.status_file);
    assert_eq!(published["category"], "healthy");
    let names: Vec<&str> = published["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| check["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["prod_ready", "root_disk"]);
}

// ---- bounds -----------------------------------------------------------------

#[test]
fn health_total_timeout_and_body_bound_are_enforced() {
    let server = router();
    // Body and header bounds.
    assert_eq!(check(&server, "/large", 500, 32).code, "body_too_large");
    assert_eq!(
        check(&server, "/unframed-flood", 500, 1024).code,
        "body_too_large"
    );
    assert_eq!(
        check(&server, "/huge-headers", 500, 1024).code,
        "headers_too_large"
    );
    assert_eq!(
        check(&server, "/chunked", 500, 1024).code,
        "unsupported_transfer_encoding"
    );
    for path in ["/duplicate-header", "/short-body", "/garbage"] {
        assert_eq!(
            check(&server, path, 500, 1024).code,
            "connection_error",
            "{path}"
        );
    }
    // A slow answer and a slow-dripped body both stop at the request deadline.
    let started = Instant::now();
    assert_eq!(check(&server, "/slow", 50, 1024).code, "timeout");
    assert!(started.elapsed() < Duration::from_millis(350));
    let started = Instant::now();
    let dripped = check(&server, "/slow-drip", 80, 1024);
    assert_eq!(dripped.code, "timeout");
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "the deadline covers headers and body together"
    );
    // Nothing listens, or the address is not numeric loopback HTTP.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    for url in [
        format!("http://127.0.0.1:{closed}/health/ready"),
        format!("http://localhost:{}/ready", server.port),
        format!("https://127.0.0.1:{}/ready", server.port),
        "http://127.0.0.1/ready".to_owned(),
        "not a url".to_owned(),
    ] {
        let result = check_readiness(
            "test",
            &url,
            "production",
            SHA,
            Duration::from_millis(300),
            1024,
        );
        assert_eq!(
            (result.category, result.code.as_str()),
            (Category::Failure, "connection_error"),
            "{url}"
        );
    }

    // The total deadline stops later checks from starting.
    let elapsed = Cell::new(Duration::ZERO);
    let called = Mutex::new(Vec::new());
    let checks: Vec<verdaxis_read::cli::monitors::health::Check<'_>> = vec![
        Box::new(|_remaining| {
            called.lock().unwrap().push("first");
            elapsed.set(Duration::from_secs(2));
            CheckResult::new("first", Category::Healthy, "ok", Instant::now())
        }),
        Box::new(|_remaining| {
            called.lock().unwrap().push("second");
            CheckResult::new("second", Category::Healthy, "ok", Instant::now())
        }),
    ];
    let results = execute_checks(checks, Duration::from_secs(1), &|| elapsed.get());
    assert_eq!(*called.lock().unwrap(), ["first"]);
    let last = results.last().unwrap();
    assert_eq!(
        (
            last.name.as_str(),
            last.category,
            last.code.as_str(),
            last.duration_ms
        ),
        ("check_2", Category::Failure, "total_deadline_exceeded", 0)
    );

    // A check that panics becomes one fixed result, and later checks still run.
    let checks: Vec<verdaxis_read::cli::monitors::health::Check<'_>> = vec![
        Box::new(|_remaining| panic!("secret-value")),
        Box::new(|_remaining| CheckResult::new("after", Category::Healthy, "ok", Instant::now())),
    ];
    let results = execute_checks(checks, Duration::from_secs(1), &|| Duration::ZERO);
    assert_eq!(results.len(), 2);
    assert_eq!(
        (results[0].name.as_str(), results[0].code.as_str()),
        ("check_1", "unexpected_error")
    );
    assert!(!format!("{results:?}").contains("secret-value"));
    assert_eq!(results[1].name, "after");

    // Each request is limited by what is left of the total.
    let scratch = TempDir::new();
    let mut config = config_for(&scratch, &server, "/slow");
    config.request_timeout = Duration::from_secs(5);
    config.total_timeout = Duration::from_millis(60);
    let started = Instant::now();
    assert_eq!(run(&config, &AdmitAll), Exit::Usage);
    assert!(started.elapsed() < Duration::from_millis(350));
    let published = read_status(&config.status_file);
    assert_eq!(published["checks"][0]["code"], "timeout");
}

#[test]
fn health_disk_warning_and_failure_boundaries() {
    for (free_percent, category, code) in [
        (20.1, Category::Healthy, "ok"),
        (20.0, Category::Warning, "disk_free_warning"),
        (15.1, Category::Warning, "disk_free_warning"),
        (15.0, Category::Failure, "disk_free_critical"),
        (1.0, Category::Failure, "disk_free_critical"),
    ] {
        let usage = move |_path: &Path| {
            let total = 10_000_u64;
            Ok(DiskUsage {
                total,
                free: (total as f64 * free_percent / 100.0) as u64,
            })
        };
        let result = check_filesystem("root_disk", Path::new("/"), 20.0, 15.0, &usage);
        assert_eq!(
            (result.category, result.code.as_str()),
            (category, code),
            "{free_percent}"
        );
        assert_eq!(result.name, "root_disk");
    }
    let usage = |_path: &Path| {
        Ok(DiskUsage {
            total: 10_000,
            free: 5_000,
        })
    };
    let invalid = check_filesystem("root_disk", Path::new("/"), 15.0, 15.0, &usage);
    assert_eq!(invalid.code, "invalid_thresholds");
    let unreadable = check_filesystem("root_disk", Path::new("/"), 20.0, 15.0, &|_path: &Path| {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    });
    assert_eq!(
        (unreadable.category, unreadable.code.as_str()),
        (Category::Failure, "filesystem_unreadable")
    );
    let empty = check_filesystem("root_disk", Path::new("/"), 20.0, 15.0, &|_path: &Path| {
        Ok(DiskUsage { total: 0, free: 0 })
    });
    assert_eq!(
        empty.code, "disk_free_critical",
        "an empty volume is never healthy"
    );
    // The system call reports a real volume.
    let real = verdaxis_read::cli::monitors::health::system_disk_usage(Path::new("/")).unwrap();
    assert!(real.total > 0 && real.free <= real.total);
}

#[test]
fn health_analytics_directory_threshold_and_limits() {
    let scratch = TempDir::new();
    std::fs::write(scratch.join("one"), [b'a'; 4]).unwrap();
    std::fs::create_dir(scratch.join("nested")).unwrap();
    std::fs::write(scratch.join("nested/two"), [b'b'; 5]).unwrap();
    let later = Instant::now() + Duration::from_secs(5);
    let healthy = check_directory_size("analytics_directory", scratch.path(), 9, later, 100);
    assert_eq!(
        (healthy.category, healthy.code.as_str()),
        (Category::Healthy, "ok")
    );
    let exceeded = check_directory_size("analytics_directory", scratch.path(), 8, later, 100);
    assert_eq!(exceeded.code, "directory_size_exceeded");
    let crowded = check_directory_size("analytics_directory", scratch.path(), 100, later, 2);
    assert_eq!(crowded.code, "directory_entry_limit");
    let missing = check_directory_size(
        "analytics_directory",
        &scratch.join("absent"),
        9,
        later,
        100,
    );
    assert_eq!(missing.code, "directory_unreadable");
    let expired = check_directory_size(
        "analytics_directory",
        scratch.path(),
        9,
        Instant::now(),
        100,
    );
    assert_eq!(expired.code, "total_deadline_exceeded");
    // A link is counted as an entry but never followed or sized as its target.
    let outside = TempDir::new();
    std::fs::write(outside.join("big"), vec![b'c'; 4096]).unwrap();
    symlink(outside.path(), scratch.join("link")).unwrap();
    let linked = check_directory_size("analytics_directory", scratch.path(), 9, later, 100);
    assert_eq!(linked.code, "ok");
}

// ---- status document --------------------------------------------------------

#[test]
fn health_quarantines_a_malformed_previous_status() {
    let server = router();
    let healthy_check = json!({"name":"root_disk","category":"healthy","code":"ok","duration_ms":1,"last_success_at":null});
    let document = |category: &str, checks: Value| json!({"schema_version":1,"generated_at":"2026-07-20T00:00:00Z","category":category,"checks":checks});
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("checks null", br#"{"schema_version":1,"checks":null}"#.to_vec()),
        ("not json", b"{".to_vec()),
        ("empty object", b"{}".to_vec()),
        ("array", b"[]".to_vec()),
        (
            "top category above its checks",
            document(
                "healthy",
                json!([{"name":"root_disk","category":"failure","code":"disk_free_critical","duration_ms":1,"last_success_at":null}]),
            )
            .to_string()
            .into_bytes(),
        ),
        ("no checks", document("failure", json!([])).to_string().into_bytes()),
        (
            "boolean schema version",
            json!({"schema_version":true,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[healthy_check.clone()]})
                .to_string()
                .into_bytes(),
        ),
        (
            "float schema version",
            br#"{"schema_version":1.0,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[{"name":"a","category":"healthy","code":"ok","duration_ms":1,"last_success_at":null}]}"#.to_vec(),
        ),
        (
            "category is a list",
            json!({"schema_version":1,"generated_at":"2026-07-20T00:00:00Z","category":[],"checks":[healthy_check.clone()]})
                .to_string()
                .into_bytes(),
        ),
        (
            "unknown schema version",
            json!({"schema_version":2,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[healthy_check.clone()]})
                .to_string()
                .into_bytes(),
        ),
        (
            "extra key",
            json!({"schema_version":1,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[healthy_check.clone()],"extra":1})
                .to_string()
                .into_bytes(),
        ),
        (
            "naive timestamp",
            json!({"schema_version":1,"generated_at":"2026-07-20T00:00:00","category":"healthy","checks":[healthy_check.clone()]})
                .to_string()
                .into_bytes(),
        ),
        (
            "duplicate check names",
            document("healthy", json!([healthy_check.clone(), healthy_check.clone()]))
                .to_string()
                .into_bytes(),
        ),
        (
            "negative duration",
            document("healthy", json!([{"name":"a","category":"healthy","code":"ok","duration_ms":-1,"last_success_at":null}]))
                .to_string()
                .into_bytes(),
        ),
        (
            "duration beyond a day",
            document("healthy", json!([{"name":"a","category":"healthy","code":"ok","duration_ms":86_400_001_u64,"last_success_at":null}]))
                .to_string()
                .into_bytes(),
        ),
        (
            "long name",
            document("healthy", json!([{"name":"n".repeat(129),"category":"healthy","code":"ok","duration_ms":1,"last_success_at":null}]))
                .to_string()
                .into_bytes(),
        ),
        (
            "bad last success",
            document("healthy", json!([{"name":"a","category":"healthy","code":"ok","duration_ms":1,"last_success_at":"yesterday"}]))
                .to_string()
                .into_bytes(),
        ),
        (
            "duplicate keys",
            br#"{"schema_version":1,"schema_version":1,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[{"name":"a","category":"healthy","code":"ok","duration_ms":1,"last_success_at":null}]}"#.to_vec(),
        ),
        ("over the size bound", vec![b' '; 65_537]),
    ];
    for (label, bytes) in cases {
        let scratch = TempDir::new();
        let config = config_for(&scratch, &server, "/ready");
        std::fs::create_dir_all(config.status_file.parent().unwrap()).unwrap();
        std::fs::write(&config.status_file, &bytes).unwrap();
        let status = collect(&config, now()).expect("a bad previous status is still a status");
        assert_eq!(status.category, Category::Failure, "{label}");
        assert_eq!(status.checks.len(), 1, "{label}");
        assert_eq!(status.checks[0].code, "previous_status_invalid", "{label}");
        assert_eq!(status.checks[0].name, "monitor", "{label}");
        assert!(
            !config.status_file.exists(),
            "{label}: the bad file leaves its path"
        );
        let kept: Vec<String> = TempDir::names_of(config.status_file.parent().unwrap())
            .into_iter()
            .filter(|name| name.starts_with("status.json.invalid-"))
            .collect();
        assert_eq!(kept.len(), 1, "{label}: exactly one quarantined copy");
        assert_eq!(server.request_count(), 0, "{label}: no check runs first");
    }

    // A linked status path is quarantined too, and its target stays untouched.
    let scratch = TempDir::new();
    let config = config_for(&scratch, &server, "/ready");
    std::fs::create_dir_all(config.status_file.parent().unwrap()).unwrap();
    let target = scratch.join("target.json");
    std::fs::write(&target, b"{}").unwrap();
    symlink(&target, &config.status_file).unwrap();
    let status = collect(&config, now()).unwrap();
    assert_eq!(status.checks[0].code, "previous_status_invalid");
    assert_eq!(std::fs::read(&target).unwrap(), b"{}");
}

impl TempDir {
    fn names_of(path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}

#[test]
fn health_status_keys_match_the_python_document() {
    let input: Value = serde_json::from_slice(
        &std::fs::read(contract_fixture("monitor/status-golden-input.json")).unwrap(),
    )
    .unwrap();
    let previous: Status = serde_json::from_value(input["previous"].clone()).unwrap();
    assert!(valid_monitor_status(&input["previous"]));
    let checks: Vec<CheckResult> = input["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| CheckResult {
            name: check["name"].as_str().unwrap().to_owned(),
            category: serde_json::from_value(check["category"].clone()).unwrap(),
            code: check["code"].as_str().unwrap().to_owned(),
            duration_ms: check["duration_ms"].as_u64().unwrap(),
        })
        .collect();
    let built = build_status(checks, &Previous::Valid(previous), now());
    let golden = std::fs::read_to_string(contract_fixture("monitor/status-golden.json")).unwrap();
    assert_eq!(serde_json::to_string(&built).unwrap(), golden.trim_end());
    assert!(valid_monitor_status(&serde_json::to_value(&built).unwrap()));
    // A healthy check records its own success time; others keep the previous one.
    assert_eq!(
        built.checks[0].last_success_at.as_deref(),
        Some("2026-07-20T00:00:00Z")
    );
    assert_eq!(
        built.checks[1].last_success_at.as_deref(),
        Some("2026-07-19T12:00:00Z")
    );
    assert_eq!(built.checks[3].last_success_at, None);
    assert_eq!(
        built.category,
        Category::Failure,
        "a warning is never green"
    );

    let failure = failure_status("previous_status_invalid", now(), "backup_monitor");
    let golden = std::fs::read_to_string(contract_fixture("monitor/failure-golden.json")).unwrap();
    assert_eq!(serde_json::to_string(&failure).unwrap(), golden.trim_end());
    assert_eq!(
        build_status(Vec::new(), &Previous::Absent, now()).checks[0].code,
        "no_checks"
    );

    // A document the Python monitor wrote loads as a valid previous status.
    let scratch = TempDir::new();
    let copy = scratch.join("status.json");
    std::fs::copy(contract_fixture("monitor/status-golden.json"), &copy).unwrap();
    let Previous::Valid(loaded) = load_monitor_status(&copy, 65_536) else {
        panic!("the Python document must load");
    };
    assert_eq!(loaded, built);
    assert_eq!(
        load_monitor_status(&scratch.join("absent.json"), 65_536),
        Previous::Absent
    );

    // The published file is private, atomic and key-compatible.
    let server = router();
    let config = config_for(&scratch, &server, "/ready");
    assert_eq!(run(&config, &AdmitAll), Exit::Ok);
    let mode = std::fs::metadata(&config.status_file)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let published = read_status(&config.status_file);
    let mut keys: Vec<&String> = published.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["category", "checks", "generated_at", "schema_version"]
    );
    for check in published["checks"].as_array().unwrap() {
        let mut keys: Vec<&String> = check.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["category", "code", "duration_ms", "last_success_at", "name"]
        );
    }
    let leftovers: Vec<String> = TempDir::names_of(config.status_file.parent().unwrap())
        .into_iter()
        .filter(|name| name != "status.json")
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temporary file remains: {leftovers:?}"
    );
    // The second run carries the first run's success time forward for a failing check.
    let failing = config_for(&scratch, &server, "/down");
    assert_eq!(run(&failing, &AdmitAll), Exit::Usage);
    let after = read_status(&failing.status_file);
    assert_eq!(
        after["checks"][0]["last_success_at"],
        published["checks"][0]["last_success_at"]
    );
    assert_eq!(after["checks"][0]["category"], "failure");
}

#[test]
fn health_status_validity_rules_match_the_python_state_module() {
    let status = |category: &str, checks: &[(&str, &str)]| {
        json!({
            "schema_version": 1,
            "generated_at": "2026-07-20T00:00:00Z",
            "category": category,
            "checks": checks.iter().map(|(name, category)| json!({
                "name": name, "category": category,
                "code": if *category == "healthy" { "ok" } else { "degraded" },
                "duration_ms": 1, "last_success_at": "2026-07-20T00:00:00Z"
            })).collect::<Vec<_>>(),
        })
    };
    assert!(!valid_monitor_status(&status("failure", &[])));
    for (top, checks) in [
        ("healthy", vec![("disk", "warning")]),
        ("warning", vec![("disk", "failure")]),
        ("failure", vec![("disk", "healthy")]),
        ("warning", vec![("disk", "healthy"), ("runtime", "healthy")]),
    ] {
        assert!(
            !valid_monitor_status(&status(top, &checks)),
            "{top} {checks:?}"
        );
    }
    assert!(valid_monitor_status(&status(
        "failure",
        &[
            ("runtime", "healthy"),
            ("disk", "warning"),
            ("backup", "failure")
        ]
    )));
    let scratch = TempDir::new();
    let path = scratch.join("status.json");
    std::fs::write(
        &path,
        br#"{"schema_version":1,"schema_version":1,"generated_at":"2026-07-20T00:00:00Z","category":"healthy","checks":[{"name":"runtime","category":"healthy","code":"ok","duration_ms":1,"last_success_at":"2026-07-20T00:00:00Z"}]}"#,
    )
    .unwrap();
    assert_eq!(load_monitor_status(&path, 65_536), Previous::Malformed);
}

// ---- outcome, publication and the handoff seam ------------------------------

#[test]
fn health_exit_is_zero_only_for_healthy() {
    use verdaxis_read::cli::monitors::status::exit_for;
    assert_eq!(exit_for(Category::Healthy), Exit::Ok);
    assert_eq!(exit_for(Category::Warning), Exit::Usage);
    assert_eq!(exit_for(Category::Failure), Exit::Usage);
    // The warning path end to end: published, and not green.
    let server = router();
    let scratch = TempDir::new();
    let mut config = config_for(&scratch, &server, "/ready");
    config.warning_free_percent = 100.0;
    config.critical_free_percent = 0.0;
    assert_eq!(run(&config, &AdmitAll), Exit::Usage);
    let published = read_status(&config.status_file);
    assert_eq!(published["category"], "warning");
}

struct Refusing(Exit);

impl Admission for Refusing {
    fn admit(&self, _status: &Status) -> Result<(), Exit> {
        Err(self.0)
    }
}

#[test]
fn health_publication_waits_for_handoff_admission() {
    let server = router();
    let scratch = TempDir::new();
    let config = config_for(&scratch, &server, "/ready");
    assert_eq!(run(&config, &AdmitAll), Exit::Ok);
    let before = std::fs::read(&config.status_file).unwrap();
    // A refused admission fails the run and leaves the document as it was.
    let failing = config_for(&scratch, &server, "/down");
    assert_eq!(run(&failing, &Refusing(Exit::Dependency)), Exit::Dependency);
    assert_eq!(std::fs::read(&config.status_file).unwrap(), before);
    let names = TempDir::names_of(config.status_file.parent().unwrap());
    assert_eq!(names, ["status.json"], "no partial or temporary file");
}

#[test]
fn health_fatal_error_never_leaks_the_cause() {
    let at = now();
    let from_error = guarded("monitor", at, || Err(Fault));
    assert_eq!(from_error.checks[0].code, "fatal_error");
    assert_eq!(from_error.checks[0].name, "monitor");
    assert_eq!(from_error.category, Category::Failure);
    let from_panic = guarded("backup_monitor", at, || -> Result<Status, Fault> {
        panic!("secret-value")
    });
    assert_eq!(from_panic.checks[0].code, "fatal_error");
    assert_eq!(from_panic.checks[0].name, "backup_monitor");
    assert!(
        !serde_json::to_string(&from_panic)
            .unwrap()
            .contains("secret-value")
    );
    let fine = guarded("monitor", at, || Ok(failure_status("x", at, "monitor")));
    assert_eq!(fine.checks[0].code, "x");
}
