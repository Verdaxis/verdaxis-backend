//! Fleet document and public HTTP contracts.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Semaphore;
use tower::ServiceExt;
pub use verdaxis_read::runtime;
use verdaxis_read::{ApiError, AppState, routes};

#[path = "../src/routes/fleet_intelligence.rs"]
mod fleet_intelligence;
use fleet_intelligence::fleet_snapshot::{MAX_BYTES, decode_snapshot, read_snapshot};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        // Canonical temporary storage also works on macOS, where /var is a link.
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("fleet-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        Self(root)
    }

    fn path(&self) -> PathBuf {
        self.0.join("data/fleet_demand.json")
    }

    fn write(&self, bytes: &[u8]) {
        std::fs::write(self.path(), bytes).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn document() -> Value {
    json!({
        "entries": [{"fuel":"Alternative fuel", "ordered_vessels":12,
            "delivered_vessels":3, "avg_consumption_mt":9500, "color":"#5DADE2"}],
        "last_updated":"2024-02-29", "sources":["Published fleet summary"]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

async fn read(path: &Path) -> Result<Value, ()> {
    read_snapshot(path, Arc::new(Semaphore::new(1)))
        .await
        .map(|value| serde_json::to_value(value).unwrap())
        .map_err(|_| ())
}

#[tokio::test]
async fn fleet_document_cases() {
    let fixture = Fixture::new();
    assert!(read(&fixture.path()).await.is_err());
    fixture.write(&bytes(&document()));
    assert_eq!(read(&fixture.path()).await.unwrap(), document());
    let empty = json!({"entries":[], "last_updated":"2000-01-01", "sources":[]});
    fixture.write(&bytes(&empty));
    assert_eq!(read(&fixture.path()).await.unwrap(), empty);
    for payload in [b"".as_slice(), b"{\"entries\":[", b"null", b"{}"] {
        fixture.write(payload);
        assert!(read(&fixture.path()).await.is_err());
    }
    for date in [
        "2023-02-29",
        "2024-2-29",
        "2024-02-29T00:00:00Z",
        "2024-02-29x",
    ] {
        let mut value = document();
        value["last_updated"] = json!(date);
        assert!(decode_snapshot(&bytes(&value)).is_err(), "{date}");
    }
    for field in [
        "fuel",
        "color",
        "ordered_vessels",
        "delivered_vessels",
        "avg_consumption_mt",
    ] {
        let mut value = document();
        value["entries"][0].as_object_mut().unwrap().remove(field);
        assert!(decode_snapshot(&bytes(&value)).is_err(), "{field}");
    }
    let duplicate = br##"{"entries":[],"entries":[],"last_updated":"2024-01-01","sources":[]}"##;
    assert!(decode_snapshot(duplicate).is_err());
}

#[tokio::test]
async fn fleet_document_limits() {
    let fixture = Fixture::new();
    let mut exact = bytes(&document());
    exact.resize(MAX_BYTES, b' ');
    fixture.write(&exact);
    assert_eq!(read(&fixture.path()).await.unwrap(), document());
    exact.push(b' ');
    fixture.write(&exact);
    assert!(read(&fixture.path()).await.is_err());
    assert!(decode_snapshot(&exact).is_err());

    for (field, cap) in [("entries", 32), ("sources", 16)] {
        let mut value = document();
        let item = value[field][0].clone();
        value[field] = Value::Array(vec![item.clone(); cap]);
        assert!(decode_snapshot(&bytes(&value)).is_ok(), "{field} at cap");
        value[field].as_array_mut().unwrap().push(item);
        assert!(decode_snapshot(&bytes(&value)).is_err(), "{field} over cap");
    }
    for (field, cap) in [("fuel", 64), ("color", 32)] {
        let mut value = document();
        value["entries"][0][field] = json!("é".repeat(cap / 2));
        assert!(decode_snapshot(&bytes(&value)).is_ok());
        value["entries"][0][field] = json!(format!("{}x", "é".repeat(cap / 2)));
        assert!(decode_snapshot(&bytes(&value)).is_err());
    }
    let mut value = document();
    value["sources"][0] = json!("s".repeat(2048));
    assert!(decode_snapshot(&bytes(&value)).is_ok());
    value["sources"][0] = json!("s".repeat(2049));
    assert!(decode_snapshot(&bytes(&value)).is_err());
    for field in ["ordered_vessels", "delivered_vessels", "avg_consumption_mt"] {
        for (count, accepted) in [
            (json!(0), true),
            (json!(2147483647), true),
            (json!(-1), false),
            (json!(2147483648_u64), false),
            (json!(1.5), false),
            (json!("12"), false),
            (Value::Null, false),
        ] {
            let mut value = document();
            value["entries"][0][field] = count;
            assert_eq!(
                decode_snapshot(&bytes(&value)).is_ok(),
                accepted,
                "{field}: {value}"
            );
        }
    }
}

fn state(path: PathBuf) -> AppState {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        db_session_time_zone: "UTC".into(),
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://fixture:fixture_pw@127.0.0.1:1/fixture_test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: path,
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: sqlx::postgres::PgPoolOptions::new()
            .max_connections(16)
            .connect_lazy(&config.database_url)
            .unwrap(),
        health: sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&config.database_url)
            .unwrap(),
    };
    AppState::new(
        config,
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap()
}

#[tokio::test]
async fn fleet_document_text_cases() {
    let fixture = Fixture::new();
    let app = fleet_intelligence::routes().with_state(state(fixture.path()));
    for field in ["unknown", "fuel"] {
        let mut value = document();
        if field == "unknown" {
            value["unknown"] = json!("MARKER");
        } else {
            value["entries"][0]["fuel"] = json!("MARKER");
        }
        let mut payload = bytes(&value);
        let position = payload
            .windows(6)
            .position(|part| part == b"MARKER")
            .unwrap();
        payload[position] = 0xff;
        fixture.write(&payload);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/fleet-intelligence")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{field}"
        );
        let body = to_bytes(response.into_body(), MAX_BYTES + 1).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["detail"]["code"], "READ_UNAVAILABLE");
        assert!(
            !std::str::from_utf8(&body)
                .unwrap()
                .contains(fixture.0.to_str().unwrap())
        );
    }
}

#[tokio::test]
async fn fleet_config_cases() {
    let fixture = Fixture::new();
    let app = verdaxis_read::runtime::http::middleware(
        fleet_intelligence::routes().with_state(state(fixture.path())),
        "test",
    );
    for available in [false, true] {
        if available {
            fixture.write(&bytes(&document()));
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/fleet-intelligence")
                    .header("Authorization", "Bearer unused")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if available {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            }
        );
        if !available {
            assert_eq!(response.headers()["retry-after"], "1");
        }
        let body = to_bytes(response.into_body(), MAX_BYTES + 1).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        if available {
            assert_eq!(value, document());
        } else {
            assert_eq!(value["detail"]["code"], "READ_UNAVAILABLE");
            assert_eq!(value.as_object().unwrap().len(), 1);
            assert!(
                !String::from_utf8(body.to_vec())
                    .unwrap()
                    .contains(fixture.0.to_str().unwrap())
            );
        }
    }
    for payload in [b"{\"entries\":[".as_slice(), b"{}"] {
        fixture.write(payload);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/fleet-intelligence")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), MAX_BYTES + 1).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["detail"]["code"], "READ_UNAVAILABLE");
        assert!(value.get("entries").is_none());
    }
    fixture.write(&bytes(&document()));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/fleet-intelligence")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/fleet-intelligence")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[cfg(unix)]
#[tokio::test]
async fn fleet_document_file_cases() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let other = Fixture::new();
    other.write(&bytes(&document()));
    symlink(other.path(), fixture.path()).unwrap();
    assert!(read(&fixture.path()).await.is_err());
    std::fs::remove_file(fixture.path()).unwrap();
    std::fs::remove_dir(fixture.0.join("data")).unwrap();
    symlink(other.0.join("data"), fixture.0.join("data")).unwrap();
    assert!(read(&fixture.path()).await.is_err());
    std::fs::remove_file(fixture.0.join("data")).unwrap();
    std::fs::create_dir(fixture.0.join("data")).unwrap();
    std::fs::create_dir(fixture.path()).unwrap();
    assert!(read(&fixture.path()).await.is_err());
    std::fs::remove_dir(fixture.path()).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(fixture.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(read(&fixture.path()).await.is_err());
}

#[tokio::test]
async fn fleet_document_admission_cases() {
    let fixture = Fixture::new();
    fixture.write(&bytes(&document()));
    let permits = Arc::new(Semaphore::new(1));
    let held = permits.clone().acquire_owned().await.unwrap();
    assert!(
        read_snapshot(&fixture.path(), permits.clone())
            .await
            .is_err()
    );
    drop(held);
    assert!(read_snapshot(&fixture.path(), permits).await.is_ok());
}
