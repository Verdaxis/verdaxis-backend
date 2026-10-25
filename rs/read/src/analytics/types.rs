//! Shared Product Analytics response types.
//!
//! These are the source response contracts from
//! `app/schemas/product_analytics.py`, translated to typed Rust. The rules
//! they encode are the client contract, not a byte format:
//!
//! - a period is half-open in UTC and its comparison period has the same
//!   length and ends exactly at the period start;
//! - `null` marks an unavailable or mathematically invalid value, while `0`
//!   is a genuine zero;
//! - a suppressed cell carries no count (`count = None`,
//!   `suppressed = true`) and derived values render only when every input is
//!   unsuppressed;
//! - authoritative, behavioral, login-history, status-history and reference
//!   coverage stay separate; a reference price is never market activity.
//!
//! Decimal values are emitted as strings without an exponent, which is what
//! the frontend already converts with `Number()`; `rust_decimal`'s optional
//! serde feature is not enabled in this workspace.
//!
//! The behavioral provider envelope is here too: its component type must be
//! shared with the adapter and the tab consumers, while the adapter owns the
//! request itself.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// JSON text for an optional `Decimal`, as a string, matching Python's
/// serialization of money, rates and durations.
mod decimal_wire {
    use std::fmt;
    use std::str::FromStr;

    use rust_decimal::Decimal;
    use serde::de::{self, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<Decimal>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_str(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DecimalVisitor;

        fn parse<E: de::Error>(value: &str) -> Result<Option<Decimal>, E> {
            Decimal::from_str(value)
                .or_else(|_| Decimal::from_scientific(value))
                .map(Some)
                .map_err(E::custom)
        }

        impl<'de> Visitor<'de> for DecimalVisitor {
            type Value = Option<Decimal>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a decimal string or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                deserializer.deserialize_str(self)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                parse(value)
            }
        }

        deserializer.deserialize_option(DecimalVisitor)
    }
}

// ---------------------------------------------------------------------------
// Query enums and period metadata
// ---------------------------------------------------------------------------

/// Audience filter accepted by every product analytics query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnalyticsAudience {
    All,
    Buyer,
    Supplier,
}

/// Provenance activity filter. `All` composes the three stored classes
/// separately and never adds reference coverage to market activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnalyticsActivity {
    Live,
    Demo,
    Reference,
    All,
}

/// Why an optional provider or derived source has no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsDiagnostic {
    Disabled,
    Configuration,
    Authentication,
    Timeout,
    Upstream,
    MalformedResponse,
    InsufficientCoverage,
}

/// Data-quality counters carried by every product analytics response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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

/// Availability of one analytics source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsSourceStatus {
    Available,
    Partial,
    Unavailable,
    NotApplicable,
}

/// Coverage bounds and diagnostic for one source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsSourceCoverage {
    #[serde(default)]
    pub coverage_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub coverage_end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    pub status: AnalyticsSourceStatus,
    #[serde(default)]
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

    pub fn available() -> Self {
        Self {
            status: AnalyticsSourceStatus::Available,
            ..Self::not_applicable()
        }
    }

    pub fn unavailable(diagnostic: AnalyticsDiagnostic) -> Self {
        Self {
            status: AnalyticsSourceStatus::Unavailable,
            diagnostic: Some(diagnostic),
            ..Self::not_applicable()
        }
    }
}

/// One coverage entry per independently sourced analytics family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsCoverage {
    pub authoritative: AnalyticsSourceCoverage,
    pub behavioral: AnalyticsSourceCoverage,
    pub login_history: AnalyticsSourceCoverage,
    pub status_history: AnalyticsSourceCoverage,
    pub reference: AnalyticsSourceCoverage,
}

/// Period, observation instant, coverage and data quality for a response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalyticsMeta {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub previous_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub previous_end: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub coverage: AnalyticsCoverage,
    pub data_quality: AnalyticsDataQuality,
}

// ---------------------------------------------------------------------------
// Count, ratio, series and ranking primitives
// ---------------------------------------------------------------------------

/// A count cell that can be privacy-suppressed.
///
/// A suppressed cell has `count = None` and `suppressed = true`; an
/// unavailable-but-unsuppressed cell is `count = None`, `suppressed = false`.
/// `Some(0)` is always a genuine zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateCell {
    pub key: String,
    #[serde(default)]
    pub count: Option<u64>,
    pub suppressed: bool,
}

