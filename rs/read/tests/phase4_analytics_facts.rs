//! Phase 4 Task 18 — authoritative overview, activation, engagement and
//! retention tabs.
//!
//! The lane's leaf modules are path-included so this target compiles before the
//! integration owner publishes `analytics/mod.rs`. `types` is the CF4_03
//! contract double: Task 15 and Task 16 replace it with the real producers at
//! integration.

#[path = "support/analytics_contract_double.rs"]
mod types;

#[path = "../src/analytics/activation.rs"]
mod activation;
#[path = "../src/analytics/aggregates.rs"]
mod aggregates;
#[path = "../src/analytics/cohorts.rs"]
mod cohorts;
#[path = "../src/analytics/engagement.rs"]
mod engagement;
#[path = "../src/analytics/overview.rs"]
mod overview;

use std::collections::BTreeMap;

use activation::{activation, aggregate_ratio};
use aggregates::{
    ActivationFacts, ActivityFacts, DropOffFacts, EngagementFacts, LoginFacts, LoginRollupRow,
    OverviewFacts, ProvenanceRow, QualityTracker, RetentionFacts, RoleCounts, StatusFacts,
    date_window, dense_series, fact_coverage, median_hours, rate_pct, week_start,
};
use chrono::{DateTime, NaiveDate, TimeDelta, TimeZone, Utc};
use cohorts::{cohorts, retention};
use engagement::engagement;
use overview::{login_metric, overview};
use rust_decimal::Decimal;
/// The leaves reach the CF4_03 declarations as `super::types`; the test body
/// uses the same names directly.
use types::*;
use uuid::Uuid;

fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
}

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn query() -> AnalyticsQuery {
    AnalyticsQuery {
        start: utc(2026, 6, 1),
        end: utc(2026, 7, 1),
        previous: Some((utc(2026, 5, 2), utc(2026, 6, 1))),
        compare: true,
        audience: AnalyticsAudience::All,
        activity: AnalyticsActivity::Live,
        product_id: None,
        delivery_point_id: None,
        availability_window: None,
    }
}

fn observed() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 2, 12, 0, 0).unwrap()
}

fn suppressed_marker() -> MetricValue {
    MetricValue {
        value: None,
        previous: None,
        suppressed: true,
    }
}

// ---------------------------------------------------------------------------
// p4_t18_c01 — the suppression scalar
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c01() {
    // Zero is genuine; one and two are suppressed; three is published.
    let zero = suppress(Some(0), None);
    assert_eq!(zero.count, Some(0));
    assert!(!zero.suppressed);
    for basis in [1, 2] {
        let cell = suppress(Some(basis), None);
        assert_eq!(cell.count, None);
        assert!(cell.suppressed);
        // An explicit entity basis overrides the event-count fallback.
        let entity = suppress(Some(9), Some(basis));
        assert_eq!(entity.count, None, "entity basis {basis} must suppress");
        assert!(entity.suppressed);
    }
    let three = suppress(Some(3), None);
    assert_eq!(three.count, Some(3));
    assert!(!three.suppressed);
    // Missing is unavailable, never suppressed and never zero.
    let missing = suppress(None, None);
    assert_eq!(missing.count, None);
    assert!(!missing.suppressed);

    let mut quality = QualityTracker::default();
    let keyed = quality.cell("visitors", Some(2), None);
    assert_eq!(keyed.key, "visitors");
    assert_eq!(quality.suppressed_cell_count, 1);
    assert_eq!(quality.to_schema().suppressed_cell_count, 1);
}

// ---------------------------------------------------------------------------
// p4_t18_c02 — dense UTC series and half-open date bounds
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c02() {
    let mut values = BTreeMap::new();
    values.insert(day(2026, 6, 7), 1);
    let series = dense_series(utc(2026, 6, 1), utc(2026, 7, 1), &values);
    assert_eq!(series.len(), 30, "every UTC day in the period is present");
    let by_date: BTreeMap<NaiveDate, Option<u64>> = series
        .iter()
        .map(|point| (point.date, point.value))
        .collect();
    assert_eq!(by_date[&day(2026, 6, 7)], Some(1));
    assert_eq!(
        by_date[&day(2026, 6, 2)],
        Some(0),
        "dense zeros are not gaps"
    );

    // A midday start excludes that date; a midday end includes it.
    assert_eq!(
        date_window(
            Some(Utc.with_ymd_and_hms(2026, 6, 25, 12, 0, 0).unwrap()),
            Some(Utc.with_ymd_and_hms(2026, 6, 30, 12, 0, 0).unwrap())
        ),
        Some((day(2026, 6, 26), day(2026, 7, 1)))
    );
    assert_eq!(
        date_window(Some(utc(2026, 6, 1)), Some(utc(2026, 7, 1))),
        Some((day(2026, 6, 1), day(2026, 7, 1)))
    );
    // The comparison window is absent when compare is off.
    let mut plain = query();
    plain.previous = None;
    plain.compare = false;
    assert_eq!(
        date_window(plain.previous_start(), plain.previous_end()),
        None
    );

    // Ratios refuse a non-positive denominator and never render zero.
    assert_eq!(rate_pct(3, 0), None);
    assert_eq!(rate_pct(3, 7), Some(Decimal::new(4286, 2)));
    assert_eq!(median_hours(&[]), None);
    assert_eq!(
        median_hours(&[
            TimeDelta::hours(2),
            TimeDelta::hours(5),
            TimeDelta::hours(8)
        ]),
        Some(Decimal::new(50, 1))
    );
    assert_eq!(
        median_hours(&[TimeDelta::hours(2), TimeDelta::hours(5)]),
        Some(Decimal::new(35, 1))
    );
}

