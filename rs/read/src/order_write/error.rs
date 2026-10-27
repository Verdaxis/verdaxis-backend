//! The single error type and the single SQLSTATE mapping of the write boundary
//! (Phases 8-9 plan §6.1, §8.5, §11.2 and §11.3; Task 2).
//!
//! Every write handler returns [`MarketWriteError`]. Its only database
//! constructor is `From<sqlx::Error>`, so no handler reads a SQLSTATE, formats a
//! database message or invents a second classification. Every code, its status
//! and its text are one row of [`market_error_codes`]; adding a code means
//! adding one row there.
//!
//! Errors reach a client as `{"detail": {"code", "message"}}`. A SQLSTATE, a
//! constraint name and the server's own text never appear in a response or a
//! log line.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Declare every stable market write error code, with its HTTP status and text.
macro_rules! market_error_codes {
    ($( $variant:ident => $name:literal, $status:expr, $message:literal; )*) => {
        /// Every stable error code a market write route can return.
        ///
        /// The names are plan §11.3 plus the three value codes §6.1 names for
        /// the monetary domains (`VALUE_OUT_OF_RANGE`, `VALUE_SCALE_INVALID`,
        /// `VALUE_OVERFLOW`), so the table is the whole outward vocabulary.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum MarketErrorCode {
            $( $variant, )*
        }

        impl MarketErrorCode {
            /// Every code, in declaration order. Totality is a test, not a hope.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )* ];

            /// The `UPPER_SNAKE` code a client reads.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $name, )*
                }
            }

            /// The refusal status the code always carries.
            pub const fn status(self) -> StatusCode {
                match self {
                    $( Self::$variant => $status, )*
                }
            }

            /// Clear text for this code. Wording is free; the code is not.
            pub const fn message(self) -> &'static str {
                match self {
                    $( Self::$variant => $message, )*
                }
            }

            /// Only a serialization failure or a deadlock may be retried with a
            /// full rerun in a fresh transaction (§8.5). The mapping gives both
            /// the same outward code, so this is the retry class itself.
            pub const fn is_retryable(self) -> bool {
                matches!(self, Self::TransactionRetryExhausted)
            }
        }
    };
}

