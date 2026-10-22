//! Task 8a pure vectors for the compliance score core and the fuel-mix rules.
//!
//! No database, no clock and no HTTP: every value below is a business rule of
//! the scoring core that the vessel, fleet and scenario reads share.

use rust_decimal::Decimal;
use serde_json::json;
use verdaxis_read::compliance::scoring::{
    cii_result, compliance_score, ets_phase_in, ets_result, fueleu_band, known_fuel_intensity,
    status_for, weighted_intensity, CalculationStatus, ComplianceStatus, ScoreInput, TrafficLight,
    DEFAULT_ETS_PRICE_EUR, FUEL_INTENSITIES,
};
use verdaxis_read::compliance::validation::{
    fuel_mix_from_json, validate_fuel_mix, FuelMix, MixErrorKind, RawMixEntry, ReportingYear,
    MAX_FUEL_KEY_BYTES, MAX_MIX_ENTRIES,
};

fn decimal(text: &str) -> Decimal {
    text.parse().expect("a test decimal")
}

fn mix(pairs: &[(&str, &str)]) -> FuelMix {
    let entries: Vec<RawMixEntry> = pairs
        .iter()
        .map(|(fuel, coefficient)| RawMixEntry::new(*fuel, *coefficient))
        .collect();
    validate_fuel_mix(&entries).expect("the test mix is valid")
}

fn reporting_year(year: i64) -> ReportingYear {
    ReportingYear::new(year).expect("the test year is valid")
}

fn score(
    fuel_mix: &FuelMix,
    year: i64,
    cii_rating: Option<&str>,
    total_co2_tonnes: Option<Decimal>,
) -> verdaxis_read::compliance::scoring::ComplianceScore {
    compliance_score(ScoreInput {
        vessel_id: "e0b1c0de-0000-4000-8000-000000000001",
        vessel_name: "MV Contract",
        fuel_mix,
        cii_rating,
        total_co2_tonnes,
        year: reporting_year(year),
        total_energy_mj: None,
    })
    .expect("valid score inputs")
}

fn rejected(pairs: &[(&str, &str)]) -> MixErrorKind {
    let entries: Vec<RawMixEntry> = pairs
        .iter()
        .map(|(fuel, coefficient)| RawMixEntry::new(*fuel, *coefficient))
        .collect();
    validate_fuel_mix(&entries)
        .expect_err("the test mix is rejected")
        .kind
}

