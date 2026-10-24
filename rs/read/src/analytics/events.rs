//! Pure analytics event policy: closed event, property and value domains.
//!
//! The browser reporting taxonomy and the four server events are fixed
//! registries, not free text. A request may ask for properties only where the
//! registry defines them, and a value is accepted only from its closed domain.
//! Identifiers, contact details and commercial values (price, quantity,
//! counterparty, order or trade identifiers) are not properties at all, so no
//! caller and no upstream row can introduce them.
//!
//! The closed value domains are decoding rules for this adapter. Umami accepts
//! looser tokens; an unknown or wrongly-cased value is dropped rather than
//! echoed, and one repeated value is aggregated before suppression.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde::{Serialize, Serializer};

use crate::{
    analytics::types::AggregateCell, business::error::refusal, market::terms::ISO_3166_1_ALPHA_2,
    market::windows::is_canonical_window, ApiError, MarketPolicy,
};

/// The one refusal shared by every policy violation in this module.
pub fn property_refusal() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "ANALYTICS_PROPERTY_INVALID",
        "The requested analytics property is not allowed.",
    )
}

// ---------------------------------------------------------------------------
// Value domains
// ---------------------------------------------------------------------------

pub const ROLES: [&str; 3] = ["BUYER", "SUPPLIER", "ADMIN"];
pub const TRADING_ROLES: [&str; 2] = ["BUYER", "SUPPLIER"];
pub const LANGUAGES: [&str; 2] = ["en", "zh"];
pub const SIDES: [&str; 2] = ["BID", "ASK"];
pub const DEMO_STATUSES: [&str; 5] = ["LIVE", "DEMO", "REFERENCE", "MIXED", "UNKNOWN"];
pub const LANDING_CTAS: [&str; 5] = [
    "pilot",
    "how_it_works",
    "register",
    "register_interest",
    "sign_in",
];
pub const PLACEMENTS: [&str; 6] = [
    "nav",
    "hero",
    "calculator",
    "landing_bottom",
    "pilot_sidebar",
    "pilot_bottom",
];
pub const DESTINATIONS: [&str; 12] = [
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
];
pub const ROUTE_FAMILIES: [&str; 4] = ["landing", "signup", "platform", "admin"];
pub const NONLANDING_ROUTE_FAMILIES: [&str; 3] = ["signup", "platform", "admin"];
pub const ERROR_CATEGORIES: [&str; 4] = ["render", "chunk", "network", "unknown"];
pub const LATENCY_BUCKETS: [&str; 5] = ["lt250", "250_500", "500_1000", "1000_2500", "gte2500"];
pub const LOGIN_FAILURES: [&str; 5] = [
    "invalid_credentials",
    "account_state",
    "network",
    "server",
    "unknown",
];
pub const ENTRY_POINTS: [&str; 7] = [
    "referral", "direct", "landing", "signup", "pilot", "invite", "other",
];
pub const ORGANIZATION_PATHS: [&str; 4] = ["create", "existing", "invitation", "other"];
pub const ORGANIZATION_TYPES: [&str; 4] =
    ["SHIP_OPERATOR", "FUEL_SUPPLIER", "FUEL_TRADER", "OTHER"];
pub const TUTORIAL_STEPS: [&str; 57] = [
    "buyer_1",
    "buyer_2",
    "buyer_3",
    "buyer_4",
    "buyer_5",
    "buyer_6",
    "buyer_7",
    "buyer_8",
    "buyer_9",
    "buyer_10",
    "buyer_11",
    "buyer_12",
    "buyer_13",
    "buyer_14",
    "buyer_15",
    "buyer_16",
    "buyer_17",
    "buyer_18",
    "buyer_19",
    "buyer_20",
    "buyer_21",
    "buyer_22",
    "buyer_23",
    "buyer_24",
    "buyer_25",
    "buyer_26",
    "buyer_27",
    "buyer_28",
    "supplier_1",
    "supplier_2",
    "supplier_3",
    "supplier_4",
    "supplier_5",
    "supplier_6",
    "supplier_7",
    "supplier_8",
    "supplier_9",
    "supplier_10",
    "supplier_11",
    "supplier_12",
    "supplier_13",
    "supplier_14",
    "supplier_15",
    "supplier_16",
    "supplier_17",
    "supplier_18",
    "supplier_19",
    "supplier_20",
    "supplier_21",
    "supplier_22",
    "supplier_23",
    "supplier_24",
    "supplier_25",
    "supplier_26",
    "supplier_27",
    "supplier_28",
    "supplier_29",
];

