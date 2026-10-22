use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use verdaxis_read::compliance::pricing::{AlcoholFamily, LifecycleOverlayCore, PricingError};
use verdaxis_read::market;

fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

#[test]
fn compliance_pricing_cases() {
    for (ci, lcv, expected) in [
        ("31", "19.9", "1.197"),
        ("20", "37", "2.633"),
        ("91.16", "37", "0"),
        ("95", "37", "0"),
        ("0", "37", "3.373"),
        ("-1", "37", "3.410"),
        ("90.66", "1", "0.001"),
        ("90.6601", "1", "0"),
    ] {
        let overlay = LifecycleOverlayCore::compute(None, Some(decimal(ci)), Some(decimal(lcv)))
            .unwrap()
            .unwrap();
        assert_eq!(overlay.tco2e_avoided_per_mt, decimal(expected), "CI={ci}");
        assert_eq!(overlay.ci_gco2_mj, decimal(ci));
        assert_eq!(overlay.lcv_mj_kg, decimal(lcv));
        let value = serde_json::to_value(overlay).unwrap();
        assert_eq!(value["ci_basis"], "LISTING");
        assert_eq!(value["lcv_basis"], "LISTING");
        assert_eq!(value["financial_benefit_status"], "UNPRICED");
        for key in ["penalty_avoided_eur_per_mt", "penalty_avoided_usd_per_mt"] {
            assert_eq!(decimal(value[key].as_str().unwrap()), Decimal::ZERO);
        }
    }
}

#[test]
fn compliance_pricing_lcv_cases() {
    for (product, expected) in [
        (AlcoholFamily::Methanol, "19.9"),
        (AlcoholFamily::Ethanol, "26.8"),
    ] {
        let overlay = LifecycleOverlayCore::compute(Some(product), Some(decimal("31")), None)
            .unwrap()
            .unwrap();
        assert_eq!(overlay.lcv_mj_kg, decimal(expected));
        let value = serde_json::to_value(overlay).unwrap();
        assert_eq!(value["ci_basis"], "LISTING");
        assert_eq!(value["lcv_basis"], "PRODUCT_DEFAULT");
        let explicit =
            LifecycleOverlayCore::compute(Some(product), Some(decimal("31")), Some(decimal("30")))
                .unwrap()
                .unwrap();
        assert_eq!(explicit.lcv_mj_kg, decimal("30"));
        assert_eq!(
            serde_json::to_value(explicit).unwrap()["lcv_basis"],
            "LISTING"
        );
        for invalid_lcv in [Decimal::ZERO, -Decimal::ONE] {
            assert!(LifecycleOverlayCore::compute(
                Some(product),
                Some(decimal("31")),
                Some(invalid_lcv),
            )
            .unwrap()
            .is_none());
        }
    }
}

#[test]
fn compliance_pricing_optional_cases() {
    for family in [
        None,
        Some(AlcoholFamily::Methanol),
        Some(AlcoholFamily::Ethanol),
    ] {
        assert!(
            LifecycleOverlayCore::compute(family, None, Some(decimal("37")))
                .unwrap()
                .is_none()
        );
    }
    assert!(
        LifecycleOverlayCore::compute(None, Some(decimal("31")), None)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        serde_json::to_value(LifecycleOverlayCore::compute(None, None, None).unwrap(),).unwrap(),
        serde_json::Value::Null
    );
}

#[test]
fn compliance_pricing_assumption_cases() {
    for count in [0, 3, u64::MAX] {
        for rate in [None, Some(decimal("1.10"))] {
            let assumptions = LifecycleOverlayCore::assumptions(2031, count, rate).unwrap();
            assert_eq!(assumptions.eur_usd_rate, rate.unwrap_or(decimal("1.08")));
            assert_eq!(assumptions.fleet_vessel_count, count);
            assert_eq!(
                assumptions.vlsfo_baseline_gco2_mj,
                market::fueleu::FUELEU_REFERENCE_GHG
            );
            assert_eq!(
                assumptions.ghgie_actual_gco2_mj,
                market::fueleu::FUELEU_REFERENCE_GHG
            );
            assert_eq!(assumptions.penalty_eur_per_tonne, decimal("2400"));
            assert_eq!(assumptions.year, 2031);
            assert_eq!(
                assumptions.year_target,
                market::fueleu::fueleu_target(2031).unwrap()
            );
            let value = serde_json::to_value(assumptions).unwrap();
            assert_eq!(value["year"], json!(2031));
            assert_eq!(value["fleet_vessel_count"], json!(count));
            assert_eq!(value["fleet_intensity_basis"], "DEFAULT_VLSFO");
            assert_eq!(
                value["excluded_factors"],
                json!([
                    "RFNBO_MULTIPLIER",
                    "DEFICIT_ESCALATION",
                    "EXTRA_EU_VOYAGE_SCOPE",
                ])
            );
            assert_eq!(
                value
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                [
                    "eur_usd_rate",
                    "vlsfo_baseline_gco2_mj",
                    "ghgie_actual_gco2_mj",
                    "fleet_intensity_basis",
                    "fleet_vessel_count",
                    "penalty_eur_per_tonne",
                    "year",
                    "year_target",
                    "excluded_factors"
                ]
                .into_iter()
                .collect(),
            );
            for key in [
                "eur_usd_rate",
                "vlsfo_baseline_gco2_mj",
                "ghgie_actual_gco2_mj",
                "penalty_eur_per_tonne",
                "year_target",
            ] {
                assert!(value[key].is_string(), "{key}");
            }
        }
    }
}

#[test]
fn compliance_pricing_reporting_year_cases() {
    for year in [
        2025, 2029, 2030, 2034, 2035, 2039, 2040, 2044, 2045, 2049, 2050, 9999,
    ] {
        assert_eq!(
            LifecycleOverlayCore::assumptions(year, 0, None)
                .unwrap()
                .year_target,
            market::fueleu::fueleu_target(year).unwrap()
        );
    }
    for year in [i32::MIN, 2024, 10000, i32::MAX] {
        assert!(matches!(
            LifecycleOverlayCore::assumptions(year, 0, None),
            Err(PricingError::ReportingYear(_))
        ));
    }
}

#[test]
fn compliance_pricing_numeric_cases() {
    assert!(matches!(
        LifecycleOverlayCore::compute(None, Some(Decimal::MIN), Some(Decimal::MAX)),
        Err(PricingError::ArithmeticRange)
    ));
    for rate in [Decimal::ZERO, -Decimal::ONE] {
        assert_eq!(
            LifecycleOverlayCore::assumptions(2026, 0, Some(rate)).unwrap_err(),
            PricingError::ExchangeRate
        );
    }
}

#[test]
fn compliance_pricing_projection_cases() {
    let overlay = LifecycleOverlayCore::compute(None, Some(decimal("31")), Some(decimal("19.9")))
        .unwrap()
        .unwrap();
    let value = serde_json::to_value(overlay).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "financial_benefit_status",
            "penalty_avoided_eur_per_mt",
            "penalty_avoided_usd_per_mt",
            "tco2e_avoided_per_mt",
            "ci_gco2_mj",
            "ci_basis",
            "lcv_mj_kg",
            "lcv_basis"
        ]
        .into_iter()
        .collect(),
    );
    for key in [
        "penalty_avoided_eur_per_mt",
        "penalty_avoided_usd_per_mt",
        "tco2e_avoided_per_mt",
        "ci_gco2_mj",
        "lcv_mj_kg",
    ] {
        assert!(value[key].is_string(), "{key}");
    }
}