// ---------------------------------------------------------------------------
// p4_t18_c03 — provider failure never erases authoritative facts
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c03() {
    let facts = OverviewFacts {
        registered: RoleCounts {
            buyer_current: 4,
            buyer_previous: 3,
            supplier_current: 5,
            supplier_previous: 4,
        },
        activity: ActivityFacts {
            trading_current: 6,
            trading_previous: 4,
            retained_current: 4,
            retained_previous: 2,
            ..Default::default()
        },
        drop_off: DropOffFacts {
            never_logged_in: 2,
            ..Default::default()
        },
        ..Default::default()
    };

    let response = overview(&facts, None, None, &query(), observed());

    let stage = |key: LifecycleStageKey| {
        let found = response
            .lifecycle
            .iter()
            .find(|stage| stage.key == key)
            .expect("declared lifecycle stage");
        (found.count, found.previous, found.coverage)
    };
    assert_eq!(stage(LifecycleStageKey::Registered).0, Some(9));
    assert_eq!(stage(LifecycleStageKey::Trading).0, Some(6));
    assert_eq!(stage(LifecycleStageKey::Retained).1, Some(2));
    // The behavioural visitor stage is unavailable, not zero.
    assert_eq!(stage(LifecycleStageKey::Visitors).0, None);
    assert_eq!(
        stage(LifecycleStageKey::Visitors).2,
        AnalyticsSourceStatus::Unavailable
    );
    assert!(response.activity_trend.visitors.is_empty());
    // Structural facts are still reported.
    assert_eq!(response.kpis.live_orders.value, Some(0));
    assert_eq!(
        response.meta.coverage.behavioral.status,
        AnalyticsSourceStatus::Unavailable
    );
    assert_eq!(
        response.meta.coverage.behavioral.diagnostic,
        Some(AnalyticsDiagnostic::Upstream)
    );
    // Degraded collection is flagged; the never-logged-in rule needs > 0.
    let rules: Vec<_> = response
        .needs_attention
        .iter()
        .map(|item| item.rule)
        .collect();
    assert!(rules.contains(&NeedsAttentionRule::DegradedAnalyticsCollection));
    assert!(rules.contains(&NeedsAttentionRule::ApprovedMembersNeverLoggedIn));
    assert!(!rules.contains(&NeedsAttentionRule::SignupSubmissionDrop));

    // A small organization cell is suppressed while order counts stay visible.
    let mut with_orders = OverviewFacts::default();
    with_orders.by_provenance.push(ProvenanceRow {
        provenance: "live".into(),
        bids_current: 4,
        asks_current: 1,
        bid_orgs_current: 2,
        ask_orgs_current: 3,
        ..Default::default()
    });
    let response = overview(&with_orders, None, None, &query(), observed());
    assert_eq!(
        response.marketplace_balance.buyer_organizations,
        suppressed_marker()
    );
    assert_eq!(
        response.marketplace_balance.supplier_organizations.value,
        Some(3)
    );
    assert_eq!(response.marketplace_balance.bid_orders.value, Some(4));
    assert_eq!(response.marketplace_balance.ask_orders.value, Some(1));
    assert!(
        response
            .needs_attention
            .iter()
            .any(|item| item.rule == NeedsAttentionRule::OneSidedLiveMarket)
    );
}

// ---------------------------------------------------------------------------
// p4_t18_c04 — history facts are never inferred from the current snapshot
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c04() {
    // Users exist and are approved, but neither history table has facts yet.
    let facts = OverviewFacts::default();
    let response = overview(&facts, None, None, &query(), observed());
    assert_eq!(response.kpis.active_members.value, None);
    assert_eq!(response.kpis.active_members.previous, None);
    assert_eq!(response.kpis.qualified_organizations.value, None);
    assert_eq!(response.kpis.qualified_organizations.previous, None);
    assert_eq!(
        response.meta.coverage.login_history.status,
        AnalyticsSourceStatus::Unavailable
    );
    assert_eq!(
        response.meta.coverage.login_history.diagnostic,
        Some(AnalyticsDiagnostic::InsufficientCoverage)
    );
    assert!(response.activity_trend.active_members.is_empty());

    // With facts present, a covered window publishes values and the
    // comparison window that predates coverage stays null.
    let facts = OverviewFacts {
        login: LoginFacts {
            coverage_start: Some(day(2026, 5, 20)),
            active_current: 4,
            active_previous: 3,
            ..Default::default()
        },
        status: StatusFacts {
            qualified_end: Some(3),
            qualified_previous_end: Some(2),
            coverage_start: Some(utc(2026, 3, 18)),
            approved_current: 1,
            approved_previous: 0,
        },
        ..Default::default()
    };
    let response = overview(&facts, None, None, &query(), observed());
    assert_eq!(response.kpis.active_members.value, Some(4));
    assert_eq!(response.kpis.active_members.previous, None);
    assert_eq!(response.kpis.qualified_organizations.value, Some(3));
    assert_eq!(response.kpis.qualified_organizations.previous, Some(2));
    // Dense member trend starts at coverage and keeps covered zeros visible.
    assert_eq!(response.activity_trend.active_members.len(), 30);
    assert_eq!(
        response.activity_trend.active_members[0].date,
        day(2026, 6, 1)
    );
    assert_eq!(response.activity_trend.active_members[0].value, Some(0));

    // A window that starts before coverage is partial, and its member count is
    // unavailable rather than truncated.
    let mut partial = query();
    partial.start = utc(2026, 5, 1);
    partial.previous = None;
    partial.compare = false;
    let coverage = fact_coverage(Some(utc(2026, 5, 20)), &partial, observed());
    assert_eq!(coverage.status, AnalyticsSourceStatus::Partial);
    assert_eq!(login_metric(&facts.login, &partial).value, None);

    // The comparison window itself may start before coverage: 2 May here.
    let comparison_partial = fact_coverage(Some(utc(2026, 5, 20)), &query(), observed());
    assert_eq!(comparison_partial.status, AnalyticsSourceStatus::Partial);
    assert_eq!(
        comparison_partial.diagnostic,
        Some(AnalyticsDiagnostic::InsufficientCoverage)
    );
    let comparison_full = fact_coverage(Some(utc(2026, 4, 1)), &query(), observed());
    assert_eq!(comparison_full.status, AnalyticsSourceStatus::Available);
}