#[test]
fn compliance_mix_cases() {
    // Every known fuel is accepted on its own and keeps its table intensity.
    for (fuel, intensity) in FUEL_INTENSITIES {
        let single = mix(&[(fuel, "1")]);
        assert_eq!(single.fraction(fuel), Some(Decimal::ONE));
        assert_eq!(weighted_intensity(&single).round_dp(2), intensity);
    }

    // A mix inside the accepted band is normalized exactly once, so the
    // weighted intensity uses the stated shares and not the raw total.
    let normalized = mix(&[("VLSFO", "0.49"), ("Methanol", "0.49")]);
    assert_eq!(normalized.fraction("VLSFO"), Some(decimal("0.5")));
    assert_eq!(normalized.fraction("Methanol"), Some(decimal("0.5")));
    assert_eq!(
        weighted_intensity(&normalized).round_dp(2),
        decimal("61.08")
    );

    // The current user interface accepts a five percent rounding tolerance.
    for total in ["0.95", "1.05", "1"] {
        let accepted = mix(&[("VLSFO", total)]);
        assert_eq!(accepted.fraction("VLSFO"), Some(Decimal::ONE));
    }
    for share in ["0.47", "0.53"] {
        assert_eq!(
            rejected(&[("VLSFO", share), ("Methanol", share)]),
            MixErrorKind::FractionSumOutOfTolerance
        );
    }

    // The total must be positive and finite: an empty or all-zero mix has no
    // defined share, and an unknown name is never silently treated as VLSFO.
    assert_eq!(rejected(&[]), MixErrorKind::FractionSumNotPositive);
    assert_eq!(
        rejected(&[("VLSFO", "0")]),
        MixErrorKind::FractionSumNotPositive
    );
    assert_eq!(
        rejected(&[("SomeFutureFuel", "1")]),
        MixErrorKind::UnknownFuel
    );
    assert_eq!(known_fuel_intensity("SomeFutureFuel"), None);

    // A key of at most 64 bytes reaches the known-fuel rule; one byte more is
    // refused as a key bound, not as a fuel name.
    let longest_key = "f".repeat(MAX_FUEL_KEY_BYTES);
    let too_long_key = "f".repeat(MAX_FUEL_KEY_BYTES + 1);
    assert_eq!(rejected(&[(&longest_key, "1")]), MixErrorKind::UnknownFuel);
    assert_eq!(
        rejected(&[(&too_long_key, "1")]),
        MixErrorKind::FuelKeyTooLong
    );

    // A share is a fraction of the mix, not an arbitrary amount.
    assert_eq!(
        rejected(&[("VLSFO", "1.06")]),
        MixErrorKind::FractionOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "-0.01")]),
        MixErrorKind::FractionOutOfRange
    );
    assert!(validate_fuel_mix(&[RawMixEntry::new("VLSFO", "1.05")]).is_ok());

    // Exponentiation is accepted when it denotes the same exact fraction.
    assert_eq!(
        mix(&[("VLSFO", "9e-1"), ("Methanol", "1e-1")]).fraction("Methanol"),
        Some(decimal("0.1"))
    );

    // One JSON object key cannot carry two shares.
    assert_eq!(
        rejected(&[("VLSFO", "0.5"), ("VLSFO", "0.5")]),
        MixErrorKind::DuplicateFuel
    );

    // The JSON adapter reads the literal number spelling, so an exponent keeps
    // its bound meaning instead of being narrowed to a float.
    let document: serde_json::Value =
        serde_json::from_str(r#"{"VLSFO": 0.9, "Methanol": 1e-1}"#).expect("valid JSON");
    let parsed = fuel_mix_from_json(&document).expect("a valid mix object");
    assert_eq!(parsed.fraction("Methanol"), Some(decimal("0.1")));
    let dashboard_request = json!({
        "vessel_id": "e0b1c0de-0000-4000-8000-000000000001",
        "fuel_mix": {"VLSFO": "0.5", "Methanol": "0.5"},
        "year": 2026
    });
    let dashboard_mix = fuel_mix_from_json(&dashboard_request["fuel_mix"])
        .expect("both dashboard clients send decimal strings");
    assert_eq!(dashboard_mix.fraction("Methanol"), Some(decimal("0.5")));
    assert_eq!(
        weighted_intensity(&dashboard_mix).round_dp(2),
        decimal("61.08")
    );
    assert_eq!(
        fuel_mix_from_json(&json!([])).unwrap_err().kind,
        MixErrorKind::NotAFuelMix
    );

    // The fallback mix for a vessel read is explicit, never an empty object.
    assert_eq!(
        FuelMix::default_vlsfo().fraction("VLSFO"),
        Some(Decimal::ONE)
    );
}

fn unnamed_entries(count: usize) -> Vec<RawMixEntry> {
    (0..count)
        .map(|index| RawMixEntry::new(format!("future-{index}"), "0.5"))
        .collect()
}

#[test]
fn compliance_mix_json_cases() {
    for (token, expected) in [("0.5", "0.5"), (".5", "0.5"), ("1e-1", "0.1")] {
        let other = (Decimal::ONE - decimal(expected)).to_string();
        let parsed = fuel_mix_from_json(&json!({"VLSFO": token, "Methanol": other}))
            .expect("decimal string shares are accepted");
        assert_eq!(parsed.fraction("VLSFO"), Some(decimal(expected)));
    }
    for (token, kind) in [
        ("NaN", MixErrorKind::InvalidCoefficient),
        ("Infinity", MixErrorKind::InvalidCoefficient),
        ("", MixErrorKind::InvalidCoefficient),
        ("abc", MixErrorKind::InvalidCoefficient),
        ("1e1", MixErrorKind::ExponentOutOfRange),
        ("0.0000000000001", MixErrorKind::ExponentOutOfRange),
        ("1234567890123456789", MixErrorKind::CoefficientTooLong),
        ("-0.1", MixErrorKind::FractionOutOfRange),
    ] {
        assert_eq!(
            fuel_mix_from_json(&json!({"VLSFO": token}))
                .unwrap_err()
                .kind,
            kind,
            "string {token}"
        );
        assert_eq!(rejected(&[("VLSFO", token)]), kind, "literal {token}");
    }
    for value in [json!(true), json!(null), json!([]), json!({})] {
        assert_eq!(
            fuel_mix_from_json(&json!({"VLSFO": value}))
                .unwrap_err()
                .kind,
            MixErrorKind::InvalidCoefficient
        );
    }
    let mixed = fuel_mix_from_json(&json!({"VLSFO": 0.5, "Methanol": "0.5"}))
        .expect("numbers and strings use the same exact parser");
    assert_eq!(mixed.fraction("VLSFO"), Some(decimal("0.5")));
}

