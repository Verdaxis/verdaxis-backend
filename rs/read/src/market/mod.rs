//! Shared market rules.
//!
//! Task 5 owns catalog policy data here. Task 6 adds the shared visibility,
//! provenance, terms, decimal, and window helpers that every route uses, so
//! no route forks a predicate.

pub mod amount;
pub mod cache_dependencies;
pub mod decimal;
pub mod legacy_curve;
pub mod live_benchmarks;
pub mod order_projection;
pub mod policy;
pub mod provenance;
pub mod terms;
pub mod visibility;
pub mod whitespace;
pub mod windows;
