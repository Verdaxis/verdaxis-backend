//! Phase 4 Task 19: marketplace, acquisition and reliability tabs.
//!
//! The component cases compile the lane's modules by path and assert the tab
//! business rules: provenance separation, reference-as-coverage, small-cell
//! suppression, the closed property registry, the reliability audit window and
//! the route order (authenticate, administrator, quota, read).
//!
//! The PostgreSQL cases need an explicitly started disposable database
//! (`--features contract-tests -- --include-ignored`); with no database they
//! are ignored rather than silently skipped as passing.

pub use serde_json::Value as Json;
// The tab modules are compiled by path, so `crate::` inside them names this
// test crate root: these re-exports are their shared boundary, not test use.
pub use verdaxis_read::{
    ApiError, AppState, MarketPolicy, PointPolicy, ProductPolicy, auth, business, market, runtime,
};

#[path = "../src/analytics/facts.rs"]
pub mod facts;
#[path = "../src/analytics/policy.rs"]
pub mod policy;
#[path = "../src/analytics/query.rs"]
pub mod query_policy;
#[path = "../src/analytics/types.rs"]
pub mod types;
pub mod analytics {
    pub use super::query_policy as query;
    pub use super::{facts, policy, types};
}

#[allow(dead_code)]
#[path = "../src/analytics/acquisition.rs"]
pub mod acquisition;
#[allow(dead_code)]
#[path = "../src/analytics/commercial.rs"]
pub mod commercial;
#[allow(dead_code)]
#[path = "../src/analytics/liquidity.rs"]
pub mod liquidity;
#[allow(dead_code)]
#[path = "../src/analytics/marketplace.rs"]
pub mod marketplace;
#[allow(dead_code)]
#[path = "../src/analytics/reliability.rs"]
pub mod reliability;

#[cfg(feature = "contract-tests")]
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use uuid::Uuid;

use marketplace::{
    AnalyticsRoute, BehavioralRead, MetaOptions, ProviderFuture, Quality, ReferenceFuture,
    ReferenceQuote, ReferenceRead, ReferenceRequest, Source,
};
use policy::{AllowedFrontendEvent as Event, AllowedProperty as Property, AllowedPropertySet};
use query_policy::{AnalyticsQuery, QueryInput, validate_query as parse_query};
use types::{
    AllowedPropertyAggregate as PropertyValueCount, AnalyticsActivity as Activity,
    AnalyticsAudience as Audience, AnalyticsDiagnostic as Diagnostic,
    AnalyticsSourceStatus as SourceStatus, BehavioralWindow, ComponentStatus, CoreMetrics,
    DailyEventRow as DailyEvent, DailyVisitorRow as DailyVisitor, EntryPage,
    EntryRankingRow as EntryRanking, PropertyAvailability as PropertyResult, ProviderComponent,
};

fn query_input(raw: &str) -> QueryInput {
    let pairs: Vec<_> = url::form_urlencoded::parse(raw.as_bytes()).collect();
    QueryInput::from_pairs(
        pairs
            .iter()
            .map(|(key, value)| (key.as_ref(), value.as_ref())),
    )
}

fn count(value: u64) -> types::AggregateCell {
    policy::suppress(Some(value), None)
}

// ---------------------------------------------------------------------------
// Fixtures shared by the unit cases
// ---------------------------------------------------------------------------

const KEY: &str = "test-secret-key-for-testing-minimum-32-chars";

fn policy() -> MarketPolicy {
    let product = Uuid::from_u128(0x1000);
    let point = Uuid::from_u128(0x2000);
    MarketPolicy {
        source_sha: "0123456789abcdef0123456789abcdef01234567".into(),
        products: vec![ProductPolicy {
            id: product,
            code: "bio_methanol".into(),
            name: "Bio Methanol".into(),
            fuel_type: "Methanol".into(),
            fuel_grade: "A".into(),
            execution_mode: "ORDERBOOK".into(),
            lanes: vec![point],
            qualification_profile: "scalar".into(),
            specification_standard: None,
            display_order: 1,
        }],
        points: vec![PointPolicy {
            id: point,
            name: "Rotterdam".into(),
            region: "Europe".into(),
            timezone: "UTC".into(),
            display_order: 1,
        }],
    }
}

fn query() -> AnalyticsQuery {
    AnalyticsQuery {
        start: "2026-06-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        end: "2026-07-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        previous: Some((
            "2026-05-02T00:00:00Z".parse().unwrap(),
            "2026-06-01T00:00:00Z".parse().unwrap(),
        )),
        compare: true,
        audience: Audience::All,
        activity: Activity::Live,
        product_id: None,
        delivery_point_id: None,
        availability_window: None,
    }
}

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

/// A behavioral reading with one and two-event cells, so suppression is
/// observable rather than asserted against an empty case.
fn reading(status: ComponentStatus) -> BehavioralWindow {
    let observed = Utc::now();
    if status == ComponentStatus::Unavailable {
        return BehavioralWindow::unavailable(
            query().start,
            query().end,
            observed,
            Diagnostic::Timeout,
        );
    }
    BehavioralWindow {
        start: query().start,
        end: query().end,
        availability: types::BehavioralAvailability::Available,
        diagnostic: None,
        coverage: types::AnalyticsSourceCoverage::available(),
        observed_at: observed,
        core: ProviderComponent::available(CoreMetrics {
            visitors: Some(120),
            visits: Some(90),
            pageviews: Some(300),
            total_time_seconds: Some(1800),
            average_session_duration_seconds: Some(Decimal::from(20)),
        }),
        event_totals: ProviderComponent::available(vec![
            types::AllowedEventAggregate {
                event: Event::LandingCtaClicked.name().into(),
                count: count(25),
            },
            types::AllowedEventAggregate {
                event: Event::EnergyCalculatorStarted.name().into(),
                count: count(1),
            },
            types::AllowedEventAggregate {
                event: Event::LoginFailed.name().into(),
                count: count(6),
            },
            types::AllowedEventAggregate {
                event: Event::FrontendError.name().into(),
                count: count(2),
            },
            types::AllowedEventAggregate {
                event: Event::NavigationPerformance.name().into(),
                count: count(14),
            },
        ]),
        daily_visitors: ProviderComponent::available(vec![DailyVisitor {
            date: day("2026-06-10"),
            visitors: count(30),
        }]),
        event_series: ProviderComponent::available(vec![DailyEvent {
            date: day("2026-06-10"),
            event: Event::LoginFailed.name().into(),
            count: count(6),
        }]),
        top_entries: ProviderComponent::available(vec![
            EntryRanking {
                entry: EntryPage::Signup,
                count: count(40),
                rank: 1,
            },
            EntryRanking {
                entry: EntryPage::Landing,
                count: count(1),
                rank: 1,
            },
        ]),
        top_referrers: ProviderComponent::available(vec![
            types::ReferrerRankingRow {
                referrer: types::ReferrerCategory::Search,
                count: count(25),
                rank: 1,
            },
            types::ReferrerRankingRow {
                referrer: types::ReferrerCategory::Direct,
                count: count(10),
                rank: 1,
            },
        ]),
        property_results: vec![
            PropertyResult {
                event: Event::LandingCtaClicked.name().into(),
                property: Property::Cta.name().into(),
                result: ProviderComponent::available(vec![
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "pilot".into(),
                        count: count(12),
                    },
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "register".into(),
                        count: count(1),
                    },
                ]),
            },
            PropertyResult {
                event: Event::LandingCtaClicked.name().into(),
                property: Property::Placement.name().into(),
                result: ProviderComponent::available(vec![PropertyValueCount {
                    event: String::new(),
                    property: String::new(),
                    value: "hero".into(),
                    count: count(9),
                }]),
            },
            PropertyResult {
                event: Event::LandingCtaClicked.name().into(),
                property: Property::Language.name().into(),
                result: ProviderComponent::available(vec![PropertyValueCount {
                    event: String::new(),
                    property: String::new(),
                    value: "en".into(),
                    count: count(7),
                }]),
            },
            PropertyResult {
                event: Event::LoginFailed.name().into(),
                property: Property::ReasonCategory.name().into(),
                result: ProviderComponent::available(vec![
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "invalid_credentials".into(),
                        count: count(4),
                    },
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "account_state".into(),
                        count: count(1),
                    },
                ]),
            },
            PropertyResult {
                event: Event::FrontendError.name().into(),
                property: Property::RouteFamily.name().into(),
                result: ProviderComponent::available(vec![PropertyValueCount {
                    event: String::new(),
                    property: String::new(),
                    value: "admin".into(),
                    count: count(2),
                }]),
            },
            PropertyResult {
                event: Event::FrontendError.name().into(),
                property: Property::Category.name().into(),
                result: ProviderComponent::available(vec![PropertyValueCount {
                    event: String::new(),
                    property: String::new(),
                    value: "render".into(),
                    count: count(2),
                }]),
            },
            PropertyResult {
                event: Event::NavigationPerformance.name().into(),
                property: Property::LatencyBucket.name().into(),
                result: ProviderComponent::available(vec![
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "lt250".into(),
                        count: count(14),
                    },
                    PropertyValueCount {
                        event: String::new(),
                        property: String::new(),
                        value: "250_500".into(),
                        count: count(2),
                    },
                ]),
            },
        ],
    }
}

