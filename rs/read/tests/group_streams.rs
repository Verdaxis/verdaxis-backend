//! Stream store, bus, dispatch and HTTP tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[cfg(feature = "contract-tests")]
#[path = "streams_auth.rs"]
mod streams_auth;
#[cfg(feature = "contract-tests")]
#[path = "streams_bus.rs"]
mod streams_bus;
#[cfg(feature = "contract-tests")]
#[path = "streams_coexistence.rs"]
mod streams_coexistence;
#[cfg(feature = "contract-tests")]
#[path = "streams_dispatch.rs"]
mod streams_dispatch;
#[cfg(feature = "contract-tests")]
#[path = "streams_election.rs"]
mod streams_election;
#[path = "streams_heartbeat.rs"]
mod streams_heartbeat;
#[cfg(feature = "contract-tests")]
#[path = "streams_http.rs"]
mod streams_http;
#[cfg(feature = "contract-tests")]
#[path = "streams_replay.rs"]
mod streams_replay;
#[cfg(feature = "contract-tests")]
#[path = "streams_runtime.rs"]
mod streams_runtime;
#[cfg(feature = "contract-tests")]
#[path = "streams_store.rs"]
mod streams_store;
#[path = "streams_watchlist_guard.rs"]
mod streams_watchlist_guard;