/// Entry pages. Raw paths are reduced to one of these; anything else is
/// `Other`, so a path that carries an identifier never leaves the adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

impl EntryPage {
    pub const ALL: [Self; 8] = [
        Self::Landing,
        Self::Signup,
        Self::Login,
        Self::Pilot,
        Self::Calculator,
        Self::Platform,
        Self::Admin,
        Self::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landing => "landing",
            Self::Signup => "signup",
            Self::Login => "login",
            Self::Pilot => "pilot",
            Self::Calculator => "calculator",
            Self::Platform => "platform",
            Self::Admin => "admin",
            Self::Other => "other",
        }
    }

    /// Classify one raw entry metric. Query strings and fragments are dropped;
    /// only a leading path segment is consulted.
    pub fn from_raw_path(raw: &str) -> Self {
        let without_origin = match raw.find("://") {
            Some(index) => match raw[index + 3..].find('/') {
                Some(slash) => &raw[index + 3 + slash..],
                None => "/",
            },
            None => raw,
        };
        let path = without_origin.split(['?', '#']).next().unwrap_or("").trim();
        let normalized = {
            let mut path = path.to_owned();
            while path.contains("//") {
                path = path.replace("//", "/");
            }
            path
        };
        let segment = normalized
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        match segment {
            "" => Self::Landing,
            "signup" => Self::Signup,
            "login" => Self::Login,
            "pilot" => Self::Pilot,
            "calculator" => Self::Calculator,
            "admin" => Self::Admin,
            "marketplace" | "map" | "curve" | "watchlist" | "analytics" | "trades" | "quotes"
            | "compliance" | "training" | "settings" | "orders" | "inventory" | "platform" => {
                Self::Platform
            }
            _ => Self::Other,
        }
    }
}

impl Serialize for EntryPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Referrer categories. Only the parsed host is inspected and only a fixed
/// category leaves the adapter; no raw host or URL reaches a response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferrerCategory {
    Direct,
    Search,
    Social,
    Referral,
    Other,
}

impl ReferrerCategory {
    pub const ALL: [Self; 5] = [
        Self::Direct,
        Self::Search,
        Self::Social,
        Self::Referral,
        Self::Other,
    ];

    const SEARCH_HOSTS: [&str; 8] = [
        "google",
        "bing",
        "duckduckgo",
        "yahoo",
        "yandex",
        "baidu",
        "ecosia",
        "brave",
    ];
    const SOCIAL_HOSTS: [&str; 10] = [
        "facebook",
        "instagram",
        "linkedin",
        "twitter",
        "x.com",
        "t.co",
        "reddit",
        "youtube",
        "tiktok",
        "telegram",
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Search => "search",
            Self::Social => "social",
            Self::Referral => "referral",
            Self::Other => "other",
        }
    }

    pub fn from_raw(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty() {
            return Self::Direct;
        }
        let host = match raw.find("://") {
            Some(_) => url::Url::parse(raw)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned)),
            None => Some(raw.split('/').next().unwrap_or("").to_owned()),
        };
        let Some(host) = host else {
            return Self::Other;
        };
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        if host.is_empty() {
            return Self::Direct;
        }
        let matches = |needle: &str| host == needle || host.ends_with(&format!(".{needle}"));
        if Self::SEARCH_HOSTS.iter().any(|needle| matches(needle)) {
            return Self::Search;
        }
        if Self::SOCIAL_HOSTS.iter().any(|needle| matches(needle)) {
            return Self::Social;
        }
        if host.contains("verdaxis") {
            return Self::Referral;
        }
        Self::Other
    }
}