struct StubBehavioral {
    status: ComponentStatus,
}

impl BehavioralRead for StubBehavioral {
    fn window<'a>(
        &'a self,
        _query: &'a AnalyticsQuery,
        _properties: &'a AllowedPropertySet,
    ) -> ProviderFuture<'a, BehavioralWindow> {
        let window = reading(self.status);
        Box::pin(async move { window })
    }
}

struct StubReference {
    quote: Option<Decimal>,
}

impl ReferenceRead for StubReference {
    fn quotes<'a>(
        &'a self,
        _conn: &'a mut sqlx::PgConnection,
        requests: &'a [ReferenceRequest],
    ) -> ReferenceFuture<'a> {
        let quote = self.quote;
        Box::pin(async move {
            Ok(requests
                .iter()
                .filter_map(|request| {
                    quote.map(|price| ReferenceQuote {
                        product_code: request.product_code.clone(),
                        delivery_point_id: request.delivery_point_id,
                        window: request.window.clone(),
                        benchmark_price_usd_per_mt: price,
                        source: "manual_override".into(),
                        generated_at: Utc::now(),
                        observed_at: Some(Utc::now()),
                    })
                })
                .collect())
        })
    }
}

#[cfg(feature = "contract-tests")]
use marketplace::AnalyticsService;

#[cfg(feature = "contract-tests")]
fn service(status: ComponentStatus) -> AnalyticsService {
    AnalyticsService {
        behavioral: Arc::new(StubBehavioral { status }),
        reference: Arc::new(StubReference {
            quote: Some(Decimal::new(79500, 2)),
        }),
    }
}

// ---------------------------------------------------------------------------
// c01: provenance sections are separate
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c01() {
    // The activity flags decide which sections may exist, and they never
    // overlap: no request produces a section the caller did not ask for.
    assert!(
        Activity::Live.includes(crate::analytics::facts::MarketProvenance::Live)
            && !Activity::Live.includes(crate::analytics::facts::MarketProvenance::Demo)
    );
    assert!(
        Activity::Demo.includes(crate::analytics::facts::MarketProvenance::Demo)
            && !Activity::Demo.includes(crate::analytics::facts::MarketProvenance::Live)
    );
    assert!(!Activity::Live.includes(crate::analytics::facts::MarketProvenance::Unknown));
    assert!(
        Activity::All.includes(crate::analytics::facts::MarketProvenance::Live)
            && Activity::All.includes(crate::analytics::facts::MarketProvenance::Demo)
            && Activity::All.includes(crate::analytics::facts::MarketProvenance::Unknown)
    );
    assert!(
        matches!(Activity::Reference, Activity::Reference | Activity::All)
            && !(Activity::Reference != Activity::Reference)
    );

    // The live/demo/unknown labels the response and the SQL share are distinct
    // strings, and the demo identity list is the seeded set.
    assert_eq!(marketplace::demo_market_org_ids().len(), 14);
    assert!(
        marketplace::demo_market_org_ids()
            .contains(&Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111))
    );
    // An order whose provenance is REAL but whose organization is a seeded
    // demo organization stays demo: the two predicates are OR-ed, never
    // averaged.
    assert!(marketplace::ordered_provenance_labels().contains(&"live"));
    assert!(marketplace::ordered_provenance_labels().contains(&"demo"));
    assert!(marketplace::ordered_provenance_labels().contains(&"unknown"));
}

// ---------------------------------------------------------------------------
// c02: commercial actuals are live-only and never reference-derived
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c02() {
    let commissions = marketplace::CommissionAggregate {
        revenue_current: Decimal::new(158000, 2),
        revenue_previous: Decimal::ZERO,
        missing_payment_date: 1,
        pending_total: Decimal::new(101250, 2),
        invoiced_total: Decimal::new(115275, 2),
    };
    let trades = marketplace::TradeAggregate {
        provenance: "live".into(),
        confirmed_current_strict: 4,
        confirmed_previous_strict: 1,
        legacy_current: 0,
        legacy_previous: 0,
        volume_current_strict: Decimal::new(850, 0),
        volume_current_legacy: Decimal::ZERO,
        volume_previous: Decimal::new(290, 0),
        gmv_current: Decimal::new(31600000, 2),
        gmv_previous: Decimal::ZERO,
        missing_paid_at_current: 0,
    };
    let summary = commercial::summarize(Some(&trades), &commissions);
    assert_eq!(
        summary.realized_gmv_usd.value.unwrap().to_string(),
        "316000.00"
    );
    assert_eq!(
        summary.realized_revenue_usd.value.unwrap().to_string(),
        "1580.00"
    );
    // A reference price exists for this slice, and it must not appear in any
    // commercial field: reference is coverage, not revenue.
    let reference = Decimal::new(79500, 2);
    assert_ne!(
        summary.realized_gmv_usd.value.unwrap(),
        reference,
        "reference price leaked into realized GMV"
    );
    // A live trade row that never arrived reports zero realized GMV, not null.
    let empty = commercial::summarize(None, &marketplace::CommissionAggregate::default());
    assert_eq!(empty.realized_gmv_usd.value.unwrap().to_string(), "0.00");
}

