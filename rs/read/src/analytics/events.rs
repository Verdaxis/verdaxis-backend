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

pub use crate::analytics::policy::{AllowedFrontendEvent, AllowedProperty, AllowedPropertySet, AllowedServerEvent, RegisteredEvent};
pub use crate::analytics::types::{EntryPage, ReferrerCategory};
use crate::analytics::policy::{allowed_properties, check_property_value, PropertyValueCheck};

use crate::{
    ApiError, MarketPolicy, analytics::types::AggregateCell, business::error::refusal,
    market::terms::ISO_3166_1_ALPHA_2, market::windows::is_canonical_window,
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

impl ReferrerCategory {
    pub const ALL: [Self; 5] = [
        Self::Direct,
        Self::Search,
        Self::Social,
        Self::Referral,
        Self::Other,
    ];

    const SEARCH_HOSTS: [&str; 9] = [
        "google.com",
        "bing.com",
        "duckduckgo.com",
        "search.yahoo.com",
        "yandex.com",
        "baidu.com",
        "ecosia.org",
        "search.brave.com",
        "startpage.com",
    ];
    const SOCIAL_HOSTS: [&str; 10] = [
        "facebook.com",
        "instagram.com",
        "linkedin.com",
        "twitter.com",
        "x.com",
        "t.co",
        "reddit.com",
        "youtube.com",
        "tiktok.com",
        "t.me",
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
        if !allowed_properties(event).contains(&property) {
            return None;
        }
        let accepted = match check_property_value(event, property, raw) {
            PropertyValueCheck::Allowed => true,
            PropertyValueCheck::Rejected => false,
            PropertyValueCheck::External => match property {
                AllowedProperty::CanonicalProduct | AllowedProperty::Product | AllowedProperty::Fuel => catalog.is_product(raw),
                AllowedProperty::DeliveryPoint | AllowedProperty::Port => catalog.is_delivery_point(raw),
                AllowedProperty::Country => catalog.is_country(raw),
                AllowedProperty::AvailabilityWindow | AllowedProperty::Window => canonical_window(raw),
                _ => false,
            },
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
                || !allowed_properties(RegisteredEvent::Server(event))
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
        allowed_properties(RegisteredEvent::Server(self.event))
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
