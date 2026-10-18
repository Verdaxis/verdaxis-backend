pub mod catalog;
pub mod curves_canonical;
pub mod curves_legacy;
pub mod listings;
pub mod news;
pub mod orderbook_lists;
pub mod orderbook_map;
pub mod orderbook_snapshot;
pub mod ports;
pub mod prices;
pub mod trade_tape;
// Phase 2 Task 9: preferences route registration
pub use crate::crud::preferences::router as preferences_router;
// Phase 2 Task 9: end preferences route registration
