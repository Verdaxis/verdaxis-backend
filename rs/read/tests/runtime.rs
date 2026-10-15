use verdaxis_read::runtime::db::log_session_timezone;
use verdaxis_read::{
    Config, check_revision, compiled_sha, connection_budget, validate_budget, validate_config,
};

fn staging() -> Config {
    Config {
        environment: "staging".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url:
            "postgres://verdaxis_app_staging:local-secret@127.0.0.1:5432/verdaxis_staging".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        pool_max: 9,
        python_budget: 52,
        configured_max_connections: 100,
    }
}

#[test]
fn revisions_fail_closed() {
    let allowed = vec!["kycg_20261014_staging".to_owned()];
    assert!(check_revision(&allowed[0], &allowed).is_ok());
    for revision in ["head", "", "kycg_20261014_prod", "unexpected"] {
        assert!(check_revision(revision, &allowed).is_err());
    }
}

#[test]
fn staging_configuration_fails_closed() {
    let base = staging();
    assert!(validate_config(&base).is_ok());
    for (from, to) in [
        ("verdaxis_app_staging", "verdaxis_migrator_staging"),
        ("/verdaxis_staging", "/verdaxis"),
        ("127.0.0.1", "localhost"),
        ("127.0.0.1", "192.0.2.1"),
        (":5432", ":5433"),
        ("local-secret", ""),
        ("local-secret", "%20%09"),
        ("local-secret", "postgres"),
        ("local-secret", "change_me"),
    ] {
        let mut config = base.clone();
        config.database_url = config.database_url.replace(from, to);
        assert!(validate_config(&config).is_err(), "accepted {from} -> {to}");
    }
    for suffix in [
        "?host=192.0.2.1",
        "?options=-crole=postgres",
        "?",
        "#fragment",
    ] {
        let mut config = base.clone();
        config.database_url.push_str(suffix);
        assert!(validate_config(&config).is_err());
    }
    for environment in ["production", "", "STAGING"] {
        let mut config = base.clone();
        config.environment = environment.into();
        assert!(validate_config(&config).is_err());
    }
    let mut config = base.clone();
    config.release_sha = "a".repeat(40);
    if config.release_sha == compiled_sha() {
        config.release_sha = "b".repeat(40);
    }
    assert!(validate_config(&config).is_err());
    let mut config = base.clone();
    config.bind = "0.0.0.0:8002".parse().unwrap();
    assert!(validate_config(&config).is_err());
    for pool_max in [0, 3, 8, 10] {
        let mut config = base.clone();
        config.pool_max = pool_max;
        assert!(validate_config(&config).is_err());
    }
    for budget in [92, u32::MAX] {
        let mut config = base.clone();
        config.python_budget = budget;
        assert!(validate_config(&config).is_err());
    }
}

#[test]
fn tests_require_explicit_numeric_loopback_database() {
    let mut config = staging();
    config.environment = "test".into();
    for url in [
        "postgres://test_app:local-secret@127.0.0.1:55432/runtime_test",
        "postgres://test_app:local-secret@[::1]:55432/runtime_test",
    ] {
        config.database_url = url.into();
        assert!(validate_config(&config).is_ok());
    }
    for url in [
        "postgres://test_app:local-secret@localhost/runtime_test",
        "postgres://test_app:local-secret@127.0.0.1/verdaxis_staging",
        "postgres:///runtime_test",
        "sqlite::memory:",
    ] {
        config.database_url = url.into();
        assert!(validate_config(&config).is_err());
    }
}

#[test]
fn budget_uses_effective_settings() {
    assert_eq!(connection_budget(2, 4, 2, 1, 20, 9), Some(61));
    assert_eq!(connection_budget(2, 4, 8, 1, 20, 9), Some(109));
    assert_eq!(connection_budget(2, 5, 2, 1, 20, 9), Some(69));
    assert_eq!(connection_budget(2, 4, 2, 2, 20, 9), Some(69));
    assert_eq!(connection_budget(2, 4, 2, 1, 21, 9), Some(62));
    assert_eq!(connection_budget(u32::MAX, 4, 2, 1, 20, 9), None);
    assert_eq!(connection_budget(2, 4, u32::MAX, 1, 20, 9), None);
    assert_eq!(connection_budget(2, 4, 2, 1, u32::MAX, 9), None);
    assert!(validate_budget(109, 109, 109).is_ok());
    assert!(validate_budget(109, 108, 109).is_err());
    assert!(validate_budget(109, 109, 108).is_err());
}

#[test]
fn startup_logs_session_timezone_field() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || log_session_timezone("Asia/Singapore"));
    let event: serde_json::Value = serde_json::from_slice(&bytes.lock().unwrap()).unwrap();
    assert_eq!(event["fields"]["database_timezone"], "Asia/Singapore");
}

#[test]
fn prepared_budget_is_bound_to_runtime() {
    let config = staging();
    let mut artifact = serde_json::json!({
        "version": 1, "release_sha": compiled_sha(), "environment_digest": "a".repeat(64),
        "services": 2, "workers": 4, "pool": 2, "overflow": 1, "reserve": 20,
        "rust_pool": 9, "python_budget": 52, "total": 61, "configured_max_connections": 100
    });
    assert!(
        verdaxis_read::runtime::config::validate_prepared_budget(&config, &artifact.to_string())
            .is_ok()
    );
    for (key, value) in [
        ("total", serde_json::json!(55)),
        ("python_budget", serde_json::json!(46)),
        ("release_sha", serde_json::json!("b".repeat(40))),
        ("environment_digest", serde_json::json!("")),
    ] {
        let original = artifact[key].clone();
        artifact[key] = value;
        assert!(
            verdaxis_read::runtime::config::validate_prepared_budget(
                &config,
                &artifact.to_string()
            )
            .is_err()
        );
        artifact[key] = original;
    }
}

#[test]
fn internal_error_has_exact_contract() {
    let error = verdaxis_read::ApiError::internal();
    assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.detail, "Database operation failed.");
    assert_eq!(error.retry_after, None);
}
