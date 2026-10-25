//! Product Analytics shared primitives and operator analytics surfaces.
//!
//! Phase 4 Task 15 owns the shared leaves (facts, policy, query, retention,
//! types); Tasks 17a/17b/18/19 add one route family each. The module mounts no
//! route, starts no sender, scheduler or timer; the legacy dashboard and the
//! identified user listing are operator reads with their own typed response
//! contracts, never payloads for the product-analytics tabs.

pub mod acquisition;
pub mod behavioral;
pub mod commercial;
pub mod events;
pub mod facts;
pub mod liquidity;
pub mod marketplace;
pub mod reliability;
pub mod policy;
pub mod query;
pub mod retention;
pub mod types;
pub mod usage;

pub mod legacy_dashboard;
pub mod users;

pub use facts::{MarketProvenance, classify_order, classify_trade};
pub use policy::{
    AllowedFrontendEvent, AllowedProperty, AllowedPropertySet, AllowedServerEvent, MAX_COHORT_ROWS,
    MAX_MATRIX_CELLS, MAX_PROVIDER_BODY_BYTES, MAX_PROVIDER_EVENTS, MAX_PROVIDER_PROPERTY_ROWS,
    MAX_RANKED_ROWS, MAX_REGISTERED_EVENTS, MAX_SERIES_POINTS, PropertySetError,
    PropertyValueCheck, PropertyValues, RegisteredEvent, SUPPRESSION_THRESHOLD,
    SanitizedProperties, allowed_event_properties, allowed_properties, check_property_value,
    property_domain, ratio_from_counts, suppress, suppress_cell, suppress_value,
};
pub use query::{
    AnalyticsQuery, MAX_AVAILABILITY_WINDOW_CHARS, MAX_RANGE_SECONDS, MIN_QUERY_INSTANT,
    QueryInput, previous_window, validate_query,
};
pub use retention::{
    AnalyticsFactFamily, MAX_SERIES_DATES, RETAINED_DATES, login_day_is_retained,
    login_retention_cutoff,
};
pub use types::*;
