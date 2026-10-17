//! False-pass tests for the Phase 1 go/no-go gate.
//!
//! Every test here is written to fail if the gate ever passes on a condition
//! that is missing, unmeasured, or spelled as a placeholder.

use verdaxis_contract::passes_gate;

#[test]
fn go_no_go_needs_all_three_conditions() {
    // All three conditions hold: zero differences, half memory, equal p95.
    assert!(passes_gate(0, 100, 50, 10.0, 10.0));
    // One unapproved difference is a no-go, whatever the memory or latency.
    assert!(!passes_gate(1, 100, 40, 10.0, 9.0));
    // Rust above half of Python's memory is a no-go.
    assert!(!passes_gate(0, 100, 51, 10.0, 9.0));
    // A p95 regression is a no-go even with zero differences and half memory.
    assert!(!passes_gate(0, 100, 40, 10.0, 10.1));
    // A missing (zero) memory baseline cannot satisfy the ratio.
    assert!(!passes_gate(0, 0, 0, 10.0, 9.0));
}

#[test]
fn exactly_half_memory_and_equal_p95_pass() {
    // The boundaries of both conditions are inclusive.
    assert!(passes_gate(0, 100, 50, 10.0, 10.0));
    assert!(passes_gate(0, 2, 1, 0.001, 0.0));
}

#[test]
fn unmeasured_or_nonfinite_latency_never_passes() {
    // A placeholder p95 of zero on the Python side is not a measurement.
    assert!(!passes_gate(0, 100, 40, 0.0, 0.0));
    // NaN and infinities are refused on either side.
    assert!(!passes_gate(0, 100, 40, f64::NAN, 9.0));
    assert!(!passes_gate(0, 100, 40, 10.0, f64::NAN));
    assert!(!passes_gate(0, 100, 40, f64::INFINITY, 9.0));
    assert!(!passes_gate(0, 100, 40, 10.0, f64::INFINITY));
    assert!(!passes_gate(0, 100, 40, 10.0, f64::NEG_INFINITY));
}

#[test]
fn zero_rust_memory_is_not_a_measurement() {
    // Rust at zero memory with a zero or positive baseline must not pass by
    // accident; only a positive Python baseline can be halved.
    assert!(!passes_gate(0, 0, 0, 10.0, 9.0));
}
