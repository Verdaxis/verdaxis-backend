//! Privacy suppression and response bounds for Product Analytics.
//!
//! Small-cell suppression applies to every segmented cell and derived value:
//! a positive basis of one or two underlying users, organizations or events
//! is suppressed, while a genuine zero stays visible. An absent or
//! unavailable count is `None` and is never turned into a zero.
//!
//! The bounds are the client contract: at most 366 daily series points, 20
//! ranked rows, 512 matrix cells and 54 cohort rows. They are separate from
//! the 365-day query range, so an inclusive daily series can still cover a
//! full year.

use rust_decimal::{Decimal, RoundingStrategy};

use super::types::{AggregateCell, RatioValue};

/// A basis below this value is suppressed.
pub const SUPPRESSION_THRESHOLD: u64 = 3;

/// Maximum dense daily series points in one response.
pub const MAX_SERIES_POINTS: usize = 366;

/// Maximum ranked rows in one response.
pub const MAX_RANKED_ROWS: usize = 20;

/// Defensive ceiling above any real catalog product × delivery-point matrix.
pub const MAX_MATRIX_CELLS: usize = 512;

/// Maximum weekly cohort rows (53 ISO weeks plus safety).
pub const MAX_COHORT_ROWS: usize = 54;

/// Suppress one count and report whether it was suppressed.
///
/// `entities` is the distinct entity basis; when it is unavailable the count
/// itself is the basis (the event-count rule). A missing count is never
/// suppressed: it is unavailable, not small.
///
/// The returned cell is unkeyed, because the privacy decision does not know
/// the breakdown key. Label it with [`AggregateCell::with_key`], or use
/// [`suppress_cell`] which does both.
pub fn suppress(count: Option<u64>, entities: Option<u64>) -> AggregateCell {
    let (count, suppressed) = suppress_value(count, entities);
    AggregateCell {
        key: String::new(),
        count,
        suppressed,
    }
}

/// The privacy decision alone: the visible count, if any, and whether the
/// value was suppressed.
pub fn suppress_value(count: Option<u64>, entities: Option<u64>) -> (Option<u64>, bool) {
    let Some(count) = count else {
        return (None, false);
    };
    let basis = entities.unwrap_or(count);
    if basis > 0 && basis < SUPPRESSION_THRESHOLD {
        (None, true)
    } else {
        (Some(count), false)
    }
}

/// Build one suppressed-or-visible count cell.
pub fn suppress_cell(
    key: impl Into<String>,
    count: Option<u64>,
    entities: Option<u64>,
) -> AggregateCell {
    suppress(count, entities).with_key(key)
}

/// Build a ratio from its exact components.
///
/// The rate is present only when both components are present and the
/// denominator is positive, rounded half away from zero to two decimals
/// (Python's `ROUND_HALF_UP`). `suppressed` is left false and
/// `cohort_complete` true; the caller sets those, and a suppressed ratio must
/// not publish a rate.
pub fn ratio_from_counts(
    numerator: Option<u64>,
    denominator: Option<u64>,
    cohort_complete: bool,
) -> RatioValue {
    let rate_pct = match (numerator, denominator) {
        (Some(numerator), Some(denominator)) if denominator > 0 => {
            let rate = Decimal::from(numerator) / Decimal::from(denominator) * Decimal::ONE_HUNDRED;
            Some(rate.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero))
        }
        _ => None,
    };
    RatioValue {
        numerator,
        denominator,
        rate_pct,
        suppressed: false,
        cohort_complete,
    }
}

// ---------------------------------------------------------------------------
// Closed event, property and value allowlists
// ---------------------------------------------------------------------------
//
// The analytics taxonomy is closed: an event, property or value outside these
// lists is discarded before aggregation and is never echoed into a response.
// A value that resolves through the pinned product catalog, delivery-point
// catalog or ISO country reference is marked `External`; the closed domains
// are inline normative data, not a claim about historical token validation.
//
// Nothing in this module sends events. Phase 4 adds no sender or scheduler;
// the existing producers keep ownership of delivery.