impl AggregateCell {
    /// Label an unkeyed privacy decision with its breakdown key.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }
}

/// Headline integer metric with previous-period comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricValue {
    #[serde(default)]
    pub value: Option<u64>,
    #[serde(default)]
    pub previous: Option<u64>,
    pub suppressed: bool,
}

/// Headline decimal metric (money, volume, duration) with comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecimalMetricValue {
    #[serde(default, with = "decimal_wire")]
    pub value: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub previous: Option<Decimal>,
    pub suppressed: bool,
}

/// A ratio published with its exact components.
///
/// `rate_pct` is present only when both components are present, unsuppressed
/// and the denominator is positive. `cohort_complete = false` marks a
/// right-censored cohort that can still change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RatioValue {
    #[serde(default)]
    pub numerator: Option<u64>,
    #[serde(default)]
    pub denominator: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub rate_pct: Option<Decimal>,
    pub suppressed: bool,
    pub cohort_complete: bool,
}

/// One dense UTC calendar-day integer point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub date: NaiveDate,
    #[serde(default)]
    pub value: Option<u64>,
}

/// One dense UTC calendar-day decimal point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecimalSeriesPoint {
    pub date: NaiveDate,
    #[serde(default, with = "decimal_wire")]
    pub value: Option<Decimal>,
}

/// One bounded ranked row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RankedRow {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub share_pct: Option<Decimal>,
    pub suppressed: bool,
}