// ---------------------------------------------------------------------------
// p4_t18_c05 — activation stages, suppression and behavioural ratios
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c05() {
    let window =
        BehavioralWindow::empty(utc(2026, 6, 1), utc(2026, 7, 1), observed()).with_events(&[
            ("signup_started", 10),
            ("signup_role_selected", 8),
            ("signup_submitted", 0),
            ("signup_organization_required", 2),
        ]);

    let facts = ActivationFacts {
        registered: RoleCounts {
            buyer_current: 2,
            buyer_previous: 1,
            supplier_current: 0,
            supplier_previous: 1,
        },
        drop_off: DropOffFacts {
            rejected: 1,
            unverified: 0,
            pending_approval: 1,
            organization_incomplete: 0,
            never_logged_in: 1,
        },
        status: StatusFacts {
            approved_current: 1,
            coverage_start: Some(utc(2026, 3, 18)),
            ..Default::default()
        },
        // One organization reached its first live order in the period.
        first_live_orders: vec![(
            Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
            utc(2026, 5, 9),
        )],
        // One member's first login falls in the period.
        logins: vec![LoginRollupRow {
            first_login_at: Utc.with_ymd_and_hms(2026, 6, 30, 9, 0, 0).unwrap(),
            first_login_date: day(2026, 6, 30),
            in_current: true,
            in_previous: false,
            registered_at: utc(2026, 6, 10),
        }],
    };

    let response = activation(&facts, Some(&window), &query(), observed());
    let stage = |key: &str| {
        response
            .journey
            .iter()
            .find(|stage| stage.key == key)
            .expect("declared journey stage")
    };
    assert_eq!(stage("signup_started").total.count, Some(10));
    assert_eq!(stage("signup_started").source, JourneySource::Behavioral);
    assert_eq!(stage("signup_submitted").total.count, Some(0));
    // One or two events suppress; the key stays.
    assert!(stage("signup_organization_required").total.suppressed);
    assert_eq!(stage("signup_organization_required").total.count, None);
    assert_eq!(stage("registered").total.count, Some(2));
    assert!(!stage("registered").total.suppressed);
    assert!(stage("registered").buyer.as_ref().unwrap().suppressed);
    assert_eq!(
        stage("registered").supplier.as_ref().unwrap().count,
        Some(0)
    );
    assert!(stage("approved").total.suppressed);
    assert!(stage("first_login").total.suppressed);
    assert!(stage("first_live_order").total.suppressed);

    let drop_off: BTreeMap<_, _> = response
        .drop_off
        .iter()
        .map(|cell| (cell.key.as_str(), (cell.count, cell.suppressed)))
        .collect();
    assert_eq!(drop_off["rejected"], (None, true));
    assert_eq!(drop_off["unverified"], (Some(0), false));
    assert_eq!(drop_off["pending_approval"], (None, true));
    assert_eq!(drop_off["organization_incomplete"], (Some(0), false));
    assert_eq!(drop_off["never_logged_in"], (None, true));

    // A suppressed sample publishes neither a median nor a sample size.
    assert!(response.time_to_first_login.suppressed);
    assert_eq!(response.time_to_first_login.median_hours, None);
    assert_eq!(response.time_to_first_login.sample_size, None);
    assert!(response.time_to_first_live_order.suppressed);
    assert_eq!(response.time_to_first_live_order.median_hours, None);
    assert!(response.time_to_first_live_order.buckets.is_empty());
    assert!(response.meta.data_quality.suppressed_cell_count >= 3);

    // Ratios are aggregate-event only and keep their components.
    let ratio = &response.ratios[0];
    assert_eq!(ratio.key, "signup_started_to_submitted");
    assert_eq!(ratio.kind, ConversionRatioKind::AggregateEvent);
    assert_eq!(ratio.ratio.numerator, Some(0));
    assert_eq!(ratio.ratio.denominator, Some(10));
    assert_eq!(ratio.ratio.rate_pct, Some(Decimal::new(0, 2)));

    // A missing denominator is unavailable, not a zero rate.
    let missing = aggregate_ratio("k", Some(3), None);
    assert_eq!(missing.ratio.rate_pct, None);
    let zero_denominator = aggregate_ratio("k", Some(3), Some(0));
    assert_eq!(zero_denominator.ratio.rate_pct, None);
    assert_eq!(zero_denominator.ratio.denominator, Some(0));
    assert!(!zero_denominator.ratio.suppressed);

    // A one- or two-event component would publish a suppressed cell through a
    // derived rate, so the whole ratio is suppressed and carries no numbers.
    for (numerator, denominator) in [(Some(1u64), Some(50u64)), (Some(40), Some(2))] {
        let ratio = aggregate_ratio("k", numerator, denominator);
        assert!(ratio.ratio.suppressed, "{numerator:?} / {denominator:?}");
        assert_eq!(ratio.ratio.numerator, None);
        assert_eq!(ratio.ratio.denominator, None);
        assert_eq!(ratio.ratio.rate_pct, None);
    }
}

