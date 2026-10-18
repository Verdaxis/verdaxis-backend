//! The single sealed order-book write boundary (Phase 2, Task 20).
//!
//! One write transaction boundary: the command, its locks, its receipt, its
//! outbox and its bounded retry. It is deliberately narrow. There is no matcher
//! and no wider market lifecycle here, no route consumes it yet, and no module
//! outside `order_write` may lock or change an order-book row.
//!
//! [`execute_order_command`] is the only entry point. `command` holds the typed
//! data, `locks` holds the canonical lock identity, and `transaction` holds the
//! shell and its [`WriteSession`] port.

pub mod command;
pub mod locks;
mod transaction;

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