market_error_codes! {
    OrderNotFound => "ORDER_NOT_FOUND", StatusCode::NOT_FOUND, "Order not found";
    OrderOwnerForbidden => "ORDER_OWNER_FORBIDDEN", StatusCode::FORBIDDEN,
        "The order is not owned by this actor";
    IfMatchRequired => "IF_MATCH_REQUIRED", StatusCode::PRECONDITION_REQUIRED,
        "If-Match is required";
    IfMatchMalformed => "IF_MATCH_MALFORMED", StatusCode::BAD_REQUEST, "If-Match is malformed";
    OrderVersionStale => "ORDER_VERSION_STALE", StatusCode::PRECONDITION_FAILED,
        "Listing changed; reload and retry";
    MarketSupportContextInvalid => "MARKET_SUPPORT_CONTEXT_INVALID", StatusCode::CONFLICT,
        "The support context is not valid";
    NotFound => "NOT_FOUND", StatusCode::NOT_FOUND, "Not found";
    AuthCredentialsInvalid => "AUTH_CREDENTIALS_INVALID", StatusCode::UNAUTHORIZED,
        "The credentials are not valid";
    AuthPasswordChanged => "AUTH_PASSWORD_CHANGED", StatusCode::UNAUTHORIZED,
        "Password was changed; sign in again";
    AuthSessionRevoked => "AUTH_SESSION_REVOKED", StatusCode::UNAUTHORIZED,
        "Session was revoked; sign in again";
    ExecutionNotAvailable => "EXECUTION_NOT_AVAILABLE", StatusCode::FORBIDDEN,
        "Execution access is not available";
    KycRequired => "KYC_REQUIRED", StatusCode::FORBIDDEN,
        "Complete KYC verification to trade";
    TradePartyForbidden => "TRADE_PARTY_FORBIDDEN", StatusCode::FORBIDDEN,
        "The actor does not own this recorded trade side";
    TradePartiesInvalid => "TRADE_PARTIES_INVALID", StatusCode::CONFLICT,
        "The recorded trade parties are not valid";
    TradeNotFound => "TRADE_NOT_FOUND", StatusCode::NOT_FOUND,
        "The trade was not found";
    MarketBusy => "MARKET_BUSY", StatusCode::CONFLICT,
        "The market rows are busy; retry the request";
    MarketTimeout => "MARKET_TIMEOUT", StatusCode::SERVICE_UNAVAILABLE,
        "The market lock timed out; retry the request";
    IdempotencyBusy => "IDEMPOTENCY_BUSY", StatusCode::SERVICE_UNAVAILABLE,
        "The idempotency key is busy; retry the same request";
    IdempotencyConflict => "IDEMPOTENCY_CONFLICT", StatusCode::CONFLICT,
        "The idempotency key was reused with a different request";
    ReceiptResourceMismatch => "RECEIPT_RESOURCE_MISMATCH", StatusCode::CONFLICT,
        "The stored receipt belongs to a different resource";
    ReplayPrincipalMismatch => "REPLAY_PRINCIPAL_MISMATCH", StatusCode::FORBIDDEN,
        "The stored receipt belongs to a different principal";
    CommitOutcomeUnknown => "COMMIT_OUTCOME_UNKNOWN", StatusCode::SERVICE_UNAVAILABLE,
        "The commit outcome is unknown; retry with the same key";
    TransactionRetryExhausted => "TRANSACTION_RETRY_EXHAUSTED", StatusCode::SERVICE_UNAVAILABLE,
        "The market transaction could not settle; retry the request";
    OrderTerminal => "ORDER_TERMINAL", StatusCode::CONFLICT,
        "The order is no longer live";
    OrderUnavailable => "ORDER_UNAVAILABLE", StatusCode::CONFLICT,
        "The order is not available";
    QuantityExceedsRemaining => "QUANTITY_EXCEEDS_REMAINING", StatusCode::CONFLICT,
        "The quantity exceeds the remaining order quantity";
    QuantityOutOfRange => "QUANTITY_OUT_OF_RANGE", StatusCode::UNPROCESSABLE_ENTITY,
        "The quantity is outside the permitted range";
    QuantityScaleInvalid => "QUANTITY_SCALE_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The quantity has too many decimal places";
    PriceOutOfRange => "PRICE_OUT_OF_RANGE", StatusCode::UNPROCESSABLE_ENTITY,
        "The price is outside the permitted range";
    PriceScaleInvalid => "PRICE_SCALE_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The price has too many decimal places";
    FeeRateInvalid => "FEE_RATE_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The fee rate is outside the permitted range";
    CommissionRateInvalid => "COMMISSION_RATE_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The commission rate is outside the permitted range";
    InventoryInconsistent => "INVENTORY_INCONSISTENT", StatusCode::CONFLICT,
        "The inventory state is inconsistent";
    InventoryReserved => "INVENTORY_RESERVED", StatusCode::CONFLICT,
        "The inventory quantity is reserved";
    InventoryInUse => "INVENTORY_IN_USE", StatusCode::CONFLICT,
        "The inventory item is in use";
    InventoryReferenced => "INVENTORY_REFERENCED", StatusCode::CONFLICT,
        "The inventory item is referenced";
    InventoryNotFound => "INVENTORY_NOT_FOUND", StatusCode::NOT_FOUND,
        "The inventory item was not found";
    PortNotFound => "PORT_NOT_FOUND", StatusCode::NOT_FOUND,
        "The port was not found";
    TradeNotPending => "TRADE_NOT_PENDING", StatusCode::CONFLICT,
        "The trade is not pending confirmation";
    TradeNotDelivered => "TRADE_NOT_DELIVERED", StatusCode::CONFLICT,
        "The trade is not delivered";
    TradeFrozen => "TRADE_FROZEN", StatusCode::CONFLICT,
        "The trade is frozen";
    TradeAlreadyFrozen => "TRADE_ALREADY_FROZEN", StatusCode::CONFLICT,
        "The trade is already frozen";
    TradeNotFrozen => "TRADE_NOT_FROZEN", StatusCode::CONFLICT,
        "The trade is not frozen";
    TradeNotSettleable => "TRADE_NOT_SETTLEABLE", StatusCode::CONFLICT,
        "The trade cannot be settled in its current state";
    FreezeHistoryImmutable => "FREEZE_HISTORY_IMMUTABLE", StatusCode::CONFLICT,
        "The freeze history is immutable";
    DeliveryQuantityInvalid => "DELIVERY_QUANTITY_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The delivered quantity is invalid";
    DeliveryPriceDeviation => "DELIVERY_PRICE_DEVIATION", StatusCode::UNPROCESSABLE_ENTITY,
        "The final price deviates more than ten percent";
    OrderFieldNull => "ORDER_FIELD_NULL", StatusCode::UNPROCESSABLE_ENTITY,
        "An economic order field cannot be null";
    OrderIdentityImmutable => "ORDER_IDENTITY_IMMUTABLE", StatusCode::UNPROCESSABLE_ENTITY,
        "An order identity field cannot change";
    MatchFanoutExceeded => "MATCH_FANOUT_EXCEEDED", StatusCode::CONFLICT,
        "The match exceeds the crossing-order bound";
    MarketSliceChanged => "MARKET_SLICE_CHANGED", StatusCode::CONFLICT,
        "The market slice changed during the command";
    PostOnlyWouldCross => "POST_ONLY_WOULD_CROSS", StatusCode::CONFLICT,
        "A post-only order would cross the book";
    PostOnlyCheckIndeterminate => "POST_ONLY_CHECK_INDETERMINATE", StatusCode::CONFLICT,
        "The post-only check could not be decided";
    MarketSupportLegacyMutationRetired => "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED",
        StatusCode::GONE, "The legacy market support mutation is retired";
    SupportForbidden => "FORBIDDEN", StatusCode::FORBIDDEN, "Administrator authority is required";
    SupportCapabilityRequired => "MARKET_SUPPORT_CAPABILITY_REQUIRED", StatusCode::FORBIDDEN, "A current support capability is required";
    SupportTargetInvalid => "MARKET_SUPPORT_TARGET_INVALID", StatusCode::CONFLICT, "The target must be an approved administrator";
    SupportAssignmentNotFound => "CAPABILITY_ASSIGNMENT_NOT_FOUND", StatusCode::NOT_FOUND, "Capability assignment not found";
    SupportOrganizationNotFound => "ORGANIZATION_NOT_FOUND", StatusCode::NOT_FOUND, "Organization not found";
    SupportOrganizationNotApproved => "ORGANIZATION_NOT_APPROVED", StatusCode::CONFLICT, "The organization is not approved";
    SupportOrganizationNotReal => "ORGANIZATION_NOT_REAL", StatusCode::CONFLICT, "Support is limited to real organizations";
    SupportIdentitiesUnavailable => "MARKET_SUPPORT_IDENTITIES_UNAVAILABLE", StatusCode::CONFLICT,
        "Support context identities are not available";
    SupportContextInvalid => "SUPPORT_CONTEXT_INVALID", StatusCode::CONFLICT,
        "The support context is not valid for this action";
    ContextReplacementRequired => "MARKET_SUPPORT_CONTEXT_REPLACEMENT_REQUIRED", StatusCode::CONFLICT,
        "A replacement support context is required";
    ValidationFailed => "VALIDATION_FAILED", StatusCode::UNPROCESSABLE_ENTITY,
        "The request could not be validated";
    DatabaseError => "DATABASE_ERROR", StatusCode::INTERNAL_SERVER_ERROR,
        "The database operation could not complete";
    ValueOutOfRange => "VALUE_OUT_OF_RANGE", StatusCode::UNPROCESSABLE_ENTITY,
        "The monetary value is outside the permitted range";
    ValueScaleInvalid => "VALUE_SCALE_INVALID", StatusCode::UNPROCESSABLE_ENTITY,
        "The monetary value has too many decimal places";
    ValueOverflow => "VALUE_OVERFLOW", StatusCode::UNPROCESSABLE_ENTITY,
        "Arithmetic on the monetary value overflowed";
}

