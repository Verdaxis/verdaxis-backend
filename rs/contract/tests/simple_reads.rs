//! Task 8 group parity for the simple public reads (R01-R05).
//!
//! The disposable runner supplies both endpoint URLs and the token through the
//! environment, so this target is compiled only with the `contract-http`
//! feature. A plain `cargo test --workspace` run compiles the file to nothing
//! and never needs a live server, which is why the whole test carries the
//! feature gate instead of a manifest entry.
//!
//! The plan names one `simple_reads` group for R01-R05; the implemented runner
//! dispatches groups, not per-route targets, so the port, catalog, and news
//! routes are compared as that single group.

#[cfg(feature = "contract-http")]
#[test]
fn simple_reads_match_python() {
    verdaxis_contract::compare_group("simple_reads").expect("simple_reads contract group");
}
