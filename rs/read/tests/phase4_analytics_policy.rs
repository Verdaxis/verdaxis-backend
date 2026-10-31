//! Phase 4 Task 15: shared analytics query, privacy, provenance and retention
//! primitives.
//!
//! The leaf modules are compiled here directly. Registering them in the crate
//! (`rs/read/src/analytics/mod.rs`) is an integration-owner edit, submitted as
//! a patch, so the leaves stay reachable without a concurrent shared-file
//! change.

pub use verdaxis_read::{ApiError, business};

#[allow(dead_code)]
#[path = "../src/analytics/facts.rs"]
mod facts;
#[allow(dead_code)]
#[path = "../src/analytics/policy.rs"]
mod policy;
#[allow(dead_code)]
#[path = "../src/analytics/query.rs"]
mod query;
#[allow(dead_code)]
#[path = "../src/analytics/retention.rs"]
mod retention;
#[allow(dead_code)]
#[path = "../src/analytics/types.rs"]
mod types;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use facts::{MarketProvenance, classify_order, classify_trade};
use policy::{
    MAX_COHORT_ROWS, MAX_MATRIX_CELLS, MAX_RANKED_ROWS, MAX_SERIES_POINTS, PropertyValueCheck,
    SUPPRESSION_THRESHOLD, allowed_event_properties, allowed_properties, check_property_value,
    property_domain, ratio_from_counts, suppress, suppress_cell,
};
use query::{
    AnalyticsQuery, MAX_AVAILABILITY_WINDOW_CHARS, MAX_RANGE_SECONDS, MIN_QUERY_INSTANT,
    QueryInput, previous_window, validate_query,
};
use retention::{
    AnalyticsFactFamily, MAX_SERIES_DATES, RETAINED_DATES, login_day_is_retained,
    login_retention_cutoff,
};
use types::{
    AggregateCell, AnalyticsActivity, AnalyticsAudience, AnalyticsDiagnostic,
    AnalyticsSourceCoverage, AnalyticsSourceStatus, ComponentStatus, DecimalMetricValue,
    ProviderComponent, ReferenceCoverageRow, ReferenceCoverageStatus, ReferenceScope,
    ReferenceSourceKind, ReferenceSourceLabel,
};

fn input(pairs: &[(&str, &str)]) -> QueryInput {
    QueryInput::from_pairs(pairs.iter().copied())
}