impl Serialize for ReferrerCategory {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AllowedProperty {
    Role,
    Side,
    CanonicalProduct,
    DeliveryPoint,
    AvailabilityWindow,
    Cta,
    Placement,
    Language,
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
    Fuel,
    Port,
    Step,
    RouteFamily,
    Category,
    LatencyBucket,
}

impl AllowedProperty {
    pub fn name(self) -> &'static str {
        match self {
            Self::Role => "role",
            Self::Side => "side",
            Self::CanonicalProduct => "canonical_product",
            Self::DeliveryPoint => "delivery_point",
            Self::AvailabilityWindow => "availability_window",
            Self::Cta => "cta",
            Self::Placement => "placement",
            Self::Language => "language",
            Self::From => "from",
            Self::To => "to",
            Self::EntryPoint => "entry_point",
            Self::OrganizationPath => "organization_path",
            Self::OrganizationType => "organization_type",
            Self::Country => "country",
            Self::ReasonCategory => "reason_category",
            Self::Destination => "destination",
            Self::ViewMode => "view_mode",
            Self::Product => "product",
            Self::Window => "window",
            Self::DemoStatus => "demo_status",
            Self::Fuel => "fuel",
            Self::Port => "port",
            Self::Step => "step",
            Self::RouteFamily => "route_family",
            Self::Category => "category",
            Self::LatencyBucket => "latency_bucket",
        }
    }

    /// The only accepted spelling. Identifiers, contact details, free text and
    /// commercial values have no name and can never be requested.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "role" => Self::Role,
            "side" => Self::Side,
            "canonical_product" => Self::CanonicalProduct,
            "delivery_point" => Self::DeliveryPoint,
            "availability_window" => Self::AvailabilityWindow,
            "cta" => Self::Cta,
            "placement" => Self::Placement,
            "language" => Self::Language,
            "from" => Self::From,
            "to" => Self::To,
            "entry_point" => Self::EntryPoint,
            "organization_path" => Self::OrganizationPath,
            "organization_type" => Self::OrganizationType,
            "country" => Self::Country,
            "reason_category" => Self::ReasonCategory,
            "destination" => Self::Destination,
            "view_mode" => Self::ViewMode,
            "product" => Self::Product,
            "window" => Self::Window,
            "demo_status" => Self::DemoStatus,
            "fuel" => Self::Fuel,
            "port" => Self::Port,
            "step" => Self::Step,
            "route_family" => Self::RouteFamily,
            "category" => Self::Category,
            "latency_bucket" => Self::LatencyBucket,
            _ => return None,
        })
    }
}

/// Pinned catalogs for the value domains that are data, not code.
///
/// Products and delivery points come from the attested market policy; the
/// assigned country set is the ISO 3166-1 alpha-2 reference list. A value
/// outside these catalogs is not a value, so a caller cannot widen the domain.
#[derive(Clone, Debug, Default)]
pub struct DomainCatalog {
    products: BTreeSet<String>,
    points: BTreeSet<String>,
    countries: BTreeSet<String>,
}

impl DomainCatalog {
    pub fn new(
        products: impl IntoIterator<Item = impl Into<String>>,
        points: impl IntoIterator<Item = impl Into<String>>,
        countries: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            products: products.into_iter().map(Into::into).collect(),
            points: points.into_iter().map(Into::into).collect(),
            countries: countries.into_iter().map(Into::into).collect(),
        }
    }

    pub fn from_policy(
        policy: &MarketPolicy,
        countries: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::new(
            policy.products.iter().map(|product| product.code.clone()),
            policy.points.iter().map(|point| point.name.clone()),
            countries,
        )
    }

    /// The full assigned-country reference set.
    pub fn assigned_countries() -> Self {
        Self::new(
            Vec::<String>::new(),
            Vec::<String>::new(),
            ISO_3166_1_ALPHA_2,
        )
    }

    pub fn is_product(&self, value: &str) -> bool {
        self.products.contains(value)
    }

    pub fn is_delivery_point(&self, value: &str) -> bool {
        self.points.contains(value)
    }

    pub fn is_country(&self, value: &str) -> bool {
        self.countries.contains(value)
    }
}

