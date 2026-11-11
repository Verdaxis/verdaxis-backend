//! Append-only agent evidence. Hash input is the previous lowercase hex hash
//! followed by compact UTF-8 JSON with recursively sorted keys. The event JSON
//! contains `kind`, `actor`, `payload`, and `at` as UTC RFC3339 microseconds.
//! This detects changed retained rows, not removal of the entire ledger or a
//! privileged rewrite of every subsequent hash. Keep trusted heads externally
//! when evidence must survive a privileged database rewrite.

use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Acquire, FromRow, PgPool, Postgres};
use uuid::Uuid;

/// ASCII `AGENTAUD`, reserved for the global audit chain. Acquire before reading
/// its head; the transaction lock remains held through the outermost commit.
const CHAIN_LOCK: i64 = i64::from_be_bytes(*b"AGENTAUD");
/// The predecessor of the first retained event is 64 ASCII zeroes.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const VERIFY_BATCH: i64 = 256;
const KEY_ENV_NAMES: [&str; 3] = [
    "OPENCODE_API_KEY",
    "OPENCODE_API_KEY_2",
    "OPENCODE_API_KEY_3",
];

/// Errors deliberately retain neither database diagnostics nor event content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditError(&'static str);
impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for AuditError {}
impl From<sqlx::Error> for AuditError {
    fn from(_: sqlx::Error) -> Self {
        Self("audit database operation failed")
    }
}
pub type Result<T> = std::result::Result<T, AuditError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    ModelRequestIntent,
    ModelRequestReceipt,
    ToolCallIntent,
    ToolCallReceipt,
    DraftCreated,
    ControlChanged,
}
impl AuditKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ModelRequestIntent => "model_request_intent",
            Self::ModelRequestReceipt => "model_request_receipt",
            Self::ToolCallIntent => "tool_call_intent",
            Self::ToolCallReceipt => "tool_call_receipt",
            Self::DraftCreated => "draft_created",
            Self::ControlChanged => "control_changed",
        }
    }
    fn is_receipt(self) -> bool {
        matches!(self, Self::ModelRequestReceipt | Self::ToolCallReceipt)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AuditActor {
    pub user_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
}

// No Debug implementation: a caller may still hold unredacted input here.
pub struct AuditEvent {
    pub kind: AuditKind,
    pub actor: AuditActor,
    pub payload: Value,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendedEvent {
    pub id: i64,
    pub hash: String,
}

#[derive(Clone, Copy, Debug)]
pub enum IntentKind {
    ModelRequest,
    ToolCall,
}
impl IntentKind {
    fn kinds(self) -> (AuditKind, AuditKind) {
        match self {
            Self::ModelRequest => (
                AuditKind::ModelRequestIntent,
                AuditKind::ModelRequestReceipt,
            ),
            Self::ToolCall => (AuditKind::ToolCallIntent, AuditKind::ToolCallReceipt),
        }
    }
}

/// Created only after the intent commits. Fields are private, and the handle
/// carries its originating pool so a receipt cannot target another database.
/// Borrow it after the external operation; an interrupted operation leaves an
/// intent without a receipt rather than an unrecorded request. Receipt retries
/// return the first stored receipt, including after an uncertain commit.
///
/// ```compile_fail
/// use verdaxis_read::agent::audit::IntentHandle;
/// let handle = IntentHandle {};
/// ```
pub struct IntentHandle {
    pool: PgPool,
    intent: AppendedEvent,
    actor: AuditActor,
    kind: IntentKind,
}
impl IntentHandle {
    pub fn id(&self) -> i64 {
        self.intent.id
    }
    pub fn hash(&self) -> &str {
        &self.intent.hash
    }
}

/// Accepts `&PgPool` or `&mut Transaction<Postgres>`. For a supplied transaction
/// SQLx opens a savepoint; the caller must commit the outer transaction. Only
/// READ COMMITTED is supported so waiting writers always read the latest head.
/// Receipt kinds are reserved for [`record_receipt`].
pub async fn append<'a, A>(target: A, event: AuditEvent) -> Result<AppendedEvent>
where
    A: Acquire<'a, Database = Postgres>,
{
    if event.kind.is_receipt() {
        return Err(AuditError("audit receipt requires intent handle"));
    }
    let mut tx = target.begin().await?;
    let result = append_locked(&mut tx, event, None).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn record_intent(
    pool: &PgPool,
    kind: IntentKind,
    actor: AuditActor,
    payload: Value,
) -> Result<IntentHandle> {
    let intent = append(
        pool,
        AuditEvent {
            kind: kind.kinds().0,
            actor,
            payload,
        },
    )
    .await?;
    Ok(IntentHandle {
        pool: pool.clone(),
        intent,
        actor,
        kind,
    })
}

/// Borrow the handle so failed or uncertain commits can be retried. Under the
/// chain lock an existing receipt wins, making retries return the original row.
pub async fn record_receipt(handle: &IntentHandle, outcome: Value) -> Result<AppendedEvent> {
    let mut tx = handle.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK)
        .execute(&mut *tx)
        .await?;
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT kind, hash FROM public.agent_audit_events WHERE id = $1")
            .bind(handle.intent.id)
            .fetch_optional(&mut *tx)
            .await?;
    if found
        != Some((
            handle.kind.kinds().0.as_str().to_owned(),
            handle.intent.hash.clone(),
        ))
    {
        return Err(AuditError("audit intent is unavailable"));
    }
    let existing: Option<(i64, String)> = sqlx::query_as("SELECT id, hash FROM public.agent_audit_events WHERE kind = $1 AND payload -> 'intent_id' = to_jsonb($2::bigint) ORDER BY id LIMIT 1")
        .bind(handle.kind.kinds().1.as_str()).bind(handle.intent.id).fetch_optional(&mut *tx).await?;
    if let Some((id, hash)) = existing {
        tx.commit().await?;
        return Ok(AppendedEvent { id, hash });
    }
    let event = AuditEvent {
        kind: handle.kind.kinds().1,
        actor: handle.actor,
        payload: outcome,
    };
    let result = append_locked(&mut tx, event, Some(handle.intent.id)).await?;
    tx.commit().await?;
    Ok(result)
}

async fn append_locked(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    event: AuditEvent,
    intent_id: Option<i64>,
) -> Result<AppendedEvent> {
    let isolation: String = sqlx::query_scalar("SHOW transaction_isolation")
        .fetch_one(&mut **tx)
        .await?;
    if isolation != "read committed" {
        return Err(AuditError("audit append requires read committed isolation"));
    }
    // Redaction takes place before any event content is sent to PostgreSQL.
    let redacted = Redactor::from_environment()?.redact(event.payload)?;
    // Structural fields are not user strings: redact the outcome before adding
    // the receipt envelope so even an unusual configured value cannot erase it.
    let payload = match intent_id {
        Some(id) => json!({"intent_id": id, "outcome": redacted}),
        None => redacted,
    };
    let payload = cap_payload(payload, intent_id)?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(CHAIN_LOCK)
        .execute(&mut **tx)
        .await?;
    let prev_hash: Option<String> =
        sqlx::query_scalar("SELECT hash FROM public.agent_audit_events ORDER BY id DESC LIMIT 1")
            .fetch_optional(&mut **tx)
            .await?;
    let prev_hash = prev_hash.as_deref().unwrap_or(GENESIS_HASH);
    let at = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or(AuditError("audit timestamp out of range"))?;
    let hash = event_hash(prev_hash, at, event.kind.as_str(), event.actor, &payload)?;
    let id = sqlx::query_scalar("INSERT INTO public.agent_audit_events (at, user_id, organization_id, conversation_id, kind, payload, prev_hash, hash) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id")
        .bind(at).bind(event.actor.user_id).bind(event.actor.organization_id).bind(event.actor.conversation_id)
        .bind(event.kind.as_str()).bind(payload).bind(prev_hash).bind(&hash).fetch_one(&mut **tx).await?;
    Ok(AppendedEvent { id, hash })
}

// JSONB expands exponents and may change zero spelling. Normalize numbers
// exactly, without f64 conversion or the financial helper's 28-digit rounding.
fn normalize_numbers(value: Value) -> Result<Value> {
    let mut budget = MAX_PAYLOAD_BYTES + 4096;
    normalize_numbers_bounded(value, &mut budget)
}

fn normalize_numbers_bounded(value: Value, budget: &mut usize) -> Result<Value> {
    match value {
        Value::Number(number) => {
            let text = number.to_string();
            let negative = text.starts_with('-');
            let unsigned = text.trim_start_matches('-');
            let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
            let exponent: i64 = exponent
                .parse()
                .map_err(|_| AuditError("audit numeric exponent out of range"))?;
            let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
            let coefficient = format!("{integer}{fraction}");
            let digits = coefficient.trim_start_matches('0');
            if digits.is_empty() {
                return Ok(json!(0));
            }
            let scale = i64::try_from(fraction.len())
                .map_err(|_| AuditError("audit numeric scale out of range"))?;
            let power = exponent
                .checked_sub(scale)
                .ok_or(AuditError("audit numeric scale out of range"))?;
            // Stay inside PostgreSQL NUMERIC bounds and bound expansion memory.
            if !(-16_383..=65_536).contains(&power) || digits.len() > 65_536 {
                return Err(AuditError("audit numeric value out of range"));
            }
            let point = i64::try_from(digits.len())
                .map_err(|_| AuditError("audit numeric value out of range"))?
                + power;
            let expanded = if power >= 0 {
                digits.len().checked_add(power as usize)
            } else if point <= 0 {
                digits
                    .len()
                    .checked_add((-point) as usize)
                    .and_then(|n| n.checked_add(2))
            } else {
                digits.len().checked_add(1)
            };
            let expanded = expanded
                .and_then(|n| n.checked_add(usize::from(negative)))
                .ok_or(AuditError("audit payload expansion limit"))?;
            *budget = budget
                .checked_sub(expanded)
                .ok_or(AuditError("audit payload expansion limit"))?;
            let body = if power >= 0 {
                format!("{digits}{}", "0".repeat(power as usize))
            } else if point <= 0 {
                format!("0.{}{digits}", "0".repeat((-point) as usize))
            } else {
                let point = point as usize;
                format!("{}.{}", &digits[..point], &digits[point..])
            };
            let body = if body.contains('.') {
                body.trim_end_matches('0').trim_end_matches('.')
            } else {
                &body
            };
            let text = format!("{}{body}", if negative { "-" } else { "" });
            serde_json::from_str(&text).map_err(|_| AuditError("audit numeric encoding failed"))
        }
        Value::Array(items) => items
            .into_iter()
            .map(|item| normalize_numbers_bounded(item, budget))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(items) => items
            .into_iter()
            .map(|(k, v)| normalize_numbers_bounded(v, budget).map(|v| (k, v)))
            .collect::<Result<serde_json::Map<_, _>>>()
            .map(Value::Object),
        other => Ok(other),
    }
}

fn canonical_json(value: &Value) -> Result<Vec<u8>> {
    let value = normalize_numbers(value.clone())?;
    serde_json::to_vec(&crate::market::terms::sorted_json(&value))
        .map_err(|_| AuditError("audit JSON encoding failed"))
}
fn event_hash(
    prev_hash: &str,
    at: DateTime<Utc>,
    kind: &str,
    actor: AuditActor,
    payload: &Value,
) -> Result<String> {
    let value = json!({"at": at.to_rfc3339_opts(SecondsFormat::Micros, true), "kind": kind, "actor": actor, "payload": payload});
    let mut digest = Sha256::new();
    digest.update(prev_hash.as_bytes());
    digest.update(canonical_json(&value)?);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

struct Redactor {
    patterns: Vec<Regex>,
    values: Vec<String>,
}
impl Redactor {
    fn from_environment() -> Result<Self> {
        Self::new(
            KEY_ENV_NAMES
                .iter()
                .filter_map(|name| std::env::var(name).ok())
                .collect(),
        )
    }
    fn new(mut values: Vec<String>) -> Result<Self> {
        values.retain(|value| !value.is_empty());
        let patterns = [
            r"sk-[A-Za-z0-9_-]{16,}",
            r"oc_sk_[A-Za-z0-9_-]+",
            r"(?i:Bearer)\s+\S+",
        ]
        .iter()
        .map(|pattern| {
            Regex::new(pattern).map_err(|_| AuditError("audit redaction configuration failed"))
        })
        .collect::<Result<Vec<_>>>()?;
        Ok(Self { patterns, values })
    }
    fn text(&self, text: &str) -> String {
        // Find all spans in the original text. Sequential replacement could
        // hide a token prefix before the full credential pattern is matched.
        let mut spans: Vec<(usize, usize)> = self
            .patterns
            .iter()
            .flat_map(|p| p.find_iter(text).map(|m| (m.start(), m.end())))
            .collect();
        for value in &self.values {
            // Include overlapping exact matches as well as disjoint ones.
            for (start, _) in text.char_indices() {
                if text[start..].starts_with(value) {
                    spans.push((start, start + value.len()));
                }
            }
        }
        spans.sort_unstable();
        let mut output = String::new();
        let mut cursor = 0;
        let mut index = 0;
        while index < spans.len() {
            let (start, mut end) = spans[index];
            index += 1;
            while index < spans.len() && spans[index].0 <= end {
                end = end.max(spans[index].1);
                index += 1;
            }
            output.push_str(&text[cursor..start]);
            output.push_str("[REDACTED]");
            cursor = end;
        }
        output.push_str(&text[cursor..]);
        output
    }
    fn redact(&self, value: Value) -> Result<Value> {
        match value {
            Value::String(text) => Ok(Value::String(self.text(&text))),
            Value::Array(items) => items
                .into_iter()
                .map(|v| self.redact(v))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
            Value::Object(items) => {
                let mut output = serde_json::Map::new();
                for (key, value) in items {
                    let key = self.text(&key);
                    if output.contains_key(&key) {
                        return Err(AuditError("audit redacted key collision"));
                    }
                    output.insert(key, self.redact(value)?);
                }
                Ok(Value::Object(output))
            }
            other => Ok(other),
        }
    }
}

/// Oversized redacted payloads are replaced, not byte-sliced: this preserves
/// valid JSON and avoids partial credentials or misleading partial evidence.
/// `limit_bytes` states the cap. Numeric expansion also shares a bounded
/// budget before allocating strings. Receipts always retain `intent_id`.
fn cap_payload(value: Value, intent_id: Option<i64>) -> Result<Value> {
    struct LimitedWriter {
        remaining: usize,
    }
    impl std::io::Write for LimitedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.remaining = self
                .remaining
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("audit payload size limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let fits = |value: &Value| {
        serde_json::to_writer(
            &mut LimitedWriter {
                remaining: MAX_PAYLOAD_BYTES,
            },
            value,
        )
        .is_ok()
    };
    // Reject oversized input before normalization, then check expanded JSON.
    if fits(&value) {
        match normalize_numbers(value) {
            Ok(normalized) if fits(&normalized) => return Ok(normalized),
            Ok(_) | Err(AuditError("audit payload expansion limit")) => {}
            Err(error) => return Err(error),
        }
    }
    let mut marker = json!({"truncated": true, "limit_bytes": MAX_PAYLOAD_BYTES});
    if let Some(id) = intent_id {
        marker["intent_id"] = json!(id);
    }
    Ok(marker)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChainReport {
    pub checked: u64,
    pub first_broken_id: Option<i64>,
}
#[derive(FromRow)]
struct StoredEvent {
    id: i64,
    at: DateTime<Utc>,
    user_id: Option<Uuid>,
    organization_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    kind: String,
    payload: Value,
    prev_hash: String,
    hash: String,
}

/// Verify inclusive bounds in a read-only, repeatable-read snapshot. A range
/// trusts the immediately preceding stored hash as its anchor. ID gaps are
/// valid (sequences survive rollback); missing interior events break links.
pub async fn verify_chain(
    pool: &PgPool,
    from_id: Option<i64>,
    to_id: Option<i64>,
) -> Result<ChainReport> {
    if matches!((from_id, to_id), (Some(from), Some(to)) if from > to) {
        return Err(AuditError("audit invalid verification range"));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let predecessor: Option<String> = sqlx::query_scalar(
        "SELECT hash FROM public.agent_audit_events WHERE id < $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(from_id.unwrap_or(i64::MIN))
    .fetch_optional(&mut *tx)
    .await?;
    let mut previous = predecessor.unwrap_or_else(|| GENESIS_HASH.to_owned());
    let mut cursor: Option<i64> = None;
    let mut report = ChainReport::default();
    loop {
        let rows: Vec<StoredEvent> = sqlx::query_as("SELECT id, at, user_id, organization_id, conversation_id, kind, payload, prev_hash, hash FROM public.agent_audit_events WHERE ($1::bigint IS NULL OR id >= $1) AND ($2::bigint IS NULL OR id <= $2) AND ($3::bigint IS NULL OR id > $3) ORDER BY id LIMIT $4")
            .bind(from_id).bind(to_id).bind(cursor).bind(VERIFY_BATCH).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            report.checked = report
                .checked
                .checked_add(1)
                .ok_or(AuditError("audit verification count out of range"))?;
            let actor = AuditActor {
                user_id: row.user_id,
                organization_id: row.organization_id,
                conversation_id: row.conversation_id,
            };
            if row.prev_hash != previous
                || event_hash(&row.prev_hash, row.at, &row.kind, actor, &row.payload)? != row.hash
            {
                report.first_broken_id = Some(row.id);
                tx.commit().await?;
                return Ok(report);
            }
            cursor = Some(row.id);
            previous = row.hash;
        }
    }
    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let a: Value = serde_json::from_str(r#"{"z":[{"b":2,"a":1}],"a":null}"#).unwrap();
        let b = json!({"a": null, "z": [{"a": 1, "b": 2}]});
        assert_eq!(canonical_json(&a).unwrap(), canonical_json(&b).unwrap());
        assert_eq!(
            canonical_json(&a).unwrap(),
            br#"{"a":null,"z":[{"a":1,"b":2}]}"#
        );
    }

    #[test]
    fn canonical_numbers_match_jsonb_spelling_without_rounding() {
        let input: Value =
            serde_json::from_str(r#"[1e2,1.2300,-0.0,1e-3,123456789012345678901234567890.12]"#)
                .unwrap();
        assert_eq!(
            canonical_json(&input).unwrap(),
            br#"[100,1.23,0,0.001,123456789012345678901234567890.12]"#
        );
    }

    #[test]
    fn redacts_all_patterns_in_keys_and_nested_values() {
        let redactor = Redactor::new(Vec::new()).unwrap();
        for secret in [
            format!("sk-{}", "x".repeat(20)),
            format!("oc_{}{}", "sk_", "x".repeat(8)),
            format!("Bearer\t{}", "fake-token"),
            format!("bearer {}", "fake-token"),
            format!("bEaReR {}", "fake-token"),
        ] {
            let input = json!({secret.clone(): [{"value": format!("before {secret} after")}]});
            assert_eq!(
                redactor.redact(input).unwrap(),
                json!({"[REDACTED]": [{"value": "before [REDACTED] after"}]})
            );
        }
    }

    #[test]
    fn redacts_configured_values_without_changing_environment() {
        // Inject the same values that from_environment reads, without unsafe
        // process-wide environment mutation in parallel Rust tests.
        let values = [
            "fake-configured-one",
            "fake-configured-two",
            "fake-configured-three",
        ];
        let redactor = Redactor::new(values.iter().map(|v| (*v).to_owned()).collect()).unwrap();
        for value in values {
            assert_eq!(
                redactor.redact(json!({value: value})).unwrap(),
                json!({"[REDACTED]": "[REDACTED]"})
            );
        }
        assert_eq!(
            Redactor::new(vec![String::new()]).unwrap().text("safe"),
            "safe"
        );
    }

    #[test]
    fn overlapping_matches_do_not_leave_token_fragments() {
        let secret = format!("sk-{}", "x".repeat(20));
        let redactor = Redactor::new(vec!["xxx".to_owned()]).unwrap();
        assert_eq!(redactor.text(&secret), "[REDACTED]");
    }

    #[test]
    fn redacted_key_collisions_fail_without_data_in_error() {
        let redactor = Redactor::new(vec!["fake-one".into(), "fake-two".into()]).unwrap();
        let error = redactor
            .redact(json!({"fake-one": 1, "fake-two": 2}))
            .unwrap_err();
        assert_eq!(error.to_string(), "audit redacted key collision");
    }

    #[test]
    fn payload_cap_replaces_content_and_retains_receipt_link() {
        let small = json!({"text": "a".repeat(100)});
        assert_eq!(cap_payload(small.clone(), None).unwrap(), small);
        let large = json!({"text": "é".repeat(MAX_PAYLOAD_BYTES)});
        let capped = cap_payload(large, Some(42)).unwrap();
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["intent_id"], 42);
        assert!(canonical_json(&capped).unwrap().len() <= MAX_PAYLOAD_BYTES);
    }

    #[test]
    fn aggregate_numeric_expansion_is_capped_before_allocation() {
        let input: Value = serde_json::from_str("[1e40000,1e40000,1e40000]").unwrap();
        let capped = cap_payload(input, None).unwrap();
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["limit_bytes"], MAX_PAYLOAD_BYTES);
    }

    #[test]
    fn redaction_precedes_payload_limit() {
        let secret = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let redactor = Redactor::new(vec![secret.clone()]).unwrap();
        let value = redactor.redact(json!({"value": secret})).unwrap();
        assert_eq!(
            cap_payload(value, None).unwrap(),
            json!({"value": "[REDACTED]"})
        );
    }
}
