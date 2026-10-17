//! The pure go/no-go gate: a pass needs all three Phase 1 conditions.
//!
//! The gate takes only the **unapproved** difference count. AD-01's rate-limit
//! threshold counts are excluded from comparison before this point and are
//! never an allowlist for other differences. `passes_gate` is pure so it can be
//! exercised without a database, a server, or a measured run.

/// Phase 1 passes only when all three declared conditions hold together:
///
/// 1. zero unapproved contract differences;
/// 2. Rust serves the ported-route workload in at most half of Python's memory
///    (`python_memory` must be a positive measured value, so neither a missing
///    nor a zero baseline can satisfy the ratio);
/// 3. Rust's p95 latency is at or below Python's, both finite and Python's
///    positive, so a NaN, an infinity, or a zero-time placeholder never passes.
pub fn passes_gate(
    unapproved_differences: u64,
    python_memory: u64,
    rust_memory: u64,
    python_p95: f64,
    rust_p95: f64,
) -> bool {
    unapproved_differences == 0
        && python_memory > 0
        && rust_memory <= python_memory / 2
        && python_p95.is_finite()
        && rust_p95.is_finite()
        && python_p95 > 0.0
        && rust_p95 >= 0.0
        && rust_p95 <= python_p95
}