/// Events the server produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AllowedServerEvent {
    RegistrationCompleted,
    OrganizationCreated,
    OrderCreated,
    TradeCreated,
}

/// Events the browser reporting taxonomy accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AllowedFrontendEvent {
    LandingCtaClicked,
    EnergyCalculatorStarted,
    EnergyCalculatorCompleted,
    PublicLanguageChanged,
    SignupStarted,
    SignupRoleSelected,
    SignupSubmitted,
    SignupOrganizationRequired,
    SignupOrganizationSubmitted,
    LoginSubmitted,
    LoginSucceeded,
    LoginFailed,
    PlatformNavigation,
    MarketSliceSelected,
    ListingOpened,
    OrderFormOpened,
    OrderFormSubmitted,
    TradeConfirmationOpened,
    TutorialStarted,
    TutorialStepCompleted,
    TutorialStepSkipped,
    TutorialCompleted,
    EstimatorOpened,
    EstimatorCompleted,
    FrontendError,
    BackendUnavailable,
    NavigationPerformance,
}

/// One registered event of either taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RegisteredEvent {
    Server(AllowedServerEvent),
    Frontend(AllowedFrontendEvent),
}

impl AllowedServerEvent {
    pub fn name(self) -> &'static str {
        match self {
            AllowedServerEvent::RegistrationCompleted => "registration_completed",
            AllowedServerEvent::OrganizationCreated => "organization_created",
            AllowedServerEvent::OrderCreated => "order_created",
            AllowedServerEvent::TradeCreated => "trade_created",
        }
    }

    pub const fn all() -> [AllowedServerEvent; 4] {
        [
            AllowedServerEvent::RegistrationCompleted,
            AllowedServerEvent::OrganizationCreated,
            AllowedServerEvent::OrderCreated,
            AllowedServerEvent::TradeCreated,
        ]
    }
}

impl AllowedFrontendEvent {
    pub fn name(self) -> &'static str {
        match self {
            AllowedFrontendEvent::LandingCtaClicked => "landing_cta_clicked",
            AllowedFrontendEvent::EnergyCalculatorStarted => "energy_calculator_started",
            AllowedFrontendEvent::EnergyCalculatorCompleted => "energy_calculator_completed",
            AllowedFrontendEvent::PublicLanguageChanged => "public_language_changed",
            AllowedFrontendEvent::SignupStarted => "signup_started",
            AllowedFrontendEvent::SignupRoleSelected => "signup_role_selected",
            AllowedFrontendEvent::SignupSubmitted => "signup_submitted",
            AllowedFrontendEvent::SignupOrganizationRequired => "signup_organization_required",
            AllowedFrontendEvent::SignupOrganizationSubmitted => "signup_organization_submitted",
            AllowedFrontendEvent::LoginSubmitted => "login_submitted",
            AllowedFrontendEvent::LoginSucceeded => "login_succeeded",
            AllowedFrontendEvent::LoginFailed => "login_failed",
            AllowedFrontendEvent::PlatformNavigation => "platform_navigation",
            AllowedFrontendEvent::MarketSliceSelected => "market_slice_selected",
            AllowedFrontendEvent::ListingOpened => "listing_opened",
            AllowedFrontendEvent::OrderFormOpened => "order_form_opened",
            AllowedFrontendEvent::OrderFormSubmitted => "order_form_submitted",
            AllowedFrontendEvent::TradeConfirmationOpened => "trade_confirmation_opened",
            AllowedFrontendEvent::TutorialStarted => "tutorial_started",
            AllowedFrontendEvent::TutorialStepCompleted => "tutorial_step_completed",
            AllowedFrontendEvent::TutorialStepSkipped => "tutorial_step_skipped",
            AllowedFrontendEvent::TutorialCompleted => "tutorial_completed",
            AllowedFrontendEvent::EstimatorOpened => "estimator_opened",
            AllowedFrontendEvent::EstimatorCompleted => "estimator_completed",
            AllowedFrontendEvent::FrontendError => "frontend_error",
            AllowedFrontendEvent::BackendUnavailable => "backend_unavailable",
            AllowedFrontendEvent::NavigationPerformance => "navigation_performance",
        }
    }

    pub const fn all() -> [AllowedFrontendEvent; 27] {
        use AllowedFrontendEvent::*;
        [
            LandingCtaClicked,
            EnergyCalculatorStarted,
            EnergyCalculatorCompleted,
            PublicLanguageChanged,
            SignupStarted,
            SignupRoleSelected,
            SignupSubmitted,
            SignupOrganizationRequired,
            SignupOrganizationSubmitted,
            LoginSubmitted,
            LoginSucceeded,
            LoginFailed,
            PlatformNavigation,
            MarketSliceSelected,
            ListingOpened,
            OrderFormOpened,
            OrderFormSubmitted,
            TradeConfirmationOpened,
            TutorialStarted,
            TutorialStepCompleted,
            TutorialStepSkipped,
            TutorialCompleted,
            EstimatorOpened,
            EstimatorCompleted,
            FrontendError,
            BackendUnavailable,
            NavigationPerformance,
        ]
    }
}

