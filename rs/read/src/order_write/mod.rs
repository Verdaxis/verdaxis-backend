//! The single sealed market row-write boundary.
//!
//! Standalone order commands use [`execute_order_command`]. Ordered business
//! commands enter only through the borrowed [`invalidate_organization`]
//! subcommand after business-owner organization/user locks; that subcommand
//! never owns completion or retries. No module outside `order_write` locks or
//! changes order/trade/inventory rows.

// Plan §6.1: no float arithmetic anywhere in the write boundary. The copy in
// `decimal.rs` keeps the rule true for that file under any module owner.
#![deny(clippy::float_arithmetic)]

mod authority;
mod benchmarks;
pub mod command;
pub mod decimal;
pub mod effects;
mod error;
mod events;
mod expiry;
mod invalidation;
pub mod locks;
pub mod matching;
pub mod outbox;
mod reservations;
mod transaction;

pub use authority::admission::{
    AdmissionClass, PartyRef, authenticate, cleanup_context_for_trade, execution_context,
    require_cleanup, require_execution,
};
pub use authority::{AuthenticatedActor, SupportContextRef, WriteContext};
pub use benchmarks::{
    BenchmarkRebuildKey, BenchmarkRebuildResult, rebuild_live_slice_benchmarks_for_keys,
};
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
pub use invalidation::{
    InvalidateOrganization, InvalidationError, InvalidationResult, invalidate_organization,
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
    pub use super::matching::contract as matching;
    pub use super::transaction::stock_contract;
    pub mod effects {
        pub use super::super::effects::MutationKind;
        pub use super::super::effects::contract::*;
    }
}

mod dismissal;
pub use dismissal::dismiss_suggestion;
