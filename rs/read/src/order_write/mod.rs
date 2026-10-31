//! The single sealed market row-write boundary.
//!
//! Standalone order commands use [`execute_order_command`]. Ordered business
//! commands enter only through the borrowed [`invalidate_organization`]
//! subcommand after business-owner organization/user locks; that subcommand
//! never owns completion or retries. No module outside `order_write` locks or
//! changes order/trade/inventory rows.

mod benchmarks;
pub mod command;
mod events;
mod invalidation;
pub mod locks;
mod reservations;
mod transaction;

pub use benchmarks::{
    BenchmarkRebuildKey, BenchmarkRebuildResult, rebuild_live_slice_benchmarks_for_keys,
};
pub use invalidation::{
    InvalidateOrganization, InvalidationError, InvalidationResult, invalidate_organization,
};

pub use command::{
    Canonical, CanonicalError, CommandContext, LockPlan, Operation, OrderCommand, OutboxEvent,
    Receipt, ReceiptRecord, command_request_hash, idempotency_request_hash, python_json,
};
pub use locks::{
    IDEMPOTENCY_KEY_BOUNDS, IDEMPOTENCY_KEY_MAX, IDEMPOTENCY_KEY_MIN, IDEMPOTENCY_LOCK_TIMEOUT,
    IDEMPOTENCY_STATEMENT_TIMEOUT, MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT, SliceKey,
    idempotency_lock_key, market_slice_lock_key, market_slice_lock_sort_key,
    normalize_idempotency_key, normalize_slice_window, ordered_slice_lock_keys,
};
pub use transaction::{
    CommittedCommand, DEFAULT_MARKET_TRANSACTION_ATTEMPTS, IDEMPOTENCY_BUSY_MESSAGE,
    IDEMPOTENCY_TIMEOUT_MESSAGE, LOCK_BUSY_SQLSTATE, LOCK_TIMEOUT_SQLSTATE,
    MARKET_ORGANIZATION_REQUIRED, MAX_MARKET_TRANSACTION_ATTEMPTS, ORDER_NOT_FOUND_MESSAGE,
    REPLAY_CONFLICT_MESSAGE, RETRYABLE_SQLSTATES, SERIALIZATION_FAILED_MESSAGE, SLICE_BUSY_MESSAGE,
    SLICE_TIMEOUT_MESSAGE, execute_order_command,
};

#[cfg(feature = "contract-tests")]
pub use transaction::contract::serve_contract;
