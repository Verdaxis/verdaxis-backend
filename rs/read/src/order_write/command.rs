//! One typed order-book command, its lock plan and its committed result.
//!
//! The request hash is Python-compatible by design. The reviewed oracle hashes
//! `json.dumps(canonical, allow_nan=False, sort_keys=True, separators=(",", ":"))`
//! with `ensure_ascii=True` after normalizing typed values, so a receipt this
//! boundary stores compares equal to one the Python path stored. A hash with
//! different bytes would let the same request execute twice.
//!
//! `/order_write` is the one place an order-book command may be built. Nothing
//! here opens a transaction, takes a lock or writes a row; `transaction` owns
//! that, and it is the only module in the crate allowed to.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Number, Value};
use std::str::FromStr;
use uuid::Uuid;

use super::locks::{SliceKey, sha256_hex};

/// The six commands a receipt may record. The listed string is the reviewed
/// `market_command_results.operation` value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    OrderAmend,
    OrderCancel,
    TradeConfirm,
    TradeDecline,
    TradeDeliver,
    TradePay,
}

impl Operation {
    pub const ALL: [Operation; 6] = [
        Operation::OrderAmend,
        Operation::OrderCancel,
        Operation::TradeConfirm,
        Operation::TradeDecline,
        Operation::TradeDeliver,
        Operation::TradePay,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Operation::OrderAmend => "order.amend",
            Operation::OrderCancel => "order.cancel",
            Operation::TradeConfirm => "trade.confirm",
            Operation::TradeDecline => "trade.decline",
            Operation::TradeDeliver => "trade.deliver",
            Operation::TradePay => "trade.pay",
        }
    }

    /// The `market_command_results.resource_type` value this operation stores.
    pub fn resource_type(self) -> &'static str {
        match self {
            Operation::OrderAmend | Operation::OrderCancel => "order",
            Operation::TradeConfirm
            | Operation::TradeDecline
            | Operation::TradeDeliver
            | Operation::TradePay => "trade",
        }
    }
}

/// A refusal raised while encoding or checking a request payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalError(pub String);

impl fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CanonicalError {}

/// One typed request value in the exact set the receipt-backed commands carry.
///
/// `Int` holds a JSON integer literal (including a value wider than `i64`);
/// [`Canonical::from_json`] refuses fractional and exponent numbers, so the
/// Python `repr` of a binary float can never become a receipt hash.
#[derive(Clone, Debug, PartialEq)]
pub enum Canonical {
    Null,
    Bool(bool),
    Int(Number),
    Text(String),
    Decimal(Decimal),
    Uuid(Uuid),
    Timestamp(DateTime<Utc>),
    Date(NaiveDate),
    Object(BTreeMap<String, Canonical>),
    Array(Vec<Canonical>),
}

fn tagged(message: &str) -> CanonicalError {
    CanonicalError(message.to_owned())
}

