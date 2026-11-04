//! Phase 10 Task 17: the demo book model equals Python's, row for row.
//!
//! The coverage refresh compares these values with the stored rows; any
//! difference would re-queue the whole demo book on the first Rust run. The
//! test runs Python's `build_demo_market_coverage` (the application on the
//! path, `RUST_TEST_PYTHON` or `python3`) for several reference instants and
//! compares every row's managed fields with the Rust builder.
use std::process::Command;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::cli::jobs::demo::model::{
    self, Catalog, CoverageOrder, DEMO_SEED_BUYERS, DEMO_SEED_SUPPLIERS,
};

struct Policy(verdaxis_read::MarketPolicy);

impl Catalog for Policy {
    fn product_id(&self, name: &str) -> Uuid {
        let id = &self
            .0
            .products
            .iter()
            .find(|product| product.name == name)
            .expect("a catalog product")
            .id;
        *id
    }
    fn delivery_point_id(&self, name: &str) -> Uuid {
        let id = &self
            .0
            .points
            .iter()
            .find(|point| point.name == name)
            .expect("a catalog point")
            .id;
        *id
    }
}

fn python() -> String {
    std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into())
}

const SCRIPT: &str = r#"
import json, sys
from datetime import datetime
from app.services.demo_activity import build_demo_market_coverage
def text(value):
    return None if value is None else str(value)
for instant in sys.argv[1:]:
    now = datetime.fromisoformat(instant)
    rows = []
    for order in build_demo_market_coverage(now):
        rows.append([
            order.idempotency_key, order.idempotency_request_hash, str(order.organization_id),
            order.side.value, str(order.product_id), str(order.delivery_point_id),
            order.availability_window, text(order.quantity_mt), text(order.price_per_mt_usd),
            list(order.certifications), order.certification_declared, order.certification_scheme,
            order.specification_standard, order.msds_available, order.is_verdaxis_verified,
            text(order.carbon_intensity_gco2_mj), order.carbon_intensity_method,
            text(order.energy_density_mj_kg), order.feedstock, order.origin,
            order.expires_at.isoformat(), order.created_at.isoformat(),
        ])
    print(json.dumps({"instant": instant, "rows": rows}))
"#;

fn rust_row(order: &CoverageOrder) -> Value {
    let text = |value: &Option<rust_decimal::Decimal>| value.map(|decimal| decimal.to_string());
    json!([
        order.key,
        order.request_hash,
        order.organization_id.to_string(),
        order.side.as_str(),
        order.product_id.to_string(),
        order.delivery_point_id.to_string(),
        order.window,
        order.quantity_mt.to_string(),
        order.price_per_mt_usd.to_string(),
        order.certifications,
        order.certification_declared,
        order.certification_scheme,
        order.specification_standard,
        order.msds_available,
        order.is_verdaxis_verified,
        text(&order.carbon_intensity_gco2_mj),
        order.carbon_intensity_method,
        text(&order.energy_density_mj_kg),
        order.feedstock,
        order.origin,
        order.expires_at.to_rfc3339(),
        order.created_at.to_rfc3339(),
    ])
}

#[test]
fn demo_coverage_rows_equal_python_for_every_field() {
    let instants = [
        "2026-11-04T01:00:00+00:00",
        "2026-12-31T23:59:59.500000+00:00",
        "2027-01-01T00:00:00+00:00",
        "2026-03-31T12:00:00+00:00",
        "2028-02-29T06:30:00+00:00",
        "2026-06-15T18:45:30.250000+00:00",
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(python())
        .arg("-c")
        .arg(SCRIPT)
        .args(instants)
        .current_dir(&root)
        .env("PYTHONPATH", &root)
        .env("ENVIRONMENT", "test")
        .env("RELEASE_SHA", "test")
        .env("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")
        .env("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
        .output()
        .expect("run the Python builder");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let policy = Policy(verdaxis_read::runtime::policy::committed_policy().unwrap());
    let mut compared = 0;
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let python: Value = serde_json::from_str(line).unwrap();
        let instant: DateTime<Utc> = python["instant"].as_str().unwrap().parse().unwrap();
        let rust = model::build_coverage(instant, &policy, &DEMO_SEED_BUYERS, &DEMO_SEED_SUPPLIERS);
        let python_rows = python["rows"].as_array().unwrap();
        assert_eq!(rust.len(), python_rows.len(), "{instant}");
        for (rust_order, python_row) in rust.iter().zip(python_rows) {
            let mut expected = python_row.clone();
            // Python's isoformat and Rust's RFC 3339 spell the same instant.
            for index in [20, 21] {
                let at: DateTime<Utc> = expected[index].as_str().unwrap().parse().unwrap();
                expected[index] = json!(at.to_rfc3339());
            }
            assert_eq!(rust_row(rust_order), expected, "{instant}");
            compared += 1;
        }
    }
    assert!(compared > 100_000, "{compared} rows compared");
}
