//! CRUD, abilities, referral and watchlist tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "abilities_http.rs"]
pub mod abilities_http;
#[path = "abilities_policy.rs"]
pub mod abilities_policy;
#[path = "abilities_postgres.rs"]
pub mod abilities_postgres;
#[path = "agent_audit_postgres.rs"]
pub mod agent_audit_postgres;
#[path = "crud_alerts.rs"]
pub mod crud_alerts;
#[path = "crud_feedback.rs"]
pub mod crud_feedback;
#[path = "crud_notifications.rs"]
pub mod crud_notifications;
#[path = "crud_preferences.rs"]
pub mod crud_preferences;
#[path = "crud_preferences_postgres.rs"]
pub mod crud_preferences_postgres;
#[path = "crud_referral_code.rs"]
pub mod crud_referral_code;
#[path = "crud_referral_invite.rs"]
pub mod crud_referral_invite;
#[path = "crud_referral_reads.rs"]
pub mod crud_referral_reads;
#[path = "crud_watchlist_containers.rs"]
pub mod crud_watchlist_containers;
#[path = "crud_watchlist_events.rs"]
pub mod crud_watchlist_events;
#[path = "crud_watchlist_reads.rs"]
pub mod crud_watchlist_reads;
#[path = "crud_watchlist_targets.rs"]
pub mod crud_watchlist_targets;
#[path = "phase2_http.rs"]
pub mod phase2_http;
#[path = "referral_mail_timeout.rs"]
pub mod referral_mail_timeout;
#[path = "watchlist_activation_source.rs"]
pub mod watchlist_activation_source;

#[path = "agent_tools.rs"]
pub mod agent_tools;
#[path = "agent_tools_postgres.rs"]
pub mod agent_tools_postgres;
#[path = "agent_turn_postgres.rs"]
pub mod agent_turn_postgres;