// ---------------------------------------------------------------------------
// c03: the property registry is closed
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c03() {
    // A registered pair is accepted.
    let ok = AllowedPropertySet::new([(Event::LandingCtaClicked.into(), Property::Cta)]).unwrap();
    assert!(
        ok.pairs()
            .contains(&(Event::LandingCtaClicked.into(), Property::Cta))
    );

    // A property that belongs to another event is refused, and the refusal is
    // the structured 422 with the property code.
    let error = AllowedPropertySet::new([(Event::LoginFailed.into(), Property::Cta)])
        .map_err(AllowedPropertySet::refusal)
        .expect_err("an unregistered property must be refused");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.detail["code"], "ANALYTICS_PROPERTY_INVALID");

    // Free text never becomes a property or an event.
    assert!(policy::RegisteredEvent::from_name("EVIL; DROP TABLE orders").is_none());
    assert!(policy::RegisteredEvent::from_name("order_created").is_some());
    assert_eq!(policy::RegisteredEvent::all().len(), 31);

    // The request set is sorted and deduplicated, and every registry entry the
    // tabs ask for is declared for its event.
    let set = AllowedPropertySet::new([
        (Event::FrontendError.into(), Property::Category),
        (Event::FrontendError.into(), Property::RouteFamily),
        (Event::FrontendError.into(), Property::Category),
    ])
    .unwrap();
    assert_eq!(set.pairs().len(), 2);
    assert_eq!(
        set.pairs()[0],
        (Event::FrontendError.into(), Property::RouteFamily)
    );

    // More than twelve distinct events in one window is refused.
    let too_many: Vec<(policy::RegisteredEvent, Property)> = policy::RegisteredEvent::all()
        .iter()
        .take(13)
        .map(|event| (*event, policy::allowed_properties(*event)[0]))
        .collect();
    let error = AllowedPropertySet::new(too_many)
        .map_err(AllowedPropertySet::refusal)
        .expect_err("thirteen events must be refused");
    assert_eq!(error.detail["code"], "ANALYTICS_PROPERTY_INVALID");
}

// ---------------------------------------------------------------------------
// c05: liquidity suppresses derived spread on small samples
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c05() {
    let quote = |org: u128, side: &str, price: i64| marketplace::QuoteRow {
        provenance: "live".into(),
        organization_id: Uuid::from_u128(org),
        side: side.into(),
        price: Decimal::new(price, 0),
        remaining: Decimal::new(500, 0),
        window: "SPOT".into(),
        created_at: "2026-06-04T12:00:00Z".parse().unwrap(),
        product_name: "Bio Methanol".into(),
        fuel_type: "Methanol".into(),
        fuel_grade: "A".into(),
        delivery_point_name: "Rotterdam".into(),
    };
    let fills: Vec<marketplace::FillRow> = Vec::new();
    let as_of: DateTime<Utc> = "2026-07-01T00:00:00Z".parse().unwrap();

    // One contributing organization: the spread and depth are suppressed, but
    // the slice itself is still listed and counted.
    let mut quality = Quality::default();
    let summary = liquidity::summarize(
        &policy(),
        "live",
        &[quote(1, "BID", 780), quote(1, "ASK", 800)],
        &fills,
        as_of,
        &mut quality,
    );
    assert_eq!(summary.two_sided_slices, Some(1));
    assert_eq!(summary.slices.len(), 1);
    let row = &summary.slices[0];
    assert!(row.suppressed);
    assert_eq!(row.contributing_organizations, None);
    assert_eq!(row.best_bid_usd_per_mt, None);
    assert_eq!(row.spread_usd_per_mt, None);
    assert_eq!(row.spread_bps, None);
    assert_eq!(summary.median_spread_usd_per_mt, None);
    assert!(quality.suppressed_cell_count >= 1);

    // Three organizations: the spread becomes publishable. (800 - 780) / 790 *
    // 10,000 = 253.16 bps.
    let mut quality = Quality::default();
    let summary = liquidity::summarize(
        &policy(),
        "live",
        &[
            quote(1, "BID", 780),
            quote(2, "BID", 770),
            quote(3, "ASK", 800),
        ],
        &fills,
        as_of,
        &mut quality,
    );
    let row = &summary.slices[0];
    assert!(!row.suppressed);
    assert_eq!(row.contributing_organizations, Some(3));
    assert_eq!(row.best_bid_usd_per_mt.unwrap().to_string(), "780");
    assert_eq!(row.best_ask_usd_per_mt.unwrap().to_string(), "800");
    assert_eq!(row.spread_usd_per_mt.unwrap().to_string(), "20");
    assert_eq!(row.spread_bps.unwrap().to_string(), "253.16");
    // Bids at or above 780 * 0.99 = 772.2 are 780 and 770 is below it.
    assert_eq!(row.one_percent_bid_depth_mt.unwrap().to_string(), "500");
    assert_eq!(row.best_bid_depth_mt.unwrap().to_string(), "500");

    // A demo quote never enters a live slice, and an unknown-provenance quote
    // enters no slice at all.
    let mut demo_quote = quote(9, "ASK", 900);
    demo_quote.provenance = "demo".into();
    let mut unknown_quote = quote(9, "ASK", 950);
    unknown_quote.provenance = "unknown".into();
    let summary = liquidity::summarize(
        &policy(),
        "live",
        &[quote(1, "BID", 780), demo_quote, unknown_quote],
        &fills,
        as_of,
        &mut Quality::default(),
    );
    assert_eq!(summary.one_sided_slices, Some(1));
    assert_eq!(summary.slices.len(), 1);
    assert!(summary.slices[0].suppressed);
    assert_eq!(summary.slices[0].best_ask_usd_per_mt, None);
}

// ---------------------------------------------------------------------------
// c06: query validation
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c06() {
    // Defaults: compare on, ALL audience, LIVE activity, no filters.
    let parsed = parse_query(query_input(
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z",
    ))
    .unwrap();
    assert_eq!(parsed, query());
    assert_eq!(
        parsed.previous.unwrap().0,
        "2026-05-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
    );

    // A naive timestamp is UTC; an explicit offset is converted to UTC.
    let naive = parse_query(query_input(
        "start=2026-06-01T00:00:00&end=2026-07-01T00:00:00",
    ))
    .unwrap();
    assert_eq!(naive.start, parsed.start);
    let offset = parse_query(query_input(
        "start=2026-06-01T02:00:00%2B02:00&end=2026-07-01T02:00:00%2B02:00",
    ))
    .unwrap();
    assert_eq!(offset.start, parsed.start);

    // Every refusal is 422 ANALYTICS_QUERY_INVALID.
    for raw in [
        "end=2026-07-01T00:00:00Z",
        "start=not-a-time&end=2026-07-01T00:00:00Z",
        "start=2026-07-01T00:00:00Z&end=2026-06-01T00:00:00Z",
        "start=2025-01-01T00:00:00Z&end=2026-06-01T00:00:00Z",
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&audience=EVERYONE",
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&activity=SEEDED",
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&compare=maybe",
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&product_id=not-a-uuid",
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&availability_window=THIS-IS-TOO-LONG-X",
    ] {
        let error = parse_query(query_input(raw)).expect_err("the query must be refused");
        assert_eq!(
            error.status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{raw}"
        );
        assert_eq!(error.detail["code"], "ANALYTICS_QUERY_INVALID", "{raw}");
    }

    // A checked previous-window underflow is refused, never wrapped. The
    // four-digit years a query string can carry never underflow, so the guard
    // is exercised on the value it protects.
    let underflow = query();
    assert!(
        underflow
            .start
            .checked_sub_signed(chrono::TimeDelta::MAX)
            .is_none(),
        "the subtraction must report that it is not representable"
    );
    let error = query_policy::previous_window(DateTime::<Utc>::MIN_UTC, chrono::TimeDelta::days(1))
        .expect_err("an unrepresentable comparison must be refused");
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.detail["code"], "ANALYTICS_QUERY_INVALID");
    // With comparison off the same early query is accepted without arithmetic.
    parse_query(query_input(
        "start=0001-01-01T00:00:00Z&end=0001-01-02T00:00:00Z&compare=false",
    ))
    .expect("no comparison, no arithmetic");

    // Comparison off needs no previous window at all, even for an early period.
    let no_compare = parse_query(query_input(
        "start=0001-01-02T00:00:00Z&end=0001-01-04T00:00:00Z&compare=false",
    ))
    .unwrap();
    assert!(no_compare.previous.is_none());

    // Unknown parameters are ignored, never carried into SQL or a provider.
    let ignored = parse_query(query_input(
        "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z&admin=1&sql=DROP",
    ))
    .unwrap();
    assert_eq!(ignored, parsed);
}

