use verdaxis_read::runtime::db::log_session_timezone;
use verdaxis_read::{
    Config, check_revision, compiled_sha, connection_budget, validate_budget, validate_config,
};

fn staging() -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "staging".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url:
            "postgres://verdaxis_app_staging:local-secret@127.0.0.1:5432/verdaxis_staging".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
    }
}

#[test]
fn kyc_runtime_input_defaults_enabled_and_rejects_ambiguous_values() {
    let mut input = serde_json::json!({
        "environment": "test", "release_sha": compiled_sha(),
        "migration_revision": "kycg_20261014_staging",
        "database_url": "postgres://rust_app:pw@127.0.0.1:12345/verdaxis_test",
        "bind": "127.0.0.1:0", "pool_max": 17, "python_budget": 52,
        "configured_max_connections": 100, "db_session_time_zone": "UTC"
    });
    assert!(
        serde_json::from_value::<Config>(input.clone())
            .unwrap()
            .kyc_execution_gate_enabled
    );
    input["KYC_EXECUTION_GATE_ENABLED"] = serde_json::json!(false);
    assert!(
        !serde_json::from_value::<Config>(input.clone())
            .unwrap()
            .kyc_execution_gate_enabled
    );
    input["kyc_execution_gate_enabled"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Config>(input.clone()).is_err());
    input
        .as_object_mut()
        .unwrap()
        .remove("kyc_execution_gate_enabled");
    input["KYC_EXECUTION_GATE_ENABLED"] = serde_json::json!("false");
    assert!(serde_json::from_value::<Config>(input).is_err());
}