fn refusal_code(error: &ApiError) -> (u16, String, String) {
    (
        error.status.as_u16(),
        error.detail["code"].as_str().unwrap_or_default().to_owned(),
        error.detail["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// c01: the comparison period is arithmetic, and an underflow is a 422.
#[test]
fn p4_t15_c01() {
    // Ten days into year 1, the comparison period begins before the earliest
    // addressable instant, so the query is refused rather than widened.
    let underflowing = input(&[
        ("start", "0001-01-10T00:00:00"),
        ("end", "0001-12-31T00:00:00"),
    ]);
    let error = validate_query(underflowing).expect_err("previous window underflows");
    let (status, code, message) = refusal_code(&error);
    assert_eq!(status, 422);
    assert_eq!(code, "ANALYTICS_QUERY_INVALID");
    // The refusal is the checked comparison window, not a malformed input.
    assert_eq!(
        message,
        "the comparison period is outside the supported range"
    );

    // The same period is valid when no comparison is asked for: the refusal
    // is the checked arithmetic, not the range.
    let without_comparison = input(&[
        ("start", "0001-01-10T00:00:00"),
        ("end", "0001-12-31T00:00:00"),
        ("compare", "false"),
    ]);
    let query = validate_query(without_comparison).expect("no previous period is computed");
    assert_eq!(query.previous, None);

    // The guard is the comparison arithmetic, not a floor on the period: the
    // shortest period inside year 1 is still refused, and the same period
    // placed after the boundary is valid.
    assert_eq!(
        refusal_code(
            &validate_query(input(&[
                ("start", "0001-01-01T00:00:00"),
                ("end", "0001-01-02T00:00:00"),
            ]))
            .unwrap_err()
        )
        .2,
        "the comparison period is outside the supported range"
    );
    let after_boundary = validate_query(input(&[
        ("start", "0001-01-02T00:00:00"),
        ("end", "0001-01-03T00:00:00"),
    ]))
    .expect("comparison fits");
    assert_eq!(
        after_boundary.previous_start(),
        Some("0001-01-01T00:00:00Z".parse().unwrap())
    );

    // The published window helper owns the same checked arithmetic.
    let earliest: DateTime<Utc> = MIN_QUERY_INSTANT.parse().unwrap();
    let (status, code, message) =
        refusal_code(&previous_window(earliest, Duration::seconds(1)).unwrap_err());
    assert_eq!(status, 422);
    assert_eq!(code, "ANALYTICS_QUERY_INVALID");
    assert_eq!(
        message,
        "the comparison period is outside the supported range"
    );
    assert_eq!(
        previous_window(earliest + Duration::days(1), Duration::days(1)).expect("comparison fits"),
        (earliest, earliest + Duration::days(1))
    );
}

/// c02: half-open UTC period, naive-as-UTC and offset normalization, checked
/// bounds and defaults.
#[test]
fn p4_t15_c02() {
    let query = validate_query(input(&[
        ("start", "2026-06-01T00:00:00"),
        ("end", "2026-07-01T00:00:00"),
    ]))
    .expect("valid naive period");
    let start: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
    let end: DateTime<Utc> = "2026-07-01T00:00:00Z".parse().unwrap();
    assert_eq!(query.start, start);
    assert_eq!(query.end, end);
    assert_eq!(query.previous, Some((start - (end - start), start)));
    assert_eq!(query.previous_start(), Some(start - Duration::days(30)));
    assert_eq!(query.previous_end(), Some(start));

    // An explicit offset is converted to UTC.
    let offset = validate_query(input(&[
        ("start", "2026-06-01T08:00:00+08:00"),
        ("end", "2026-07-01T08:00:00+08:00"),
    ]))
    .expect("valid offset period");
    assert_eq!(offset.start, start);
    assert_eq!(offset.end, end);

    // Defaults: compare on, ALL audience, LIVE activity, no filters.
    assert!(query.compare);
    assert_eq!(query.audience, AnalyticsAudience::All);
    assert_eq!(query.activity, AnalyticsActivity::Live);
    assert_eq!(query.product_id, None);
    assert_eq!(query.delivery_point_id, None);
    assert_eq!(query.availability_window, None);

    // Enum and UUID values are typed, and unknown parameters are ignored
    // rather than copied into the query.
    let typed = validate_query(input(&[
        ("start", "2026-06-01T00:00:00Z"),
        ("end", "2026-06-02T00:00:00Z"),
        ("audience", "SUPPLIER"),
        ("activity", "ALL"),
        ("product_id", "0e4d6a04-7a8a-4a3e-9d7a-6f8a43a1b2c3"),
        ("availability_window", "2026-Q3"),
        ("unknown_parameter", "SELECT 1"),
        ("offset", "20"),
    ]))
    .expect("valid typed period");
    assert_eq!(typed.audience, AnalyticsAudience::Supplier);
    assert_eq!(typed.activity, AnalyticsActivity::All);
    assert_eq!(typed.availability_window.as_deref(), Some("2026-Q3"));
    assert!(typed.product_id.is_some());

    // Strictly increasing, at most 365 days, otherwise 422.
    assert_eq!(
        refusal_code(
            &validate_query(input(&[
                ("start", "2026-06-01T00:00:00Z"),
                ("end", "2026-06-01T00:00:00Z"),
            ]))
            .unwrap_err()
        )
        .0,
        422
    );
    assert_eq!(
        refusal_code(
            &validate_query(input(&[
                ("start", "2026-07-01T00:00:00Z"),
                ("end", "2026-06-01T00:00:00Z"),
            ]))
            .unwrap_err()
        )
        .0,
        422
    );
    assert!(
        validate_query(input(&[
            ("start", "2026-01-01T00:00:00Z"),
            ("end", "2027-01-01T00:00:00Z"),
        ]))
        .is_ok()
    );
    let over = validate_query(input(&[
        ("start", "2026-01-01T00:00:00Z"),
        ("end", "2027-01-01T00:00:01Z"),
    ]));
    assert_eq!(refusal_code(&over.unwrap_err()).0, 422);
    assert_eq!(MAX_RANGE_SECONDS, 31_536_000);

    // A 16-character window is accepted, 17 is refused.
    let longest = "x".repeat(MAX_AVAILABILITY_WINDOW_CHARS);
    assert!(
        validate_query(input(&[
            ("start", "2026-06-01T00:00:00Z"),
            ("end", "2026-06-02T00:00:00Z"),
            ("availability_window", longest.as_str()),
        ]))
        .is_ok()
    );
    let too_long = "x".repeat(MAX_AVAILABILITY_WINDOW_CHARS + 1);
    assert_eq!(
        refusal_code(
            &validate_query(input(&[
                ("start", "2026-06-01T00:00:00Z"),
                ("end", "2026-06-02T00:00:00Z"),
                ("availability_window", too_long.as_str()),
            ]))
            .unwrap_err()
        )
        .0,
        422
    );

    // Missing, malformed and out-of-domain values are all 422 refusals with
    // the same envelope, never a database read.
    let refused_cases: [&[(&str, &str)]; 9] = [
        &[("end", "2026-06-02T00:00:00Z")],
        &[("start", "2026-06-01T00:00:00Z")],
        &[
            ("start", "not-a-timestamp"),
            ("end", "2026-06-02T00:00:00Z"),
        ],
        &[
            ("start", "2026-06-01T00:00:00Z"),
            ("end", "2026-06-02T00:00:00Z"),
            ("compare", "yes"),
        ],
        &[
            ("start", "2026-06-01T00:00:00Z"),
            ("end", "2026-06-02T00:00:00Z"),
            ("audience", "EVERYONE"),
        ],
        &[
            ("start", "2026-06-01T00:00:00Z"),
            ("end", "2026-06-02T00:00:00Z"),
            ("activity", "MIXED"),
        ],
        &[
            ("start", "2026-06-01T00:00:00Z"),
            ("end", "2026-06-02T00:00:00Z"),
            ("product_id", "not-a-uuid"),
        ],
        // Year zero and expanded years are outside the four-digit client
        // calendar, so they are refused at parse time.
        &[
            ("start", "0000-01-01T00:00:00"),
            ("end", "0000-02-01T00:00:00"),
        ],
        &[
            ("start", "-0001-01-01T00:00:00"),
            ("end", "0001-02-01T00:00:00"),
        ],
    ];
    for bad in refused_cases {
        let error = validate_query(input(bad)).expect_err("refused");
        let (status, code, message) = refusal_code(&error);
        assert_eq!(status, 422);
        assert_eq!(code, "ANALYTICS_QUERY_INVALID");
        assert!(!message.is_empty());
    }

    // compare=false has no previous period.
    let plain = validate_query(input(&[
        ("start", "2026-06-01T00:00:00Z"),
        ("end", "2026-06-02T00:00:00Z"),
        ("compare", "false"),
    ]))
    .unwrap();
    assert_eq!(plain.previous, None);
    assert_eq!(plain.previous_start(), None);
    assert_eq!(plain.previous_end(), None);
    let query: AnalyticsQuery = plain;
    assert!(!query.compare);
}

/// c03: a positive basis of one or two is suppressed; zero and three are
/// visible; a missing count is unavailable, not suppressed.
#[test]
fn p4_t15_c03() {
    assert_eq!(SUPPRESSION_THRESHOLD, 3);
    let decision = |count, entities| {
        let cell = suppress(count, entities);
        (cell.count, cell.suppressed)
    };
    assert_eq!(decision(Some(0), None), (Some(0), false));
    assert_eq!(decision(Some(1), None), (None, true));
    assert_eq!(decision(Some(2), None), (None, true));
    assert_eq!(decision(Some(3), None), (Some(3), false));
    assert_eq!(decision(None, None), (None, false));
    assert_eq!(decision(None, Some(1)), (None, false));

    // The entity basis governs when it is available, even against a large
    // event count.
    assert_eq!(decision(Some(500), Some(2)), (None, true));
    assert_eq!(decision(Some(500), Some(0)), (Some(500), false));
    assert_eq!(decision(Some(500), Some(3)), (Some(500), false));

    // The privacy decision is unkeyed; a labelled cell carries the key.
    let unkeyed = suppress(Some(2), None);
    assert_eq!(unkeyed.key, "");
    assert_eq!(unkeyed.count, None);
    assert!(unkeyed.suppressed);

    let cell = suppress_cell("BUYER", Some(2), None);
    assert_eq!(
        cell,
        AggregateCell {
            key: "BUYER".into(),
            count: None,
            suppressed: true,
        }
    );
    assert_eq!(
        suppress(Some(2), None).with_key("BUYER"),
        cell,
        "with_key labels the same decision"
    );
    // A suppressed cell never carries a count on the wire; a genuine zero does.
    assert_eq!(
        serde_json::to_value(&cell).unwrap(),
        json!({"key": "BUYER", "count": null, "suppressed": true})
    );
    assert_eq!(
        serde_json::to_value(suppress_cell("SUPPLIER", Some(0), None)).unwrap(),
        json!({"key": "SUPPLIER", "count": 0, "suppressed": false})
    );
}

/// c04: provenance is stored, classified and reported separately; reference
/// coverage is never activity.
#[test]
fn p4_t15_c04() {
    assert_eq!(classify_order("REAL", false), MarketProvenance::Live);
    assert_eq!(classify_order("DEMO", false), MarketProvenance::Demo);
    assert_eq!(classify_order("REAL", true), MarketProvenance::Demo);
    assert_eq!(classify_order("UNKNOWN", false), MarketProvenance::Unknown);
    assert_eq!(classify_order("TEST", false), MarketProvenance::Unknown);
    assert_eq!(classify_order("CANARY", false), MarketProvenance::Unknown);

    assert_eq!(
        classify_trade("REAL", false, "REAL", false),
        MarketProvenance::Live
    );
    // One demo party contaminates the trade: it is never live.
    assert_eq!(
        classify_trade("REAL", false, "DEMO", false),
        MarketProvenance::Demo
    );
    assert_eq!(
        classify_trade("REAL", false, "REAL", true),
        MarketProvenance::Demo
    );
    assert_eq!(
        classify_trade("REAL", false, "UNKNOWN", false),
        MarketProvenance::Unknown
    );
    assert_eq!(
        classify_trade("UNKNOWN", false, "UNKNOWN", false),
        MarketProvenance::Unknown
    );

    // Each stored class is admitted only by its own filter.
    assert!(AnalyticsActivity::Live.includes(MarketProvenance::Live));
    assert!(!AnalyticsActivity::Live.includes(MarketProvenance::Demo));
    assert!(!AnalyticsActivity::Live.includes(MarketProvenance::Unknown));
    assert!(AnalyticsActivity::Demo.includes(MarketProvenance::Demo));
    assert!(AnalyticsActivity::All.includes(MarketProvenance::Live));
    assert!(AnalyticsActivity::All.includes(MarketProvenance::Demo));
    assert!(AnalyticsActivity::All.includes(MarketProvenance::Unknown));

    // Reference rows are benchmark coverage, not market activity: the filter
    // admits no stored provenance at all.
    for provenance in [
        MarketProvenance::Live,
        MarketProvenance::Demo,
        MarketProvenance::Unknown,
    ] {
        assert!(!AnalyticsActivity::Reference.includes(provenance));
    }
    // The response label stays the client's SCREAMING_SNAKE_CASE value.
    assert_eq!(
        serde_json::to_value(AnalyticsActivity::Reference).unwrap(),
        json!("REFERENCE")
    );
    // Only three stored classes exist; there is no reference provenance.
    assert_eq!(
        [
            MarketProvenance::Live.provenance_label(),
            MarketProvenance::Demo.provenance_label(),
            MarketProvenance::Unknown.provenance_label(),
        ],
        ["live", "demo", "unknown"]
    );
}

/// c05: login-day retention keeps today and the previous 799 UTC dates, and
/// status history is never pruned.
#[test]
fn p4_t15_c05() {
    let today = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
    let cutoff = login_retention_cutoff(today);
    assert_eq!(cutoff, today - Duration::days(799));
    assert_eq!(RETAINED_DATES, 800);
    assert_eq!((today - cutoff).num_days() + 1, RETAINED_DATES);

    // The boundary is inclusive: the oldest retained date is visible and the
    // day before it is prunable.
    assert!(login_day_is_retained(cutoff, today));
    assert!(login_day_is_retained(today, today));
    assert!(!login_day_is_retained(cutoff - Duration::days(1), today));
    // The window is fixed by the calendar date, not the wall clock.
    assert_eq!(
        login_retention_cutoff(NaiveDate::from_ymd_opt(2027, 7, 15).unwrap()),
        NaiveDate::from_ymd_opt(2025, 5, 7).unwrap()
    );

    assert!(AnalyticsFactFamily::LoginDay.is_pruned());
    assert!(!AnalyticsFactFamily::StatusTransition.is_pruned());

    // The retained window always covers a full period series and its
    // comparison series.
    assert_eq!(MAX_SERIES_DATES, 732);
    assert_eq!(RETAINED_DATES - MAX_SERIES_DATES, 68);

    // An extreme input saturates instead of panicking; nothing is prunable
    // there.
    assert_eq!(login_retention_cutoff(NaiveDate::MIN), NaiveDate::MIN);
}

/// c06: response bounds come from the client contract, not the query range.
#[test]
fn p4_t15_c06() {
    assert_eq!(MAX_SERIES_POINTS, 366);
    assert_eq!(MAX_RANKED_ROWS, 20);
    assert_eq!(MAX_MATRIX_CELLS, 512);
    assert_eq!(MAX_COHORT_ROWS, 54);
    // A maximum-length period spans at most 366 inclusive UTC dates when both
    // bounds fall mid-day, so a 366-point series is reachable without widening
    // the 365-day query range.
    let start: DateTime<Utc> = "2026-01-01T12:00:00Z".parse().unwrap();
    let end: DateTime<Utc> = "2027-01-01T12:00:00Z".parse().unwrap();
    assert_eq!((end - start).num_seconds(), MAX_RANGE_SECONDS);
    let days = (end.date_naive() - start.date_naive()).num_days() + 1;
    assert_eq!(days, MAX_SERIES_POINTS as i64);
}

/// c07: an absent source and a reference row never become fabricated values.
#[test]
fn p4_t15_c07() {
    let absent = AnalyticsSourceCoverage::not_applicable();
    assert_eq!(absent.status, AnalyticsSourceStatus::NotApplicable);
    assert_eq!(absent.coverage_start, None);
    assert_eq!(absent.observed_at, None);
    assert_eq!(absent.diagnostic, None);
    assert_eq!(
        serde_json::to_value(&absent).unwrap(),
        json!({
            "coverage_start": null,
            "coverage_end": null,
            "observed_at": null,
            "status": "not_applicable",
            "diagnostic": null,
        })
    );
    assert_eq!(
        serde_json::to_value(AnalyticsSourceCoverage::unavailable(
            AnalyticsDiagnostic::InsufficientCoverage
        ))
        .unwrap()["diagnostic"],
        json!("insufficient_coverage")
    );

    // A current reference row must carry its price and observation; an
    // unavailable row exposes neither.
    let current = reference_row(ReferenceCoverageStatus::Current);
    assert!(current.validate().is_ok());
    let mut missing_observation = reference_row(ReferenceCoverageStatus::Current);
    missing_observation.observed_at = None;
    assert!(missing_observation.validate().is_err());
    let mut missing_price = reference_row(ReferenceCoverageStatus::Current);
    missing_price.benchmark_price_usd_per_mt = None;
    assert!(missing_price.validate().is_err());

    let unavailable = reference_row(ReferenceCoverageStatus::Unavailable);
    assert!(unavailable.validate().is_ok());
    assert_eq!(unavailable.benchmark_price_usd_per_mt, None);
    assert_eq!(unavailable.observed_at, None);
    let mut leaked = reference_row(ReferenceCoverageStatus::Unavailable);
    leaked.benchmark_price_usd_per_mt = Some(Decimal::new(81250, 2));
    assert!(leaked.validate().is_err());

    // Stale coverage may keep a price without a fresh observation.
    let mut stale = reference_row(ReferenceCoverageStatus::Stale);
    stale.observed_at = None;
    assert!(stale.validate().is_ok());
}

fn reference_row(status: ReferenceCoverageStatus) -> ReferenceCoverageRow {
    let generated: DateTime<Utc> = "2026-07-01T00:00:00Z".parse().unwrap();
    match status {
        ReferenceCoverageStatus::Current => ReferenceCoverageRow {
            product_key: "BIO_METHANOL".into(),
            product_label: "Bio Methanol".into(),
            delivery_point_key: "singapore".into(),
            delivery_point_label: "Singapore".into(),
            availability_window: "SPOT".into(),
            availability_window_label: "Spot".into(),
            benchmark_price_usd_per_mt: Some(Decimal::new(81250, 2)),
            source_label: ReferenceSourceLabel::ManualOverride,
            generated_at: generated,
            observed_at: Some(generated - Duration::hours(4)),
            source_kind: ReferenceSourceKind::AdminBenchmark,
            scope: ReferenceScope::ExactSlice,
            coverage_status: ReferenceCoverageStatus::Current,
        },
        ReferenceCoverageStatus::Stale => ReferenceCoverageRow {
            benchmark_price_usd_per_mt: Some(Decimal::new(81250, 2)),
            source_label: ReferenceSourceLabel::SeedMatrix,
            observed_at: None,
            source_kind: ReferenceSourceKind::SeededBenchmark,
            scope: ReferenceScope::WindowAdjusted,
            coverage_status: ReferenceCoverageStatus::Stale,
            ..reference_row(ReferenceCoverageStatus::Current)
        },
        ReferenceCoverageStatus::Unavailable => ReferenceCoverageRow {
            benchmark_price_usd_per_mt: None,
            observed_at: None,
            coverage_status: ReferenceCoverageStatus::Unavailable,
            ..reference_row(ReferenceCoverageStatus::Current)
        },
    }
}

/// c08: a provider component reports absence, never a made-up zero or list.
#[test]
fn p4_t15_c08() {
    let not_requested: ProviderComponent<Decimal> = ProviderComponent::not_requested();
    assert_eq!(not_requested.status(), ComponentStatus::NotRequested);
    assert_eq!(not_requested.data(), None);
    assert!(not_requested.is_coherent());
    assert_eq!(
        serde_json::to_value(&not_requested).unwrap(),
        json!({"status": "not_requested", "diagnostic": null, "data": null})
    );

    let unavailable: ProviderComponent<Vec<u64>> =
        ProviderComponent::unavailable(AnalyticsDiagnostic::Timeout);
    assert_eq!(unavailable.status(), ComponentStatus::Unavailable);
    assert_eq!(unavailable.data(), None);
    assert!(unavailable.is_coherent());
    // The status fields are private, so absence cannot be paired with data by
    // a struct literal; only the constructors decide.
    assert_eq!(
        serde_json::to_value(&unavailable).unwrap()["diagnostic"],
        json!("timeout")
    );
    assert_eq!(
        unavailable.into_data(),
        None::<Vec<u64>>,
        "an unavailable component yields no data"
    );

    // An available empty collection is a genuine empty response.
    let empty = ProviderComponent::available(Vec::<u64>::new());
    assert_eq!(empty.status(), ComponentStatus::Available);
    assert_eq!(empty.data(), Some(&Vec::new()));
    assert!(empty.is_coherent());
    assert_eq!(serde_json::to_value(&empty).unwrap()["data"], json!([]));

    // A partial component keeps only confirmed data; the missing part stays
    // absent.
    let partial = ProviderComponent::partial(Some(vec![1]), Some(AnalyticsDiagnostic::Upstream));
    assert_eq!(partial.status(), ComponentStatus::Partial);
    assert_eq!(partial.data(), Some(&vec![1]));
    assert!(partial.is_coherent());
    let partial_none: ProviderComponent<Vec<u64>> =
        ProviderComponent::partial(None, Some(AnalyticsDiagnostic::Upstream));
    assert_eq!(partial_none.data(), None);
    assert!(partial_none.is_coherent());
}

/// c09: decimals keep their exact wire spelling and ratios publish only with
/// a positive denominator.
#[test]
fn p4_t15_c09() {
    let metric = DecimalMetricValue {
        value: Some(Decimal::new(1234, 2)),
        previous: None,
        suppressed: false,
    };
    assert_eq!(
        serde_json::to_value(&metric).unwrap(),
        json!({"value": "12.34", "previous": null, "suppressed": false})
    );

    let third = ratio_from_counts(Some(1), Some(3), true);
    assert_eq!(
        third.rate_pct.map(|rate| rate.to_string()),
        Some("33.33".into())
    );
    let two_thirds = ratio_from_counts(Some(2), Some(3), true);
    assert_eq!(
        two_thirds.rate_pct.map(|rate| rate.to_string()),
        Some("66.67".into())
    );
    assert_eq!(
        serde_json::to_value(&two_thirds).unwrap(),
        json!({
            "numerator": 2,
            "denominator": 3,
            "rate_pct": "66.67",
            "suppressed": false,
            "cohort_complete": true,
        })
    );

    // No denominator, a zero denominator or a missing numerator yields no
    // rate, and never a zero percent.
    assert_eq!(ratio_from_counts(Some(4), Some(0), true).rate_pct, None);
    assert_eq!(ratio_from_counts(Some(4), None, true).rate_pct, None);
    assert_eq!(ratio_from_counts(None, Some(4), true).rate_pct, None);
    assert_eq!(
        serde_json::to_value(ratio_from_counts(None, Some(4), false)).unwrap(),
        json!({
            "numerator": null,
            "denominator": 4,
            "rate_pct": null,
            "suppressed": false,
            "cohort_complete": false,
        })
    );
}

/// c10: the event, property and value taxonomy is closed.
#[test]
fn p4_t15_c10() {
    let events = policy::RegisteredEvent::all();
    assert_eq!(events.len(), 31);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.is_server_event())
            .count(),
        4
    );
    assert_eq!(
        policy::RegisteredEvent::from_name("order_created"),
        Some(policy::RegisteredEvent::Server(
            policy::AllowedServerEvent::OrderCreated
        ))
    );
    for unknown in [
        "",
        "order_created_extra",
        "ORDER_CREATED",
        "click",
        "landing_cta_click",
    ] {
        assert_eq!(policy::RegisteredEvent::from_name(unknown), None);
    }

    let order_created = policy::RegisteredEvent::Server(policy::AllowedServerEvent::OrderCreated);
    assert_eq!(
        allowed_properties(order_created),
        vec![
            policy::AllowedProperty::Role,
            policy::AllowedProperty::Side,
            policy::AllowedProperty::CanonicalProduct,
            policy::AllowedProperty::DeliveryPoint,
            policy::AllowedProperty::AvailabilityWindow,
        ]
    );
    // A property registered for another event is not accepted here.
    assert_eq!(
        property_domain(order_created, policy::AllowedProperty::DemoStatus),
        None
    );
    assert_eq!(
        allowed_event_properties(policy::AllowedServerEvent::OrderCreated).properties,
        allowed_properties(order_created)
    );
    assert_eq!(
        allowed_event_properties(policy::AllowedServerEvent::TradeCreated).event,
        policy::RegisteredEvent::Server(policy::AllowedServerEvent::TradeCreated)
    );

    // Every registered event is answered and its properties are its own.
    for event in &events {
        let properties = allowed_properties(*event);
        for property in &properties {
            assert!(property_domain(*event, *property).is_some());
        }
        // No event accepts a property twice, and every accepted property is
        // in the union domain.
        let mut unique = properties.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), properties.len(), "{}", event.name());
        for property in &properties {
            assert!(policy::AllowedProperty::from_name(property.name()).is_some());
        }
    }
    // The server allowlist is exactly the four server events' declarations.
    for event in policy::AllowedServerEvent::all() {
        let sanitized = allowed_event_properties(event);
        assert_eq!(
            sanitized.properties,
            allowed_properties(policy::RegisteredEvent::Server(event))
        );
    }

    // Closed value domains: trading roles exclude ADMIN, the login role
    // domain includes it.
    let registration =
        policy::RegisteredEvent::Server(policy::AllowedServerEvent::RegistrationCompleted);
    assert_eq!(
        check_property_value(registration, policy::AllowedProperty::Role, "BUYER"),
        PropertyValueCheck::Allowed
    );
    assert_eq!(
        check_property_value(registration, policy::AllowedProperty::Role, "ADMIN"),
        PropertyValueCheck::Rejected
    );
    let login = policy::RegisteredEvent::Frontend(policy::AllowedFrontendEvent::LoginSucceeded);
    assert_eq!(
        check_property_value(login, policy::AllowedProperty::Role, "ADMIN"),
        PropertyValueCheck::Allowed
    );
    assert_eq!(
        check_property_value(login, policy::AllowedProperty::Role, "admin"),
        PropertyValueCheck::Rejected
    );
    // A property this event does not accept is rejected outright, not
    // resolved externally.
    assert_eq!(
        check_property_value(login, policy::AllowedProperty::Product, "BIO_METHANOL"),
        PropertyValueCheck::Rejected
    );

    // Tutorial steps are a bounded numbered domain per role.
    let step =
        policy::RegisteredEvent::Frontend(policy::AllowedFrontendEvent::TutorialStepCompleted);
    for (value, allowed) in [
        ("buyer_1", true),
        ("buyer_28", true),
        ("buyer_29", false),
        ("buyer_0", false),
        ("supplier_29", true),
        ("supplier_30", false),
        ("buyer_x", false),
        ("admin_1", false),
        ("buyer", false),
        ("", false),
    ] {
        assert_eq!(
            check_property_value(step, policy::AllowedProperty::Step, value),
            if allowed {
                PropertyValueCheck::Allowed
            } else {
                PropertyValueCheck::Rejected
            },
            "{value}"
        );
    }

    // A catalog-resolved value is not closed here: the route boundary resolves
    // it through the pinned reference and never forwards raw text.
    assert_eq!(
        check_property_value(
            order_created,
            policy::AllowedProperty::CanonicalProduct,
            "anything"
        ),
        PropertyValueCheck::External
    );
    assert_eq!(
        check_property_value(order_created, policy::AllowedProperty::Role, "BUYER"),
        PropertyValueCheck::Allowed
    );
    assert_eq!(
        check_property_value(order_created, policy::AllowedProperty::Role, "VIEWER"),
        PropertyValueCheck::Rejected
    );
    assert_eq!(
        policy::AllowedProperty::from_name("role"),
        Some(policy::AllowedProperty::Role)
    );
    assert_eq!(policy::AllowedProperty::from_name("email"), None);
}

