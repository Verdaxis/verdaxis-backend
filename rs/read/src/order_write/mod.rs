//! The single sealed market row-write boundary.
//!
//! Standalone order commands use [`execute_order_command`]. Ordered business
//! commands enter only through the borrowed [`invalidate_organization`] and
//! [`invalidate_user`] subcommands after business-owner organization/user
//! locks; those subcommands never own completion or retries. The order-expiry
//! reminder job takes its scan key and its user-then-order row locks through
//! the borrowed `reminders` functions and writes only notification rows itself.
//! No module outside `order_write` locks or changes order/trade/inventory rows.

// Plan §6.1: no float arithmetic anywhere in the write boundary. The copy in
// `decimal.rs` keeps the rule true for that file under any module owner.
#![deny(clippy::float_arithmetic)]

mod authority;
mod benchmarks;
pub mod command;
pub mod commission;
mod confirmed_effect;
pub mod decimal;
pub mod effects;
mod error;
mod events;
mod expiry;
pub mod freeze;
mod invalidation;
pub(crate) mod inventory;
pub use inventory::{InventoryAction, InventoryInput, execute_inventory};
/// The demo activity job's market phases (the sealed actorless system path).
pub use transaction::system::demo::{
    DemoMarketFailure, DemoMarketReport, TickOutcome, run_demo_market_phases,
};
pub(crate) mod amend;
pub mod locks;
pub mod matching;
pub(crate) mod order_create;
pub(crate) mod orders;
pub mod outbox;
pub mod party_policy;
mod post_only;
pub mod producer;
pub mod reassignment;
mod reminders;
mod reservations;
pub mod support;
pub mod trade_lifecycle;
pub mod trades;
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
    InvalidateOrganization, InvalidateUser, InvalidationError, InvalidationResult, UserScopeCounts,
    count_user_scope, invalidate_organization, invalidate_user,
};
pub use locks::{
    IDEMPOTENCY_KEY_BOUNDS, IDEMPOTENCY_KEY_MAX, IDEMPOTENCY_KEY_MIN, IDEMPOTENCY_LOCK_TIMEOUT,
    IDEMPOTENCY_STATEMENT_TIMEOUT, MARKET_LOCK_TIMEOUT, MARKET_STATEMENT_TIMEOUT, SliceKey,
    idempotency_lock_key, market_slice_lock_key, market_slice_lock_sort_key,
    normalize_idempotency_key, normalize_slice_window, ordered_slice_lock_keys,
};
pub use reminders::{
    REMINDER_SCAN_KEY, RecipientsLocked, ReminderCandidate, ReminderRow, ReminderWindow,
    lock_reminder_delivery, lock_reminder_recipients, reminder_candidates,
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
    pub use super::post_only::contract as post_only;
    pub use super::producer::contract as producer;
    pub use super::transaction::stock_contract;
    pub mod effects {
        pub use super::super::effects::MutationKind;
        pub use super::super::effects::contract::*;
    }
}

mod dismissal;
pub use dismissal::dismiss_suggestion;