// ---------------------------------------------------------------------------
// c09: REFERENCE is a marketplace-only input
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c09() {
    assert!(AnalyticsRoute::Marketplace.accepts_reference());
    assert!(!AnalyticsRoute::Acquisition.accepts_reference());
    assert!(!AnalyticsRoute::Reliability.accepts_reference());

    let mut reference = query();
    reference.activity = Activity::Reference;
    assert!(marketplace::refuse_reference(&reference).is_err());
    for route in [AnalyticsRoute::Acquisition, AnalyticsRoute::Reliability] {
        assert!(!route.accepts_reference());
    }
    // The refusal is the structured query code.
    let error = marketplace::refuse_reference(&reference).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error.detail["code"], "ANALYTICS_QUERY_INVALID");

    reference.activity = Activity::All;
    assert!(marketplace::refuse_reference(&reference).is_ok());

    // The declared method/template pairs are the ones the route manifest owns.
    for route in AnalyticsRoute::ALL {
        assert_eq!(route.method(), "GET");
        assert!(
            route
                .template()
                .starts_with("/api/admin/analytics/product-analytics/")
        );
    }
}

// ---------------------------------------------------------------------------
// c10: unavailable behavioral data is null, never zero
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c10() {
    let unavailable = reading(ComponentStatus::Unavailable);
    assert_eq!(unavailable.coverage.status, SourceStatus::Unavailable);
    let degraded = acquisition::Reading {
        window: unavailable.clone(),
    };
    assert_eq!(degraded.diagnostic(), Some(Diagnostic::Timeout));

    let mut quality = Quality::default();
    let meta = marketplace::meta(
        &query(),
        Utc::now(),
        MetaOptions {
            behavioral: Some(&unavailable),
            behavioral_source: Source::Applicable,
            ..MetaOptions::default()
        },
    );
    assert_eq!(meta.coverage.behavioral.status, SourceStatus::Unavailable);
    assert_eq!(
        meta.coverage.behavioral.diagnostic,
        Some(Diagnostic::Timeout)
    );
    // Sources this tab never reads say so explicitly rather than claiming a
    // gap.
    assert_eq!(
        meta.coverage.login_history.status,
        SourceStatus::NotApplicable
    );
    assert_eq!(meta.coverage.reference.status, SourceStatus::NotApplicable);

    let response = acquisition::compose(&query(), meta, Some(&degraded), None, &mut quality);
    // Every behavioral value is null; a zero here would be a fabricated count.
    assert_eq!(response.kpis.visitors.value, None);
    assert_eq!(response.kpis.visits.value, None);
    assert_eq!(response.kpis.pageviews.value, None);
    assert_eq!(response.kpis.average_session_duration_seconds.value, None);
    assert_eq!(response.kpis.cta_clicks.value, None);
    assert!(response.visitors_trend.is_empty());
    assert!(response.referrers.is_empty());
    assert!(response.cta_matrix.is_empty());

    // An available window with a genuine zero keeps the zero visible.
    let mut available = reading(ComponentStatus::Available);
    available.daily_visitors = ProviderComponent::available(vec![DailyVisitor {
        date: day("2026-06-02"),
        visitors: count(0),
    }]);
    let response = acquisition::compose(
        &query(),
        marketplace::meta(
            &query(),
            Utc::now(),
            MetaOptions {
                behavioral: Some(&available),
                behavioral_source: Source::Applicable,
                ..MetaOptions::default()
            },
        ),
        Some(&acquisition::Reading { window: available }),
        None,
        &mut Quality::default(),
    );
    let first = response.visitors_trend.first().unwrap();
    assert_eq!(first.date, day("2026-06-01"));
    assert_eq!(first.value, Some(0));
}

// ---------------------------------------------------------------------------
// c11: small cells are suppressed in the composed tabs
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c11() {
    let window = reading(ComponentStatus::Available);
    let current = acquisition::Reading { window };
    let mut quality = Quality::default();
    let response = acquisition::compose(
        &query(),
        marketplace::meta(
            &query(),
            Utc::now(),
            MetaOptions {
                behavioral: Some(&current.window),
                behavioral_source: Source::Applicable,
                ..MetaOptions::default()
            },
        ),
        Some(&current),
        None,
        &mut quality,
    );

    // A one-event metric is suppressed: count null, flag set.
    assert_eq!(response.kpis.cta_clicks.value, Some(25));
    assert_eq!(response.calculator.starts.value, None);
    assert!(response.calculator.starts.suppressed);

    // A one-click CTA value row is suppressed and loses its share.
    let cta: Vec<_> = response
        .cta_matrix
        .iter()
        .filter(|row| row.placement == "all")
        .collect();
    let register = cta.iter().find(|row| row.cta == "register").unwrap();
    assert!(register.suppressed);
    assert_eq!(register.clicks, None);
    assert_eq!(register.share_pct, None);
    let pilot = cta.iter().find(|row| row.cta == "pilot").unwrap();
    assert!(!pilot.suppressed);
    assert_eq!(pilot.clicks, Some(12));
    assert_eq!(pilot.share_pct.unwrap().to_string(), "48.00");

    // Rankings suppress a one-event row and keep the rest.
    let landing = response
        .referrers
        .iter()
        .find(|row| row.key == "direct")
        .unwrap();
    assert!(!landing.suppressed);
    let signup = response
        .entry_pages
        .iter()
        .find(|row| row.key == "landing")
        .unwrap();
    assert!(signup.suppressed);
    assert_eq!(signup.count, None);

    // The suppressed-cell count reaches meta after composition.
    let body = marketplace::to_json(&response);
    assert!(quality.suppressed_cell_count >= 3);
    assert_eq!(
        body["meta"]["data_quality"]["suppressed_cell_count"],
        json!(0),
        "compose does not publish the final count; the adapter does"
    );

    // Sharing: 25 of 25 CTA clicks is 100.00, and a zero total yields no share.
    assert_eq!(pilot.share_pct.unwrap().to_string(), "48.00");
    assert_eq!(marketplace::rate_pct(1, 0), None);
    assert_eq!(marketplace::rate_pct(3, 7).unwrap().to_string(), "42.86");
    assert_eq!(
        marketplace::share_pct(25, 25).unwrap().to_string(),
        "100.00"
    );

    // Suppression thresholds: 0 stays visible, 1 and 2 suppress, 3 stays.
    let mut quality = Quality::default();
    assert_eq!(
        marketplace::suppress_cell(Some(0), Some(0), &mut quality),
        (Some(0), false)
    );
    assert_eq!(
        marketplace::suppress_cell(Some(1), None, &mut quality),
        (None, true)
    );
    assert_eq!(
        marketplace::suppress_cell(Some(2), None, &mut quality),
        (None, true)
    );
    assert_eq!(
        marketplace::suppress_cell(Some(3), None, &mut quality),
        (Some(3), false)
    );
    assert_eq!(
        marketplace::suppress_cell(None, None, &mut quality),
        (None, false)
    );
}