impl Canonical {
    /// Build from the tagged form both stacks share: `{"k": kind, "v": value}`.
    ///
    /// The tag removes every cross-language ambiguity (a Python `Decimal`
    /// against a binary float, a `UUID` against a string) while staying
    /// readable in review.
    pub fn from_tagged(node: &Value) -> Result<Self, CanonicalError> {
        let object = node
            .as_object()
            .ok_or_else(|| tagged("tagged value must be an object"))?;
        let kind = object
            .get("k")
            .and_then(Value::as_str)
            .ok_or_else(|| tagged("tagged value must carry a kind"))?;
        let payload = object.get("v");
        Ok(match kind {
            "null" => Canonical::Null,
            "bool" => Canonical::Bool(
                payload
                    .and_then(Value::as_bool)
                    .ok_or_else(|| tagged("tagged bool must carry a boolean"))?,
            ),
            "int" => Canonical::Int(match payload {
                Some(Value::Number(number)) => number.clone(),
                _ => return Err(tagged("tagged int must carry a number")),
            }),
            "text" => Canonical::Text(
                payload
                    .and_then(Value::as_str)
                    .ok_or_else(|| tagged("tagged text must carry a string"))?
                    .to_owned(),
            ),
            "decimal" => Canonical::Decimal(
                Decimal::from_str(
                    payload
                        .and_then(Value::as_str)
                        .ok_or_else(|| tagged("tagged decimal must carry a string"))?,
                )
                .map_err(|_| tagged("tagged decimal is not a decimal"))?,
            ),
            "uuid" => Canonical::Uuid(
                Uuid::parse_str(
                    payload
                        .and_then(Value::as_str)
                        .ok_or_else(|| tagged("tagged uuid must carry a string"))?,
                )
                .map_err(|_| tagged("tagged uuid is not a uuid"))?,
            ),
            "timestamp" => Canonical::Timestamp(
                DateTime::parse_from_rfc3339(
                    payload
                        .and_then(Value::as_str)
                        .ok_or_else(|| tagged("tagged timestamp must carry a string"))?,
                )
                .map_err(|_| tagged("tagged timestamp is not RFC 3339"))?
                .with_timezone(&Utc),
            ),
            "date" => Canonical::Date(
                NaiveDate::parse_from_str(
                    payload
                        .and_then(Value::as_str)
                        .ok_or_else(|| tagged("tagged date must carry a string"))?,
                    "%Y-%m-%d",
                )
                .map_err(|_| tagged("tagged date is not ISO 8601"))?,
            ),
            "object" => {
                let map = payload
                    .and_then(Value::as_object)
                    .ok_or_else(|| tagged("tagged object must carry an object"))?;
                let mut out = BTreeMap::new();
                for (key, item) in map {
                    out.insert(key.clone(), Canonical::from_tagged(item)?);
                }
                Canonical::Object(out)
            }
            "array" => {
                let items = payload
                    .and_then(Value::as_array)
                    .ok_or_else(|| tagged("tagged array must carry an array"))?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(Canonical::from_tagged(item)?);
                }
                Canonical::Array(out)
            }
            other => return Err(CanonicalError(format!("unknown tagged kind: {other}"))),
        })
    }

    /// Build from a JSON-native value, refusing anything Python would have to
    /// render through `repr` rather than `int`.
    pub fn from_json(value: &Value) -> Result<Self, CanonicalError> {
        Ok(match value {
            Value::Null => Canonical::Null,
            Value::Bool(flag) => Canonical::Bool(*flag),
            Value::Number(number) => {
                let literal = number.to_string();
                if literal.contains(['.', 'e', 'E']) {
                    return Err(tagged("market write payloads must not carry binary floats"));
                }
                Canonical::Int(number.clone())
            }
            Value::String(text) => Canonical::Text(text.clone()),
            Value::Object(map) => {
                let mut out = BTreeMap::new();
                for (key, item) in map {
                    out.insert(key.clone(), Canonical::from_json(item)?);
                }
                Canonical::Object(out)
            }
            Value::Array(items) => Canonical::Array(
                items
                    .iter()
                    .map(Canonical::from_json)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    /// Refuse the typed values that are not part of a `model_dump(mode="json")`
    /// payload. The idempotency hash fails closed rather than guessing a
    /// rendering the Python path would not produce.
    fn require_json_native(&self) -> Result<(), CanonicalError> {
        match self {
            Canonical::Decimal(_)
            | Canonical::Uuid(_)
            | Canonical::Timestamp(_)
            | Canonical::Date(_) => Err(tagged("idempotency payloads must be JSON-native values")),
            Canonical::Object(map) => map.values().try_for_each(Canonical::require_json_native),
            Canonical::Array(items) => items.iter().try_for_each(Canonical::require_json_native),
            _ => Ok(()),
        }
    }
}

impl<'de> Deserialize<'de> for Canonical {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let node = Value::deserialize(deserializer)?;
        Canonical::from_tagged(&node).map_err(serde::de::Error::custom)
    }
}

/// The exact UTF-8 byte image Python's `json.dumps` writes for one value with
/// `sort_keys=True`, `separators=(",", ":")` and its default `ensure_ascii=True`.
pub fn python_json(value: &Canonical) -> String {
    let mut out = String::new();
    write_python_json(value, &mut out);
    out
}

fn write_python_json(value: &Canonical, out: &mut String) {
    match value {
        Canonical::Null => out.push_str("null"),
        Canonical::Bool(true) => out.push_str("true"),
        Canonical::Bool(false) => out.push_str("false"),
        Canonical::Int(number) => out.push_str(&number.to_string()),
        Canonical::Text(text) => write_python_string(text, out),
        Canonical::Decimal(decimal) => write_python_string(&decimal.normalize().to_string(), out),
        Canonical::Uuid(id) => write_python_string(&id.to_string(), out),
        Canonical::Timestamp(instant) => write_python_string(&python_timestamp(instant), out),
        Canonical::Date(date) => write_python_string(&date.format("%Y-%m-%d").to_string(), out),
        Canonical::Object(map) => {
            out.push('{');
            for (position, (key, item)) in map.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write_python_string(key, out);
                out.push(':');
                write_python_json(item, out);
            }
            out.push('}');
        }
        Canonical::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write_python_json(item, out);
            }
            out.push(']');
        }
    }
}

