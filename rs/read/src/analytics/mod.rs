//! Product Analytics shared primitives.
//!
//! Phase 4 Task 15 owns the leaves; the integration owner adds this module
//! file and its declaration in `rs/read/src/lib.rs`. It mounts no route and
//! starts no sender, scheduler or timer.

pub mod facts;
pub mod policy;
pub mod query;
pub mod retention;
pub mod types;

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
    AnalyticsQuery, MAX_AVAILABILITY_WINDOW_CHARS, MAX_RANGE_SECONDS, MIN_QUERY_INSTANT, QueryInput,
    previous_window, validate_query,
};
pub use retention::{
    AnalyticsFactFamily, MAX_SERIES_DATES, RETAINED_DATES, login_day_is_retained,
    login_retention_cutoff,
};
pub use types::*;