#[test]
fn compliance_mix_json_key_cases() {
    for (key, kind) in [
        (
            "f".repeat(MAX_FUEL_KEY_BYTES + 1),
            MixErrorKind::FuelKeyTooLong,
        ),
        ("SomeFutureFuel".to_owned(), MixErrorKind::UnknownFuel),
    ] {
        let error = fuel_mix_from_json(&json!({key.clone(): null})).unwrap_err();
        assert_eq!(error.kind, kind);
        if kind == MixErrorKind::FuelKeyTooLong {
            assert!(!error.message.contains(&key));
        }
    }
}

#[test]
fn compliance_mix_literal_cases() {
    for token in ["0.1e1", "0e1", "0e-13", "0.0000000000000"] {
        assert_eq!(
            rejected(&[("VLSFO", token)]),
            MixErrorKind::ExponentOutOfRange
        );
    }
    assert_eq!(
        rejected(&[("VLSFO", "0.1000000000000000000")]),
        MixErrorKind::CoefficientTooLong
    );
    for token in ["NaN", "Infinity", "1e1e1"] {
        assert_eq!(
            rejected(&[("VLSFO", token)]),
            MixErrorKind::InvalidCoefficient
        );
    }
}

#[test]
fn compliance_mix_limits() {
    // The container cap is checked before any key semantics, so a request with
    // too many entries is refused as a shape bound whatever the names are.
    assert_eq!(
        validate_fuel_mix(&unnamed_entries(MAX_MIX_ENTRIES + 1))
            .unwrap_err()
            .kind,
        MixErrorKind::TooManyFuels
    );
    assert_eq!(
        validate_fuel_mix(&unnamed_entries(MAX_MIX_ENTRIES))
            .unwrap_err()
            .kind,
        MixErrorKind::UnknownFuel
    );

    // The whole known table fits one accepted mix.
    let all_known: Vec<RawMixEntry> = FUEL_INTENSITIES
        .iter()
        .map(|(fuel, _)| RawMixEntry::new(*fuel, "0.111111111111"))
        .collect();
    assert_eq!(
        validate_fuel_mix(&all_known)
            .expect("nine known fuels are a valid container")
            .entries()
            .len(),
        FUEL_INTENSITIES.len()
    );

    // An 18 digit coefficient passes the digit bound and fails only the value
    // range; a 19 digit coefficient is a lexical bound of its own.
    assert_eq!(
        rejected(&[("VLSFO", "123456789012345678")]),
        MixErrorKind::FractionOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "1234567890123456789")]),
        MixErrorKind::CoefficientTooLong
    );

    // Twelve decimal places are the stored limit, and a lexical exponent that
    // would underflow the value is still refused rather than read as zero.
    for tiny in ["0.000000000001", "1e-12"] {
        assert!(
            validate_fuel_mix(&[
                RawMixEntry::new("VLSFO", "1"),
                RawMixEntry::new("LNG", tiny),
            ])
            .is_ok(),
            "twelve decimals {tiny}"
        );
    }
    assert_eq!(
        rejected(&[("VLSFO", "1"), ("LNG", "0.0000000000001")]),
        MixErrorKind::ExponentOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "1"), ("LNG", "1e-13")]),
        MixErrorKind::ExponentOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "0e-999999999")]),
        MixErrorKind::ExponentOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "1e400")]),
        MixErrorKind::ExponentOutOfRange
    );
    assert_eq!(
        rejected(&[("VLSFO", "5e0")]),
        MixErrorKind::FractionOutOfRange
    );

    // Reporting years run from the shared schedule's first year to 9999.
    assert_eq!(
        ReportingYear::new(2024).unwrap_err().kind,
        MixErrorKind::YearOutOfRange
    );
    assert_eq!(
        ReportingYear::new(10_000).unwrap_err().kind,
        MixErrorKind::YearOutOfRange
    );
    assert_eq!(ReportingYear::new(2025).unwrap().get(), 2025);
    assert_eq!(ReportingYear::new(9999).unwrap().get(), 9999);
    assert_eq!(ReportingYear::default().get(), 2026);
    assert_eq!(reporting_year(9999).target(), decimal("18.2320"));
}