impl RegisteredEvent {
    pub fn name(self) -> &'static str {
        match self {
            RegisteredEvent::Server(event) => event.name(),
            RegisteredEvent::Frontend(event) => event.name(),
        }
    }

    /// Look up a registered event by its stored name. Unknown names are
    /// discarded, never forwarded to a provider.
    pub fn from_name(name: &str) -> Option<Self> {
        AllowedServerEvent::all()
            .into_iter()
            .find(|event| event.name() == name)
            .map(RegisteredEvent::Server)
            .or_else(|| {
                AllowedFrontendEvent::all()
                    .into_iter()
                    .find(|event| event.name() == name)
                    .map(RegisteredEvent::Frontend)
            })
    }

    pub fn is_server_event(self) -> bool {
        matches!(self, RegisteredEvent::Server(_))
    }

    /// Every registered event, server taxonomy first.
    pub fn all() -> Vec<RegisteredEvent> {
        AllowedServerEvent::all()
            .into_iter()
            .map(RegisteredEvent::Server)
            .chain(
                AllowedFrontendEvent::all()
                    .into_iter()
                    .map(RegisteredEvent::Frontend),
            )
            .collect()
    }
}

/// Union of every property accepted for at least one registered event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AllowedProperty {
    Role,
    Side,
    CanonicalProduct,
    DeliveryPoint,
    AvailabilityWindow,
    Cta,
    Placement,
    Language,
    Fuel,
    Port,
    From,
    To,
    EntryPoint,
    OrganizationPath,
    OrganizationType,
    Country,
    ReasonCategory,
    Destination,
    ViewMode,
    Product,
    Window,
    DemoStatus,
    Step,
    RouteFamily,
    Category,
    LatencyBucket,
}

