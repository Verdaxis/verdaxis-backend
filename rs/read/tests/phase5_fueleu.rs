//! One module owns the FuelEU reference value and the reporting-year limits.
use rust_decimal::Decimal;
use std::str::FromStr;
use verdaxis_read::compliance::pricing::LifecycleOverlayCore;
use verdaxis_read::market::fueleu::{
    FUELEU_PENALTY_EUR_PER_TONNE, FUELEU_REFERENCE_GHG, YearError, fueleu_target, mcp_penalty,
    mcp_reference, mcp_target,
};
use verdaxis_read::mcp::voyage::voyage_plan;

/// The first and the last year of each step, with the regulation's limit.
const STEPS: [(i32, &str); 12] = [
    (2025, "89.3368"),
    (2029, "89.3368"),
    (2030, "85.6904"),
    (2034, "85.6904"),
    (2035, "77.9418"),
    (2039, "77.9418"),
    (2040, "62.9004"),
    (2044, "62.9004"),
    (2045, "34.6408"),
    (2049, "34.6408"),
    (2050, "18.232"),
    (9999, "18.232"),
];

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn voyage_with_reporting_year(year: i32) -> Result<serde_json::Value, String> {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../deploy/rust/voyage-vectors.json")).unwrap();
    let vector = &vectors[0];
    let today = vector["today"].as_str().unwrap().parse().unwrap();
    let mut request = vector["request"].clone();
    request["assumptions"]["reporting_year"] = serde_json::json!(year);
    voyage_plan(&request, &vector["prices"], today)
}

#[test]
fn fueleu_boundary_equivalence() {
    for (year, expected) in STEPS {
        let expected = decimal(expected);
        assert_eq!(fueleu_target(year), Ok(expected), "shared limit for {year}");
        assert_eq!(decimal(&mcp_target(year).unwrap().text()), expected);
        // The voyage tool keeps its own 2025-2050 reporting-year input range.
        if year <= 2050 {
            let plan = voyage_with_reporting_year(year).unwrap();
            let strategies = plan["strategies"].as_array().unwrap();
            assert!(!strategies.is_empty());
            for strategy in strategies {
                let target = strategy["fueleu_target_gco2e_per_mj"].as_str().unwrap();
                assert_eq!(decimal(target), expected, "voyage limit for {year}");
            }
        }
    }
    for year in [2024, 10_000, i32::MIN, i32::MAX] {
        assert_eq!(fueleu_target(year), Err(YearError));
        assert!(mcp_target(year).is_err());
    }
    for year in [2024, 2051] {
        assert!(voyage_with_reporting_year(year).is_err());
    }
}

#[test]
fn fueleu_reference_cases() {
    assert_eq!(FUELEU_REFERENCE_GHG, decimal("91.16"));
    assert_eq!(decimal(&mcp_reference().text()), FUELEU_REFERENCE_GHG);
    assert_eq!(FUELEU_PENALTY_EUR_PER_TONNE, decimal("2400"));
    assert_eq!(decimal(&mcp_penalty().text()), FUELEU_PENALTY_EUR_PER_TONNE);
    let assumptions = LifecycleOverlayCore::assumptions(2026, 0, None).unwrap();
    assert_eq!(
        assumptions.penalty_eur_per_tonne,
        FUELEU_PENALTY_EUR_PER_TONNE
    );
    assert_eq!(assumptions.vlsfo_baseline_gco2_mj, FUELEU_REFERENCE_GHG);
    // The consumers read these values from the shared module. The voyage fuel
    // properties keep their own illustrative fuel intensities in deploy data.
    let consumers = [
        ("mcp/voyage.rs", include_str!("../src/mcp/voyage.rs")),
        (
            "routes/orderbook_lists.rs",
            include_str!("../src/routes/orderbook_lists.rs"),
        ),
        (
            "compliance/pricing.rs",
            include_str!("../src/compliance/pricing.rs"),
        ),
        (
            "compliance/scoring.rs",
            include_str!("../src/compliance/scoring.rs"),
        ),
    ];
    // Rust accepts `2_400` as well as `2400`, so both spellings of the penalty
    // are checked.
    let regulatory_literals = [
        "9116", "91.16", "89.3368", "85.6904", "77.9418", "62.9004", "34.6408", "18.232", "2400",
        "2_400",
    ];
    for (path, source) in consumers {
        for literal in regulatory_literals {
            assert!(
                !source.contains(literal),
                "{path} keeps the FuelEU value {literal} outside the shared module"
            );
        }
    }
}