/// The one error a market write handler returns.
#[derive(Debug)]
pub struct MarketWriteError {
    pub status: StatusCode,
    pub code: MarketErrorCode,
    pub message: &'static str,
    retry_sqlstate: Option<&'static str>,
}

impl MarketWriteError {
    /// The refusal this code always carries.
    pub const fn new(code: MarketErrorCode) -> Self {
        Self {
            status: code.status(),
            code,
            message: code.message(),
            retry_sqlstate: None,
        }
    }

    /// Refine only lock contention after the sole SQLSTATE conversion.
    pub fn from_lock(error: sqlx::Error, idempotency: bool) -> Self {
        let converted = Self::from(error);
        if idempotency && converted.code == MarketErrorCode::MarketBusy {
            Self::new(MarketErrorCode::IdempotencyBusy)
        } else {
            converted
        }
    }

    #[cfg(any(test, feature = "contract-tests"))]
    pub fn lock_code_for_test(state: &str, idempotency: bool) -> Self {
        let code = classify(Some(state), None);
        Self::new(if idempotency && code == MarketErrorCode::MarketBusy {
            MarketErrorCode::IdempotencyBusy
        } else {
            code
        })
    }

    /// Whether a full rerun in a fresh transaction may repeat the command.
    pub const fn is_retryable(&self) -> bool {
        self.retry_sqlstate.is_some()
    }
}