// ---------------------------------------------------------------------------
// Closed enums used by tab responses
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteFamily {
    Landing,
    Signup,
    Platform,
    Admin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontendErrorCategory {
    Render,
    Chunk,
    Network,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NavigationDestination {
    Home,
    Map,
    Marketplace,
    Curve,
    Watchlist,
    Analytics,
    Trades,
    Quotes,
    Compliance,
    Training,
    Settings,
    Admin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationLatencyBucket {
    #[serde(rename = "lt250")]
    Lt250,
    #[serde(rename = "250_500")]
    From250To500,
    #[serde(rename = "500_1000")]
    From500To1000,
    #[serde(rename = "1000_2500")]
    From1000To2500,
    #[serde(rename = "gte2500")]
    Gte2500,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleStageKey {
    Visitors,
    Registered,
    Active,
    Participating,
    Trading,
    Retained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsAttentionRule {
    ApprovedMembersNeverLoggedIn,
    SignupSubmissionDrop,
    OneSidedLiveMarket,
    ElevatedLoginFailures,
    DegradedAnalyticsCollection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JourneySource {
    Behavioral,
    Authoritative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionRatioKind {
    UserCohort,
    OrganizationCohort,
    AggregateEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConcentrationBand {
    Low,
    Moderate,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReferenceSourceLabel {
    ManualOverride,
    SeedMatrix,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReferenceSourceKind {
    AdminBenchmark,
    SeededBenchmark,
    TradeDerivedVwap,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReferenceScope {
    ExactSlice,
    WindowAdjusted,
    Other,
}

/// Reference rows are labelled reference coverage, never market activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceCoverageStatus {
    Current,
    Stale,
    Unavailable,
}

/// Navigation latency rows accept either one destination or the `all` rollup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatencyDestination {
    All,
    Destination(NavigationDestination),
}

impl Serialize for LatencyDestination {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            LatencyDestination::All => serializer.serialize_str("all"),
            LatencyDestination::Destination(destination) => destination.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for LatencyDestination {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let destination = match value.as_str() {
            "all" => return Ok(LatencyDestination::All),
            "home" => NavigationDestination::Home,
            "map" => NavigationDestination::Map,
            "marketplace" => NavigationDestination::Marketplace,
            "curve" => NavigationDestination::Curve,
            "watchlist" => NavigationDestination::Watchlist,
            "analytics" => NavigationDestination::Analytics,
            "trades" => NavigationDestination::Trades,
            "quotes" => NavigationDestination::Quotes,
            "compliance" => NavigationDestination::Compliance,
            "training" => NavigationDestination::Training,
            "settings" => NavigationDestination::Settings,
            "admin" => NavigationDestination::Admin,
            other => {
                return Err(<D::Error as serde::de::Error>::unknown_variant(
                    other,
                    &[
                        "all",
                        "home",
                        "map",
                        "marketplace",
                        "curve",
                        "watchlist",
                        "analytics",
                        "trades",
                        "quotes",
                        "compliance",
                        "training",
                        "settings",
                        "admin",
                    ],
                ));
            }
        };
        Ok(LatencyDestination::Destination(destination))
    }
}

// ---------------------------------------------------------------------------
// Provider component envelope (behavioral data)
// ---------------------------------------------------------------------------

/// Availability of one provider-backed component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Available,
    Partial,
    Unavailable,
    NotRequested,
}

/// A provider-backed component that is either present or explicitly absent.
///
/// `Unavailable` and `NotRequested` never carry data, and the fields are
/// private so no caller can build the forbidden combination: absence is
/// constructed, never assigned field by field. An available empty collection
/// is a genuine empty response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderComponent<T> {
    status: ComponentStatus,
    #[serde(default)]
    diagnostic: Option<AnalyticsDiagnostic>,
    // A missing `Option` field deserializes as `None`; `#[serde(default)]`
    // here would make the derive demand `T: Default` instead.
    data: Option<T>,
}

impl<T> ProviderComponent<T> {
    pub fn available(data: T) -> Self {
        Self {
            status: ComponentStatus::Available,
            diagnostic: None,
            data: Some(data),
        }
    }

    pub fn partial(data: Option<T>, diagnostic: Option<AnalyticsDiagnostic>) -> Self {
        Self {
            status: ComponentStatus::Partial,
            diagnostic,
            data,
        }
    }

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

    pub fn status(&self) -> ComponentStatus {
        self.status
    }

    pub fn diagnostic(&self) -> Option<AnalyticsDiagnostic> {
        self.diagnostic
    }

    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    pub fn into_data(self) -> Option<T> {
        self.data
    }

    /// Whether this component obeys the absence rule.
    ///
    /// The constructors cannot produce a violation; this catches one that
    /// arrived as JSON, where a caller could pair `unavailable` with data.
    pub fn is_coherent(&self) -> bool {
        match self.status {
            ComponentStatus::Unavailable | ComponentStatus::NotRequested => self.data.is_none(),
            ComponentStatus::Available => self.diagnostic.is_none(),
            ComponentStatus::Partial => true,
        }
    }
}

/// Core provider aggregate. A session average exists only when visits are
/// positive and both inputs are available.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoreMetrics {
    #[serde(default)]
    pub visitors: Option<u64>,
    #[serde(default)]
    pub visits: Option<u64>,
    #[serde(default)]
    pub pageviews: Option<u64>,
    #[serde(default)]
    pub total_time_seconds: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub average_session_duration_seconds: Option<Decimal>,
}

/// One dense UTC daily visitor point from the behavioral provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailyVisitorRow {
    pub date: NaiveDate,
    pub visitors: AggregateCell,
}

/// Closed entry-page domain; an unknown path maps to `Other` and the raw path
/// is never exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryPage {
    Landing,
    Signup,
    Login,
    Pilot,
    Calculator,
    Platform,
    Admin,
    Other,
}

/// Closed referrer domain; the host is parsed internally and never exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferrerCategory {
    Direct,
    Search,
    Social,
    Referral,
    Other,
}

/// Whole-window count for one registered event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AllowedEventAggregate {
    pub event: String,
    pub count: AggregateCell,
}

/// Count for one requested event property value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AllowedPropertyAggregate {
    pub event: String,
    pub property: String,
    pub value: String,
    pub count: AggregateCell,
}

/// One dense UTC daily event point from the behavioral provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailyEventRow {
    pub date: NaiveDate,
    pub event: String,
    pub count: AggregateCell,
}

/// One bounded ranked entry page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntryRankingRow {
    pub entry: EntryPage,
    pub count: AggregateCell,
    pub rank: u8,
}

/// One bounded ranked referrer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferrerRankingRow {
    pub referrer: ReferrerCategory,
    pub count: AggregateCell,
    pub rank: u8,
}