/// A validated property value: the property plus one token from its domain.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllowedPropertyValue {
    property: AllowedProperty,
    value: String,
}

impl AllowedPropertyValue {
    /// Validate `raw` for `property` on `event`, or `None` when the pair is not
    /// registered or the token is outside the closed domain.
    pub fn parse(
        event: RegisteredEvent,
        property: AllowedProperty,
        raw: &str,
        catalog: &DomainCatalog,
    ) -> Option<Self> {
        if !event.properties().contains(&property) {
            return None;
        }
        let accepted = match property {
            AllowedProperty::Role => {
                let domain = if matches!(
                    event.name(),
                    "login_succeeded"
                        | "tutorial_started"
                        | "tutorial_step_completed"
                        | "tutorial_step_skipped"
                        | "tutorial_completed"
                ) {
                    &ROLES[..]
                } else {
                    &TRADING_ROLES[..]
                };
                domain.contains(&raw)
            }
            AllowedProperty::Side => SIDES.contains(&raw),
            AllowedProperty::CanonicalProduct
            | AllowedProperty::Product
            | AllowedProperty::Fuel => catalog.is_product(raw),
            AllowedProperty::DeliveryPoint | AllowedProperty::Port => {
                catalog.is_delivery_point(raw)
            }
            AllowedProperty::AvailabilityWindow | AllowedProperty::Window => canonical_window(raw),
            AllowedProperty::Cta => LANDING_CTAS.contains(&raw),
            AllowedProperty::Placement => PLACEMENTS.contains(&raw),
            AllowedProperty::Language | AllowedProperty::From | AllowedProperty::To => {
                LANGUAGES.contains(&raw)
            }
            AllowedProperty::EntryPoint => ENTRY_POINTS.contains(&raw),
            AllowedProperty::OrganizationPath => ORGANIZATION_PATHS.contains(&raw),
            AllowedProperty::OrganizationType => ORGANIZATION_TYPES.contains(&raw),
            AllowedProperty::Country => raw.chars().count() == 2 && catalog.is_country(raw),
            AllowedProperty::ReasonCategory => LOGIN_FAILURES.contains(&raw),
            AllowedProperty::Destination | AllowedProperty::ViewMode => {
                if property == AllowedProperty::ViewMode {
                    TRADING_ROLES.contains(&raw)
                } else {
                    DESTINATIONS.contains(&raw)
                }
            }
            AllowedProperty::DemoStatus => DEMO_STATUSES.contains(&raw),
            AllowedProperty::Step => TUTORIAL_STEPS.contains(&raw),
            AllowedProperty::RouteFamily => {
                if event.name() == "backend_unavailable" {
                    NONLANDING_ROUTE_FAMILIES.contains(&raw)
                } else {
                    ROUTE_FAMILIES.contains(&raw)
                }
            }
            AllowedProperty::Category => ERROR_CATEGORIES.contains(&raw),
            AllowedProperty::LatencyBucket => LATENCY_BUCKETS.contains(&raw),
        };
        accepted.then(|| Self {
            property,
            value: raw.to_owned(),
        })
    }

    pub fn property(&self) -> AllowedProperty {
        self.property
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl Serialize for AllowedPropertyValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.value)
    }
}

