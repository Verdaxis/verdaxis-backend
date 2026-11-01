//! Monitor and recovery commands (`verdaxis-read monitor <kind>`).
//!
//! Each monitor task adds its own file here and one `pub mod` line below.

pub mod alert;
pub mod alert_journal;
pub mod backup;
pub mod health;
pub mod outbox;
pub mod status;
