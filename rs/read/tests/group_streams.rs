//! Stream store, bus, dispatch and HTTP tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[cfg(feature = "contract-tests")]
#[path = "streams_auth.rs"]
pub mod streams_auth;
#[cfg(feature = "contract-tests")]
#[path = "streams_bus.rs"]
pub mod streams_bus;
#[cfg(feature = "contract-tests")]
#[path = "streams_coexistence.rs"]
pub mod streams_coexistence;
#[cfg(feature = "contract-tests")]
#[path = "streams_dispatch.rs"]
pub mod streams_dispatch;
#[cfg(feature = "contract-tests")]
#[path = "streams_election.rs"]
pub mod streams_election;
#[path = "streams_heartbeat.rs"]
pub mod streams_heartbeat;
#[cfg(feature = "contract-tests")]
#[path = "streams_http.rs"]
pub mod streams_http;
#[cfg(feature = "contract-tests")]
#[path = "streams_replay.rs"]
pub mod streams_replay;
#[cfg(feature = "contract-tests")]
#[path = "streams_runtime.rs"]
pub mod streams_runtime;
#[cfg(feature = "contract-tests")]
#[path = "streams_store.rs"]
pub mod streams_store;
#[path = "streams_watchlist_guard.rs"]
pub mod streams_watchlist_guard;
