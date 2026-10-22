pub mod catalog;
// Phase 2 Task 10: feedback route registration.
pub use crate::crud::feedback::routes as feedback_routes;
// Phase 2 Task 10: end feedback route registration.
pub mod curves_canonical;
pub mod curves_legacy;
pub mod listings;
pub mod market_data_error;
pub mod news;
pub mod orderbook_lists;
pub mod orderbook_map;
pub mod orderbook_snapshot;
pub mod ports;
pub mod prices;
pub mod trade_tape;
// Phase 2 Task 11: watchlist routes
pub use crate::crud::watchlists::router as watchlist_router;
// Phase 2 Task 11: end watchlist routes

// Phase 2 Task 8: notifications — start
pub use crate::crud::notifications::routes as notification_routes;
// Phase 2 Task 8: notifications — end

// Phase 2 Task 9: preferences route registration
pub use crate::crud::preferences::router as preferences_router;
// Phase 2 Task 9: end preferences route registration

// Phase 2 Task 16: organization price-alert CRUD
pub use crate::crud::alerts::router as alerts_router;
// Phase 2 Task 16: end organization price-alert CRUD

// Phase 2 Task 17: referral reads and public resolution (start)
pub use crate::crud::referrals::routes as referral_routes;
// Phase 2 Task 17: referral reads and public resolution (end)
