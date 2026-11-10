//! Runtime, admission and HTTP contract tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[path = "ad01_limiter.rs"]
pub mod ad01_limiter;
#[cfg(feature = "contract-tests")]
#[path = "admission_postgres.rs"]
pub mod admission_postgres;
#[path = "host_validation.rs"]
pub mod host_validation;
#[path = "http_contract.rs"]
pub mod http_contract;
#[path = "policy.rs"]
pub mod policy;
#[path = "request_log.rs"]
pub mod request_log;
#[path = "route_mounts.rs"]
pub mod route_mounts;
#[path = "runtime.rs"]
pub mod runtime;
#[cfg(feature = "contract-tests")]
#[path = "runtime_postgres.rs"]
pub mod runtime_postgres;