impl std::fmt::Display for MarketWriteError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for MarketWriteError {}

/// The sole SQLSTATE mapping point (§8.5, §11.2).
///
/// Two classes are decided by the code alone: a serialization failure or a
/// deadlock (`40001`/`40P01`, the only retryable pair), then lock contention
/// (`55P03`) and cancellation (`57014`). Unique violations depend on which
/// constraint fired: the receipt keys are idempotency conflicts and the one
/// active freeze is a frozen trade; every other constraint is an unclassified
/// database failure. Everything else is a check violation, one of the freeze
/// guard's codes, or an unclassified database failure. No server text,
/// SQLSTATE or constraint name survives the conversion.
impl From<sqlx::Error> for MarketWriteError {
    fn from(error: sqlx::Error) -> Self {
        let Some(failure) = error.as_database_error() else {
            // A protocol, connection or pool failure never reached the server.
            return Self::new(MarketErrorCode::DatabaseError);
        };
        let sqlstate = failure.code().map(|code| code.into_owned());
        let constraint = failure.constraint().map(str::to_owned);
        let mut converted = Self::new(classify(sqlstate.as_deref(), constraint.as_deref()));
        converted.retry_sqlstate = match sqlstate.as_deref() {
            Some("40001") => Some("40001"),
            Some("40P01") => Some("40P01"),
            _ => None,
        };
        converted
    }
}

/// The unique constraints a `23505` turns into its own code (§11.2).
///
/// Only the receipt keys and the one active freeze are named by the plan.
/// Every other unique violation — the acceptance ordinal, the support
/// authorization — is an unclassified database failure; the receipt race they
/// guard belongs to the receipt protocol, not this table.
const UNIQUE_CONSTRAINT_CODES: [(&str, MarketErrorCode); 4] = [
    (
        "uq_market_command_results_actor_operation_key",
        MarketErrorCode::IdempotencyConflict,
    ),
    (
        "uq_orderbook_orders_org_operation_idempotency",
        MarketErrorCode::IdempotencyConflict,
    ),
    (
        "uq_trades_initiator_operation_idempotency",
        MarketErrorCode::IdempotencyConflict,
    ),
    (
        "uq_trade_freezes_one_active",
        MarketErrorCode::TradeAlreadyFrozen,
    ),
];

fn classify(sqlstate: Option<&str>, constraint: Option<&str>) -> MarketErrorCode {
    match sqlstate {
        Some("40001" | "40P01") => MarketErrorCode::TransactionRetryExhausted,
        Some("55P03") => MarketErrorCode::MarketBusy,
        Some("57014") => MarketErrorCode::MarketTimeout,
        Some("23505") => constraint
            .and_then(|name| {
                UNIQUE_CONSTRAINT_CODES
                    .iter()
                    .find(|(constraint, _)| *constraint == name)
                    .map(|(_, code)| *code)
            })
            .unwrap_or(MarketErrorCode::DatabaseError),
        Some("23514") => MarketErrorCode::ValidationFailed,
        Some("ZF001") => MarketErrorCode::TradeFrozen,
        Some("ZF002" | "ZF003") => MarketErrorCode::FreezeHistoryImmutable,
        _ => MarketErrorCode::DatabaseError,
    }
}

impl IntoResponse for MarketWriteError {
    fn into_response(self) -> Response {
        let code = self.code.as_str();
        let detail = serde_json::json!({
            "code": code,
            "message": self.message,
        });
        let mut response = (
            self.status,
            axum::Json(serde_json::json!({ "detail": detail })),
        )
            .into_response();
        // Every support-context refusal is also marked in headers, so a client
        // can leave a context that is no longer valid.
        if code.starts_with("MARKET_SUPPORT_CONTEXT_") {
            let headers = response.headers_mut();
            headers.insert(
                "x-verdaxis-market-support-context-invalid",
                axum::http::HeaderValue::from_static("true"),
            );
            headers.insert(
                "x-verdaxis-market-support-context-code",
                axum::http::HeaderValue::from_static(code),
            );
        }
        response
    }
}