impl AllowedProperty {
    /// The stored property name.
    pub fn name(self) -> &'static str {
        match self {
            AllowedProperty::Role => "role",
            AllowedProperty::Side => "side",
            AllowedProperty::CanonicalProduct => "canonical_product",
            AllowedProperty::DeliveryPoint => "delivery_point",
            AllowedProperty::AvailabilityWindow => "availability_window",
            AllowedProperty::Cta => "cta",
            AllowedProperty::Placement => "placement",
            AllowedProperty::Language => "language",
            AllowedProperty::Fuel => "fuel",
            AllowedProperty::Port => "port",
            AllowedProperty::From => "from",
            AllowedProperty::To => "to",
            AllowedProperty::EntryPoint => "entry_point",
            AllowedProperty::OrganizationPath => "organization_path",
            AllowedProperty::OrganizationType => "organization_type",
            AllowedProperty::Country => "country",
            AllowedProperty::ReasonCategory => "reason_category",
            AllowedProperty::Destination => "destination",
            AllowedProperty::ViewMode => "view_mode",
            AllowedProperty::Product => "product",
            AllowedProperty::Window => "window",
            AllowedProperty::DemoStatus => "demo_status",
            AllowedProperty::Step => "step",
            AllowedProperty::RouteFamily => "route_family",
            AllowedProperty::Category => "category",
            AllowedProperty::LatencyBucket => "latency_bucket",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        const ALL: [AllowedProperty; 26] = [
            AllowedProperty::Role,
            AllowedProperty::Side,
            AllowedProperty::CanonicalProduct,
            AllowedProperty::DeliveryPoint,
            AllowedProperty::AvailabilityWindow,
            AllowedProperty::Cta,
            AllowedProperty::Placement,
            AllowedProperty::Language,
            AllowedProperty::Fuel,
            AllowedProperty::Port,
            AllowedProperty::From,
            AllowedProperty::To,
            AllowedProperty::EntryPoint,
            AllowedProperty::OrganizationPath,
            AllowedProperty::OrganizationType,
            AllowedProperty::Country,
            AllowedProperty::ReasonCategory,
            AllowedProperty::Destination,
            AllowedProperty::ViewMode,
            AllowedProperty::Product,
            AllowedProperty::Window,
            AllowedProperty::DemoStatus,
            AllowedProperty::Step,
            AllowedProperty::RouteFamily,
            AllowedProperty::Category,
            AllowedProperty::LatencyBucket,
        ];
        ALL.into_iter().find(|property| property.name() == name)
    }
}

/// Accepted values for one event property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyValues {
    /// A closed inline list.
    Closed(&'static [&'static str]),
    /// Buyer/supplier tutorial steps: `buyer_1..=buyer_28`,
    /// `supplier_1..=supplier_29`.
    TutorialSteps,
    /// Resolved through the pinned product catalog, delivery-point catalog or
    /// ISO country reference at the route boundary; not inline data.
    External,
}

impl PropertyValues {
    /// Whether this domain accepts the value, or `None` when the value must be
    /// resolved through an external pinned reference.
    pub fn accepts(self, value: &str) -> Option<bool> {
        match self {
            PropertyValues::Closed(values) => Some(values.contains(&value)),
            PropertyValues::TutorialSteps => Some(tutorial_step(value)),
            PropertyValues::External => None,
        }
    }
}

fn tutorial_step(value: &str) -> bool {
    let Some((prefix, number)) = value.split_once('_') else {
        return false;
    };
    let limit = match prefix {
        "buyer" => 28,
        "supplier" => 29,
        _ => return false,
    };
    number
        .parse::<u8>()
        .is_ok_and(|number| (1..=limit).contains(&number))
}

const ROLE: PropertyValues = PropertyValues::Closed(&["BUYER", "SUPPLIER", "ADMIN"]);
const TRADING_ROLE: PropertyValues = PropertyValues::Closed(&["BUYER", "SUPPLIER"]);
const LANGUAGE: PropertyValues = PropertyValues::Closed(&["en", "zh"]);
const SIDE: PropertyValues = PropertyValues::Closed(&["BID", "ASK"]);
const DEMO_STATUS: PropertyValues =
    PropertyValues::Closed(&["LIVE", "DEMO", "REFERENCE", "MIXED", "UNKNOWN"]);
const LANDING_CTA: PropertyValues = PropertyValues::Closed(&[
    "pilot",
    "how_it_works",
    "register",
    "register_interest",
    "sign_in",
]);
const PLACEMENT: PropertyValues = PropertyValues::Closed(&[
    "nav",
    "hero",
    "calculator",
    "landing_bottom",
    "pilot_sidebar",
    "pilot_bottom",
]);
const DESTINATION: PropertyValues = PropertyValues::Closed(&[
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
]);
const ROUTE_FAMILY: PropertyValues =
    PropertyValues::Closed(&["landing", "signup", "platform", "admin"]);
const NONLANDING_ROUTE_FAMILY: PropertyValues =
    PropertyValues::Closed(&["signup", "platform", "admin"]);