/// c11: the response types keep the client field names and null shape.
#[test]
fn p4_t15_c11() {
    assert_eq!(
        serde_json::to_value(AnalyticsAudience::Buyer).unwrap(),
        json!("BUYER")
    );
    assert_eq!(
        serde_json::to_value(AnalyticsSourceStatus::NotApplicable).unwrap(),
        json!("not_applicable")
    );
    assert_eq!(
        serde_json::to_value(AnalyticsDiagnostic::MalformedResponse).unwrap(),
        json!("malformed_response")
    );
    assert_eq!(
        serde_json::to_value(ReferenceCoverageStatus::Current).unwrap(),
        json!("current")
    );

    // SeriesPoint keeps the client's `date`/`value` shape, and a missing
    // value is null rather than a zero.
    let point = types::SeriesPoint {
        date: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
        value: None,
    };
    assert_eq!(
        serde_json::to_value(&point).unwrap(),
        json!({"date": "2026-07-01", "value": null})
    );

    // A metric with no previous-period sample keeps null, not zero.
    let metric = types::MetricValue {
        value: None,
        previous: None,
        suppressed: false,
    };
    assert_eq!(
        serde_json::to_value(&metric).unwrap(),
        json!({"value": null, "previous": null, "suppressed": false})
    );

    // The reference coverage status is a literal string the client reads.
    assert_eq!(
        serde_json::to_value(ReferenceScope::WindowAdjusted).unwrap(),
        json!("WINDOW_ADJUSTED")
    );
    assert_eq!(
        serde_json::to_value(ReferenceSourceKind::TradeDerivedVwap).unwrap(),
        json!("TRADE_DERIVED_VWAP")
    );

    // A registered event maps to its stored provider name; an unregistered
    // name never reaches a response.
    assert_eq!(
        policy::RegisteredEvent::from_name("market_slice_selected")
            .map(policy::RegisteredEvent::name),
        Some("market_slice_selected")
    );
    assert_eq!(policy::RegisteredEvent::from_name("raw_user_value"), None);
}

