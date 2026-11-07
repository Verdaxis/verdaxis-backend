//! CRUD, abilities, referral and watchlist tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "abilities_http.rs"]
mod abilities_http;
#[path = "abilities_policy.rs"]
mod abilities_policy;
#[path = "abilities_postgres.rs"]
mod abilities_postgres;
#[path = "crud_alerts.rs"]
mod crud_alerts;
#[path = "crud_feedback.rs"]
mod crud_feedback;
#[path = "crud_notifications.rs"]
mod crud_notifications;
#[path = "crud_preferences.rs"]
mod crud_preferences;
#[path = "crud_preferences_postgres.rs"]
mod crud_preferences_postgres;
#[path = "crud_referral_code.rs"]
mod crud_referral_code;
#[path = "crud_referral_invite.rs"]
mod crud_referral_invite;
#[path = "crud_referral_reads.rs"]
mod crud_referral_reads;
#[path = "crud_watchlist_containers.rs"]
mod crud_watchlist_containers;
#[path = "crud_watchlist_events.rs"]
mod crud_watchlist_events;
#[path = "crud_watchlist_reads.rs"]
mod crud_watchlist_reads;
#[path = "crud_watchlist_targets.rs"]
mod crud_watchlist_targets;
#[path = "phase2_http.rs"]
mod phase2_http;
#[path = "phase2_routes.rs"]
mod phase2_routes;
#[path = "phase3_routes.rs"]
mod phase3_routes;
#[path = "referral_mail_timeout.rs"]
mod referral_mail_timeout;
#[path = "watchlist_activation_source.rs"]
mod watchlist_activation_source;
