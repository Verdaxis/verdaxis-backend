mod containers;
mod http;
pub use http::{ControlSource, WatchlistSettings, WriteControls, router};
mod metrics;
mod projection;
mod reads;
mod targets;
mod validation;
pub use reads::{
    LegacyEntry, LegacyWatchlist, VisibleOrder, VisibleSnapshot, capture_visible, load_detail,
    load_legacy_list,
};

pub use metrics::{Event, Metrics, visible_order_rows};
pub use projection::{Slice, Summary, Target, project_summary};