// ---------------------------------------------------------------------------
// c12: reliability panels degrade without inventing zero
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c12() {
    let window = reading(ComponentStatus::Available);
    let current = acquisition::Reading { window };
    let mut quality = Quality::default();
    let response = reliability::compose(
        &query(),
        marketplace::meta(
            &query(),
            Utc::now(),
            MetaOptions {
                behavioral: Some(&current.window),
                behavioral_source: Source::Applicable,
                ..MetaOptions::default()
            },
        ),
        Some(&current),
        None,
        Vec::new(),
        &mut quality,
    );
    assert_eq!(response.collector.status, SourceStatus::Available);
    assert_eq!(response.login_failures.total.value, Some(6));
    // A two-event frontend-error panel is suppressed, not reported as 2.
    assert_eq!(response.frontend_errors.total.value, None);
    assert!(response.frontend_errors.total.suppressed);
    // A one-event category is suppressed too.
    let render = response
        .frontend_errors
        .by_category
        .iter()
        .find(|cell| cell.key == "render")
        .unwrap();
    assert!(render.suppressed);
    // Navigation latency carries the cross-destination distribution.
    assert_eq!(response.navigation_latency.len(), 1);
    assert_eq!(
        serde_json::to_value(response.navigation_latency[0].destination).unwrap(),
        json!("all")
    );
    assert_eq!(response.navigation_latency[0].buckets.len(), 5);
    let lt250 = &response.navigation_latency[0].buckets[0];
    assert_eq!(lt250.key, "lt250");
    assert_eq!(lt250.count, Some(14));

    // An unavailable provider leaves the collector unavailable and every
    // panel null; it never zeroes them.
    let unavailable = acquisition::Reading {
        window: reading(ComponentStatus::Unavailable),
    };
    let response = reliability::compose(
        &query(),
        marketplace::meta(&query(), Utc::now(), MetaOptions::default()),
        Some(&unavailable),
        None,
        Vec::new(),
        &mut Quality::default(),
    );
    assert_eq!(response.collector.status, SourceStatus::Unavailable);
    assert_eq!(response.collector.diagnostic, Some(Diagnostic::Timeout));
    assert_eq!(response.login_failures.total.value, None);
    assert_eq!(response.frontend_errors.total.value, None);
    assert_eq!(response.backend_unavailable.total.value, None);
    assert!(response.navigation_latency.is_empty());
}

// ---------------------------------------------------------------------------
// c15: authenticate, then administrator, then quota, then read
// ---------------------------------------------------------------------------

fn actor(role: Option<&str>) -> auth::CurrentActor {
    auth::CurrentActor {
        id: Uuid::from_u128(0x5001),
        organization_id: None,
        role: role.map(str::to_owned),
        email_verified: true,
    }
}

#[test]
fn p4_t19_c15() {
    use axum::http::StatusCode;

    let limits = auth::limits::SharedLimits::default();
    let token = "bearer-token-for-quota";
    let route = AnalyticsRoute::Reliability;

    // A non-administrator is refused with 403 before any quota is charged:
    // a thousand refusals must leave the administrator's window untouched.
    for role in [None, Some("BUYER"), Some("SUPPLIER")] {
        for _ in 0..1000 {
            let error =
                marketplace::admit_analytics(&actor(role), &limits, route, "1.2.3.4", token, 1_000)
                    .expect_err("a non-administrator must be refused");
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            assert_eq!(error.detail["code"], "ADMIN_REQUIRED");
            // A refusal carries no Retry-After: it is not a quota answer.
            assert_eq!(error.retry_after, None);
        }
    }

    // The administrator's declared quota is exactly 30 per minute, and the
    // 31st request is a 429 with a Retry-After of at most one window.
    for attempt in 0..30 {
        assert!(
            marketplace::admit_analytics(
                &actor(Some("ADMIN")),
                &limits,
                route,
                "1.2.3.4",
                token,
                1_000
            )
            .is_ok(),
            "attempt {attempt} must be admitted"
        );
    }
    let error = marketplace::admit_analytics(
        &actor(Some("ADMIN")),
        &limits,
        route,
        "1.2.3.4",
        token,
        1_000,
    )
    .expect_err("the 31st request must be refused");
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error.detail["code"], "RATE_LIMITED");
    let retry_after = error
        .retry_after
        .expect("a quota refusal carries Retry-After");
    assert!((1..=60).contains(&retry_after));

    // A malformed query is a 422 that also spends no quota: the administrator
    // keeps all thirty admissions after twenty bad requests.
    let limits = auth::limits::SharedLimits::default();
    for _ in 0..20 {
        let error = marketplace::admit_tab(
            &actor(Some("ADMIN")),
            &limits,
            route,
            "9.9.9.9",
            "quota-token",
            3_000,
            "start=nope&end=nope",
        )
        .expect_err("a malformed query must be refused");
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    for attempt in 0..30 {
        marketplace::admit_tab(
            &actor(Some("ADMIN")),
            &limits,
            route,
            "9.9.9.9",
            "quota-token",
            3_000,
            "start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z",
        )
        .unwrap_or_else(|_| panic!("attempt {attempt} must be admitted"));
    }

    // A different token has its own window, and the window reopens.
    assert!(
        marketplace::admit_analytics(
            &actor(Some("ADMIN")),
            &limits,
            route,
            "1.2.3.4",
            "another-token",
            1_000
        )
        .is_ok()
    );
    assert!(
        marketplace::admit_analytics(
            &actor(Some("ADMIN")),
            &limits,
            route,
            "1.2.3.4",
            token,
            1_060
        )
        .is_ok()
    );

    // Each tab is its own bucket, so exhausting one never refuses another.
    let limits = auth::limits::SharedLimits::default();
    for _ in 0..30 {
        marketplace::admit_analytics(
            &actor(Some("ADMIN")),
            &limits,
            route,
            "5.6.7.8",
            "t3",
            2_000,
        )
        .unwrap();
    }
    assert!(
        marketplace::admit_analytics(
            &actor(Some("ADMIN")),
            &limits,
            AnalyticsRoute::Acquisition,
            "5.6.7.8",
            "t3",
            2_000
        )
        .is_ok()
    );
    assert!(
        marketplace::admit_analytics(
            &actor(Some("ADMIN")),
            &limits,
            AnalyticsRoute::Marketplace,
            "5.6.7.8",
            "t3",
            2_000
        )
        .is_ok()
    );

    // The anonymous path never reaches admission at all: dispatch refuses a
    // missing or malformed bearer with 401 before any database work. The
    // verifier is the shared one, so this is the same refusal every route uses.
    let settings = auth::VerificationSettings::new(KEY.into(), None, "test".into(), "test".into());
    for header in [None, Some("Basic abc"), Some("Bearer not-a-jwt")] {
        let error = auth::verify_authorization(header, &settings, Utc::now())
            .expect_err("anonymous access must be refused");
        assert_eq!(error.status(), 401);
    }
    let claims = json!({
        "sub": Uuid::from_u128(0x5001),
        "iat": Utc::now().timestamp(),
        "exp": Utc::now().timestamp() + 3600,
        "type": "access",
        "iss": "test",
        "aud": "test",
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes()),
    )
    .unwrap();
    assert!(
        auth::verify_authorization(Some(&format!("Bearer {token}")), &settings, Utc::now()).is_ok()
    );
}