/// The result of one requested `(event, property)` pair.
///
/// Every requested pair has exactly one status and is answered separately: an
/// event-endpoint failure marks every requested property of that event
/// unavailable, and a missing value is never an inferred zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropertyAvailability {
    pub event: String,
    pub property: String,
    pub result: ProviderComponent<Vec<AllowedPropertyAggregate>>,
}

/// Overall availability of one behavioral window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehavioralAvailability {
    Available,
    Partial,
    Unavailable,
}

/// One typed behavioral window.
///
/// Every component is present so a consumer can tell "not requested" from
/// "unavailable" from a genuine empty response. A component never carries a
/// fabricated zero, and no raw provider body is retained.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BehavioralWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub availability: BehavioralAvailability,
    #[serde(default)]
    pub diagnostic: Option<AnalyticsDiagnostic>,
    pub coverage: AnalyticsSourceCoverage,
    pub core: ProviderComponent<CoreMetrics>,
    pub event_totals: ProviderComponent<Vec<AllowedEventAggregate>>,
    pub daily_visitors: ProviderComponent<Vec<DailyVisitorRow>>,
    pub event_series: ProviderComponent<Vec<DailyEventRow>>,
    pub top_entries: ProviderComponent<Vec<EntryRankingRow>>,
    pub top_referrers: ProviderComponent<Vec<ReferrerRankingRow>>,
    pub property_results: Vec<PropertyAvailability>,
}

impl BehavioralWindow {
    /// A window whose provider was never reached. Every component is
    /// `Unavailable` with the same diagnostic, so no consumer can mistake an
    /// absent provider for an empty result.
    pub fn unavailable(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        diagnostic: AnalyticsDiagnostic,
    ) -> Self {
        Self {
            start,
            end,
            observed_at,
            availability: BehavioralAvailability::Unavailable,
            diagnostic: Some(diagnostic),
            coverage: AnalyticsSourceCoverage::unavailable(diagnostic),
            core: ProviderComponent::unavailable(diagnostic),
            event_totals: ProviderComponent::unavailable(diagnostic),
            daily_visitors: ProviderComponent::unavailable(diagnostic),
            event_series: ProviderComponent::unavailable(diagnostic),
            top_entries: ProviderComponent::unavailable(diagnostic),
            top_referrers: ProviderComponent::unavailable(diagnostic),
            property_results: Vec::new(),
        }
    }

    /// Every component this window carries, in response order.
    pub fn components(&self) -> [ComponentStatus; 6] {
        [
            self.core.status(),
            self.event_totals.status(),
            self.daily_visitors.status(),
            self.event_series.status(),
            self.top_entries.status(),
            self.top_referrers.status(),
        ]
    }

    /// Check that the window status and its components agree.
    ///
    /// An unavailable provider marks every component unavailable; an available
    /// window contains none; a partial window has at least one incomplete
    /// component. A component never carries data while unavailable or not
    /// requested, so an absent provider can never be read as an empty result.
    pub fn validate(&self) -> Result<(), BehavioralWindowError> {
        let components = [
            self.core.is_coherent(),
            self.event_totals.is_coherent(),
            self.daily_visitors.is_coherent(),
            self.event_series.is_coherent(),
            self.top_entries.is_coherent(),
            self.top_referrers.is_coherent(),
        ];
        if components.iter().any(|coherent| !coherent) {
            return Err(BehavioralWindowError::IncoherentComponent);
        }
        let statuses = self.components();
        let core_unavailable = statuses
            .iter()
            .filter(|status| **status == ComponentStatus::Unavailable)
            .count();
        let core_incomplete = statuses
            .iter()
            .filter(|status| **status == ComponentStatus::Partial)
            .count();
        // Per-property components follow the same rule, counted separately:
        // the unavailable rule compares each family against its own length,
        // and a partial property may accompany available core components.
        let mut property_unavailable = 0;
        let mut property_incomplete = 0;
        for property in &self.property_results {
            match property.result.status() {
                ComponentStatus::Unavailable => property_unavailable += 1,
                ComponentStatus::Partial => property_incomplete += 1,
                ComponentStatus::Available => {}
                // A requested pair is either available or unavailable; a
                // not-requested property result contradicts the request.
                ComponentStatus::NotRequested => {
                    return Err(BehavioralWindowError::IncoherentAvailability)
                }
            }
        }
        let properties = self.property_results.len();
        let consistent = match self.availability {
            BehavioralAvailability::Unavailable => {
                core_unavailable == statuses.len()
                    && property_unavailable == properties
                    && self.diagnostic.is_some()
            }
            BehavioralAvailability::Available => {
                core_unavailable == 0 && property_unavailable == 0
            }
            BehavioralAvailability::Partial => {
                core_unavailable + core_incomplete > 0
                    || property_unavailable + property_incomplete > 0
            }
        };
        if consistent {
            Ok(())
        } else {
            Err(BehavioralWindowError::IncoherentAvailability)
        }
    }
}