#[test]
fn timezone_policy_is_required_and_cannot_inject_startup_options() {
    let input = serde_json::json!({
        "environment":"test", "release_sha":compiled_sha(), "migration_revision":"kycg_20261014_staging",
        "database_url":"postgres://rust_app:pw@127.0.0.1:12345/verdaxis_test",
        "bind":"127.0.0.1:0", "pool_max":17, "python_budget":52, "configured_max_connections":100
    });
    assert!(serde_json::from_value::<Config>(input).is_err());
    let mut config = staging();
    for zone in ["", "UTC -c default_transaction_read_only=off", "not/a-zone"] {
        config.db_session_time_zone = zone.into();
        assert!(validate_config(&config).is_err());
    }
    for zone in ["Asia/Singapore", "UTC"] {
        config.db_session_time_zone = zone.into();
        assert!(validate_config(&config).is_ok());
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
fn union_checkpoint_is_accepted_by_compiled_policy() {
    let mut config = staging();
    config.migration_revision = "p0_20261016_union".into();
    assert!(validate_config(&config).is_ok());
    config.environment = "production".into();
    assert!(validate_config(&config).is_err());
}

#[test]
fn cache_checkpoint_is_accepted_and_earlier_checkpoints_stay_accepted() {
    let mut config = staging();
    for revision in [
        "kycg_20261014_staging",
        "p0_20261016_union",
        "prc_20261018_cache",
        "prc_20261026_changes",
        "p3_20261020_auth_tokens",
    ] {
        config.migration_revision = revision.into();
        assert!(validate_config(&config).is_ok(), "{revision}");
    }
    config.migration_revision = "prc_20261018_other".into();
    assert!(validate_config(&config).is_err());
}

#[test]
fn production_requires_its_exact_database_and_union_checkpoint() {
    let mut base = staging();
    base.environment = "production".into();
    base.database_url = "postgres://verdaxis_app:local-secret@127.0.0.1:5432/verdaxis_prod".into();
    base.migration_revision = "p0_20261016_union".into();
    base.bind = "127.0.0.1:8010".parse().unwrap();
    assert!(validate_config(&base).is_err());
    base.prepared_budget = Some(budget_document(compiled_sha()));
    assert!(validate_config(&base).is_ok());
    for (from, to) in [
        ("verdaxis_app", "verdaxis_app_staging"),
        ("/verdaxis", "/verdaxis_staging"),
        (":5432", ":5433"),
        ("127.0.0.1", "127.0.0.2"),
    ] {
        let mut config = base.clone();
        config.database_url = config.database_url.replace(from, to);
        assert!(validate_config(&config).is_err());
    }
    for revision in ["prc_20261026_changes", "p3_20261020_auth_tokens"] {
        base.migration_revision = revision.into();
        assert!(validate_config(&base).is_ok());
    }
    base.migration_revision = "kycg_20261014_staging".into();
    assert!(validate_config(&base).is_err());
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
fn rust_admission_is_independent_of_python_capacity() {
    for slots in [1_u32, 8, 16, 24, 47] {
        let config: Config = serde_json::from_value(serde_json::json!({
            "environment": "staging", "release_sha": compiled_sha(),
            "migration_revision": "kycg_20261014_staging",
            "database_url": "postgres://verdaxis_app_staging:local-secret@127.0.0.1:5432/verdaxis_staging",
            "db_session_time_zone": "UTC", "bind": "127.0.0.1:0",
            "read_admission_slots": slots, "pool_max": slots + 1,
            "python_budget": 52, "configured_max_connections": 100
        })).unwrap();
        assert!(validate_config(&config).is_ok(), "refused {slots} slots");
        let artifact = serde_json::json!({
            "version": 1, "release_sha": compiled_sha(), "environment_digest": "a".repeat(64),
            "services": 2, "workers": 4, "pool": 2, "overflow": 1, "reserve": 20,
            "rust_pool": slots + 1, "python_budget": 52, "total": 53 + slots,
            "configured_max_connections": 100
        });
        assert!(
            verdaxis_read::runtime::config::validate_prepared_budget(
                &config,
                &artifact.to_string()
            )
            .is_ok()
        );
    }
}

#[test]
fn admission_configuration_refuses_zero_overflow_and_excess_budget() {
    for (slots, pool) in [(0, 1), (u32::MAX, 0), (16, 16), (48, 49)] {
        let mut config = staging();
        config.read_admission_slots = slots;
        config.pool_max = pool;
        assert!(validate_config(&config).is_err());
    }
    let mut environment = deployed_environment();
    let bind = "127.0.0.1:0".parse().unwrap();
    for value in ["0", "", "-1", "abc", "4294967295"] {
        environment.insert("READ_ADMISSION_SLOTS".into(), value.into());
        assert!(
            verdaxis_read::runtime::config::config_from_deployed_environment(
                &environment,
                bind,
                &budget_document(compiled_sha())
            )
            .is_err()
        );
    }
    environment.insert("READ_ADMISSION_SLOTS".into(), "24".into());
    let mut budget: serde_json::Value =
        serde_json::from_str(&budget_document(compiled_sha())).unwrap();
    budget["rust_pool"] = serde_json::json!(25);
    budget["total"] = serde_json::json!(77);
    let config = verdaxis_read::runtime::config::config_from_deployed_environment(
        &environment,
        bind,
        &budget.to_string(),
    )
    .unwrap();
    assert_eq!(config.read_admission_slots, 24);
    assert!(validate_config(&config).is_ok());
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
        "rust_pool": 17, "python_budget": 52, "total": 69, "configured_max_connections": 100
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
fn cache_listener_connection_is_part_of_the_prepared_budget() {
    use verdaxis_read::runtime::config::validate_prepared_budget;

    let mut config = staging();
    let artifact = |listener: u32, total: u32, configured: u32| {
        serde_json::json!({
            "version": 1, "release_sha": compiled_sha(), "environment_digest": "a".repeat(64),
            "services": 2, "workers": 4, "pool": 2, "overflow": 1, "reserve": 20,
            "rust_pool": 17, "python_budget": 52, "cache_listener": listener,
            "total": total, "configured_max_connections": configured
        })
        .to_string()
    };
    assert!(validate_prepared_budget(&config, &artifact(0, 69, 100)).is_ok());
    assert!(validate_prepared_budget(&config, &artifact(1, 70, 100)).is_ok());
    // Two listener connections, or a total that ignores the listener, are not
    // the one dedicated connection the budget allows.
    assert!(validate_prepared_budget(&config, &artifact(2, 71, 100)).is_err());
    assert!(validate_prepared_budget(&config, &artifact(1, 69, 100)).is_err());
    assert!(validate_prepared_budget(&config, &artifact(0, 70, 100)).is_err());
    // The listener must fit inside the configured maximum on its own.
    config.configured_max_connections = 69;
    assert!(validate_prepared_budget(&config, &artifact(0, 69, 69)).is_ok());
    assert!(validate_prepared_budget(&config, &artifact(1, 70, 69)).is_err());
}

fn budget_document(release_sha: &str) -> String {
    serde_json::json!({
        "version": 1, "release_sha": release_sha, "environment_digest": "a".repeat(64),
        "services": 2, "workers": 4, "pool": 2, "overflow": 1, "reserve": 20,
        "rust_pool": 17, "python_budget": 52, "total": 69, "configured_max_connections": 100
    })
    .to_string()
}

fn deployed_environment() -> std::collections::HashMap<String, String> {
    [
        ("ENVIRONMENT", "staging"),
        ("RELEASE_SHA", compiled_sha()),
        ("MIGRATION_REVISION", "kycg_20261014_staging"),
        (
            "DATABASE_URL",
            "postgresql://verdaxis_app_staging:local-secret@127.0.0.1:5432/verdaxis_staging",
        ),
        ("DB_SESSION_TIME_ZONE", "UTC"),
        ("KYC_EXECUTION_GATE_ENABLED", "false"),
        // Task 23: the deploy transaction projects this private set into the
        // Rust runtime file only.
        ("JWT_SECRET", "phase2-deployed-secret-key-minimum-40"),
        ("JWT_ISSUER", "verdaxis-staging-api"),
        ("JWT_AUDIENCE", "verdaxis-staging-web"),
        ("FRONTEND_URL", "https://staging.verdaxis.exchange"),
        ("EMAIL_FROM", "Verdaxis <noreply@verdaxis.exchange>"),
        ("RESEND_API_KEY", "re_deployed_key"),
        (
            "AUTH_DOMAIN_DATASETS_FILE",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/auth-domains.json"
            ),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

#[test]
fn deployed_environment_refuses_a_missing_phase2_key() {
    use verdaxis_read::runtime::config::config_from_deployed_environment;
    let bind: std::net::SocketAddr = "127.0.0.1:8011".parse().unwrap();
    let mut environment = deployed_environment();
    environment.remove("JWT_AUDIENCE");
    let error =
        config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
            .err()
            .expect("a missing Phase 2 key must be refused");
    assert!(error.contains("JWT_AUDIENCE"), "{error}");
    // A blank previous key is the ordinary "no rotation" state, not a value.
    environment.insert("JWT_AUDIENCE".into(), "verdaxis-staging-web".into());
    environment.insert("JWT_SECRET_PREVIOUS".into(), "  ".into());
    let config =
        config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
            .unwrap();
    assert!(config.phase2.jwt_secret_previous.is_none());
}

#[test]
fn deployed_start_refuses_an_unreadable_watchlist_activation() {
    use verdaxis_read::runtime::config::config_from_deployed_environment;
    use verdaxis_read::runtime::phase2::Phase2Settings;
    let bind: std::net::SocketAddr = "127.0.0.1:8011".parse().unwrap();
    // The release metadata records an explicit instant with its offset. Any
    // other spelling stops the start instead of becoming some other instant.
    for recorded in [
        "",
        "  ",
        "now",
        "2026-11-01",
        "2026-11-01T00:00:00",
        "1793491200",
    ] {
        let mut environment = deployed_environment();
        environment.insert("WATCHLIST_PRODUCER_ACTIVATED_AT".into(), recorded.into());
        let refused =
            config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
                .and_then(|config| Phase2Settings::from_options(&config.phase2).map(|_| ()));
        let error = refused
            .err()
            .unwrap_or_else(|| panic!("accepted the recorded activation {recorded:?}"));
        assert!(
            error.contains("WATCHLIST_PRODUCER_ACTIVATED_AT"),
            "{recorded:?}: {error}"
        );
    }
}

#[test]
fn deployed_environment_builds_the_runtime_configuration() {
    use verdaxis_read::runtime::config::config_from_deployed_environment;
    let bind: std::net::SocketAddr = "127.0.0.1:8011".parse().unwrap();
    let config = config_from_deployed_environment(
        &deployed_environment(),
        bind,
        &budget_document(compiled_sha()),
    )
    .unwrap();

    assert_eq!(config.environment, "staging");
    assert_eq!(config.bind, bind);
    assert_eq!(config.pool_max, 17);
    assert_eq!(config.python_budget, 52);
    assert_eq!(config.configured_max_connections, 100);
    assert!(!config.kyc_execution_gate_enabled);
    assert!(config.prepared_budget.is_some());
    assert!(validate_config(&config).is_ok());

    // An absent switch keeps Python's default; a spelled-out value still maps.
    let mut environment = deployed_environment();
    environment.remove("KYC_EXECUTION_GATE_ENABLED");
    let default =
        config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
            .unwrap();
    assert!(default.kyc_execution_gate_enabled);
    environment.insert("KYC_EXECUTION_GATE_ENABLED".into(), "OFF".into());
    let spelled =
        config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
            .unwrap();
    assert!(!spelled.kyc_execution_gate_enabled);
}

#[test]
fn deployed_environment_refuses_an_incomplete_or_foreign_contract() {
    use verdaxis_read::runtime::config::config_from_deployed_environment;
    let bind: std::net::SocketAddr = "127.0.0.1:8011".parse().unwrap();
    for missing in [
        "ENVIRONMENT",
        "RELEASE_SHA",
        "MIGRATION_REVISION",
        "DATABASE_URL",
        "DB_SESSION_TIME_ZONE",
    ] {
        let mut environment = deployed_environment();
        environment.remove(missing);
        assert!(
            config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
                .is_err(),
            "accepted a missing {missing}"
        );
        environment.insert(missing.into(), "   ".into());
        assert!(
            config_from_deployed_environment(&environment, bind, &budget_document(compiled_sha()))
                .is_err(),
            "accepted a blank {missing}"
        );
    }
    let environment = deployed_environment();
    let mut foreign: serde_json::Value =
        serde_json::from_str(&budget_document(&"b".repeat(40))).unwrap();
    assert!(config_from_deployed_environment(&environment, bind, &foreign.to_string()).is_err());
    foreign["release_sha"] = serde_json::json!(compiled_sha());
    foreign["total"] = serde_json::json!(55);
    assert!(config_from_deployed_environment(&environment, bind, &foreign.to_string()).is_err());
    foreign["total"] = serde_json::json!(60);
    foreign["rust_pool"] = serde_json::json!(8);
    assert!(config_from_deployed_environment(&environment, bind, &foreign.to_string()).is_err());
    let mut ambiguous = deployed_environment();
    ambiguous.insert("KYC_EXECUTION_GATE_ENABLED".into(), "maybe".into());
    assert!(
        config_from_deployed_environment(&ambiguous, bind, &budget_document(compiled_sha()))
            .is_err()
    );
}

#[test]
fn deployed_arguments_require_both_flags_and_absolute_paths() {
    use verdaxis_read::runtime::config::deployed_arguments;
    let budget_path = "/home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/budget.json";
    let good: Vec<String> = ["--bind", "127.0.0.1:8011", "--budget-file", budget_path]
        .iter()
        .map(|value| value.to_string())
        .collect();
    let (bind, budget) = deployed_arguments(&good).unwrap();
    assert_eq!(bind, "127.0.0.1:8011".parse().unwrap());
    assert_eq!(budget, std::path::PathBuf::from(budget_path));

    let cases: Vec<Vec<&str>> = vec![
        vec!["--bind", "127.0.0.1:8011"],
        vec!["--budget-file", "/tmp/budget.json"],
        vec![
            "--bind",
            "127.0.0.1:8011",
            "--budget-file",
            "relative/budget.json",
        ],
        vec![
            "--bind",
            "not-a-socket",
            "--budget-file",
            "/tmp/budget.json",
        ],
        vec![
            "--bind",
            "127.0.0.1:8011",
            "--bind",
            "127.0.0.1:8012",
            "--budget-file",
            "/tmp/budget.json",
        ],
        vec![
            "--bind",
            "127.0.0.1:8011",
            "--budget-file",
            "/tmp/budget.json",
            "--unknown",
            "1",
        ],
        vec!["--bind"],
        vec![],
    ];
    for case in cases {
        let arguments: Vec<String> = case.iter().map(|value| value.to_string()).collect();
        assert!(deployed_arguments(&arguments).is_err(), "accepted {case:?}");
    }
}

#[test]
fn internal_error_has_exact_contract() {
    let error = verdaxis_read::ApiError::internal();
    assert_eq!(error.status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.detail, "Database operation failed.");
    assert_eq!(error.retry_after, None);
}
