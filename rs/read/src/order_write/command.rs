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
/// writes six fractional digits when microseconds are nonzero, then `Z`.
fn python_timestamp(instant: &DateTime<Utc>) -> String {
    let micros = instant.timestamp_subsec_nanos() / 1_000;
    let mut text = instant.format("%Y-%m-%dT%H:%M:%S").to_string();
    if micros != 0 {
        text.push_str(&format!(".{micros:06}"));
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

/// Validate a receipt after the transaction owner has authorized the actor and
/// derived the effective organization (recorded participation for cleanup).
/// Actor/operation/key scope is enforced by the database lookup, not by a tenant
/// value from an HTTP request.
pub fn validate_receipt(
    receipt: &Receipt,
    context: &CommandContext,
    operation: Operation,
    resource_id: Uuid,
    request_hash: &str,
) -> Result<(), super::error::MarketWriteError> {
    use super::error::{MarketErrorCode, MarketWriteError};
    if receipt.resource_id != resource_id {
        return Err(MarketWriteError::new(
            MarketErrorCode::ReceiptResourceMismatch,
        ));
    }
    if receipt.request_hash != request_hash
        || Some(receipt.effective_organization_id) != context.effective_organization_id
        || receipt.support_context_id != context.support_context_id
        || receipt.resource_type != operation.resource_type()
        || !(200..300).contains(&receipt.response_status)
    {
        return Err(MarketWriteError::new(MarketErrorCode::IdempotencyConflict));
    }
    Ok(())
}

/// Publication replay is narrower than current organization membership. Both
/// concrete ownership and accountability must identify the authenticated actor.
pub fn validate_inventory_replay(
    actor: Uuid,
    owner_user_id: Option<Uuid>,
    accountable_actor_user_id: Option<Uuid>,
    stored_hash: &str,
    request_hash: &str,
) -> Result<(), super::error::MarketWriteError> {
    use super::error::{MarketErrorCode, MarketWriteError};
    if owner_user_id != Some(actor) || accountable_actor_user_id != Some(actor) {
        return Err(MarketWriteError::new(
            MarketErrorCode::ReplayPrincipalMismatch,
        ));
    }
    if stored_hash != request_hash {
        return Err(MarketWriteError::new(MarketErrorCode::IdempotencyConflict));
    }
    Ok(())
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

#[cfg(all(test, feature = "contract-tests"))]
mod replay_tests {
    use super::*;
    use crate::order_write::transaction::{self, ReplayFault};
    use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    struct Fixture {
        pool: sqlx::PgPool,
        admin: PgConnection,
        schema: String,
        actor: Uuid,
        org: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").unwrap();
            let runtime_url = std::env::var("RUST_TEST_DATABASE_URL").unwrap();
            let admin_target = url::Url::parse(&admin_url).unwrap();
            let runtime_target = url::Url::parse(&runtime_url).unwrap();
            assert_eq!(admin_target.host_str(), Some("127.0.0.1"));
            assert_eq!(runtime_target.host_str(), admin_target.host_str());
            assert_eq!(runtime_target.port(), admin_target.port());
            assert_eq!(runtime_target.path(), admin_target.path());
            assert!(runtime_target.path().ends_with("_test"));
            assert_eq!(runtime_target.username(), "rust_app");
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            let schema = format!("receipt_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(include_str!("../../tests/fixtures/phase8_receipts.sql"))
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT,INSERT ON ALL TABLES IN SCHEMA {schema} TO rust_app")))
                .execute(&mut admin).await.unwrap();
            let path = format!("SET search_path TO {schema}");
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .after_connect(move |conn, _| {
                    let path = path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                            .execute(conn)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&runtime_url)
                .await
                .unwrap();
            Self {
                pool,
                admin,
                schema,
                actor: Uuid::new_v4(),
                org: Uuid::new_v4(),
            }
        }

        fn request(&self, operation: Operation) -> (CommandContext, OrderCommand) {
            (
                CommandContext {
                    actor_user_id: self.actor,
                    effective_organization_id: Some(self.org),
                    support_context_id: None,
                    idempotency_key: Some(Uuid::new_v4().to_string()),
                },
                OrderCommand {
                    operation,
                    resource_id: Uuid::new_v4(),
                    payload: Canonical::Text("request".into()),
                    locks: LockPlan::default(),
                    events: vec![OutboxEvent {
                        event_type: "receipt.case".into(),
                        aggregate_type: "case".into(),
                        aggregate_id: Uuid::new_v4().to_string(),
                        participant_org_ids: vec![self.org.to_string()],
                        payload: serde_json::json!({"effect":1}),
                    }],
                    response_status: if operation == Operation::OrderCancel {
                        204
                    } else {
                        200
                    },
                    response_body: if operation == Operation::OrderCancel {
                        Value::Null
                    } else {
                        serde_json::json!({"protected":"stored"})
                    },
                },
            )
        }

        async fn run(
            &self,
            context: CommandContext,
            command: OrderCommand,
            fault: ReplayFault,
        ) -> transaction::ReplayProbe {
            transaction::replay_probe(self.pool.clone(), context, command, fault).await
        }

        async fn effects(&self) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
                .fetch_one(&self.pool)
                .await
                .unwrap()
        }

        async fn finish(mut self) {
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn receipt_rejects_wrong_resource_and_context() {
        let mut f = Fixture::new().await;
        let (context, command) = f.request(Operation::TradeDeliver);
        assert!(
            f.run(context.clone(), command.clone(), ReplayFault::None)
                .await
                .result
                .is_ok()
        );
        let mut wrong = command.clone();
        wrong.resource_id = Uuid::new_v4();
        let refused = f
            .run(context.clone(), wrong, ReplayFault::None)
            .await
            .result
            .unwrap_err();
        assert_eq!(refused.status.as_u16(), 409);
        assert_eq!(refused.detail["code"], "RECEIPT_RESOURCE_MISMATCH");
        assert!(!refused.detail.to_string().contains("protected"));
        for field in ["hash", "organization", "support", "type", "status"] {
            let mut ctx = context.clone();
            let mut cmd = command.clone();
            match field {
                "hash" => cmd.payload = Canonical::Text("different".into()),
                "organization" => ctx.effective_organization_id = Some(Uuid::new_v4()),
                "support" => ctx.support_context_id = Some(Uuid::new_v4()),
                "type" => {
                    sqlx::query("UPDATE market_command_results SET resource_type='order'")
                        .execute(&mut f.admin)
                        .await
                        .unwrap();
                }
                "status" => {
                    sqlx::query("UPDATE market_command_results SET response_status=500")
                        .execute(&mut f.admin)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let error = f.run(ctx, cmd, ReplayFault::None).await.result.unwrap_err();
            assert_eq!(error.status.as_u16(), 409);
            assert_eq!(error.detail["code"], "IDEMPOTENCY_CONFLICT");
            sqlx::query(
                "UPDATE market_command_results SET resource_type='trade',response_status=200",
            )
            .execute(&mut f.admin)
            .await
            .unwrap();
        }
        assert_eq!(f.effects().await, 1);
        f.finish().await;
    }

    #[tokio::test]
    async fn receipt_replay_matrix() {
        let f = Fixture::new().await;
        for operation in Operation::ALL {
            let (context, command) = f.request(operation);
            let first = f
                .run(context.clone(), command.clone(), ReplayFault::None)
                .await
                .result
                .unwrap();
            assert!(!first.replayed);
            // Admission is deliberately unavailable: the exact stored replay precedes it.
            let replay = f
                .run(context.clone(), command.clone(), ReplayFault::RejectFresh)
                .await
                .result
                .unwrap();
            assert!(replay.replayed);
            assert_eq!(replay.response_status, first.response_status);
            assert_eq!(replay.response_body, first.response_body);
            let mut stranger = context;
            stranger.actor_user_id = Uuid::new_v4();
            let denied = f
                .run(stranger, command, ReplayFault::RejectFresh)
                .await
                .result
                .unwrap_err();
            assert_eq!(denied.status.as_u16(), 403);
        }
        assert_eq!(f.effects().await, 6);
        f.finish().await;
    }

    #[test]
    fn receipt_hash_matches_python_vector() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../../../tests/rust_contract/phase8/hash-vectors.json"
        ))
        .unwrap();
        for vector in vectors["command_request_hashes"].as_array().unwrap() {
            let payload = Canonical::from_tagged(&vector["payload"]).unwrap();
            assert_eq!(
                command_request_hash(&payload),
                vector["sha256"].as_str().unwrap()
            );
        }
        for vector in vectors["idempotency_request_hashes"].as_array().unwrap() {
            let payload = Canonical::from_tagged(&vector["payload"]).unwrap();
            assert_eq!(
                idempotency_request_hash(&payload).unwrap(),
                vector["sha256"].as_str().unwrap()
            );
        }
    }

    #[tokio::test]
    async fn inventory_replay_is_principal_bound() {
        let f = Fixture::new().await;
        let owner = f.actor;
        let accountable = f.actor;
        assert!(
            validate_inventory_replay(owner, Some(owner), Some(accountable), "same", "same")
                .is_ok()
        );
        for (actor, recorded_owner, recorded_actor) in [
            (Uuid::new_v4(), Some(owner), Some(accountable)),
            (owner, Some(Uuid::new_v4()), Some(accountable)),
            (owner, Some(owner), Some(Uuid::new_v4())),
            (owner, None, Some(accountable)),
            (owner, Some(owner), None),
        ] {
            let error =
                validate_inventory_replay(actor, recorded_owner, recorded_actor, "same", "same")
                    .unwrap_err();
            assert_eq!(error.status.as_u16(), 403);
            assert_eq!(error.code.as_str(), "REPLAY_PRINCIPAL_MISMATCH");
        }
        assert_eq!(
            validate_inventory_replay(owner, Some(owner), Some(accountable), "other", "same")
                .unwrap_err()
                .status
                .as_u16(),
            409
        );
        let mut f = f;
        let publication = Uuid::new_v4();
        sqlx::query("INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,idempotency_operation,idempotency_key,idempotency_request_hash) VALUES($1,$2,$3,$3,'inventory.publish','publication','same')")
            .bind(publication).bind(f.org).bind(owner).execute(&mut f.admin).await.unwrap();
        assert_eq!(transaction::inventory_replay_probe(&f.pool, owner, f.org, "publication", "same").await.unwrap(), Some(publication));
        let stranger = transaction::inventory_replay_probe(&f.pool, Uuid::new_v4(), f.org, "publication", "same").await.unwrap_err();
        assert_eq!(stranger.code.as_str(), "REPLAY_PRINCIPAL_MISMATCH");
        // Admission or membership changes do not rewrite recorded ownership.
        assert_eq!(transaction::inventory_replay_probe(&f.pool, owner, f.org, "publication", "same").await.unwrap(), Some(publication));
        assert_eq!(f.effects().await, 0);
        f.finish().await;
    }

    #[tokio::test]
    async fn no_query_after_commit_for_keyed_and_unkeyed() {
        let f = Fixture::new().await;
        for keyed in [true, false] {
            let (mut context, command) = f.request(Operation::OrderAmend);
            if !keyed {
                context.idempotency_key = None;
            }
            let before = f.effects().await;
            let failed = f
                .run(
                    context.clone(),
                    command.clone(),
                    ReplayFault::BeforeResponse,
                )
                .await;
            assert!(failed.result.is_err());
            assert_eq!(failed.attempts, 1);
            assert_eq!(f.effects().await, before);
            let committed = f.run(context, command, ReplayFault::AfterCommit).await;
            assert!(committed.result.is_ok());
            assert_eq!(committed.attempts, 1);
            assert_eq!(committed.queries_after_commit, 0);
            assert_eq!(f.effects().await, before + 1);
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn receipt_insert_contention_replays_the_winner() {
        let f = Fixture::new().await;
        let (context, command) = f.request(Operation::TradeConfirm);
        let barrier = Arc::new(Barrier::new(2));
        let (a, b) = tokio::join!(
            f.run(
                context.clone(),
                command.clone(),
                ReplayFault::ReceiptRace(barrier.clone())
            ),
            f.run(context, command, ReplayFault::ReceiptRace(barrier))
        );
        let a = a.result.unwrap();
        let b = b.result.unwrap();
        assert_ne!(a.replayed, b.replayed);
        assert_eq!(a.response_body, b.response_body);
        assert_eq!(f.effects().await, 1);
        f.finish().await;
    }

    #[tokio::test]
    async fn receipt_insert_contention_revalidates_the_winner() {
        let f = Fixture::new().await;
        let (context, command) = f.request(Operation::TradeConfirm);
        let mut different = command.clone();
        different.resource_id = Uuid::new_v4();
        let barrier = Arc::new(Barrier::new(2));
        let (a, b) = tokio::join!(
            f.run(
                context.clone(),
                command,
                ReplayFault::ReceiptRace(barrier.clone())
            ),
            f.run(context, different, ReplayFault::ReceiptRace(barrier))
        );
        let (success, refusal) = if a.result.is_ok() {
            (a.result, b.result)
        } else {
            (b.result, a.result)
        };
        assert!(!success.unwrap().replayed);
        assert_eq!(
            refusal.unwrap_err().detail["code"],
            "RECEIPT_RESOURCE_MISMATCH"
        );
        assert_eq!(f.effects().await, 1);
        f.finish().await;
    }

    #[tokio::test]
    async fn ambiguous_commit_is_not_retried() {
        let f = Fixture::new().await;
        for keyed in [true, false] {
            let (mut context, command) = f.request(Operation::TradePay);
            if !keyed {
                context.idempotency_key = None;
            }
            let result = f
                .run(context.clone(), command.clone(), ReplayFault::LostCommitAck)
                .await;
            let error = result.result.unwrap_err();
            assert_eq!(error.status.as_u16(), 503);
            assert_eq!(error.detail["code"], "COMMIT_OUTCOME_UNKNOWN");
            assert_eq!(result.attempts, 1);
            if keyed {
                assert!(
                    f.run(context, command, ReplayFault::RejectFresh)
                        .await
                        .result
                        .unwrap()
                        .replayed
                );
            }
        }
        assert_eq!(f.effects().await, 2);
        f.finish().await;
    }

    #[tokio::test]
    async fn safely_aborted_commit_retries_only_the_approved_states() {
        let f = Fixture::new().await;
        for state in ["40001", "40P01", "23505", "55P03", "57014"] {
            let (context, command) = f.request(Operation::TradeDeliver);
            let result = f
                .run(context, command, ReplayFault::CommitFailure(state))
                .await;
            if ["40001", "40P01"].contains(&state) {
                assert_eq!(result.attempts, 2);
                assert!(result.result.is_ok());
            } else {
                assert_eq!(result.attempts, 1);
                assert!(result.result.is_err());
            }
        }
        assert_eq!(f.effects().await, 2);
        f.finish().await;
    }
}