/// Why a behavioral window contradicts itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BehavioralWindowError {
    /// A component carries data while unavailable or not requested, or
    /// reports a diagnostic while available.
    IncoherentComponent,
    /// The window availability contradicts its components.
    IncoherentAvailability,
}

// ---------------------------------------------------------------------------
// Overview (P05)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LifecycleStage {
    pub key: LifecycleStageKey,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub previous: Option<u64>,
    pub coverage: AnalyticsSourceStatus,
    pub detail_tab: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivityTrend {
    pub visitors: Vec<SeriesPoint>,
    pub active_members: Vec<SeriesPoint>,
    pub orders: Vec<SeriesPoint>,
    pub confirmed_trades: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketplaceBalance {
    pub buyer_organizations: MetricValue,
    pub supplier_organizations: MetricValue,
    pub bid_orders: MetricValue,
    pub ask_orders: MetricValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NeedsAttentionItem {
    pub rule: NeedsAttentionRule,
    #[serde(default)]
    pub count: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewKpis {
    pub qualified_organizations: MetricValue,
    pub active_members: MetricValue,
    pub participating_organizations: MetricValue,
    pub live_orders: MetricValue,
    pub confirmed_trades: MetricValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverviewResponse {
    pub meta: AnalyticsMeta,
    pub kpis: OverviewKpis,
    pub lifecycle: Vec<LifecycleStage>,
    pub activity_trend: ActivityTrend,
    pub marketplace_balance: MarketplaceBalance,
    pub needs_attention: Vec<NeedsAttentionItem>,
}

// ---------------------------------------------------------------------------
// Acquisition (P06)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionKpis {
    pub visitors: MetricValue,
    pub visits: MetricValue,
    pub pageviews: MetricValue,
    pub average_session_duration_seconds: DecimalMetricValue,
    pub cta_clicks: MetricValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CtaMatrixRow {
    pub cta: String,
    pub placement: String,
    #[serde(default)]
    pub clicks: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub share_pct: Option<Decimal>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalculatorFunnel {
    pub starts: MetricValue,
    pub completions: MetricValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionResponse {
    pub meta: AnalyticsMeta,
    pub kpis: AcquisitionKpis,
    pub visitors_trend: Vec<SeriesPoint>,
    pub previous_visitors_trend: Vec<SeriesPoint>,
    pub visits_trend: Vec<SeriesPoint>,
    pub referrers: Vec<RankedRow>,
    pub entry_pages: Vec<RankedRow>,
    pub cta_matrix: Vec<CtaMatrixRow>,
    pub languages: Vec<RankedRow>,
    pub calculator: CalculatorFunnel,
}

// ---------------------------------------------------------------------------
// Activation (P07)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JourneyStage {
    pub key: String,
    pub source: JourneySource,
    pub total: AggregateCell,
    #[serde(default)]
    pub buyer: Option<AggregateCell>,
    #[serde(default)]
    pub supplier: Option<AggregateCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurationBucket {
    pub bucket: String,
    pub cell: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurationDistribution {
    pub buckets: Vec<DurationBucket>,
    #[serde(default, with = "decimal_wire")]
    pub median_hours: Option<Decimal>,
    #[serde(default)]
    pub sample_size: Option<u64>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabeledRatio {
    pub key: String,
    pub kind: ConversionRatioKind,
    pub ratio: RatioValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivationResponse {
    pub meta: AnalyticsMeta,
    pub journey: Vec<JourneyStage>,
    pub time_to_first_login: DurationDistribution,
    pub time_to_first_live_order: DurationDistribution,
    pub drop_off: Vec<AggregateCell>,
    pub ratios: Vec<LabeledRatio>,
}

// ---------------------------------------------------------------------------
// Engagement (P08)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngagementKpis {
    pub dau: MetricValue,
    pub wau: MetricValue,
    pub mau: MetricValue,
    #[serde(default, with = "decimal_wire")]
    pub stickiness_pct: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureAdoptionRow {
    pub family: String,
    #[serde(default)]
    pub events: Option<u64>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationDestinationRow {
    pub destination: NavigationDestination,
    pub total: AggregateCell,
    #[serde(default)]
    pub buyer: Option<AggregateCell>,
    #[serde(default)]
    pub supplier: Option<AggregateCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TutorialStepRow {
    pub step: String,
    pub completed: AggregateCell,
    pub skipped: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngagementResponse {
    pub meta: AnalyticsMeta,
    pub kpis: EngagementKpis,
    pub active_members_trend: Vec<SeriesPoint>,
    pub feature_adoption: Vec<FeatureAdoptionRow>,
    pub workflow_ratios: Vec<LabeledRatio>,
    pub navigation_destinations: Vec<NavigationDestinationRow>,
    pub tutorial_steps: Vec<TutorialStepRow>,
}

// ---------------------------------------------------------------------------
// Marketplace (P09)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketplaceKpis {
    pub participating_organizations: MetricValue,
    pub open_bids: MetricValue,
    pub open_asks: MetricValue,
    pub confirmed_trades: MetricValue,
    pub confirmed_volume_mt: DecimalMetricValue,
    pub execution_rate: RatioValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SliceLiquidityRow {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub availability_window: String,
    pub availability_window_label: String,
    #[serde(default)]
    pub contributing_organizations: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub best_bid_usd_per_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub best_ask_usd_per_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub spread_usd_per_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub spread_bps: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub best_bid_depth_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub best_ask_depth_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub one_percent_bid_depth_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub one_percent_ask_depth_mt: Option<Decimal>,
    #[serde(default)]
    pub crossed: Option<bool>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquiditySummary {
    #[serde(default)]
    pub two_sided_slices: Option<u64>,
    #[serde(default)]
    pub one_sided_slices: Option<u64>,
    #[serde(default)]
    pub crossed_slices: Option<u64>,
    #[serde(default, with = "decimal_wire")]
    pub median_spread_usd_per_mt: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub median_spread_bps: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub median_open_order_age_hours: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub median_hours_to_first_fill: Option<Decimal>,
    pub slices: Vec<SliceLiquidityRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketBalanceTrend {
    pub buyer_organizations: Vec<SeriesPoint>,
    pub supplier_organizations: Vec<SeriesPoint>,
    pub bids: Vec<SeriesPoint>,
    pub asks: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProductPortCell {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub orders: AggregateCell,
    pub organizations: AggregateCell,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrganizationConcentration {
    #[serde(default)]
    pub hhi_band: Option<ConcentrationBand>,
    pub suppressed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommercialSummary {
    pub realized_gmv_usd: DecimalMetricValue,
    pub realized_revenue_usd: DecimalMetricValue,
    #[serde(default, with = "decimal_wire")]
    pub commission_pending_usd: Option<Decimal>,
    #[serde(default, with = "decimal_wire")]
    pub commission_invoiced_usd: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketActivitySection {
    pub kpis: MarketplaceKpis,
    pub liquidity: LiquiditySummary,
    pub balance_trend: MarketBalanceTrend,
    pub product_port_matrix: Vec<ProductPortCell>,
    pub window_distribution: Vec<RankedRow>,
    pub order_status_distribution: Vec<AggregateCell>,
    pub trade_status_distribution: Vec<AggregateCell>,
    pub concentration: OrganizationConcentration,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceCoverageRow {
    pub product_key: String,
    pub product_label: String,
    pub delivery_point_key: String,
    pub delivery_point_label: String,
    pub availability_window: String,
    pub availability_window_label: String,
    #[serde(default, with = "decimal_wire")]
    pub benchmark_price_usd_per_mt: Option<Decimal>,
    pub source_label: ReferenceSourceLabel,
    pub generated_at: DateTime<Utc>,
    #[serde(default)]
    pub observed_at: Option<DateTime<Utc>>,
    pub source_kind: ReferenceSourceKind,
    pub scope: ReferenceScope,
    pub coverage_status: ReferenceCoverageStatus,
}

/// Why a reference coverage row is inconsistent with its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceCoverageError {
    /// `current` without both a benchmark price and an observation instant.
    CurrentWithoutEvidence,
    /// `unavailable` that still exposes a price or an observation instant.
    UnavailableWithEvidence,
}

impl ReferenceCoverageRow {
    /// Check the coverage-status invariants.
    ///
    /// `current` requires both a price and an observation instant; `stale` may
    /// keep a price without a fresh observation; `unavailable` exposes
    /// neither. A reference price is coverage, never market activity.
    pub fn validate(&self) -> Result<(), ReferenceCoverageError> {
        match self.coverage_status {
            ReferenceCoverageStatus::Current => {
                if self.benchmark_price_usd_per_mt.is_none() || self.observed_at.is_none() {
                    return Err(ReferenceCoverageError::CurrentWithoutEvidence);
                }
            }
            ReferenceCoverageStatus::Unavailable => {
                if self.benchmark_price_usd_per_mt.is_some() || self.observed_at.is_some() {
                    return Err(ReferenceCoverageError::UnavailableWithEvidence);
                }
            }
            ReferenceCoverageStatus::Stale => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceCoverageSection {
    pub rows: Vec<ReferenceCoverageRow>,
}

/// Marketplace sections are separate per provenance; reference coverage is
/// never folded into one of the activity sections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketplaceResponse {
    pub meta: AnalyticsMeta,
    #[serde(default)]
    pub live: Option<MarketActivitySection>,
    #[serde(default)]
    pub demo: Option<MarketActivitySection>,
    #[serde(default)]
    pub unknown: Option<MarketActivitySection>,
    #[serde(default)]
    pub reference: Option<ReferenceCoverageSection>,
    #[serde(default)]
    pub commercial: Option<CommercialSummary>,
}

// ---------------------------------------------------------------------------
// Retention (P10)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CohortCell {
    pub offset: u32,
    pub cell: AggregateCell,
    #[serde(default, with = "decimal_wire")]
    pub pct: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CohortRow {
    pub cohort_start: NaiveDate,
    pub size: AggregateCell,
    pub cells: Vec<CohortCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetentionKpis {
    pub returning_members: MetricValue,
    pub retained_organizations: MetricValue,
    pub reactivated_organizations: MetricValue,
    pub dormant_approved_members: MetricValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetentionResponse {
    pub meta: AnalyticsMeta,
    pub kpis: RetentionKpis,
    pub member_cohorts: Vec<CohortRow>,
    pub organization_cohorts: Vec<CohortRow>,
    pub repeat_participation: Vec<AggregateCell>,
}

// ---------------------------------------------------------------------------
// Reliability (P11)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollectorState {
    pub status: AnalyticsSourceStatus,
    #[serde(default)]
    pub diagnostic: Option<AnalyticsDiagnostic>,
    #[serde(default)]
    pub last_observation_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginFailurePanel {
    pub total: MetricValue,
    pub categories: Vec<AggregateCell>,
    pub trend: Vec<SeriesPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrontendErrorPanel {
    pub total: MetricValue,
    pub by_route_family: Vec<AggregateCell>,
    pub by_category: Vec<AggregateCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackendUnavailablePanel {
    pub total: MetricValue,
    pub by_route_family: Vec<AggregateCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavigationLatencyRow {
    pub destination: LatencyDestination,
    pub buckets: Vec<AggregateCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditActivityRow {
    pub occurred_at: DateTime<Utc>,
    pub action: String,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub actor_role: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReliabilityResponse {
    pub meta: AnalyticsMeta,
    pub collector: CollectorState,
    pub login_failures: LoginFailurePanel,
    pub frontend_errors: FrontendErrorPanel,
    pub backend_unavailable: BackendUnavailablePanel,
    pub navigation_latency: Vec<NavigationLatencyRow>,
    pub audit_activity: Vec<AuditActivityRow>,
}
