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
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
    }
}

#[tokio::test]
async fn fleet_config_cases() {
    use std::path::Path;
    use verdaxis_read::runtime::config::{
        config_from_deployed_environment, fleet_startup_probe, resolve_fleet_artifact_path,
    };
    let mut input = serde_json::json!({
        "environment": "test", "release_sha": compiled_sha(),
        "migration_revision": "kycg_20261014_staging",
        "database_url": "postgres://rust_app:pw@127.0.0.1:12345/verdaxis_test",
        "bind": "127.0.0.1:0", "pool_max": 17, "python_budget": 52,
        "configured_max_connections": 100, "db_session_time_zone": "UTC"
    });
    let default: Config = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(
        default.fleet_artifact_path,
        Path::new("data/fleet_demand.json")
    );
    input["FLEET_ARTIFACT_PATH"] = serde_json::json!("data/other.json");
    let alias: Config = serde_json::from_value(input).unwrap();
    assert_eq!(alias.fleet_artifact_path, Path::new("data/other.json"));
    let mut environment = deployed_environment();
    let load = |environment: &std::collections::HashMap<String, String>| {
        config_from_deployed_environment(
            environment,
            "127.0.0.1:0".parse().unwrap(),
            &budget_document(compiled_sha()),
        )
        .unwrap()
    };
    assert_eq!(
        load(&environment).fleet_artifact_path,
        default.fleet_artifact_path
    );
    environment.insert("FLEET_ARTIFACT_PATH".into(), "data/other.json".into());
    assert_eq!(
        load(&environment).fleet_artifact_path,
        alias.fleet_artifact_path
    );

    let root = std::env::temp_dir().join(format!("fleet-config-{}", uuid::Uuid::new_v4()));
    let cwd = root.join("checkout");
    std::fs::create_dir_all(cwd.join("data")).unwrap();
    let cwd = std::fs::canonicalize(cwd).unwrap();
    let resolved = resolve_fleet_artifact_path(&default.fleet_artifact_path, &cwd).unwrap();
    assert_eq!(resolved, cwd.join("data/fleet_demand.json"));
    assert!(!fleet_startup_probe(&resolved).await);
    for invalid in [
        "",
        " ",
        "data/../other.json",
        "../data/fleet.json",
        "other/file.json",
        "data/fleet.json/",
        "data/escape/",
    ] {
        assert!(resolve_fleet_artifact_path(Path::new(invalid), &cwd).is_err());
    }
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, cwd.join("data/escape")).unwrap();
        assert!(resolve_fleet_artifact_path(Path::new("data/escape/missing.json"), &cwd).is_err());
        std::os::unix::fs::symlink(outside.join("missing.json"), cwd.join("data/dangling.json"))
            .unwrap();
        assert!(resolve_fleet_artifact_path(Path::new("data/dangling.json"), &cwd).is_err());
    }
    assert!(resolve_fleet_artifact_path(&outside.join("fleet.json"), &cwd).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::ffi::OsString::from_vec(b"data/fleet-\xff.json".to_vec());
        assert!(resolve_fleet_artifact_path(Path::new(&invalid), &cwd).is_err());
        assert!(resolve_fleet_artifact_path(Path::new("data/escape/"), &cwd).is_err());
        std::fs::create_dir(cwd.join("data/feeds")).unwrap();
        std::os::unix::fs::symlink("../fleet_demand.json", cwd.join("data/feeds/fleet.json"))
            .unwrap();
        let internal =
            resolve_fleet_artifact_path(Path::new("data/feeds/fleet.json"), &cwd).unwrap();
        assert_eq!(internal, resolved);
        assert_eq!(
            resolve_fleet_artifact_path(&internal, &cwd).unwrap(),
            internal
        );
    }
    assert!(resolve_fleet_artifact_path(&resolved, &cwd).is_ok());
    std::fs::write(&resolved, b"not JSON").unwrap();
    assert!(!fleet_startup_probe(&resolved).await);
    std::fs::write(
        &resolved,
        br#"{"entries":[],"last_updated":"2026-04-03","sources":[]}"#,
    )
    .unwrap();
    assert!(fleet_startup_probe(&resolved).await);
    std::fs::write(&resolved, vec![b' '; 262_145]).unwrap();
    assert!(!fleet_startup_probe(&resolved).await);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn fleet_config_link_order_and_probe_cases() {
    use verdaxis_read::runtime::config::{resolve_fleet_artifact_path, fleet_startup_probe};
    let temp = std::env::temp_dir().join(format!("fleet-links-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(temp.join("data")).unwrap();
    let cwd = std::fs::canonicalize(&temp).unwrap();
    let data = cwd.join("data");
    let outside = cwd.join("outside");
    std::fs::create_dir_all(outside.join("subdir")).unwrap();
    let document = br#"{"entries":[],"last_updated":"2026-04-03","sources":[]}"#;
    std::fs::write(outside.join("fleet.json"), document).unwrap();
    std::fs::write(data.join("fleet.json"), document).unwrap();
    std::os::unix::fs::symlink(outside.join("subdir"), data.join("hop")).unwrap();
    for target in ["hop/../fleet.json", "hop/.."] {
        let link = data.join("entry.json");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert!(resolve_fleet_artifact_path(std::path::Path::new("data/entry.json"), &cwd).is_err());
        std::fs::remove_file(link).unwrap();
    }
    std::os::unix::fs::symlink("hop/..", data.join("a")).unwrap();
    std::os::unix::fs::symlink("a/fleet.json", data.join("entry.json")).unwrap();
    assert!(resolve_fleet_artifact_path(std::path::Path::new("data/entry.json"), &cwd).is_err());
    std::fs::create_dir(data.join("feeds")).unwrap();
    std::fs::write(data.join("feeds/fleet.json"), document).unwrap();
    let resolved = resolve_fleet_artifact_path(std::path::Path::new("data/feeds/fleet.json"), &cwd).unwrap();
    std::fs::rename(data.join("feeds"), data.join("old-feeds")).unwrap();
    std::os::unix::fs::symlink(&outside, data.join("feeds")).unwrap();
    assert!(!fleet_startup_probe(&resolved).await);
    let direct = resolve_fleet_artifact_path(std::path::Path::new("data/fleet.json"), &cwd).unwrap();
    std::fs::remove_file(data.join("fleet.json")).unwrap();
    rustix::fs::mknodat(rustix::fs::CWD, &direct, rustix::fs::FileType::Fifo, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR, 0).unwrap();
    assert!(!tokio::time::timeout(std::time::Duration::from_secs(2), fleet_startup_probe(&direct)).await.unwrap());
    assert!(!fleet_startup_probe(std::path::Path::new("/dev/null")).await);
    std::fs::remove_dir_all(temp).unwrap();
}

#[cfg(unix)]
#[test]
fn fleet_config_environment_encoding() {
    use std::os::unix::ffi::OsStringExt;
    let root = std::env::temp_dir().join(format!("fleet-env-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let budget = root.join("budget.json");
    std::fs::write(&budget, budget_document(compiled_sha())).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_verdaxis-read"))
        .current_dir(&root)
        .env_remove("RUST_CONFIG")
        .envs(deployed_environment())
        .env(
            "FLEET_ARTIFACT_PATH",
            std::ffi::OsString::from_vec(b"data/\xff.json".to_vec()),
        )
        .args([
            "--bind",
            "127.0.0.1:0",
            "--budget-file",
            budget.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let log = String::from_utf8(output.stdout).unwrap();
    assert!(!log.contains("optional fleet artifact startup probe"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fleet_config_syntax_is_checked_by_runtime_validation() {
    let mut config = staging();
    for path in [
        "",
        "data/../fleet.json",
        "../fleet.json",
        "other/fleet.json",
    ] {
        config.fleet_artifact_path = path.into();
        assert!(validate_config(&config).is_err());
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
    ] {
        config.migration_revision = revision.into();
        assert!(validate_config(&config).is_ok(), "{revision}");
    }
    config.migration_revision = "prc_20261018_other".into();
    assert!(validate_config(&config).is_err());
}

#[test]
fn read_index_checkpoint_is_accepted_and_the_cache_checkpoint_stays_accepted() {
    let mut config = staging();
    // The cache checkpoint stays accepted so one change can roll the route back.
    for revision in ["prc_20261018_cache", "p5_20261020_read_indexes"] {
        config.migration_revision = revision.into();
        assert!(validate_config(&config).is_ok(), "{revision}");
    }
    config.migration_revision = "p5_20261020_other".into();
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
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
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
fn compliance_exchange_rate_is_an_optional_positive_decimal() {
    use verdaxis_read::runtime::config::config_from_deployed_environment;
    let rate = |text: &str| text.parse::<rust_decimal::Decimal>().unwrap();

    // The contract harness document: an absent key leaves the published
    // default rate to the pricing core.
    let mut input = serde_json::json!({
        "environment": "test", "release_sha": compiled_sha(),
        "migration_revision": "kycg_20261014_staging",
        "database_url": "postgres://rust_app:pw@127.0.0.1:12345/verdaxis_test",
        "bind": "127.0.0.1:0", "pool_max": 17, "python_budget": 52,
        "configured_max_connections": 100, "db_session_time_zone": "UTC"
    });
    let config: Config = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(config.compliance_eur_usd_rate, None);
    assert!(validate_config(&config).is_ok());
    input["COMPLIANCE_EUR_USD_RATE"] = serde_json::json!("1.10");
    let config: Config = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(config.compliance_eur_usd_rate, Some(rate("1.10")));
    assert!(validate_config(&config).is_ok());
    for refused in ["0", "-1.08"] {
        input["COMPLIANCE_EUR_USD_RATE"] = serde_json::json!(refused);
        let config: Config = serde_json::from_value(input.clone()).unwrap();
        assert!(validate_config(&config).is_err(), "accepted rate {refused}");
    }
    input["COMPLIANCE_EUR_USD_RATE"] = serde_json::json!("one");
    assert!(serde_json::from_value::<Config>(input).is_err());

    // The deployed environment carries the same key.
    let bind: std::net::SocketAddr = "127.0.0.1:8011".parse().unwrap();
    let budget = budget_document(compiled_sha());
    let mut environment = deployed_environment();
    let config = config_from_deployed_environment(&environment, bind, &budget).unwrap();
    assert_eq!(config.compliance_eur_usd_rate, None);
    environment.insert("COMPLIANCE_EUR_USD_RATE".into(), " 1.10 ".into());
    let config = config_from_deployed_environment(&environment, bind, &budget).unwrap();
    assert_eq!(config.compliance_eur_usd_rate, Some(rate("1.10")));
    assert!(validate_config(&config).is_ok());
    for refused in ["", "one", "1,10", "0", "-1.08"] {
        environment.insert("COMPLIANCE_EUR_USD_RATE".into(), refused.into());
        let accepted = config_from_deployed_environment(&environment, bind, &budget)
            .is_ok_and(|config| validate_config(&config).is_ok());
        assert!(!accepted, "accepted rate {refused:?}");
    }
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
