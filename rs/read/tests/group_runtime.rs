//! Runtime, admission and HTTP contract tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[path = "ad01_limiter.rs"]
mod ad01_limiter;
#[cfg(feature = "contract-tests")]
#[path = "admission_postgres.rs"]
mod admission_postgres;
#[path = "host_validation.rs"]
mod host_validation;
#[path = "http_contract.rs"]
mod http_contract;
#[path = "identity.rs"]
mod identity;
#[path = "policy.rs"]
mod policy;
#[path = "request_log.rs"]
mod request_log;
#[path = "runtime.rs"]
mod runtime;
#[cfg(feature = "contract-tests")]
#[path = "runtime_postgres.rs"]
mod runtime_postgres;