// ---------------------------------------------------------------------------
// p4_t18_c06 — engagement coverage gates and closed destination domain
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c06() {
    let window = BehavioralWindow::empty(utc(2026, 6, 1), utc(2026, 7, 1), observed())
        .with_events(&[
            ("platform_navigation", 30),
            ("market_slice_selected", 1),
            ("listing_opened", 12),
            ("order_form_opened", 6),
            ("order_form_submitted", 3),
            ("tutorial_started", 4),
            ("tutorial_completed", 1),
            ("estimator_opened", 9),
            ("estimator_completed", 9),
        ])
        .with_property(
            "platform_navigation",
            "destination",
            &[("home", 12), ("not-a-destination", 5), ("admin", 4)],
        )
        .with_property(
            "tutorial_step_completed",
            "step",
            &[("buyer_1", 5), ("buyer_2", 1)],
        )
        .with_property(
            "tutorial_step_skipped",
            "step",
            &[("buyer_1", 2), ("buyer_3", 7)],
        );

    let facts = EngagementFacts {
        login: LoginFacts {
            coverage_start: Some(day(2026, 5, 20)),
            dau: 4,
            wau: 9,
            mau: 11,
            daily: BTreeMap::from([(day(2026, 6, 15), 4)]),
            ..Default::default()
        },
    };

    let response = engagement(&facts, Some(&window), &query(), observed());
    // The 30-day rolling window is covered by the 20 May first fact.
    assert_eq!(response.kpis.dau.value, Some(4));
    assert_eq!(response.kpis.wau.value, Some(9));
    assert_eq!(response.kpis.mau.value, Some(11));
    assert_eq!(response.kpis.stickiness_pct, Some(Decimal::new(3636, 2)));

    // Adoption is sorted by value and keeps suppressed rows null.
    let families: Vec<_> = response
        .feature_adoption
        .iter()
        .map(|row| (row.family.as_str(), row.events, row.suppressed))
        .collect();
    assert_eq!(families[0], ("platform_navigation", Some(30), false));
    assert!(families.contains(&("market_slice_selected", None, true)));
    assert!(families.contains(&("tutorial_completed", None, true)));
    assert_eq!(
        families.last().unwrap().1,
        None,
        "suppressed rows sort last"
    );

    let ratios: BTreeMap<_, _> = response
        .workflow_ratios
        .iter()
        .map(|ratio| (ratio.key.as_str(), ratio.ratio.clone()))
        .collect();
    assert_eq!(
        ratios["listing_to_order_form"].rate_pct,
        Some(Decimal::new(5000, 2))
    );
    assert_eq!(
        ratios["order_form_to_submit"].rate_pct,
        Some(Decimal::new(5000, 2))
    );
    assert_eq!(
        ratios["tutorial_start_to_complete"].rate_pct,
        Some(Decimal::new(2500, 2))
    );

    // The closed destination domain drops the unknown value before output.
    let destinations: Vec<_> = response
        .navigation_destinations
        .iter()
        .map(|row| (row.destination, row.total.count, row.buyer.is_none()))
        .collect();
    assert_eq!(
        destinations,
        vec![
            (NavigationDestination::Home, Some(12), true),
            (NavigationDestination::Admin, Some(4), true),
        ]
    );

    // Tutorial rows merge completed and skipped per step and sort by step key.
    let steps: Vec<_> = response
        .tutorial_steps
        .iter()
        .map(|row| (row.step.as_str(), row.completed.count, row.skipped.count))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("buyer_1", Some(5), None),
            ("buyer_2", None, None),
            ("buyer_3", Some(7), None)
        ]
    );

    // A short history leaves the long windows unavailable.
    let mut shallow = facts.clone();
    shallow.login.coverage_start = Some(day(2026, 6, 25));
    let response = engagement(&shallow, None, &query(), observed());
    assert_eq!(response.kpis.dau.value, Some(4));
    assert_eq!(response.kpis.wau.value, None);
    assert_eq!(response.kpis.mau.value, None);
    assert_eq!(response.kpis.stickiness_pct, None);
    assert!(
        response
            .feature_adoption
            .iter()
            .all(|row| row.events.is_none())
    );
    assert!(response.navigation_destinations.is_empty());
}