/// c12: the requested property set is validated, sorted and bounded.
#[test]
fn p4_t15_c12() {
    use policy::{AllowedProperty as Property, AllowedPropertySet, RegisteredEvent as Event};
    use policy::{AllowedServerEvent as Server, PropertySetError};

    let order = Event::Server(Server::OrderCreated);
    let navigation = Event::Frontend(policy::AllowedFrontendEvent::PlatformNavigation);

    // Duplicates collapse and the set is sorted, so a caller cannot inflate
    // the request or vary the provider query by input order.
    let first = AllowedPropertySet::new([
        (order, Property::Side),
        (order, Property::Role),
        (order, Property::Side),
    ])
    .expect("declared properties");
    let second = AllowedPropertySet::new([(order, Property::Role), (order, Property::Side)])
        .expect("declared properties");
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    assert!(!first.is_empty());
    assert_eq!(first.event_count(), 1);
    assert_eq!(first.events(), vec![order]);

    // A property registered for a different event is refused outright.
    let error = AllowedPropertySet::new([(order, Property::DemoStatus)])
        .expect_err("demo_status is not an order_created property");
    assert_eq!(error, PropertySetError::PropertyNotRegistered);
    let refused = AllowedPropertySet::refusal(error);
    assert_eq!(refused.status.as_u16(), 422);
    assert_eq!(refused.detail["code"], "ANALYTICS_PROPERTY_INVALID");

    // An empty property list is valid and asks for no breakdown.
    let empty = AllowedPropertySet::new(Vec::<(Event, Property)>::new()).expect("empty set");
    assert!(empty.is_empty());
    assert_eq!(empty.event_count(), 0);

    // Twelve distinct events are accepted, thirteen are refused before any
    // provider call.
    let twelve: Vec<(Event, Property)> = [
        order,
        navigation,
        Event::Server(Server::TradeCreated),
        Event::Server(Server::RegistrationCompleted),
        Event::Frontend(policy::AllowedFrontendEvent::LoginSucceeded),
        Event::Frontend(policy::AllowedFrontendEvent::LoginFailed),
        Event::Frontend(policy::AllowedFrontendEvent::FrontendError),
        Event::Frontend(policy::AllowedFrontendEvent::EstimatorCompleted),
        Event::Frontend(policy::AllowedFrontendEvent::TutorialStarted),
        Event::Frontend(policy::AllowedFrontendEvent::SignupStarted),
        Event::Frontend(policy::AllowedFrontendEvent::ListingOpened),
        Event::Frontend(policy::AllowedFrontendEvent::OrderFormOpened),
    ]
    .into_iter()
    .map(|event| (event, policy::allowed_properties(event)[0]))
    .collect();
    let accepted = AllowedPropertySet::new(twelve.clone()).expect("twelve events");
    assert_eq!(accepted.event_count(), 12);
    let mut too_many = twelve;
    too_many.push((
        Event::Frontend(policy::AllowedFrontendEvent::MarketSliceSelected),
        Property::Product,
    ));
    assert_eq!(
        AllowedPropertySet::new(too_many).unwrap_err(),
        PropertySetError::TooManyEvents
    );

    // Shared provider bounds.
    assert_eq!(policy::MAX_PROVIDER_EVENTS, 12);
    assert_eq!(policy::MAX_PROVIDER_PROPERTY_ROWS, 500);
    assert_eq!(policy::MAX_PROVIDER_BODY_BYTES, 1_000_000);
    assert_eq!(policy::MAX_REGISTERED_EVENTS, 32);
    assert!(policy::RegisteredEvent::all().len() <= policy::MAX_REGISTERED_EVENTS);
}

