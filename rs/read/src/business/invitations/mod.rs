//! Administrator invitations (Phase 4 Task 5): A04 organization list, A05
//! create-or-rotate and A06 public resolve.
//!
//! Shared-file note: `rs/read/src/business/mod.rs` is integration-owned, so
//! this module is not yet reachable as `business::invitations`. The lane's
//! owner patch adds `pub mod invitations;` there; until integration applies
//! it, the test target includes these files by path. No route is mounted by
//! this module alone.

pub mod request;
pub mod resolve;
pub mod store;