// ---------------------------------------------------------------------------
// p4_t18_c07 — Monday-anchored, suppressed cohorts
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c07() {
    let user_a = Uuid::from_u128(0xa);
    let user_b = Uuid::from_u128(0xb);
    let user_c = Uuid::from_u128(0xc);
    let user_d = Uuid::from_u128(0xd);
    // Wednesday 20 May and Saturday 20 June belong to the 18 May and 15 June
    // Monday-anchored weeks.
    let days = vec![
        (user_a, day(2026, 5, 20)),
        (user_a, day(2026, 6, 20)),
        (user_b, day(2026, 5, 25)),
        (user_b, day(2026, 6, 15)),
        (user_c, day(2026, 5, 28)),
        (user_c, day(2026, 6, 25)),
        (user_d, day(2026, 6, 30)),
    ];
    let mut quality = QualityTracker::default();
    let rows = cohorts(&days, &mut quality);
    assert_eq!(rows.len(), 2, "two first-activity weeks");
    assert_eq!(rows[0].cohort_start, day(2026, 5, 18));
    assert_eq!(rows[0].size.count, Some(3));
    assert!(!rows[0].size.suppressed);
    let offsets: Vec<_> = rows[0]
        .cells
        .iter()
        .map(|cell| (cell.offset, cell.cell.count, cell.pct))
        .collect();
    assert_eq!(offsets[0], (0, Some(3), Some(Decimal::new(10000, 2))));
    // Second-week retention is a two-member cell: suppressed, no percentage.
    assert_eq!(offsets[1].0, 4);
    assert_eq!(offsets[1].1, None);
    assert!(rows[0].cells[1].cell.suppressed);
    assert_eq!(offsets[1].2, None);
    // The later cohort has one member and stays suppressed.
    assert_eq!(rows[1].cohort_start, day(2026, 6, 29));
    assert!(rows[1].size.suppressed);
    assert_eq!(rows[1].size.count, None);
    assert!(rows[1].cells.iter().all(|cell| cell.pct.is_none()));
    assert!(quality.suppressed_cell_count >= 4);

    // A Sunday start still anchors to the preceding Monday (ISO weeks).
    assert_eq!(week_start(day(2026, 5, 24)), day(2026, 5, 18));
    assert_eq!(week_start(day(2026, 5, 18)), day(2026, 5, 18));
}

// ---------------------------------------------------------------------------
// p4_t18_c08 — returning members need a covered comparison period
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c08() {
    let rollup = vec![
        LoginRollupRow {
            first_login_at: utc(2026, 5, 20),
            first_login_date: day(2026, 5, 20),
            in_current: true,
            in_previous: true,
            registered_at: utc(2026, 3, 1),
        },
        LoginRollupRow {
            first_login_at: utc(2026, 5, 25),
            first_login_date: day(2026, 5, 25),
            in_current: false,
            in_previous: true,
            registered_at: utc(2026, 3, 2),
        },
    ];
    let mut facts = RetentionFacts {
        rollup,
        ..Default::default()
    };
    let mut response = retention(&facts, None, &query(), observed());
    assert_eq!(response.kpis.returning_members.value, None);
    assert_eq!(
        response.meta.coverage.login_history.status,
        AnalyticsSourceStatus::Partial
    );

    // Once the facts cover the comparison period the count is published.
    facts.rollup[0].first_login_date = day(2026, 4, 1);
    facts.rollup[1].first_login_date = day(2026, 4, 2);
    response = retention(&facts, None, &query(), observed());
    assert_eq!(response.kpis.returning_members.value, Some(1));
    assert_eq!(
        response.meta.coverage.login_history.status,
        AnalyticsSourceStatus::Available
    );

    // Without the comparison request the gate does not apply.
    let mut plain = query();
    plain.previous = None;
    plain.compare = false;
    facts.rollup[0].first_login_date = day(2026, 5, 20);
    facts.rollup[1].first_login_date = day(2026, 5, 25);
    let response = retention(&facts, None, &plain, observed());
    assert_eq!(response.kpis.returning_members.value, Some(1));
}

// ---------------------------------------------------------------------------
// p4_t18_c09 — behavioural small cells across series, totals and rankings
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c09() {
    let mut window = BehavioralWindow::empty(utc(2026, 6, 1), utc(2026, 7, 1), observed())
        .with_events(&[
            ("signup_started", 1),
            ("login_failed", 2),
            ("trade_created", 9),
        ])
        .with_daily_visitors(&[
            (day(2026, 6, 2), 0),
            (day(2026, 6, 3), 2),
            (day(2026, 6, 4), 5),
        ]);
    window.core = ProviderComponent::available(CoreMetrics {
        visitors: Some(1),
        visits: Some(5),
        pageviews: Some(9),
        total_time_seconds: Some(250),
        average_session_duration_seconds: Some(Decimal::new(50, 0)),
    });

    let facts = OverviewFacts::default();
    let response = overview(&facts, Some(&window), None, &query(), observed());

    let visitors = &response.activity_trend.visitors;
    assert_eq!(visitors.len(), 30);
    let by_date: BTreeMap<_, _> = visitors.iter().map(|p| (p.date, p.value)).collect();
    assert_eq!(
        by_date[&day(2026, 6, 2)],
        Some(0),
        "a genuine zero stays visible"
    );
    assert_eq!(
        by_date[&day(2026, 6, 3)],
        None,
        "two visitors are suppressed"
    );
    assert_eq!(by_date[&day(2026, 6, 4)], Some(5));
    // The headline visitor metric is suppressed at one visitor.
    let stage = response
        .lifecycle
        .iter()
        .find(|stage| stage.key == LifecycleStageKey::Visitors)
        .unwrap();
    assert_eq!(stage.count, None);
    assert_eq!(stage.previous, None);
    assert_eq!(stage.coverage, AnalyticsSourceStatus::Available);
    // A single start still raises the operational rule, but its derived
    // single-event count is not published.
    let drop_rule = response
        .needs_attention
        .iter()
        .find(|item| item.rule == NeedsAttentionRule::SignupSubmissionDrop)
        .expect("the rule still fires");
    assert_eq!(drop_rule.count, None);

    // An unavailable component yields no series and no totals.
    let mut window = BehavioralWindow::empty(utc(2026, 6, 1), utc(2026, 7, 1), observed());
    window.event_totals = ProviderComponent::unavailable(AnalyticsDiagnostic::Timeout);
    window.daily_visitors = ProviderComponent::unavailable(AnalyticsDiagnostic::Timeout);
    window.core = ProviderComponent::unavailable(AnalyticsDiagnostic::Timeout);
    let response = overview(&facts, Some(&window), None, &query(), observed());
    assert!(response.activity_trend.visitors.is_empty());
    let stage = response
        .lifecycle
        .iter()
        .find(|stage| stage.key == LifecycleStageKey::Visitors)
        .unwrap();
    assert_eq!(stage.count, None);
    assert_eq!(
        response.meta.coverage.behavioral.diagnostic,
        Some(AnalyticsDiagnostic::Timeout)
    );
}

