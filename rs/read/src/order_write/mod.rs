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

// Plan §6.1: no float arithmetic anywhere in the write boundary. The copy in
// `decimal.rs` keeps the rule true for that file under any module owner.
#![deny(clippy::float_arithmetic)]

mod authority;
pub mod command;
pub mod decimal;
pub mod effects;
mod error;
mod expiry;
pub mod locks;
pub mod outbox;
mod transaction;

pub use authority::admission::{
    AdmissionClass, PartyRef, authenticate, cleanup_context_for_trade, execution_context,
    require_cleanup, require_execution,
};
pub use authority::{AuthenticatedActor, SupportContextRef, WriteContext};
pub use command::{
    Canonical, CanonicalError, CommandContext, LockPlan, Operation, OrderCommand, OutboxEvent,
    Receipt, ReceiptRecord, command_request_hash, idempotency_request_hash, python_json,
};
pub use decimal::{
    AggregateMoney, CommissionPercent, Domain, ExactValue, FeePerMt, Money, OrderQuantity,
    OrderRemainder, Price, StockQuantity, TradeQuantity, checked_add, checked_multiply,
    checked_subtract, cmp_percentage, quantize_cents,
};
pub use error::{MarketErrorCode, MarketWriteError};
pub use expiry::{ExpiryEffect, ExpirySlice, MAX_EXPIRY_ORDERS};
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

/// Feature-only access to the integrated transaction test ports.
#[cfg(feature = "contract-tests")]
pub mod test_contract {
    pub use super::{MarketErrorCode, MarketWriteError, StockQuantity};
    pub mod authority {
        pub use super::super::authority::{ActorRole, AuthenticatedActor, TradeSide};
    }
    pub mod admission {
        pub use super::super::authority::admission::*;
    }
    pub mod error {
        pub use super::super::{MarketErrorCode, MarketWriteError};
    }
    pub mod locks {
        pub use super::super::locks::*;
    }
    pub mod transaction {
        pub use super::super::transaction::{
            BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
            install_acquisition_abort, protocol_hold_key, protocol_preview, run_market_command,
            run_paused_market_probe,
        };
    }
    pub use super::transaction::stock_contract;
    pub mod effects {
        pub use super::super::effects::MutationKind;
        pub use super::super::effects::contract::*;
    }
}

mod dismissal;
pub use dismissal::dismiss_suggestion;
