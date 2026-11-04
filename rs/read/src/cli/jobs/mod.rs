//! Scheduled job commands (`verdaxis-read job <kind>`).
//!
//! Each job task adds its own file here and one `pub mod` line below.
pub mod auth;
pub mod demo;
pub mod kyc;
pub mod news;
pub mod onboarding;
pub mod prune;
pub mod reminders;