// ---------------------------------------------------------------------------
// p4_t18_c10 — grouped loaders with a bounded statement shape
// ---------------------------------------------------------------------------

#[test]
fn p4_t18_c10() {
    let source = include_str!("../src/analytics/aggregates.rs");
    // Every loop in the fact layer shapes rows that were already fetched. A
    // loop that issued SQL would be the per-user query loop this forbids.
    let mut loops = 0_usize;
    let mut index = 0_usize;
    while index < source.len() {
        let rest = &source[index..];
        let next = ["for ", "while ", "loop {"]
            .iter()
            .filter_map(|needle| rest.find(needle).map(|at| (at, needle.len())))
            .min_by_key(|(at, _)| *at);
        let Some((at, length)) = next else {
            break;
        };
        let keyword_at = index + at;
        let boundary = !source[..keyword_at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if !boundary {
            index = keyword_at + length;
            continue;
        }
        let after = keyword_at + length;
        let Some(open) = source[after..].find('{').map(|offset| after + offset) else {
            index = after;
            continue;
        };
        let mut depth = 0_i32;
        let mut end = open;
        for (offset, character) in source[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        loops += 1;
        let body = &source[open..=end];
        assert!(
            !body.contains("sqlx::query(") && !body.contains(".fetch_"),
            "a fact-layer loop issues SQL: {body}"
        );
        index = end + 1;
    }
    assert!(loops >= 3, "the shaping loops under test are still present");
    // A fixed statement budget for the whole fact layer.
    assert!(
        source.matches("sqlx::query(").count() <= 16,
        "the fact layer must keep a small fixed statement set"
    );
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection};

    /// A disposable schema with production table shapes, a distinct restricted
    /// runtime role and the fixture rows. Never a live database.
    struct Fixture {
        app: PgConnection,
        admin: PgConnection,
        schema: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL required");
            let app_url = std::env::var("RUST_TEST_DATABASE_URL")
                .expect("explicit disposable runtime URL required");
            let admin_parsed = url::Url::parse(&admin_url).unwrap();
            let app_parsed = url::Url::parse(&app_url).unwrap();
            assert_eq!(admin_parsed.host_str(), Some("127.0.0.1"));
            assert_eq!(admin_parsed.host_str(), app_parsed.host_str());
            assert_eq!(admin_parsed.port(), app_parsed.port());
            assert_eq!(admin_parsed.path(), app_parsed.path());
            assert!(
                admin_parsed.path().ends_with("_test"),
                "only an explicitly disposable database may carry this fixture"
            );
            assert_eq!(app_parsed.username(), "rust_app");
            assert_ne!(admin_parsed.username(), app_parsed.username());

            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            let schema = format!("p4_t18_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(
                include_str!("fixtures/analytics_facts.sql").to_owned(),
            ))
            .execute(&mut admin)
            .await
            .unwrap();
            // The runtime role reads facts and never writes them.
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "GRANT USAGE ON SCHEMA {schema} TO rust_app;
                 GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            let path = format!("SET search_path TO {schema}");
            let mut app = PgConnection::connect(&app_url).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                .execute(&mut app)
                .await
                .unwrap();
            // The fixture rows are created by the migrator role, so the runtime
            // role must be able to read them through the granted schema.
            Self { app, admin, schema }
        }

        async fn finish(mut self) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn p4_t18_c11() {
        let mut f = Fixture::new().await;
        let facts = aggregates::load_overview_facts(&mut f.app, &query())
            .await
            .unwrap();

        assert_eq!(facts.registered.buyer_current, 1);
        assert_eq!(facts.registered.buyer_previous, 1);
        assert_eq!(facts.registered.supplier_current, 0);
        assert_eq!(facts.registered.supplier_previous, 1);
        // Demo and admin accounts never enter the member population.
        assert_eq!(facts.drop_off.rejected, 1);
        assert_eq!(facts.drop_off.unverified, 0);
        assert_eq!(facts.drop_off.pending_approval, 1);
        assert_eq!(facts.drop_off.organization_incomplete, 0);
        assert_eq!(facts.drop_off.never_logged_in, 1);

        let live = facts.orders("live").unwrap();
        assert_eq!(live.orders_current, 6);
        assert_eq!(live.orders_previous, 1);
        assert_eq!(live.bids_current, 3);
        assert_eq!(live.asks_current, 3);
        assert_eq!(live.orgs_current, 3);
        assert_eq!(live.orgs_previous, 1);
        assert_eq!(live.bid_orgs_current, 2);
        assert_eq!(live.ask_orgs_current, 2);
        assert_eq!(live.executed_current, 5);
        assert_eq!(live.executed_previous, 1);
        assert_eq!(live.still_open_current, 4);
        // Sources are reported separately and never summed.
        assert_eq!(facts.orders("demo").unwrap().orders_current, 1);
        assert_eq!(facts.orders("unknown").unwrap().orders_current, 1);

        let trades = facts.trades("live").unwrap();
        assert_eq!(trades.confirmed_current_strict, 3);
        assert_eq!(trades.legacy_current, 1);
        assert_eq!(trades.confirmed_previous_strict, 1);
        assert_eq!(trades.volume_current_strict, Decimal::new(16500, 2));
        assert_eq!(trades.volume_current_legacy, Decimal::new(1000, 2));
        assert_eq!(trades.volume_previous, Decimal::new(3000, 2));
        assert_eq!(trades.gmv_current, Decimal::new(8000000, 2));
        assert_eq!(trades.missing_paid_at_current, 1);
        assert_eq!(facts.trades("demo").unwrap().confirmed_current_strict, 1);
        assert_eq!(facts.trades("unknown").unwrap().confirmed_current_strict, 1);

        assert_eq!(facts.login.coverage_start, Some(day(2026, 5, 20)));
        assert_eq!(facts.login.active_current, 4);
        assert_eq!(facts.login.active_previous, 3);
        assert_eq!(facts.login.dau, 1);
        assert_eq!(facts.login.wau, 2);
        assert_eq!(facts.login.mau, 4);
        assert_eq!(facts.login.daily.get(&day(2026, 6, 15)), Some(&1));
        assert_eq!(facts.status.qualified_end, Some(3));
        assert_eq!(facts.status.qualified_previous_end, Some(3));
        assert_eq!(facts.status.approved_current, 1);
        assert_eq!(facts.status.coverage_start, Some(utc(2026, 3, 18)));

        assert_eq!(facts.daily_orders.get(&day(2026, 6, 2)), Some(&1));
        assert_eq!(facts.daily_trades.get(&day(2026, 6, 9)), Some(&1));

        // Composition from those facts.
        let response = overview(&facts, None, None, &query(), observed());
        assert_eq!(response.kpis.live_orders.value, Some(6));
        assert_eq!(response.kpis.live_orders.previous, Some(1));
        assert_eq!(response.kpis.confirmed_trades.value, Some(4));
        assert_eq!(response.kpis.active_members.value, Some(4));
        assert_eq!(response.kpis.active_members.previous, None);
        assert_eq!(
            response.meta.data_quality.legacy_timestamp_fallback_count,
            1
        );
        assert_eq!(response.meta.data_quality.missing_paid_at_count, 1);
        assert_eq!(response.activity_trend.orders.len(), 30);
        assert_eq!(
            response
                .activity_trend
                .orders
                .iter()
                .map(|point| point.value.unwrap_or(0))
                .sum::<u64>(),
            6
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t18_c12() {
        let mut f = Fixture::new().await;
        let facts = aggregates::load_activation_facts(&mut f.app, &query())
            .await
            .unwrap();
        // Two recognized organizations reached their first live order in the
        // period; the third's first order predates it.
        let in_period: Vec<_> = facts
            .first_live_orders
            .iter()
            .filter(|(first, _)| *first >= query().start && *first < query().end)
            .collect();
        assert_eq!(in_period.len(), 2);
        assert_eq!(facts.logins.len(), 4);
        // Demo and admin logins are excluded from the member rollup.
        assert!(
            facts
                .logins
                .iter()
                .all(|row| row.first_login_date >= day(2026, 5, 20))
        );
        assert_eq!(facts.status.approved_current, 1);

        let response = activation(&facts, None, &query(), observed());
        let stage = |key: &str| {
            response
                .journey
                .iter()
                .find(|stage| stage.key == key)
                .expect("declared journey stage")
        };
        assert_eq!(stage("registered").total.count, Some(1));
        assert!(stage("registered").buyer.as_ref().unwrap().suppressed);
        assert_eq!(
            stage("registered").supplier.as_ref().unwrap().count,
            Some(0)
        );
        assert!(stage("approved").total.suppressed);
        assert!(stage("first_login").total.suppressed);
        assert!(stage("first_live_order").total.suppressed);
        assert!(response.meta.data_quality.cohort_complete);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t18_c13() {
        let mut f = Fixture::new().await;
        let engagement_facts = aggregates::load_engagement_facts(&mut f.app, &query())
            .await
            .unwrap();
        let response = engagement(&engagement_facts, None, &query(), observed());
        assert_eq!(response.kpis.dau.value, Some(1));
        assert_eq!(response.kpis.wau.value, Some(2));
        assert_eq!(response.kpis.mau.value, Some(4));
        assert_eq!(response.kpis.stickiness_pct, Some(Decimal::new(2500, 2)));
        // A covered zero is visible, an uncovered day is not published.
        assert_eq!(response.active_members_trend.len(), 30);
        assert_eq!(response.active_members_trend[0].value, Some(0));
        assert_eq!(response.active_members_trend[14].value, Some(1));

        // A stored-REAL trade with a demo identity party is demo, never live.
        // The demo seller is active in both periods through trades alone, so a
        // missing identity predicate would inflate the retained count.
        sqlx::query(
            "INSERT INTO trades (id, buyer_id, seller_id, buyer_provenance, seller_provenance,
             status, created_at, confirmed_at, quantity_mt)
             VALUES ('00000000-0000-4000-8000-0000000000aa',
                     '00000000-0000-4000-8000-000000000001',
                     'd1e43e55-3fb0-4b5e-9f0b-93aa10050222', 'REAL', 'REAL',
                     'CONFIRMED', '2026-05-20T10:00:00Z', '2026-05-20T11:00:00Z', 8.00),
                    ('00000000-0000-4000-8000-0000000000ab',
                     '00000000-0000-4000-8000-000000000001',
                     'd1e43e55-3fb0-4b5e-9f0b-93aa10050222', 'REAL', 'REAL',
                     'CONFIRMED', '2026-06-16T10:00:00Z', '2026-06-16T11:00:00Z', 8.00)",
        )
        .execute(&mut f.admin)
        .await
        .unwrap();
        let retention_facts = aggregates::load_retention_facts(&mut f.app, &query())
            .await
            .unwrap();
        assert_eq!(retention_facts.activity.retained_current, 3);
        assert_eq!(retention_facts.activity.retained_previous, 0);
        assert_eq!(retention_facts.activity.reactivated_current, 0);
        assert_eq!(retention_facts.activity.active_days_1, 0);
        assert_eq!(retention_facts.activity.active_days_2, 0);
        assert_eq!(retention_facts.activity.active_days_3_plus, 3);
        assert_eq!(retention_facts.drop_off.never_logged_in, 1);

        let response = retention(&retention_facts, None, &query(), observed());
        assert_eq!(response.kpis.retained_organizations.value, Some(3));
        assert_eq!(response.kpis.retained_organizations.previous, Some(0));
        assert_eq!(response.kpis.reactivated_organizations.value, Some(0));
        assert_eq!(response.kpis.dormant_approved_members.value, Some(1));
        // Returning members need the comparison window covered: 2 May is
        // before the 20 May first fact, so the count is unavailable.
        assert_eq!(response.kpis.returning_members.value, None);
        let repeat: BTreeMap<_, _> = response
            .repeat_participation
            .iter()
            .map(|cell| (cell.key.as_str(), cell.count))
            .collect();
        assert_eq!(repeat["1"], Some(0));
        assert_eq!(repeat["2"], Some(0));
        assert_eq!(repeat["3_plus"], Some(3));
        // Member cohorts are Monday-anchored and start at the first fact week.
        let cohort_starts: Vec<_> = response
            .member_cohorts
            .iter()
            .map(|row| row.cohort_start)
            .collect();
        assert_eq!(
            cohort_starts,
            vec![day(2026, 5, 18), day(2026, 5, 25), day(2026, 6, 29)]
        );
        assert_eq!(response.member_cohorts[0].size.count, Some(3));
        assert!(!response.member_cohorts[0].size.suppressed);
        assert!(response.member_cohorts[1].size.suppressed);
        assert!(response.member_cohorts[2].size.suppressed);
        // Organization cohorts ride order/trade activity, not logins.
        assert_eq!(response.organization_cohorts.len(), 2);
        assert!(
            response
                .organization_cohorts
                .iter()
                .all(|row| row.size.suppressed)
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t18_c14() {
        // A wider member population must be aggregated by the same grouped
        // statement, with exact totals and no per-user round trips.
        let mut f = Fixture::new().await;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, status, organization_id, last_login, created_at, email_verified)
             SELECT md5(i::text)::uuid, 'bulk' || i || '@fixture.test', 'x',
                    CASE WHEN i % 2 = 0 THEN 'BUYER' ELSE 'SUPPLIER' END,
                    'APPROVED', '00000000-0000-4000-8000-000000000001',
                    NULL, '2026-06-05T00:00:00Z', true
             FROM generate_series(1, 300) i",
        )
        .execute(&mut f.admin)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_login_days (id, activity_date, user_id, organization_id, role, login_count, first_login_at, last_login_at)
             SELECT md5('day' || i::text)::uuid, DATE '2026-06-10', md5(i::text)::uuid,
                    '00000000-0000-4000-8000-000000000001',
                    CASE WHEN i % 2 = 0 THEN 'BUYER' ELSE 'SUPPLIER' END,
                    1, '2026-06-10T00:00:00Z', '2026-06-10T00:00:00Z'
             FROM generate_series(1, 300) i",
        )
        .execute(&mut f.admin)
        .await
        .unwrap();
        let facts = aggregates::load_overview_facts(&mut f.app, &query())
            .await
            .unwrap();
        assert_eq!(facts.registered.buyer_current, 151);
        assert_eq!(facts.registered.supplier_current, 150);
        // Every bulk member is counted once, on one grouped statement.
        assert_eq!(facts.drop_off.never_logged_in, 301);
        assert_eq!(facts.login.active_current, 304);
        assert_eq!(facts.login.daily.get(&day(2026, 6, 10)), Some(&300));
        let response = overview(&facts, None, None, &query(), observed());
        assert_eq!(response.kpis.active_members.value, Some(304));
        f.finish().await;
    }
}