/// Python `datetime.isoformat()` inside an explicit UTC zone: `timespec="auto"`
/// writes no fraction, three digits or six, then `Z` for the `+00:00` offset.
fn python_timestamp(instant: &DateTime<Utc>) -> String {
    let micros = instant.timestamp_subsec_nanos() / 1_000;
    let mut text = instant.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros != 0 {
        if micros.is_multiple_of(1_000) {
            text.push_str(&format!(".{:03}", micros / 1_000));
        } else {
            text.push_str(&format!(".{micros:06}"));
        }
    }
    text.push('Z');
    text
}

/// One JSON string body exactly as Python writes it with `ensure_ascii=True`:
/// `\\`, `"`, the six named escapes, every other code point outside
/// `0x20..=0x7e` as `\uXXXX`, and an astral code point as its surrogate pair.
fn write_python_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (' '..='~').contains(&character) => out.push(character),
            character => {
                let code = character as u32;
                if code < 0x1_0000 {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    let beyond = code - 0x1_0000;
                    let high = 0xd800 + (beyond >> 10);
                    let low = 0xdc00 + (beyond & 0x3ff);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// SHA-256 of [`python_json`]. This is `command_request_hash` in
/// `app/services/command_results.py`.
pub fn command_request_hash(payload: &Canonical) -> String {
    sha256_hex(python_json(payload).as_bytes())
}

/// SHA-256 of the same encoding for a JSON-native payload. This is
/// `idempotency_request_hash` in `app/services/idempotency.py`.
pub fn idempotency_request_hash(payload: &Canonical) -> Result<String, CanonicalError> {
    payload.require_json_native()?;
    Ok(sha256_hex(python_json(payload).as_bytes()))
}

/// The rows the boundary locks, in the order the operation acquires them.
///
/// Order is significant and is never replaced with a global identifier order:
/// the reviewed routes lock one market row at a time, in an order that differs
/// per operation, and a changed order is a new deadlock graph.
#[derive(Clone, Debug, Default)]
pub struct LockPlan {
    /// Affected market slices. The boundary deduplicates and sorts these.
    pub slices: Vec<SliceKey>,
    /// `orderbook_orders` rows, in acquisition order.
    pub order_rows: Vec<Uuid>,
    /// Actor user rows. The boundary locks these in ascending id order.
    pub actor_users: Vec<Uuid>,
    /// Organization rows, locked after the user rows in ascending id order.
    pub organizations: Vec<Uuid>,
}

/// One participant-scoped event the committed transaction must emit.
#[derive(Clone, Debug)]
pub struct OutboxEvent {
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub participant_org_ids: Vec<String>,
    pub payload: Value,
}

/// One typed order-book command.
#[derive(Clone, Debug)]
pub struct OrderCommand {
    pub operation: Operation,
    pub resource_id: Uuid,
    /// The typed request payload the receipt hash covers.
    pub payload: Canonical,
    pub locks: LockPlan,
    /// Events emitted by this command. A replay emits none.
    pub events: Vec<OutboxEvent>,
    pub response_status: u16,
    pub response_body: Value,
}

/// The authenticated caller's command context.
///
/// `effective_organization_id` is `None` when no organization applies; the
/// boundary refuses the command in that case rather than writing a receipt
/// without a market owner.
#[derive(Clone, Debug)]
pub struct CommandContext {
    pub actor_user_id: Uuid,
    pub effective_organization_id: Option<Uuid>,
    pub support_context_id: Option<Uuid>,
    /// The raw `Idempotency-Key` header value, if the request carried one.
    pub idempotency_key: Option<String>,
}

/// One stored receipt, read back for the replay decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    pub request_hash: String,
    pub effective_organization_id: Uuid,
    pub support_context_id: Option<Uuid>,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub response_status: u16,
    pub response_body: Value,
}

/// One receipt row to insert with the successful economic transaction.
#[derive(Clone, Debug)]
pub struct ReceiptRecord {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub effective_organization_id: Uuid,
    pub support_context_id: Option<Uuid>,
    pub operation: Operation,
    pub idempotency_key: String,
    pub request_hash: String,
    pub resource_id: Uuid,
    pub response_status: u16,
    pub response_body: Value,
}