#[test]
fn compliance_reporting_year_cases() {
    use verdaxis_read::compliance::scoring::fueleu_result;
    let fuel_mix = FuelMix::default_vlsfo();
    for (year, expected) in [
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
        (2051, "18.232"),
        (9999, "18.232"),
    ] {
        let year = reporting_year(year);
        assert_eq!(year.target(), decimal(expected), "year {}", year.get());
        assert_eq!(
            fueleu_result(&fuel_mix, year, None)
                .unwrap()
                .target_intensity_gco2_mj,
            decimal(expected),
            "year {}",
            year.get()
        );
    }
}

#[test]
fn compliance_score_cases() {
    // 100 % VLSFO is just above the 2026 limit: a small deficit, a penalty over
    // the illustrative annual energy and the -5 % score band.
    let vlsfo = mix(&[("VLSFO", "1")]);
    let result = score(&vlsfo, 2026, None, None);
    assert_eq!(result.fueleu.ghg_intensity_gco2_mj, decimal("91.16"));
    assert_eq!(result.fueleu.target_intensity_gco2_mj, decimal("89.3368"));
    assert_eq!(result.fueleu.reduction_pct, decimal("-2.04"));
    assert_eq!(
        result.fueleu.compliance_balance_gco2,
        decimal("-546960000.00")
    );
    assert_eq!(result.fueleu.estimated_penalty_eur, decimal("351219.51"));
    assert_eq!(result.fueleu.score, 40);

    // A blended mix below the limit is compliant with no penalty.
    let blended = mix(&[("Methanol", "0.5"), ("VLSFO", "0.5")]);
    let blended_result = score(&blended, 2026, None, None);
    assert_eq!(
        blended_result.fueleu.ghg_intensity_gco2_mj,
        decimal("61.08")
    );
    assert_eq!(blended_result.fueleu.reduction_pct, decimal("31.63"));
    assert_eq!(blended_result.fueleu.estimated_penalty_eur, Decimal::ZERO);
    assert_eq!(blended_result.fueleu.score, 100);

    // The low-carbon mix reaches the same conclusion without pricing emissions.
    let e_methanol = mix(&[("E-Methanol", "1")]);
    let low_carbon = score(&e_methanol, 2026, Some("A"), None);
    assert_eq!(low_carbon.fueleu.ghg_intensity_gco2_mj, decimal("8.00"));
    assert_eq!(low_carbon.fueleu.reduction_pct, decimal("91.05"));
    assert_eq!(low_carbon.fueleu.score, 100);
    assert!(low_carbon.eu_ets.total_co2_tonnes.is_none());
    assert!(low_carbon.eu_ets.estimated_cost_eur.is_none());
    assert_eq!(
        low_carbon.eu_ets.calculation_status,
        CalculationStatus::Unpriced
    );

    // The 2030 step is stricter than 2026 for the same fuel.
    let lng_2026 = score(&mix(&[("LNG", "1")]), 2026, None, None);
    let lng_2030 = score(&mix(&[("LNG", "1")]), 2030, None, None);
    assert_eq!(lng_2026.fueleu.target_intensity_gco2_mj, decimal("89.3368"));
    assert_eq!(lng_2030.fueleu.target_intensity_gco2_mj, decimal("85.6904"));
    assert_eq!(lng_2026.fueleu.reduction_pct, decimal("22.76"));
    assert_eq!(lng_2030.fueleu.reduction_pct, decimal("19.48"));
    assert_eq!(lng_2026.fueleu.score, 100);
    assert_eq!(lng_2030.fueleu.score, 90);
    assert!(lng_2026.fueleu.reduction_pct > lng_2030.fueleu.reduction_pct);

    // The 2025 and 2026 reporting years share one schedule step.
    assert_eq!(
        score(&vlsfo, 2025, None, None)
            .fueleu
            .target_intensity_gco2_mj,
        score(&vlsfo, 2026, None, None)
            .fueleu
            .target_intensity_gco2_mj
    );

    // One tonne of VLSFO energy: deficit balance and first-year penalty units.
    let metre_tonne = compliance_score(ScoreInput {
        vessel_id: "vessel",
        vessel_name: "Vessel vessel",
        fuel_mix: &vlsfo,
        cii_rating: None,
        total_co2_tonnes: None,
        year: reporting_year(2025),
        total_energy_mj: Some(decimal("41000")),
    })
    .expect("valid energy");
    assert_eq!(
        metre_tonne.fueleu.compliance_balance_gco2,
        decimal("-74751.20")
    );
    assert_eq!(metre_tonne.fueleu.estimated_penalty_eur, decimal("48.00"));

    // Score bands are inclusive at each boundary.
    for (reduction, band) in [
        ("20", 100u8),
        ("19.99", 90),
        ("10", 90),
        ("9.99", 80),
        ("5", 80),
        ("4.99", 70),
        ("0", 70),
        ("-0.01", 40),
        ("-5", 40),
        ("-5.01", 20),
        ("-10", 20),
        ("-10.01", 0),
    ] {
        assert_eq!(
            fueleu_band(decimal(reduction)),
            band,
            "reduction {reduction}"
        );
    }

    // ETS status and score follow the declared emissions, never the fuel's CI.
    for (tonnes, ets_score) in [
        ("10000", 95u8),
        ("25000", 80),
        ("35000", 60),
        ("45000", 40),
        ("46000", 20),
    ] {
        let ets = ets_result(Some(decimal(tonnes)), DEFAULT_ETS_PRICE_EUR, 2026)
            .expect("valid emissions");
        assert_eq!(ets.score, ets_score, "tonnes {tonnes}");
        assert_eq!(ets.calculation_status, CalculationStatus::Scenario);
    }
    let priced = ets_result(Some(decimal("30000")), decimal("70"), 2026).expect("valid emissions");
    assert_eq!(priced.estimated_cost_eur, Some(decimal("2100000.00")));
    let unpriced = ets_result(None, DEFAULT_ETS_PRICE_EUR, 2026).expect("valid emissions");
    assert_eq!(unpriced.calculation_status, CalculationStatus::Unpriced);
    assert_eq!(unpriced.score, 50);
    for (year, phase_in) in [(2024, "40"), (2025, "70"), (2026, "100"), (2030, "100")] {
        assert_eq!(ets_phase_in(year), decimal(phase_in));
    }

    // CII ratings map to their fixed bands; an absent rating is the C band and
    // an unrecognised code keeps its normalized text with the neutral score.
    for (rating, expected_score) in [("A", 100u8), ("B", 80), ("C", 60), ("D", 30), ("E", 10)] {
        assert_eq!(cii_result(Some(rating)).score, expected_score);
    }
    assert_eq!(cii_result(None).rating, "C");
    assert_eq!(cii_result(None).score, 60);
    assert_eq!(cii_result(Some("a")).rating, "A");
    assert_eq!(cii_result(Some("a")).score, 100);
    assert_eq!(cii_result(Some("F")).score, 50);

    // The overall score weights FuelEU, ETS and CII at 50/30/20 and rounds
    // half to even.
    let excellent = score(&e_methanol, 2026, Some("A"), Some(decimal("10000")));
    assert_eq!(excellent.overall_score, 98);
    assert_eq!(excellent.status, ComplianceStatus::Excellent);
    assert_eq!(excellent.traffic_light, TrafficLight::Green);
    assert_eq!(excellent.recommendations.len(), 0);

    let poor = score(&vlsfo, 2026, Some("E"), Some(decimal("60000")));
    assert_eq!(poor.overall_score, 28);
    assert_eq!(poor.status, ComplianceStatus::Critical);
    assert_eq!(poor.traffic_light, TrafficLight::Red);
    assert!(poor.recommendations.len() >= 3);
    for prefix in [
        "Consider increasing",
        "High ETS exposure",
        "CII rating declining",
    ] {
        assert!(
            poor.recommendations
                .iter()
                .any(|hint| hint.starts_with(prefix)),
            "{prefix}"
        );
    }
    let penalty = poor
        .recommendations
        .iter()
        .find_map(|hint| hint.split_once("EUR ").map(|(_, amount)| amount))
        .expect("the dashboard extracts the EUR amount");
    assert!(penalty.starts_with(|ch: char| ch.is_ascii_digit()));
    assert!(poor
        .recommendations
        .iter()
        .any(|hint| hint.contains("FuelEU")));
    assert!(poor.recommendations.iter().any(|hint| hint.contains("CII")));
    assert!(poor
        .recommendations
        .iter()
        .any(|hint| hint.to_lowercase().contains("penalty")));

    // Half-even rounding of the weighted total keeps .5 values on the even side.
    assert_eq!(
        score(&e_methanol, 2026, Some("C"), Some(decimal("10000"))).overall_score,
        90
    );

    // Status thresholds.
    for (overall, status, light) in [
        (100u8, ComplianceStatus::Excellent, TrafficLight::Green),
        (90, ComplianceStatus::Excellent, TrafficLight::Green),
        (89, ComplianceStatus::Good, TrafficLight::Green),
        (70, ComplianceStatus::Good, TrafficLight::Green),
        (69, ComplianceStatus::Adequate, TrafficLight::Amber),
        (50, ComplianceStatus::Adequate, TrafficLight::Amber),
        (49, ComplianceStatus::AtRisk, TrafficLight::Amber),
        (30, ComplianceStatus::AtRisk, TrafficLight::Amber),
        (29, ComplianceStatus::Critical, TrafficLight::Red),
        (0, ComplianceStatus::Critical, TrafficLight::Red),
    ] {
        let (derived_status, derived_light) = status_for(overall);
        assert_eq!(derived_status, status, "overall {overall}");
        assert_eq!(derived_light, light, "overall {overall}");
    }
    assert_eq!(status_for(90).0.as_str(), "EXCELLENT");
    assert_eq!(status_for(29).1.as_str(), "RED");

    // A stricter year never improves the FuelEU contribution.
    let declining = mix(&[("LNG", "0.7"), ("VLSFO", "0.3")]);
    assert!(
        score(&declining, 2026, None, None).fueleu.score
            >= score(&declining, 2030, None, None).fueleu.score
    );

    // Identifier and label are echoed unchanged.
    assert_eq!(result.vessel_id, "e0b1c0de-0000-4000-8000-000000000001");
    assert_eq!(result.vessel_name, "MV Contract");

    // The response keeps the client field names and exact decimal text.
    let wire = excellent.to_json();
    assert_eq!(wire["overall_score"], json!(98));
    assert_eq!(wire["status"], json!("EXCELLENT"));
    assert_eq!(wire["traffic_light"], json!("GREEN"));
    assert_eq!(wire["fueleu"]["ghg_intensity_gco2_mj"], json!("8.00"));
    assert_eq!(wire["fueleu"]["target_intensity_gco2_mj"], json!("89.3368"));
    assert_eq!(wire["fueleu"]["estimated_penalty_eur"], json!("0.00"));
    assert_eq!(wire["eu_ets"]["calculation_status"], json!("SCENARIO"));
    assert_eq!(wire["eu_ets"]["total_co2_tonnes"], json!("10000"));
    assert_eq!(wire["eu_ets"]["ets_price_per_tonne_eur"], json!("68"));
    assert_eq!(wire["eu_ets"]["phase_in_pct"], json!("100"));
    assert_eq!(wire["eu_ets"]["estimated_cost_eur"], json!("680000.00"));
    assert_eq!(wire["cii"]["rating"], json!("A"));
    assert_eq!(wire["cii"]["score"], json!(100));

    // An absent emissions input stays unpriced with explicit nulls, never a
    // zero cost and never an inferred tonnes figure.
    let unpriced_wire = low_carbon.to_json();
    assert_eq!(
        unpriced_wire["eu_ets"]["calculation_status"],
        json!("UNPRICED")
    );
    assert_eq!(
        unpriced_wire["eu_ets"]["total_co2_tonnes"],
        serde_json::Value::Null
    );
    assert_eq!(
        unpriced_wire["eu_ets"]["estimated_cost_eur"],
        serde_json::Value::Null
    );
    assert_eq!(unpriced_wire["eu_ets"]["score"], json!(50));
}