/// Canonical availability window only: no aliases, no case folding, and the
/// year is inside 0001..=9999.
fn canonical_window(raw: &str) -> bool {
    if !is_canonical_window(raw) {
        return false;
    }
    if raw == "SPOT" {
        return true;
    }
    let year: u32 = raw[..4].parse().unwrap_or(0);
    (1..=9999).contains(&year)
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AllowedServerEvent {
    RegistrationCompleted,
    OrganizationCreated,
    OrderCreated,
    TradeCreated,
}

impl AllowedServerEvent {
    pub const ALL: [Self; 4] = [
        Self::RegistrationCompleted,
        Self::OrganizationCreated,
        Self::OrderCreated,
        Self::TradeCreated,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::RegistrationCompleted => "registration_completed",
            Self::OrganizationCreated => "organization_created",
            Self::OrderCreated => "order_created",
            Self::TradeCreated => "trade_created",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

impl AllowedFrontendEvent {
    pub const ALL: [Self; 27] = [
        Self::LandingCtaClicked,
        Self::EnergyCalculatorStarted,
        Self::EnergyCalculatorCompleted,
        Self::PublicLanguageChanged,
        Self::SignupStarted,
        Self::SignupRoleSelected,
        Self::SignupSubmitted,
        Self::SignupOrganizationRequired,
        Self::SignupOrganizationSubmitted,
        Self::LoginSubmitted,
        Self::LoginSucceeded,
        Self::LoginFailed,
        Self::PlatformNavigation,
        Self::MarketSliceSelected,
        Self::ListingOpened,
        Self::OrderFormOpened,
        Self::OrderFormSubmitted,
        Self::TradeConfirmationOpened,
        Self::TutorialStarted,
        Self::TutorialStepCompleted,
        Self::TutorialStepSkipped,
        Self::TutorialCompleted,
        Self::EstimatorOpened,
        Self::EstimatorCompleted,
        Self::FrontendError,
        Self::BackendUnavailable,
        Self::NavigationPerformance,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::LandingCtaClicked => "landing_cta_clicked",
            Self::EnergyCalculatorStarted => "energy_calculator_started",
            Self::EnergyCalculatorCompleted => "energy_calculator_completed",
            Self::PublicLanguageChanged => "public_language_changed",
            Self::SignupStarted => "signup_started",
            Self::SignupRoleSelected => "signup_role_selected",
            Self::SignupSubmitted => "signup_submitted",
            Self::SignupOrganizationRequired => "signup_organization_required",
            Self::SignupOrganizationSubmitted => "signup_organization_submitted",
            Self::LoginSubmitted => "login_submitted",
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::PlatformNavigation => "platform_navigation",
            Self::MarketSliceSelected => "market_slice_selected",
            Self::ListingOpened => "listing_opened",
            Self::OrderFormOpened => "order_form_opened",
            Self::OrderFormSubmitted => "order_form_submitted",
            Self::TradeConfirmationOpened => "trade_confirmation_opened",
            Self::TutorialStarted => "tutorial_started",
            Self::TutorialStepCompleted => "tutorial_step_completed",
            Self::TutorialStepSkipped => "tutorial_step_skipped",
            Self::TutorialCompleted => "tutorial_completed",
            Self::EstimatorOpened => "estimator_opened",
            Self::EstimatorCompleted => "estimator_completed",
            Self::FrontendError => "frontend_error",
            Self::BackendUnavailable => "backend_unavailable",
            Self::NavigationPerformance => "navigation_performance",
        }
    }
}

/// One registered event: either a server-produced event or a browser event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegisteredEvent {
    Server(AllowedServerEvent),
    Frontend(AllowedFrontendEvent),
}

impl RegisteredEvent {
    /// Server events first, then the browser taxonomy in contract order.
    pub const ALL: [Self; 31] = [
        Self::Server(AllowedServerEvent::RegistrationCompleted),
        Self::Server(AllowedServerEvent::OrganizationCreated),
        Self::Server(AllowedServerEvent::OrderCreated),
        Self::Server(AllowedServerEvent::TradeCreated),
        Self::Frontend(AllowedFrontendEvent::LandingCtaClicked),
        Self::Frontend(AllowedFrontendEvent::EnergyCalculatorStarted),
        Self::Frontend(AllowedFrontendEvent::EnergyCalculatorCompleted),
        Self::Frontend(AllowedFrontendEvent::PublicLanguageChanged),
        Self::Frontend(AllowedFrontendEvent::SignupStarted),
        Self::Frontend(AllowedFrontendEvent::SignupRoleSelected),
        Self::Frontend(AllowedFrontendEvent::SignupSubmitted),
        Self::Frontend(AllowedFrontendEvent::SignupOrganizationRequired),
        Self::Frontend(AllowedFrontendEvent::SignupOrganizationSubmitted),
        Self::Frontend(AllowedFrontendEvent::LoginSubmitted),
        Self::Frontend(AllowedFrontendEvent::LoginSucceeded),
        Self::Frontend(AllowedFrontendEvent::LoginFailed),
        Self::Frontend(AllowedFrontendEvent::PlatformNavigation),
        Self::Frontend(AllowedFrontendEvent::MarketSliceSelected),
        Self::Frontend(AllowedFrontendEvent::ListingOpened),
        Self::Frontend(AllowedFrontendEvent::OrderFormOpened),
        Self::Frontend(AllowedFrontendEvent::OrderFormSubmitted),
        Self::Frontend(AllowedFrontendEvent::TradeConfirmationOpened),
        Self::Frontend(AllowedFrontendEvent::TutorialStarted),
        Self::Frontend(AllowedFrontendEvent::TutorialStepCompleted),
        Self::Frontend(AllowedFrontendEvent::TutorialStepSkipped),
        Self::Frontend(AllowedFrontendEvent::TutorialCompleted),
        Self::Frontend(AllowedFrontendEvent::EstimatorOpened),
        Self::Frontend(AllowedFrontendEvent::EstimatorCompleted),
        Self::Frontend(AllowedFrontendEvent::FrontendError),
        Self::Frontend(AllowedFrontendEvent::BackendUnavailable),
        Self::Frontend(AllowedFrontendEvent::NavigationPerformance),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Server(event) => event.name(),
            Self::Frontend(event) => event.name(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|event| event.name() == name)
    }

    /// The registered properties of this event, in contract order.
    pub fn properties(self) -> &'static [AllowedProperty] {
        use AllowedProperty::*;
        match self {
            Self::Server(AllowedServerEvent::RegistrationCompleted)
            | Self::Server(AllowedServerEvent::OrganizationCreated) => &[Role],
            Self::Server(AllowedServerEvent::OrderCreated)
            | Self::Server(AllowedServerEvent::TradeCreated) => &[
                Role,
                Side,
                CanonicalProduct,
                DeliveryPoint,
                AvailabilityWindow,
            ],
            Self::Frontend(AllowedFrontendEvent::LandingCtaClicked) => &[Cta, Placement, Language],
            Self::Frontend(AllowedFrontendEvent::EnergyCalculatorStarted) => &[Language],
            Self::Frontend(AllowedFrontendEvent::EnergyCalculatorCompleted) => {
                &[Language, Fuel, Port]
            }
            Self::Frontend(AllowedFrontendEvent::PublicLanguageChanged) => &[From, To],
            Self::Frontend(AllowedFrontendEvent::SignupStarted) => &[EntryPoint, Language],
            Self::Frontend(AllowedFrontendEvent::SignupRoleSelected)
            | Self::Frontend(AllowedFrontendEvent::SignupOrganizationRequired) => &[Role],
            Self::Frontend(AllowedFrontendEvent::SignupSubmitted) => &[Role, OrganizationPath],
            Self::Frontend(AllowedFrontendEvent::SignupOrganizationSubmitted) => {
                &[Role, OrganizationType, Country]
            }
            Self::Frontend(AllowedFrontendEvent::LoginSubmitted) => &[],
            Self::Frontend(AllowedFrontendEvent::LoginSucceeded) => &[Role],
            Self::Frontend(AllowedFrontendEvent::LoginFailed) => &[ReasonCategory],
            Self::Frontend(AllowedFrontendEvent::PlatformNavigation) => &[Destination, ViewMode],
            Self::Frontend(AllowedFrontendEvent::MarketSliceSelected) => {
                &[Product, DeliveryPoint, Window]
            }
            Self::Frontend(AllowedFrontendEvent::ListingOpened) => {
                &[Product, DeliveryPoint, Window, Side, DemoStatus]
            }
            Self::Frontend(AllowedFrontendEvent::OrderFormOpened)
            | Self::Frontend(AllowedFrontendEvent::OrderFormSubmitted) => {
                &[Product, DeliveryPoint, Window, Side]
            }
            Self::Frontend(AllowedFrontendEvent::TradeConfirmationOpened) => &[Side, DemoStatus],
            Self::Frontend(AllowedFrontendEvent::TutorialStarted)
            | Self::Frontend(AllowedFrontendEvent::TutorialCompleted) => &[Role],
            Self::Frontend(AllowedFrontendEvent::TutorialStepCompleted)
            | Self::Frontend(AllowedFrontendEvent::TutorialStepSkipped) => &[Step, Role],
            Self::Frontend(AllowedFrontendEvent::EstimatorOpened) => &[],
            Self::Frontend(AllowedFrontendEvent::EstimatorCompleted) => &[Port, Fuel],
            Self::Frontend(AllowedFrontendEvent::FrontendError) => &[RouteFamily, Category],
            Self::Frontend(AllowedFrontendEvent::BackendUnavailable) => &[RouteFamily],
            Self::Frontend(AllowedFrontendEvent::NavigationPerformance) => {
                &[Destination, ViewMode, LatencyBucket]
            }
        }
    }
}