const ERROR_CATEGORY: PropertyValues =
    PropertyValues::Closed(&["render", "chunk", "network", "unknown"]);
const LATENCY_BUCKET: PropertyValues =
    PropertyValues::Closed(&["lt250", "250_500", "500_1000", "1000_2500", "gte2500"]);
const LOGIN_FAILURE: PropertyValues = PropertyValues::Closed(&[
    "invalid_credentials",
    "account_state",
    "network",
    "server",
    "unknown",
]);
const ENTRY_POINT: PropertyValues = PropertyValues::Closed(&[
    "referral", "direct", "landing", "signup", "pilot", "invite", "other",
]);
const ORGANIZATION_PATH: PropertyValues =
    PropertyValues::Closed(&["create", "existing", "invitation", "other"]);
const ORGANIZATION_TYPE: PropertyValues =
    PropertyValues::Closed(&["SHIP_OPERATOR", "FUEL_SUPPLIER", "FUEL_TRADER", "OTHER"]);

/// The event-specific property domains. One function owns both the accepted
/// property set and its value domain, so a property can never be accepted for
/// an event while its domain is unrecorded.
fn property_domains(event: RegisteredEvent) -> &'static [(AllowedProperty, PropertyValues)] {
    use AllowedFrontendEvent as Frontend;
    use AllowedProperty as Property;
    use AllowedServerEvent as Server;
    use PropertyValues::External;
    match event {
        RegisteredEvent::Server(Server::RegistrationCompleted)
        | RegisteredEvent::Server(Server::OrganizationCreated) => &[(Property::Role, TRADING_ROLE)],
        RegisteredEvent::Server(Server::OrderCreated)
        | RegisteredEvent::Server(Server::TradeCreated) => &[
            (Property::Role, TRADING_ROLE),
            (Property::Side, SIDE),
            (Property::CanonicalProduct, External),
            (Property::DeliveryPoint, External),
            (Property::AvailabilityWindow, External),
        ],
        RegisteredEvent::Frontend(Frontend::LandingCtaClicked) => &[
            (Property::Cta, LANDING_CTA),
            (Property::Placement, PLACEMENT),
            (Property::Language, LANGUAGE),
        ],
        RegisteredEvent::Frontend(Frontend::EnergyCalculatorStarted) => {
            &[(Property::Language, LANGUAGE)]
        }
        RegisteredEvent::Frontend(Frontend::EnergyCalculatorCompleted) => &[
            (Property::Language, LANGUAGE),
            (Property::Fuel, External),
            (Property::Port, External),
        ],
        RegisteredEvent::Frontend(Frontend::PublicLanguageChanged) => {
            &[(Property::From, LANGUAGE), (Property::To, LANGUAGE)]
        }
        RegisteredEvent::Frontend(Frontend::SignupStarted) => &[
            (Property::EntryPoint, ENTRY_POINT),
            (Property::Language, LANGUAGE),
        ],
        RegisteredEvent::Frontend(Frontend::SignupRoleSelected)
        | RegisteredEvent::Frontend(Frontend::SignupOrganizationRequired) => {
            &[(Property::Role, TRADING_ROLE)]
        }
        RegisteredEvent::Frontend(Frontend::SignupSubmitted) => &[
            (Property::Role, TRADING_ROLE),
            (Property::OrganizationPath, ORGANIZATION_PATH),
        ],
        RegisteredEvent::Frontend(Frontend::SignupOrganizationSubmitted) => &[
            (Property::Role, TRADING_ROLE),
            (Property::OrganizationType, ORGANIZATION_TYPE),
            (Property::Country, External),
        ],
        RegisteredEvent::Frontend(Frontend::LoginSubmitted)
        | RegisteredEvent::Frontend(Frontend::EstimatorOpened) => &[],
        RegisteredEvent::Frontend(Frontend::LoginSucceeded)
        | RegisteredEvent::Frontend(Frontend::TutorialStarted)
        | RegisteredEvent::Frontend(Frontend::TutorialCompleted) => &[(Property::Role, ROLE)],
        RegisteredEvent::Frontend(Frontend::LoginFailed) => {
            &[(Property::ReasonCategory, LOGIN_FAILURE)]
        }
        RegisteredEvent::Frontend(Frontend::PlatformNavigation) => &[
            (Property::Destination, DESTINATION),
            (Property::ViewMode, TRADING_ROLE),
        ],
        RegisteredEvent::Frontend(Frontend::MarketSliceSelected) => &[
            (Property::Product, External),
            (Property::DeliveryPoint, External),
            (Property::Window, External),
        ],
        RegisteredEvent::Frontend(Frontend::ListingOpened) => &[
            (Property::Product, External),
            (Property::DeliveryPoint, External),
            (Property::Window, External),
            (Property::Side, SIDE),
            (Property::DemoStatus, DEMO_STATUS),
        ],
        RegisteredEvent::Frontend(Frontend::OrderFormOpened)
        | RegisteredEvent::Frontend(Frontend::OrderFormSubmitted) => &[
            (Property::Product, External),
            (Property::DeliveryPoint, External),
            (Property::Window, External),
            (Property::Side, SIDE),
        ],
        RegisteredEvent::Frontend(Frontend::TradeConfirmationOpened) => {
            &[(Property::Side, SIDE), (Property::DemoStatus, DEMO_STATUS)]
        }
        RegisteredEvent::Frontend(Frontend::TutorialStepCompleted)
        | RegisteredEvent::Frontend(Frontend::TutorialStepSkipped) => &[
            (Property::Step, PropertyValues::TutorialSteps),
            (Property::Role, ROLE),
        ],
        RegisteredEvent::Frontend(Frontend::EstimatorCompleted) => {
            &[(Property::Port, External), (Property::Fuel, External)]
        }
        RegisteredEvent::Frontend(Frontend::FrontendError) => &[
            (Property::RouteFamily, ROUTE_FAMILY),
            (Property::Category, ERROR_CATEGORY),
        ],
        RegisteredEvent::Frontend(Frontend::BackendUnavailable) => {
            &[(Property::RouteFamily, NONLANDING_ROUTE_FAMILY)]
        }
        RegisteredEvent::Frontend(Frontend::NavigationPerformance) => &[
            (Property::Destination, DESTINATION),
            (Property::ViewMode, TRADING_ROLE),
            (Property::LatencyBucket, LATENCY_BUCKET),
        ],
    }
}

