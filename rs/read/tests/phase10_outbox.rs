//! Phase 10 Task 28: the read-only outbox backlog probe (`monitor outbox`).
//!
//! The threshold and exit-code cases compare with the unchanged Python probe
//! (`deploy/monitor/outbox_backlog_probe.py`). The query case runs against the
//! disposable PostgreSQL of the contract-tests harness
//! (`RUST_TEST_MIGRATOR_DATABASE_URL`, `RUST_TEST_DATABASE_URL`).
#![cfg(feature = "contract-tests")]
#[path = "support/monitor_fixture.rs"]
mod support;

use chrono::{TimeZone, Utc};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};
use support::repository_root;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::monitors::outbox::{
    BACKLOG_QUERY, Backlog, BacklogSource, MAX_OUTPUT_BYTES, ProbeError, Target, Thresholds,
    evaluate, open_read_only, probe, report,
};

fn python() -> String {
    std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".to_owned())
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must name a disposable database"))
}

/// A fixed measurement, standing in for the database.
struct Fixed(Result<Backlog, ProbeError>);

impl BacklogSource for Fixed {
    async fn measure(&self, _timeout: Duration) -> Result<Backlog, ProbeError> {
        self.0
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Python's `evaluate` for the same inputs: (ok, breaches).
fn python_evaluate(cases: &[(i64, i64, i64, i64)]) -> Vec<(bool, Vec<String>)> {
    let script = "import sys, json; sys.path.insert(0, sys.argv[1]); \
                  import outbox_backlog_probe as p; \
                  cases = json.loads(sys.argv[2]); \
                  print(json.dumps([p.evaluate(c[0], c[1], max_pending=c[2], max_age_seconds=c[3]) for c in cases]))";
    let output = Command::new(python())
        .args(["-c", script])
        .arg(repository_root().join("deploy/monitor"))
        .arg(serde_json::to_string(cases).unwrap())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run Python");
    assert!(output.status.success(), "{output:?}");
    let results: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    results
        .into_iter()
        .map(|result| {
            (
                result["ok"].as_bool().unwrap(),
                result["breaches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|breach| breach.as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn outbox_probe_exit_codes_and_thresholds_match_python() {
    let cases: Vec<(i64, i64, i64, i64)> = vec![
        (0, 0, 1000, 300),
        (1000, 300, 1000, 300),
        (1001, 0, 1000, 300),
        (0, 301, 1000, 300),
        (5000, 9000, 1000, 300),
        (3, 3, 0, 0),
        (0, 0, -1, -1),
    ];
    let expected = python_evaluate(&cases);
    let at = Utc.with_ymd_and_hms(2026, 7, 20, 4, 0, 0).unwrap();
    for (case, (ok, breaches)) in cases.iter().zip(expected) {
        let backlog = Backlog {
            pending_count: case.0,
            oldest_pending_seconds: case.1,
        };
        let thresholds = Thresholds {
            max_pending: case.2,
            max_age_seconds: case.3,
        };
        let status = evaluate(backlog, thresholds, at);
        assert_eq!(status["ok"], Value::Bool(ok), "{case:?}");
        let rust_breaches: Vec<String> = status["breaches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|breach| breach.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(rust_breaches, breaches, "{case:?}");
        assert_eq!(status["probe"], "outbox_backlog");
        assert_eq!(status["pending_count"], case.0);
        assert_eq!(status["oldest_pending_seconds"], case.1);
        assert_eq!(status["max_pending"], case.2);
        assert_eq!(status["max_age_seconds"], case.3);
        // Exit 0 within the thresholds, 1 on a breach: the caller's contract.
        let (exit, line) = report(Ok(backlog), thresholds, at);
        assert_eq!(exit as i32, if ok { 0 } else { 1 }, "{case:?}");
        assert!(line.len() < MAX_OUTPUT_BYTES);
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["ok"], Value::Bool(ok));
    }
    // Defaults are Python's: 1000 rows and 300 s.
    assert_eq!(
        Thresholds::default(),
        Thresholds {
            max_pending: 1000,
            max_age_seconds: 300
        }
    );
    // No trustworthy measurement is exit 2 with a fixed error.
    for (result, message) in [
        (Err(ProbeError::TimedOut), "backlog query timed out"),
        (Err(ProbeError::QueryFailed), "backlog query failed"),
        (
            Ok(Backlog {
                pending_count: -1,
                oldest_pending_seconds: 0,
            }),
            "non-integer or negative backlog measurement",
        ),
    ] {
        let (exit, line) = runtime().block_on(probe(
            &Fixed(result),
            Thresholds::default(),
            Duration::from_secs(1),
        ));
        assert_eq!(exit as i32, 2);
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["ok"], Value::Bool(false));
        assert_eq!(parsed["error"], message);
        assert_eq!(parsed["probe"], "outbox_backlog");
    }
    // The command: a missing DSN or another monitor's flag is a usage refusal.
    for arguments in [
        vec!["monitor", "outbox"],
        vec![
            "monitor", "outbox", "--dsn", "dbname=x", "--event", "health",
        ],
        vec![
            "monitor",
            "outbox",
            "--dsn",
            "dbname=x",
            "--query-timeout",
            "21",
        ],
        vec![
            "monitor",
            "outbox",
            "--dsn",
            "dbname=x",
            "--query-timeout",
            "0",
        ],
        vec!["monitor", "outbox", "--dsn", "dbname=x", "--dry-run"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
            .args(&arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
    }
}

#[test]
fn outbox_output_never_contains_the_dsn() {
    // A closed loopback port: the connection fails and the probe reports it.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    for dsn in [
        format!("host=127.0.0.1 port={port} dbname=secretdb user=secretuser password='secret pw'"),
        format!("postgres://secretuser:secretpw@127.0.0.1:{port}/secretdb"),
        "host=127.0.0.1 nonsense secretdb".to_owned(),
        "host=127.0.0.1 port=notaport user=secretuser".to_owned(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
            .args(["monitor", "outbox", "--dsn", &dsn, "--query-timeout", "3"])
            .env_remove("PGPASSWORD")
            .env("PGPASSFILE", "/nonexistent")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{dsn}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for text in [&stdout, &stderr] {
            for secret in [
                "secretdb",
                "secretuser",
                "secret pw",
                "secretpw",
                "notaport",
            ] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
        let parsed: Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(parsed["ok"], Value::Bool(false));
        assert!(stdout.len() < MAX_OUTPUT_BYTES);
    }
    let target = Target::from_dsn("host=127.0.0.1 user=secretuser password=secretpw").unwrap();
    let debug = format!("{target:?}");
    assert!(!debug.contains("secret"), "{debug}");
}

#[test]
fn outbox_query_is_bounded_read_only_and_times_out() {
    let rt = runtime();
    let migrator_url = env("RUST_TEST_MIGRATOR_DATABASE_URL");
    let app_url = env("RUST_TEST_DATABASE_URL");
    let app_role = url::Url::parse(&app_url).unwrap().username().to_owned();
    rt.block_on(async {
        let mut owner = PgConnection::connect(&migrator_url).await.unwrap();
        for statement in [
            "DROP TABLE IF EXISTS public.market_event_outbox",
            "CREATE TABLE public.market_event_outbox (id bigserial PRIMARY KEY, \
             created_at timestamptz NOT NULL, stream_seq bigint)",
            "INSERT INTO public.market_event_outbox (created_at, stream_seq) VALUES \
             (now() - interval '400 seconds', NULL), (now() - interval '10 seconds', NULL), \
             (now() - interval '900 seconds', 7)",
        ] {
            sqlx::query(statement).execute(&mut owner).await.unwrap();
        }
        // The role name comes from the disposable harness URL.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "GRANT SELECT, INSERT ON public.market_event_outbox TO {app_role}"
        )))
        .execute(&mut owner)
        .await
        .unwrap();
    });

    // The measurement: two unsequenced rows, the oldest about 400 s old.
    let target = Target::from_dsn(&app_url).unwrap();
    let thresholds = Thresholds::default();
    let (exit, line) = rt.block_on(probe(&target, thresholds, Duration::from_secs(20)));
    let status: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(status["pending_count"], 2, "{line}");
    let age = status["oldest_pending_seconds"].as_i64().unwrap();
    assert!((400..460).contains(&age), "{line}");
    assert_eq!(
        status["breaches"],
        serde_json::json!(["oldest_pending_seconds"])
    );
    assert_eq!(exit, Exit::Breached);
    assert!(line.len() < MAX_OUTPUT_BYTES);

    // The keyword DSN form reaches the same database.
    let parsed = url::Url::parse(&app_url).unwrap();
    let keyword = format!(
        "host={} port={} dbname={} user={} password={}",
        parsed.host_str().unwrap(),
        parsed.port().unwrap_or(5432),
        parsed.path().trim_start_matches('/'),
        parsed.username(),
        parsed.password().unwrap_or_default(),
    );
    let (exit, _) = rt.block_on(probe(
        &Target::from_dsn(&keyword).unwrap(),
        Thresholds {
            max_pending: 5,
            max_age_seconds: 3600,
        },
        Duration::from_secs(20),
    ));
    assert_eq!(exit, Exit::Ok);

    // The probe's session is read-only even for a role that may write, and
    // the query is one bounded aggregate.
    rt.block_on(async {
        let mut session = open_read_only(&target, Duration::from_secs(20))
            .await
            .unwrap();
        let refused = sqlx::query(
            "INSERT INTO public.market_event_outbox (created_at, stream_seq) VALUES (now(), NULL)",
        )
        .execute(&mut session)
        .await
        .expect_err("a write in the probe session must fail");
        let code = refused
            .as_database_error()
            .and_then(|error| error.code())
            .map(|code| code.into_owned());
        assert_eq!(code.as_deref(), Some("25006"), "read_only_sql_transaction");
        let timeout: (String,) = sqlx::query_as("SHOW statement_timeout")
            .fetch_one(&mut session)
            .await
            .unwrap();
        assert_eq!(timeout.0, "20s");
    });
    let query = BACKLOG_QUERY.to_ascii_lowercase();
    assert!(query.starts_with("select count(*)"));
    for word in ["insert", "update", "delete", ";", "for update"] {
        assert!(!query.contains(word), "{word}");
    }

    // A lock holder makes the query wait: the probe stops at its deadline.
    rt.block_on(async {
        let mut holder = PgConnection::connect(&migrator_url).await.unwrap();
        sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
        sqlx::query("LOCK TABLE public.market_event_outbox IN ACCESS EXCLUSIVE MODE")
            .execute(&mut holder)
            .await
            .unwrap();
        let started = Instant::now();
        let (exit, line) = probe(&target, Thresholds::default(), Duration::from_secs(2)).await;
        let elapsed = started.elapsed();
        assert_eq!(exit as i32, 2, "{line}");
        let status: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(status["error"], "backlog query timed out");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        sqlx::query("ROLLBACK").execute(&mut holder).await.unwrap();
    });

    // A server that accepts and never answers is cut off at the deadline too.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let started = Instant::now();
    let (exit, line) = rt.block_on(probe(
        &Target::from_dsn(&format!("host=127.0.0.1 port={port} dbname=x user=y")).unwrap(),
        Thresholds::default(),
        Duration::from_secs(2),
    ));
    assert_eq!(exit as i32, 2, "{line}");
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(silent);
}

#[test]
fn outbox_keyword_dsn_without_host_uses_the_local_default() {
    // libpq connects through its default (PGHOST, else the local socket) when
    // the DSN names no host; the probe must not force a TCP localhost.
    // SAFETY: the other tests in this binary name their host explicitly, so the
    // short-lived PGHOST cannot change their targets.
    unsafe { std::env::set_var("PGHOST", "/run/postgresql-probe-default") };
    let target = Target::from_dsn("dbname=verdaxis_prod user=verdaxis_backup").unwrap();
    let named =
        Target::from_dsn("host=127.0.0.1 dbname=verdaxis_prod user=verdaxis_backup").unwrap();
    unsafe { std::env::remove_var("PGHOST") };
    assert_eq!(target.host(), "/run/postgresql-probe-default");
    assert_eq!(named.host(), "127.0.0.1");
}