impl Serialize for RegisteredEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

// ---------------------------------------------------------------------------
// Requested property set
// ---------------------------------------------------------------------------

/// A sorted, duplicate-free set of registered (event, property) pairs.
///
/// At most twelve distinct events may be requested; each pair must exist in the
/// event registry. Unknown pairs and free-text extensions are refused before
/// any provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedPropertySet {
    pairs: Vec<(RegisteredEvent, AllowedProperty)>,
    events: Vec<RegisteredEvent>,
}

impl AllowedPropertySet {
    pub const MAX_EVENTS: usize = 12;

    pub fn new(
        pairs: impl IntoIterator<Item = (RegisteredEvent, AllowedProperty)>,
    ) -> Result<Self, ApiError> {
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        for (event, property) in &pairs {
            if !event.properties().contains(property) {
                return Err(property_refusal());
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let mut events: Vec<RegisteredEvent> = pairs.iter().map(|(event, _)| *event).collect();
        events.dedup();
        if events.len() > Self::MAX_EVENTS {
            return Err(property_refusal());
        }
        Ok(Self { pairs, events })
    }

    pub fn empty() -> Self {
        Self {
            pairs: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn pairs(&self) -> &[(RegisteredEvent, AllowedProperty)] {
        &self.pairs
    }

    pub fn events(&self) -> &[RegisteredEvent] {
        &self.events
    }

    pub fn properties_for(&self, event: RegisteredEvent) -> Vec<AllowedProperty> {
        self.pairs
            .iter()
            .filter(|(candidate, _)| *candidate == event)
            .map(|(_, property)| *property)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Server event payload
// ---------------------------------------------------------------------------

/// One server event ready for a collector payload. Only registered properties
/// can be carried, so identifiers and commercial values cannot be added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEvent {
    event: AllowedServerEvent,
    values: Vec<(AllowedProperty, AllowedPropertyValue)>,
}

impl ServerEvent {
    pub fn new(
        event: AllowedServerEvent,
        values: impl IntoIterator<Item = (AllowedProperty, AllowedPropertyValue)>,
    ) -> Result<Self, ApiError> {
        let values: Vec<_> = values.into_iter().collect();
        for (property, value) in &values {
            if value.property() != *property
                || !RegisteredEvent::Server(event)
                    .properties()
                    .contains(property)
            {
                return Err(property_refusal());
            }
        }
        Ok(Self { event, values })
    }

    pub fn name(&self) -> &'static str {
        self.event.name()
    }

    /// Property name and value, in registry order. Absent properties stay
    /// absent; they are never sent as empty strings.
    pub fn data(&self) -> Vec<(&'static str, String)> {
        RegisteredEvent::Server(self.event)
            .properties()
            .iter()
            .filter_map(|property| {
                self.values
                    .iter()
                    .find(|(candidate, _)| candidate == property)
                    .map(|(property, value)| (property.name(), value.as_str().to_owned()))
            })
            .collect()
    }
}

/// Suppression for one named cell: a positive basis of one or two hides the
/// count. Zero stays a genuine zero.
pub fn named_cell(key: impl Into<String>, count: u64) -> AggregateCell {
    let mut cell = crate::analytics::policy::suppress(Some(count), None);
    cell.key = key.into();
    cell
}