/// Properties accepted for one event, in contract order.
pub fn allowed_properties(event: RegisteredEvent) -> Vec<AllowedProperty> {
    property_domains(event)
        .iter()
        .map(|(property, _)| *property)
        .collect()
}

/// The value domain for one event property, or `None` when the event does not
/// accept that property at all.
pub fn property_domain(
    event: RegisteredEvent,
    property: AllowedProperty,
) -> Option<PropertyValues> {
    property_domains(event)
        .iter()
        .find(|(accepted, _)| *accepted == property)
        .map(|(_, values)| *values)
}

/// The outcome of checking one reported property value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyValueCheck {
    /// Inside the closed domain; the value may be aggregated.
    Allowed,
    /// Outside a closed domain, or a property this event does not accept.
    /// The value is discarded and never echoed.
    Rejected,
    /// The property is accepted, and its value must be resolved through the
    /// pinned product catalog, delivery-point catalog or ISO country
    /// reference before it can be aggregated.
    External,
}

/// Check one reported property value for a registered event.
pub fn check_property_value(
    event: RegisteredEvent,
    property: AllowedProperty,
    value: &str,
) -> PropertyValueCheck {
    match property_domain(event, property) {
        None => PropertyValueCheck::Rejected,
        Some(PropertyValues::External) => PropertyValueCheck::External,
        Some(domain) => match domain.accepts(value) {
            Some(true) => PropertyValueCheck::Allowed,
            _ => PropertyValueCheck::Rejected,
        },
    }
}