/// c13: an unavailable behavioral window leaves every component absent, with
/// its coverage marked unavailable and no inferred zero.
#[test]
fn p4_t15_c13() {
    let start: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
    let end: DateTime<Utc> = "2026-07-01T00:00:00Z".parse().unwrap();
    let window =
        types::BehavioralWindow::unavailable(start, end, end, AnalyticsDiagnostic::Authentication);
    assert_eq!(window.start, start);
    assert_eq!(window.end, end);
    assert_eq!(
        window.availability,
        types::BehavioralAvailability::Unavailable
    );
    assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Authentication));
    assert_eq!(window.coverage.status, AnalyticsSourceStatus::Unavailable);
    for status in window.components() {
        assert_eq!(status, ComponentStatus::Unavailable);
    }
    assert!(window.core.data().is_none());
    assert!(window.event_totals.data().is_none());
    assert!(window.daily_visitors.data().is_none());
    assert!(window.property_results.is_empty());
    assert_eq!(window.validate(), Ok(()));
    assert_eq!(
        serde_json::to_value(&window).unwrap()["daily_visitors"],
        json!({
            "status": "unavailable",
            "diagnostic": "authentication",
            "data": null,
        })
    );

    // A window that claims unavailable while one component carries data is
    // rejected, so an absent provider is never read as an empty result.
    let mut fabricated = serde_json::to_value(&window).unwrap();
    fabricated["daily_visitors"] = json!({
        "status": "unavailable",
        "diagnostic": "authentication",
        "data": [],
    });
    let fabricated: types::BehavioralWindow = serde_json::from_value(fabricated).unwrap();
    assert_eq!(
        fabricated.validate(),
        Err(types::BehavioralWindowError::IncoherentComponent)
    );

    // A window whose six core components are available but whose property
    // result is partial is itself partial: the per-property components
    // count toward the availability agreement.
    let mut property_partial = serde_json::to_value(&window).unwrap();
    property_partial["availability"] = json!("partial");
    for key in [
        "core",
        "event_totals",
        "daily_visitors",
        "event_series",
        "top_entries",
        "top_referrers",
    ] {
        property_partial[key] = json!({
            "status": "available",
            "data": null,
        });
    }
    property_partial["property_results"] = json!([{
        "event": "visit",
        "property": "ref",
        "result": {
            "status": "partial",
            "diagnostic": "insufficient_coverage",
            "data": null,
        }
    }]);
    let property_partial: types::BehavioralWindow =
        serde_json::from_value(property_partial).unwrap();
    assert_eq!(property_partial.validate(), Ok(()));

    // An unavailable window as the provider builds it — every core component
    // and every allowed property unavailable with the same diagnostic — is
    // coherent. The property components count toward the same rule.
    let mut provider_unavailable = serde_json::to_value(&window).unwrap();
    provider_unavailable["property_results"] = json!([{
        "event": "visit",
        "property": "ref",
        "result": {
            "status": "unavailable",
            "diagnostic": "authentication",
            "data": null,
        }
    }]);
    let provider_unavailable: types::BehavioralWindow =
        serde_json::from_value(provider_unavailable).unwrap();
    assert_eq!(provider_unavailable.validate(), Ok(()));

    // An unavailable property that carries data is incoherent as a component.
    let mut fabricated = serde_json::to_value(&provider_unavailable).unwrap();
    fabricated["property_results"][0]["result"] = json!({
        "status": "unavailable",
        "diagnostic": "authentication",
        "data": [],
    });
    let fabricated: types::BehavioralWindow = serde_json::from_value(fabricated).unwrap();
    assert_eq!(
        fabricated.validate(),
        Err(types::BehavioralWindowError::IncoherentComponent)
    );

    // An unavailable window refuses a property that reports available.
    let mut fabricated = serde_json::to_value(&provider_unavailable).unwrap();
    fabricated["property_results"][0]["result"] = json!({
        "status": "available",
        "data": [],
    });
    let fabricated: types::BehavioralWindow = serde_json::from_value(fabricated).unwrap();
    assert_eq!(
        fabricated.validate(),
        Err(types::BehavioralWindowError::IncoherentAvailability)
    );

    // An available window refuses any property that is partial or
    // unavailable, and accepts when every property is available.
    let mut all_available = serde_json::to_value(&window).unwrap();
    all_available["availability"] = json!("available");
    for key in [
        "core",
        "event_totals",
        "daily_visitors",
        "event_series",
        "top_entries",
        "top_referrers",
    ] {
        all_available[key] = json!({
            "status": "available",
            "data": null,
        });
    }
    all_available["property_results"] = json!([{
        "event": "visit",
        "property": "ref",
        "result": {"status": "available", "data": []}
    }]);
    let all_available: types::BehavioralWindow = serde_json::from_value(all_available).unwrap();
    assert_eq!(all_available.validate(), Ok(()));
    for bad in [
        json!({"status": "partial", "diagnostic": "insufficient_coverage", "data": null}),
        json!({"status": "unavailable", "diagnostic": "timeout", "data": null}),
    ] {
        let mut fabricated = serde_json::to_value(&all_available).unwrap();
        fabricated["property_results"][0]["result"] = bad;
        let fabricated: types::BehavioralWindow = serde_json::from_value(fabricated).unwrap();
        assert_eq!(
            fabricated.validate(),
            Err(types::BehavioralWindowError::IncoherentAvailability)
        );
    }

    // The provider never marks a property not-requested: a requested pair is
    // either available or unavailable. A not-requested property result is a
    // contradiction, whatever the window claims.
    let mut not_requested = serde_json::to_value(&all_available).unwrap();
    not_requested["property_results"][0]["result"] =
        json!({"status": "not_requested", "data": null});
    let not_requested: types::BehavioralWindow = serde_json::from_value(not_requested).unwrap();
    assert_eq!(
        not_requested.validate(),
        Err(types::BehavioralWindowError::IncoherentAvailability)
    );

    // A window that claims available while every component is unavailable is
    // equally rejected.
    let mut mislabelled = serde_json::to_value(&window).unwrap();
    mislabelled["availability"] = json!("available");
    let mislabelled: types::BehavioralWindow = serde_json::from_value(mislabelled).unwrap();
    assert_eq!(
        mislabelled.validate(),
        Err(types::BehavioralWindowError::IncoherentAvailability)
    );

    // Ranking and row types keep their bounded shapes.
    let row = types::EntryRankingRow {
        entry: types::EntryPage::Login,
        count: AggregateCell {
            key: "login".into(),
            count: Some(7),
            suppressed: false,
        },
        rank: 1,
    };
    assert_eq!(
        serde_json::to_value(&row).unwrap(),
        json!({
            "entry": "login",
            "count": {"key": "login", "count": 7, "suppressed": false},
            "rank": 1,
        })
    );
    assert_eq!(
        serde_json::to_value(types::ReferrerCategory::Other).unwrap(),
        json!("other")
    );
}
