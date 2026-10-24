//! Test double for the CF4_03 analytics contract.
//!
//! Built strictly from the accepted CF4_03 declarations in
//! `tests/rust_contract/phase4/contracts-v2.json` at `b57affd3`
//! (artifact `3c658a4b…`, CF4_03 entry `d85957d2…`). Task 15 owns the real
//! `analytics::types` and `analytics::policy`; Task 16 owns the real
//! `BehavioralWindow` producer. This double exists only to let Task 18's
//! composition be tested before those producers are composed. It is not a
//! promise that a permissive implementation is acceptable: every field is
//! the declared field, and unavailable/not-requested data is `None`, never a
//! fabricated zero or empty list.
#![allow(dead_code)]

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(
                #[serde(rename = $value)]
                $variant,
            )*
        }
    };
}

string_enum!(AnalyticsAudience {
    All => "ALL",
    Buyer => "BUYER",
    Supplier => "SUPPLIER",
});
string_enum!(AnalyticsActivity {
    Live => "LIVE",
    Demo => "DEMO",
    Reference => "REFERENCE",
    All => "ALL",
});
string_enum!(AnalyticsSourceStatus {
    Available => "available",
    Partial => "partial",
    Unavailable => "unavailable",
    NotApplicable => "not_applicable",
});
string_enum!(AnalyticsDiagnostic {
    Disabled => "disabled",
    Configuration => "configuration",
    Authentication => "authentication",
    Timeout => "timeout",
    Upstream => "upstream",
    MalformedResponse => "malformed_response",
    InsufficientCoverage => "insufficient_coverage",
});
string_enum!(LifecycleStageKey {
    Visitors => "visitors",
    Registered => "registered",
    Active => "active",
    Participating => "participating",
    Trading => "trading",
    Retained => "retained",
});
string_enum!(NeedsAttentionRule {
    ApprovedMembersNeverLoggedIn => "approved_members_never_logged_in",
    SignupSubmissionDrop => "signup_submission_drop",
    OneSidedLiveMarket => "one_sided_live_market",
    ElevatedLoginFailures => "elevated_login_failures",
    DegradedAnalyticsCollection => "degraded_analytics_collection",
});
string_enum!(JourneySource {
    Behavioral => "behavioral",
    Authoritative => "authoritative",
});
string_enum!(ConversionRatioKind {
    UserCohort => "user_cohort",
    OrganizationCohort => "organization_cohort",
    AggregateEvent => "aggregate_event",
});
string_enum!(NavigationDestination {
    Home => "home",
    Map => "map",
    Marketplace => "marketplace",
    Curve => "curve",
    Watchlist => "watchlist",
    Analytics => "analytics",
    Trades => "trades",
    Quotes => "quotes",
    Compliance => "compliance",
    Training => "training",
    Settings => "settings",
    Admin => "admin",
});
string_enum!(ComponentStatus {
    Available => "available",
    Partial => "partial",
    Unavailable => "unavailable",
    NotRequested => "not_requested",
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub previous: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub compare: bool,
    pub audience: AnalyticsAudience,
    pub activity: AnalyticsActivity,
    pub product_id: Option<Uuid>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: Option<String>,
}

impl AnalyticsQuery {
    pub fn previous_start(&self) -> Option<DateTime<Utc>> {
        self.previous.map(|(start, _)| start)
    }

    pub fn previous_end(&self) -> Option<DateTime<Utc>> {
        self.previous.map(|(_, end)| end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateCell {
    pub key: String,
    pub count: Option<u64>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricValue {
    pub value: Option<u64>,
    pub previous: Option<u64>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecimalMetricValue {
    pub value: Option<Decimal>,
    pub previous: Option<Decimal>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatioValue {
    pub numerator: Option<u64>,
    pub denominator: Option<u64>,
    pub rate_pct: Option<Decimal>,
    pub suppressed: bool,
    pub cohort_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub date: NaiveDate,
    pub value: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsDataQuality {
    pub legacy_timestamp_fallback_count: u64,
    pub missing_paid_at_count: u64,
    pub missing_commission_payment_date_count: u64,
    pub suppressed_cell_count: u64,
    pub cohort_complete: bool,
}

impl Default for AnalyticsDataQuality {
    fn default() -> Self {
        Self {
            legacy_timestamp_fallback_count: 0,
            missing_paid_at_count: 0,
            missing_commission_payment_date_count: 0,
            suppressed_cell_count: 0,
            cohort_complete: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsSourceCoverage {
    pub coverage_start: Option<DateTime<Utc>>,
    pub coverage_end: Option<DateTime<Utc>>,
    pub observed_at: Option<DateTime<Utc>>,
    pub status: AnalyticsSourceStatus,
    pub diagnostic: Option<AnalyticsDiagnostic>,
}

impl AnalyticsSourceCoverage {
    pub fn not_applicable() -> Self {
        Self {
            coverage_start: None,
            coverage_end: None,
            observed_at: None,
            status: AnalyticsSourceStatus::NotApplicable,
            diagnostic: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsCoverage {
    pub authoritative: AnalyticsSourceCoverage,
    pub behavioral: AnalyticsSourceCoverage,
    pub login_history: AnalyticsSourceCoverage,
    pub status_history: AnalyticsSourceCoverage,
    pub reference: AnalyticsSourceCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsMeta {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub previous_start: Option<DateTime<Utc>>,
    pub previous_end: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub coverage: AnalyticsCoverage,
    pub data_quality: AnalyticsDataQuality,
}

// ---------------------------------------------------------------------------
// Provider components (CF4_03 `ProviderComponent<T>` and behavioural rows)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderComponent<T> {
    pub status: ComponentStatus,
    pub diagnostic: Option<AnalyticsDiagnostic>,
    pub data: Option<T>,
}

impl<T> ProviderComponent<T> {
    pub fn unavailable(diagnostic: AnalyticsDiagnostic) -> Self {
        Self {
            status: ComponentStatus::Unavailable,
            diagnostic: Some(diagnostic),
            data: None,
        }
    }

    pub fn not_requested() -> Self {
        Self {
            status: ComponentStatus::NotRequested,
            diagnostic: None,
            data: None,
        }
    }

    pub fn available(data: T) -> Self {
        Self {
            status: ComponentStatus::Available,
            diagnostic: None,
            data: Some(data),
        }
    }

    pub fn data(&self) -> Option<&T> {
        match self.status {
            ComponentStatus::Unavailable | ComponentStatus::NotRequested => None,
            ComponentStatus::Available | ComponentStatus::Partial => self.data.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreMetrics {
    pub visitors: Option<u64>,
    pub visits: Option<u64>,
    pub pageviews: Option<u64>,
    pub total_time_seconds: Option<u64>,
    pub average_session_duration_seconds: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyVisitorRow {
    pub date: NaiveDate,
    pub visitors: AggregateCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedEventAggregate {
    pub event: String,
    pub count: AggregateCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedPropertyAggregate {
    pub event: String,
    pub property: String,
    pub value: String,
    pub count: AggregateCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyAvailability {
    pub event: String,
    pub property: String,
    pub result: ProviderComponent<Vec<AllowedPropertyAggregate>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehavioralWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub availability: AnalyticsSourceStatus,
    pub diagnostic: Option<AnalyticsDiagnostic>,
    pub coverage: AnalyticsSourceCoverage,
    pub core: ProviderComponent<CoreMetrics>,
    pub event_totals: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub daily_visitors: ProviderComponent<Vec<DailyVisitorRow>>,
    pub event_series: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub top_entries: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub top_referrers: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub property_results: Vec<PropertyAvailability>,
}

impl BehavioralWindow {
    /// A minimal available window with no rows; tests add only what they assert.
    pub fn empty(start: DateTime<Utc>, end: DateTime<Utc>, observed_at: DateTime<Utc>) -> Self {
        Self {
            start,
            end,
            observed_at,
            availability: AnalyticsSourceStatus::Available,
            diagnostic: None,
            coverage: AnalyticsSourceCoverage {
                coverage_start: Some(start),
                coverage_end: Some(end),
                observed_at: Some(observed_at),
                status: AnalyticsSourceStatus::Available,
                diagnostic: None,
            },
            core: ProviderComponent::available(CoreMetrics::default()),
            event_totals: ProviderComponent::available(Vec::new()),
            daily_visitors: ProviderComponent::available(Vec::new()),
            event_series: ProviderComponent::available(Vec::new()),
            top_entries: ProviderComponent::available(Vec::new()),
            top_referrers: ProviderComponent::available(Vec::new()),
            property_results: Vec::new(),
        }
    }

    pub fn with_events(mut self, totals: &[(&str, u64)]) -> Self {
        self.event_totals = ProviderComponent::available(
            totals
                .iter()
                .map(|(event, count)| AllowedEventAggregate {
                    event: (*event).to_owned(),
                    count: AggregateCell {
                        key: (*event).to_owned(),
                        count: Some(*count),
                        suppressed: false,
                    },
                })
                .collect(),
        );
        self
    }

    pub fn with_daily_visitors(mut self, rows: &[(NaiveDate, u64)]) -> Self {
        self.daily_visitors = ProviderComponent::available(
            rows.iter()
                .map(|(date, visitors)| DailyVisitorRow {
                    date: *date,
                    visitors: AggregateCell {
                        key: date.to_string(),
                        count: Some(*visitors),
                        suppressed: false,
                    },
                })
                .collect(),
        );
        self
    }

    pub fn with_property(mut self, event: &str, property: &str, values: &[(&str, u64)]) -> Self {
        self.property_results.push(PropertyAvailability {
            event: event.to_owned(),
            property: property.to_owned(),
            result: ProviderComponent::available(
                values
                    .iter()
                    .map(|(value, count)| AllowedPropertyAggregate {
                        event: event.to_owned(),
                        property: property.to_owned(),
                        value: (*value).to_owned(),
                        count: AggregateCell {
                            key: (*value).to_owned(),
                            count: Some(*count),
                            suppressed: false,
                        },
                    })
                    .collect(),
            ),
        });
        self
    }
}

// ---------------------------------------------------------------------------
// Task 15 primitives (declared by CF4_03)
// ---------------------------------------------------------------------------

/// CF4_03: positive count or entity basis 1 or 2 suppresses; zero is genuine.
pub fn suppress(count: Option<u64>, entities: Option<u64>) -> AggregateCell {
    AggregateCell {
        key: String::new(),
        count: match count {
            None => None,
            Some(value) => {
                let basis = entities.unwrap_or(value);
                if (1..3).contains(&basis) {
                    None
                } else {
                    Some(value)
                }
            }
        },
        suppressed: match count {
            None => false,
            Some(value) => (1..3).contains(&entities.unwrap_or(value)),
        },
    }
}

// ---------------------------------------------------------------------------
// Response schemas used by this lane (CF4_03 `source_response_types`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverviewResponse {
    pub meta: AnalyticsMeta,
    pub kpis: OverviewKpis,
    pub lifecycle: Vec<LifecycleStage>,
    pub activity_trend: ActivityTrend,
    pub marketplace_balance: MarketplaceBalance,
    pub needs_attention: Vec<NeedsAttentionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewKpis {
    pub qualified_organizations: MetricValue,
    pub active_members: MetricValue,
    pub participating_organizations: MetricValue,
    pub live_orders: MetricValue,
    pub confirmed_trades: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleStage {
    pub key: LifecycleStageKey,
    pub count: Option<u64>,
    pub previous: Option<u64>,
    pub coverage: AnalyticsSourceStatus,
    pub detail_tab: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityTrend {
    pub visitors: Vec<SeriesPoint>,
    pub active_members: Vec<SeriesPoint>,
    pub orders: Vec<SeriesPoint>,
    pub confirmed_trades: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceBalance {
    pub buyer_organizations: MetricValue,
    pub supplier_organizations: MetricValue,
    pub bid_orders: MetricValue,
    pub ask_orders: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsAttentionItem {
    pub rule: NeedsAttentionRule,
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivationResponse {
    pub meta: AnalyticsMeta,
    pub journey: Vec<JourneyStage>,
    pub time_to_first_login: DurationDistribution,
    pub time_to_first_live_order: DurationDistribution,
    pub drop_off: Vec<AggregateCell>,
    pub ratios: Vec<LabeledRatio>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JourneyStage {
    pub key: String,
    pub source: JourneySource,
    pub total: AggregateCell,
    pub buyer: Option<AggregateCell>,
    pub supplier: Option<AggregateCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationDistribution {
    pub buckets: Vec<DurationBucket>,
    pub median_hours: Option<Decimal>,
    pub sample_size: Option<u64>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationBucket {
    pub bucket: String,
    pub cell: AggregateCell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabeledRatio {
    pub key: String,
    pub kind: ConversionRatioKind,
    pub ratio: RatioValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngagementResponse {
    pub meta: AnalyticsMeta,
    pub kpis: EngagementKpis,
    pub active_members_trend: Vec<SeriesPoint>,
    pub feature_adoption: Vec<FeatureAdoptionRow>,
    pub workflow_ratios: Vec<LabeledRatio>,
    pub navigation_destinations: Vec<NavigationDestinationRow>,
    pub tutorial_steps: Vec<TutorialStepRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngagementKpis {
    pub dau: MetricValue,
    pub wau: MetricValue,
    pub mau: MetricValue,
    pub stickiness_pct: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureAdoptionRow {
    pub family: String,
    pub events: Option<u64>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationDestinationRow {
    pub destination: NavigationDestination,
    pub total: AggregateCell,
    pub buyer: Option<AggregateCell>,
    pub supplier: Option<AggregateCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TutorialStepRow {
    pub step: String,
    pub completed: AggregateCell,
    pub skipped: AggregateCell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetentionResponse {
    pub meta: AnalyticsMeta,
    pub kpis: RetentionKpis,
    pub member_cohorts: Vec<CohortRow>,
    pub organization_cohorts: Vec<CohortRow>,
    pub repeat_participation: Vec<AggregateCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionKpis {
    pub returning_members: MetricValue,
    pub retained_organizations: MetricValue,
    pub reactivated_organizations: MetricValue,
    pub dormant_approved_members: MetricValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CohortRow {
    pub cohort_start: NaiveDate,
    pub size: AggregateCell,
    pub cells: Vec<CohortCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CohortCell {
    pub offset: i64,
    pub cell: AggregateCell,
    pub pct: Option<Decimal>,
}