#[test]
fn compliance_score_input_cases() {
    use verdaxis_read::compliance::scoring::{fueleu_result, ScoreError};
    let fuel_mix = mix(&[("VLSFO", "1")]);
    assert!(matches!(
        fueleu_result(&fuel_mix, reporting_year(2026), Some(Decimal::MAX)),
        Err(ScoreError::ArithmeticOverflow)
    ));
    assert!(matches!(
        fueleu_result(&fuel_mix, reporting_year(2026), Some(Decimal::NEGATIVE_ONE)),
        Err(ScoreError::NegativeInput)
    ));
    assert_eq!(
        fueleu_result(&fuel_mix, reporting_year(2026), Some(Decimal::ZERO))
            .unwrap()
            .compliance_balance_gco2,
        Decimal::ZERO
    );
    assert!(matches!(
        ets_result(Some(Decimal::MAX), DEFAULT_ETS_PRICE_EUR, 2026),
        Err(ScoreError::ArithmeticOverflow)
    ));
    assert!(matches!(
        ets_result(Some(Decimal::NEGATIVE_ONE), DEFAULT_ETS_PRICE_EUR, 2026),
        Err(ScoreError::NegativeInput)
    ));
    assert!(matches!(
        ets_result(None, Decimal::NEGATIVE_ONE, 2026),
        Err(ScoreError::NegativeInput)
    ));
}