/// The declared property allowlist for one event.
///
/// No free text, identifier or commercial value can pass this type: the only
/// content is a registered event and its declared properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SanitizedProperties {
    pub event: RegisteredEvent,
    pub properties: Vec<AllowedProperty>,
}

/// Return the declared property allowlist for one server-produced event.
///
/// This is the plan's published signature. The browser taxonomy uses
/// [`allowed_properties`] with its [`RegisteredEvent`], which covers both
/// taxonomies.
pub fn allowed_event_properties(event: AllowedServerEvent) -> SanitizedProperties {
    let event = RegisteredEvent::Server(event);
    SanitizedProperties {
        event,
        properties: allowed_properties(event),
    }
}

impl From<AllowedServerEvent> for RegisteredEvent {
    fn from(event: AllowedServerEvent) -> Self {
        RegisteredEvent::Server(event)
    }
}

impl From<AllowedFrontendEvent> for RegisteredEvent {
    fn from(event: AllowedFrontendEvent) -> Self {
        RegisteredEvent::Frontend(event)
    }
}

// ---------------------------------------------------------------------------
// Requested property set
// ---------------------------------------------------------------------------

/// Maximum distinct events one provider request may ask about.
pub const MAX_PROVIDER_EVENTS: usize = 12;

/// Maximum property rows retained per requested event.
pub const MAX_PROVIDER_PROPERTY_ROWS: usize = 500;

/// Maximum provider response body retained, in bytes.
pub const MAX_PROVIDER_BODY_BYTES: usize = 1_000_000;

/// Maximum registered events; the event-series bound multiplies this by the
/// daily series limit.
pub const MAX_REGISTERED_EVENTS: usize = 32;

/// Why a requested property set is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertySetError {
    /// The property is not declared for that event.
    PropertyNotRegistered,
    /// More than twelve distinct events were requested.
    TooManyEvents,
}

/// A validated, sorted, duplicate-free set of `(event, property)` pairs.
///
/// Construction is the only way to make one, so a provider adapter can never
/// forward an undeclared property or an unbounded event list. A refusal is
/// HTTP 422 `ANALYTICS_PROPERTY_INVALID`; no free-text extension is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedPropertySet(Vec<(RegisteredEvent, AllowedProperty)>);

impl AllowedPropertySet {
    pub fn new(
        requested: impl IntoIterator<Item = (RegisteredEvent, AllowedProperty)>,
    ) -> Result<Self, PropertySetError> {
        let mut pairs: Vec<(RegisteredEvent, AllowedProperty)> = requested.into_iter().collect();
        for (event, property) in &pairs {
            if property_domain(*event, *property).is_none() {
                return Err(PropertySetError::PropertyNotRegistered);
            }
        }
        pairs.sort();
        pairs.dedup();
        let events = pairs
            .iter()
            .map(|(event, _)| *event)
            .collect::<std::collections::BTreeSet<_>>();
        if events.len() > MAX_PROVIDER_EVENTS {
            return Err(PropertySetError::TooManyEvents);
        }
        Ok(Self(pairs))
    }

    /// Build the refusal an invalid request produces.
    pub fn refusal(error: PropertySetError) -> crate::ApiError {
        let message = match error {
            PropertySetError::PropertyNotRegistered => {
                "a requested property is not registered for its event"
            }
            PropertySetError::TooManyEvents => "at most 12 events may be requested",
        };
        crate::business::error::refusal(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "ANALYTICS_PROPERTY_INVALID",
            message,
        )
    }

    /// The validated pairs, sorted and duplicate-free.
    pub fn pairs(&self) -> &[(RegisteredEvent, AllowedProperty)] {
        &self.0
    }

    /// Distinct events in the set.
    pub fn events(&self) -> Vec<RegisteredEvent> {
        self.0
            .iter()
            .map(|(event, _)| *event)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Distinct event count.
    pub fn event_count(&self) -> usize {
        self.events().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}