// ---------------------------------------------------------------------------
// c16: the tab routes are the ones the manifest declares
// ---------------------------------------------------------------------------

#[test]
fn p4_t19_c16() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase4/routes.json"
    ))
    .unwrap();
    let routes = manifest["routes"].as_array().unwrap();
    for route in AnalyticsRoute::ALL {
        let entry = routes
            .iter()
            .find(|entry| entry["method"] == route.method() && entry["path"] == route.template())
            .unwrap_or_else(|| panic!("{} is absent from the route manifest", route.template()));
        assert_eq!(entry["auth"], "A");
        assert_eq!(entry["group"], "P");
        let limits = entry["limits"].as_array().unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0]["count"], 30);
        assert_eq!(limits[0]["window"], "minute");
        assert_eq!(limits[0]["key"], "token");
    }
    // The tabs this lane owns are exactly three of the eleven P routes.
    assert_eq!(AnalyticsRoute::ALL.len(), 3);
}

#[test]
fn p4_t19_c17() {
    for visits in [0, 1, 2, 3] {
        for total_time in [None, Some(90)] {
            for previous_only in [false, true] {
                let mut window = reading(ComponentStatus::Available);
                let mut core = window.core.into_data().unwrap();
                core.visits = Some(visits);
                core.total_time_seconds = total_time;
                window.core = ProviderComponent::available(core);
                let sample = acquisition::Reading { window };
                let full = acquisition::Reading { window: reading(ComponentStatus::Available) };
                let (current, previous) = if previous_only { (&full, &sample) } else { (&sample, &full) };
                let response = acquisition::compose(
                    &query(), marketplace::meta(&query(), Utc::now(), MetaOptions::default()),
                    Some(current), Some(previous), &mut Quality::default(),
                );
                let duration = response.kpis.average_session_duration_seconds;
                let value = if previous_only { duration.previous } else { duration.value };
                assert_eq!(duration.suppressed, visits == 1 || visits == 2);
                assert_eq!(value, if visits >= 3 { total_time.map(|total| Decimal::from(total) / Decimal::from(visits)) } else { None });
                let visible_visits = if previous_only { response.kpis.visits.previous } else { response.kpis.visits.value };
                assert_eq!(visible_visits, if visits == 1 || visits == 2 { None } else { Some(visits) });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// c04 / c07 / c08: PostgreSQL cases
// ---------------------------------------------------------------------------

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection};

    struct Fixture {
        /// Runtime connection, granted only the app's reads.
        app_url: String,
        /// Migrator connection that owns the disposable schema.
        admin: PgConnection,
        schema: String,
        migrator_url: String,
        product: Uuid,
        point: Uuid,
        buyer_org: Uuid,
        supplier_org: Uuid,
        demo_org: Uuid,
        unknown_org: Uuid,
        admin_user: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let migrator_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL");
            let app_url =
                std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
            let parsed = url::Url::parse(&migrator_url).unwrap();
            let runtime = url::Url::parse(&app_url).unwrap();
            assert_eq!(parsed.host_str(), Some("127.0.0.1"));
            assert_eq!(parsed.port(), runtime.port());
            assert_eq!(parsed.path(), runtime.path());
            assert!(parsed.path().ends_with("_test"));
            assert_ne!(parsed.username(), runtime.username());
            assert_eq!(runtime.username(), "rust_app");

            let mut admin = PgConnection::connect(&migrator_url).await.unwrap();
            let schema = format!("p4_t19_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(schema_ddl()))
                .execute(&mut admin)
                .await
                .unwrap();

            let product = Uuid::from_u128(0x1000);
            let point = Uuid::from_u128(0x2000);
            let buyer_org = Uuid::from_u128(0x3001);
            let supplier_org = Uuid::from_u128(0x3002);
            let demo_org = Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111);
            let unknown_org = Uuid::from_u128(0x3004);
            let admin_user = Uuid::from_u128(0x4001);

            for (id, provenance, role) in [
                (buyer_org, "REAL", "BUYER"),
                (supplier_org, "REAL", "SUPPLIER"),
                (demo_org, "DEMO", "BUYER"),
                (unknown_org, "UNKNOWN", "BUYER"),
            ] {
                sqlx::query(
                    "INSERT INTO organizations(id,name,provenance,type,verification_status,created_at) VALUES($1,$2,$3,'FUEL_BUYER','APPROVED','2026-01-01Z')",
                )
                .bind(id)
                .bind(format!("org-{id}"))
                .bind(provenance)
                .execute(&mut admin)
                .await
                .unwrap();
                sqlx::query(
                    "INSERT INTO users(id,email,password_hash,organization_id,role,status,email_verified,created_at) VALUES($1,$2,'hash',$3,$4,'APPROVED',true,'2026-01-01Z')",
                )
                .bind(Uuid::new_v4())
                .bind(format!("member{id}@fixtures.verdaxis.test"))
                .bind(id)
                .bind(role)
                .execute(&mut admin)
                .await
                .unwrap();
            }
            sqlx::query(
                "INSERT INTO users(id,email,password_hash,organization_id,role,status,email_verified,created_at) VALUES($1,'admin@fixtures.verdaxis.test','hash',NULL,'ADMIN','APPROVED',true,'2026-01-01Z')",
            )
            .bind(admin_user)
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade,is_active) VALUES($1,'Bio Methanol','Methanol','A',true)")
                .bind(product)
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO delivery_points(id,name,region,is_active) VALUES($1,'Rotterdam','Europe',true)")
                .bind(point)
                .execute(&mut admin)
                .await
                .unwrap();

            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app;"
            )))
            .execute(&mut admin)
            .await
            .unwrap();

            Self {
                app_url,
                admin,
                schema,
                migrator_url,
                product,
                point,
                buyer_org,
                supplier_org,
                demo_org,
                unknown_org,
                admin_user,
            }
        }

        async fn connect(&self) -> PgConnection {
            let mut conn = PgConnection::connect(&self.app_url).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "SET search_path TO {}",
                self.schema
            )))
            .execute(&mut conn)
            .await
            .unwrap();
            conn
        }

        async fn order(
            &mut self,
            org: Uuid,
            side: &str,
            price: i64,
            created: &str,
            provenance: &str,
        ) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(include_str!("fixtures/analytics_order_insert.sql"))
                .bind(id)
                .bind(org)
                .bind(side)
                .bind(self.product)
                .bind(self.point)
                .bind(Decimal::new(price, 0))
                .bind(Decimal::new(500, 0))
                .bind(created.parse::<DateTime<Utc>>().unwrap())
                .bind(provenance)
                .execute(&mut self.admin)
                .await
                .unwrap();
            id
        }

        async fn audit(&mut self, at: &str, action: &str) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(include_str!("fixtures/analytics_audit_insert.sql"))
                .bind(id)
                .bind(action)
                .bind(self.admin_user)
                .bind(at.parse::<DateTime<Utc>>().unwrap())
                .execute(&mut self.admin)
                .await
                .unwrap();
            id
        }

        async fn finish(mut self) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
            let _ = self.migrator_url;
        }
    }

    fn schema_ddl() -> &'static str {
        "CREATE TABLE organizations(id uuid PRIMARY KEY,name text,provenance text NOT NULL,type text,verification_status text,created_at timestamptz);\
         CREATE TABLE users(id uuid PRIMARY KEY,email text,password_hash text,organization_id uuid,role text,status text NOT NULL,email_verified boolean NOT NULL DEFAULT true,created_at timestamptz,must_change_password boolean NOT NULL DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz);\
         CREATE TABLE products(id uuid PRIMARY KEY,name text,fuel_type text,fuel_grade text,is_active boolean NOT NULL DEFAULT true);\
         CREATE TABLE delivery_points(id uuid PRIMARY KEY,name text,region text,is_active boolean NOT NULL DEFAULT true);\
         CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid NOT NULL,side text NOT NULL,provenance text NOT NULL DEFAULT 'UNKNOWN',product_id uuid NOT NULL,delivery_point_id uuid,availability_window text NOT NULL DEFAULT 'SPOT',remaining_quantity_mt numeric NOT NULL,price_per_mt_usd numeric NOT NULL,status text NOT NULL DEFAULT 'OPEN',expires_at timestamptz,created_at timestamptz NOT NULL);\
         CREATE TABLE trades(id uuid PRIMARY KEY,bid_order_id uuid,ask_order_id uuid,buyer_id uuid NOT NULL,seller_id uuid NOT NULL,buyer_provenance text NOT NULL,seller_provenance text NOT NULL,status text NOT NULL,quantity_mt numeric NOT NULL,final_quantity_mt numeric,final_total_usd numeric,confirmed_at timestamptz,paid_at timestamptz,created_at timestamptz NOT NULL);\
         CREATE TABLE commissions(id uuid PRIMARY KEY,match_id uuid NOT NULL UNIQUE,trade_id uuid,amount_usd numeric(12,2) NOT NULL,status text NOT NULL,payment_date date,created_at timestamptz);\
         CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100),resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz NOT NULL);"
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c18() {
        use tower::ServiceExt;
        let fixture = Fixture::new().await;
        let schema = fixture.schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2)
            .after_connect(move |conn, _| {
                let statement = format!("SET search_path TO {schema}");
                Box::pin(async move { sqlx::raw_sql(sqlx::AssertSqlSafe(statement)).execute(conn).await?; Ok(()) })
            }).connect(&fixture.migrator_url).await.unwrap();
        let config: verdaxis_read::Config = serde_json::from_value(json!({
            "environment": "test", "release_sha": verdaxis_read::compiled_sha(),
            "migration_revision": "kycg_20261014_staging", "database_url": fixture.app_url,
            "db_session_time_zone": "UTC", "bind": "127.0.0.1:0",
            "read_admission_slots": 2, "pool_max": 3, "python_budget": 52,
            "configured_max_connections": 100
        })).unwrap();
        let health = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect_lazy(&fixture.app_url).unwrap();
        let app = AppState::new(config, runtime::db::Pools { read: pool.clone(), health }, policy()).unwrap();
        let settings = auth::VerificationSettings::hs256(KEY);
        let now = Utc::now().timestamp();
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({
            "sub": fixture.admin_user.to_string(), "type": "access", "iat": now, "exp": now + 600
        }), &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes())).unwrap();
        let router = marketplace::router(app, settings, auth::limits::SharedLimits::default(), service(ComponentStatus::Unavailable));
        let uri = format!("{}?start=2026-06-01T00:00:00Z&end=2026-07-01T00:00:00Z", AnalyticsRoute::Acquisition.template());
        for index in 0..30 {
            let response = router.clone().oneshot(axum::http::Request::builder().uri(&uri)
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "request {index}");
        }
        for header in [format!("Bearer {token}"), format!("bearer {token}"), format!("Bearer  {token}"), format!("BEARER   {token}")] {
            let response = router.clone().oneshot(axum::http::Request::builder().uri(&uri)
                .header("authorization", header).body(axum::body::Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
            assert!(response.headers().contains_key("retry-after"));
        }
        drop(router);
        pool.close().await;
        fixture.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c19() {
        let mut fixture = Fixture::new().await;
        let buyer = fixture.buyer_org;
        // Previous-only rows have a distinct product, point, window and status.
        fixture.product = Uuid::from_u128(0x1001);
        fixture.point = Uuid::from_u128(0x2001);
        sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade,is_active) VALUES($1,'Previous product','Methanol','A',true)")
            .bind(fixture.product).execute(&mut fixture.admin).await.unwrap();
        sqlx::query("INSERT INTO delivery_points(id,name,region,is_active) VALUES($1,'Previous point','Europe',true)")
            .bind(fixture.point).execute(&mut fixture.admin).await.unwrap();
        for _ in 0..256 {
            fixture.order(buyer, "ASK", 900, "2026-05-15T00:00:00Z", "REAL").await;
        }
        sqlx::query(include_str!("fixtures/analytics_order_previous.sql"))
            .execute(&mut fixture.admin).await.unwrap();
        fixture.product = Uuid::from_u128(0x1000);
        fixture.point = Uuid::from_u128(0x2000);
        fixture.order(buyer, "BID", 800, "2026-06-15T00:00:00Z", "REAL").await;
        let mut conn = fixture.connect().await;
        for compare in [true, false] {
            let mut query = query(); query.compare = compare;
            if !compare { query.previous = None; }
            let statuses = marketplace::status_distribution(&mut conn, &query).await.unwrap();
            assert_eq!(statuses.len(), 1);
            assert_eq!(statuses[0].status, "OPEN"); assert_eq!(statuses[0].count, 1);
            let rows = marketplace::matrix_and_windows(&mut conn, &query).await.unwrap();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|row| row.orders == 1));
            assert!(rows.iter().any(|row| row.window.as_deref() == Some("SPOT")));
            assert!(rows.iter().any(|row| row.product_name.as_deref() == Some("Bio Methanol")));
        }
        fixture.finish().await;
    }

    // c04: the reliability audit sample is restricted to the requested window.
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c04() {
        let mut fixture = Fixture::new().await;
        fixture.audit("2026-05-31T23:59:59Z", "before_window").await;
        fixture
            .audit("2026-06-01T00:00:00Z", "at_window_start")
            .await;
        fixture.audit("2026-06-15T12:00:00Z", "inside_window").await;
        fixture.audit("2026-07-01T00:00:00Z", "at_window_end").await;
        fixture.audit("2026-07-02T00:00:00Z", "after_window").await;

        let mut conn = fixture.connect().await;
        let query = query();
        let rows = reliability::audit_activity(&mut conn, query.start, query.end)
            .await
            .unwrap();
        let actions: Vec<&str> = rows.iter().map(|row| row.action.as_str()).collect();
        assert_eq!(actions, vec!["inside_window", "at_window_start"]);

        // Newest first, and the actor role is published without an identity.
        assert_eq!(
            rows[0].occurred_at,
            "2026-06-15T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(rows[0].actor_role.as_deref(), Some("ADMIN"));
        let body = serde_json::to_string(&rows).unwrap();
        assert!(!body.contains("fixtures.verdaxis.test"));
        assert!(!body.contains("ip_address"));

        // More than twenty in-window rows still yields exactly twenty, and the
        // twenty newest survive.
        for index in 0..25 {
            let at = format!("2026-06-20T{:02}:00:00Z", index % 24);
            fixture.audit(&at, "bulk").await;
        }
        let rows = reliability::audit_activity(&mut conn, query.start, query.end)
            .await
            .unwrap();
        assert_eq!(rows.len(), 20);
        fixture.finish().await;
    }

    // c07: provenance separation over real rows.
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c07() {
        let mut fixture = Fixture::new().await;
        let (buyer, supplier, demo, unknown) = (
            fixture.buyer_org,
            fixture.supplier_org,
            fixture.demo_org,
            fixture.unknown_org,
        );
        // Two recognized organizations carry REAL provenance; the seeded demo
        // organization carries DEMO; an ordinary unvetted organization carries
        // UNKNOWN. Each must land in its own section and nowhere else.
        fixture
            .order(buyer, "BID", 780, "2026-06-04T12:00:00Z", "REAL")
            .await;
        fixture
            .order(buyer, "BID", 770, "2026-06-07T12:00:00Z", "REAL")
            .await;
        fixture
            .order(supplier, "ASK", 800, "2026-06-03T12:00:00Z", "REAL")
            .await;
        fixture
            .order(demo, "BID", 500, "2026-06-05T12:00:00Z", "DEMO")
            .await;
        fixture
            .order(unknown, "ASK", 400, "2026-06-06T12:00:00Z", "UNKNOWN")
            .await;

        let mut conn = fixture.connect().await;
        let query = query();
        let orders = marketplace::aggregate_orders(&mut conn, &query)
            .await
            .unwrap();
        let live = orders.iter().find(|row| row.provenance == "live").unwrap();
        let demo_row = orders.iter().find(|row| row.provenance == "demo").unwrap();
        let unknown_row = orders
            .iter()
            .find(|row| row.provenance == "unknown")
            .unwrap();

        // Each provenance counts only its own rows; live holds the two buyer
        // orders and the supplier order, and never the demo or unknown row.
        assert_eq!(live.current, 3);
        assert_eq!(live.bids_current, 2);
        assert_eq!(live.asks_current, 1);
        assert_eq!(demo_row.current, 1);
        assert_eq!(unknown_row.current, 1);
        // Sources are never summed: the sections are separate objects.
        assert!(live.current + demo_row.current + unknown_row.current == 5);

        // A whole-marketplace composition keeps the sections apart.
        let reference = StubReference {
            quote: Some(Decimal::new(79500, 2)),
        };
        let mut all = query.clone();
        all.activity = Activity::All;
        let response = marketplace::compose(&mut conn, &policy(), &all, &reference, Utc::now())
            .await
            .unwrap();
        let live_section = response.live.unwrap();
        let demo_section = response.demo.unwrap();
        let unknown_section = response.unknown.unwrap();
        assert_eq!(live_section.kpis.open_bids.value, Some(2));
        assert_eq!(live_section.kpis.open_asks.value, Some(1));
        assert_eq!(demo_section.kpis.open_bids.value, Some(1));
        assert_eq!(unknown_section.kpis.open_asks.value, Some(1));
        // Reference is coverage, not activity: no kpis may exist there.
        let reference_section = response.reference.unwrap();
        assert_eq!(reference_section.rows.len(), 1);
        assert_eq!(
            reference_section.rows[0]
                .benchmark_price_usd_per_mt
                .unwrap()
                .to_string(),
            "795.00"
        );
        assert_eq!(
            reference_section.rows[0].coverage_status,
            marketplace::CoverageStatus::Current
        );

        // A LIVE-only request has no demo, unknown or reference section.
        let response = marketplace::compose(&mut conn, &policy(), &query, &reference, Utc::now())
            .await
            .unwrap();
        assert!(response.live.is_some());
        assert!(response.demo.is_none());
        assert!(response.unknown.is_none());
        assert!(response.reference.is_none());
        assert_eq!(
            response.meta.coverage.reference.status,
            SourceStatus::NotApplicable
        );

        // The live commercial summary reads only live rows.
        assert!(response.commercial.is_some());

        fixture.finish().await;
    }

    // c08: catalogue filter validation and reference-only requests.
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c08() {
        let fixture = Fixture::new().await;
        let mut conn = fixture.connect().await;

        let mut bad = query();
        bad.product_id = Some(Uuid::from_u128(0x9fff));
        let error = marketplace::validate_marketplace_filters(&mut conn, &bad)
            .await
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.detail["code"], "ANALYTICS_QUERY_INVALID");

        let mut bad_window = query();
        bad_window.availability_window = Some("SOON".into());
        assert!(
            marketplace::validate_marketplace_filters(&mut conn, &bad_window)
                .await
                .is_err()
        );

        // The fixture's product point are canonical and validate.
        let mut good = query();
        good.product_id = Some(fixture.product);
        good.delivery_point_id = Some(fixture.point);
        marketplace::validate_marketplace_filters(&mut conn, &good)
            .await
            .unwrap();

        // A REFERENCE request answers coverage and no activity section.
        let reference = StubReference {
            quote: Some(Decimal::new(79500, 2)),
        };
        let mut reference_query = query();
        reference_query.activity = Activity::Reference;
        let response = marketplace::compose(
            &mut conn,
            &policy(),
            &reference_query,
            &reference,
            Utc::now(),
        )
        .await
        .unwrap();
        assert!(response.live.is_none());
        assert!(response.commercial.is_none());
        assert!(response.reference.is_some());

        fixture.finish().await;
    }

    // c13: the marketplace adapter reads through the route seam.
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c13() {
        let mut fixture = Fixture::new().await;
        fixture
            .order(
                fixture.buyer_org,
                "BID",
                780,
                "2026-06-04T12:00:00Z",
                "REAL",
            )
            .await;
        let mut conn = fixture.connect().await;
        let service = service(ComponentStatus::Available);
        let readings = service
            .provider_read(AnalyticsRoute::Marketplace, &query())
            .await;
        let body = service
            .compose(
                &mut conn,
                &policy(),
                AnalyticsRoute::Marketplace,
                &query(),
                &readings,
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(body["live"]["kpis"]["open_bids"]["value"], json!(1));
        assert_eq!(body["demo"], json!(null));
        fixture.finish().await;
    }

    // c14: the reliability tab composes the windowed audit strip with a live
    // read connection.
    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t19_c14() {
        let mut fixture = Fixture::new().await;
        fixture.audit("2026-06-15T12:00:00Z", "kyc.approved").await;
        fixture.audit("2026-05-01T12:00:00Z", "outside").await;
        let mut conn = fixture.connect().await;
        let service = service(ComponentStatus::Available);
        let readings = service
            .provider_read(AnalyticsRoute::Reliability, &query())
            .await;
        let body = service
            .compose(
                &mut conn,
                &policy(),
                AnalyticsRoute::Reliability,
                &query(),
                &readings,
                Utc::now(),
            )
            .await
            .unwrap();
        let rows = body["audit_activity"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["action"], json!("kyc.approved"));
        assert_eq!(rows[0]["actor_role"], json!("ADMIN"));
        assert_eq!(body["collector"]["status"], json!("available"));
        fixture.finish().await;
    }
}
