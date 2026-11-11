//! Typed, bounded agent operations. Drafts never enter the order write path.
use super::audit::{self, AuditActor, AuditEvent, AuditKind, IntentKind};
use crate::{ApiError, AppState, MarketPolicy};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rmcp::schemars::{self, JsonSchema};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

pub const MAX_CONTENT_CHARS: usize = 12_000;
const MAX_DRAFT_AMOUNT: Decimal = Decimal::from_parts(100_000, 0, 0, false, 0);

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListMarketsArguments {}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMarketSliceArguments {
    pub market_product: String,
    pub delivery_point: String,
    pub availability_window: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanVoyageFuelArguments {
    pub request: Value,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}
impl OrderStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "OPEN",
            Self::PartiallyFilled => "PARTIALLY_FILLED",
            Self::Filled => "FILLED",
            Self::Cancelled => "CANCELLED",
            Self::Expired => "EXPIRED",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMyOrdersArguments {
    pub status: Option<OrderStatus>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMyTradesArguments {
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<u32>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum DraftSide {
    Bid,
    Ask,
}
impl DraftSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDraftArguments {
    pub side: DraftSide,
    pub market_product: String,
    pub delivery_point: String,
    pub availability_window: String,
    /// Exact decimal text, greater than zero and at most 100000 metric tonnes.
    pub quantity_mt: String,
    /// Optional exact decimal text, greater than zero and at most 100000 USD.
    pub limit_price_usd: Option<String>,
    /// RFC3339 timestamp, in the future and at most 90 days from execution.
    pub expires_at: Option<String>,
    #[schemars(range(min = 1))]
    pub source_message_seq: i32,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskUserArguments {
    #[schemars(length(min = 1, max = 500))]
    pub question: String,
    #[schemars(schema_with = "options_schema")]
    pub options: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(
    tag = "tool",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentTool {
    ListMarkets(ListMarketsArguments),
    GetMarketSlice(GetMarketSliceArguments),
    PlanVoyageFuel(PlanVoyageFuelArguments),
    GetMyOrders(GetMyOrdersArguments),
    GetMyTrades(GetMyTradesArguments),
    CreateDraft(CreateDraftArguments),
    AskUser(AskUserArguments),
}

/// Constructed by the signed-in request boundary, never from model arguments.
/// Execution rechecks the user, organization and conversation in PostgreSQL.
pub struct ToolContext {
    pub state: AppState,
    pub user_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub conversation_id: Uuid,
}
impl ToolContext {
    fn actor(&self) -> AuditActor {
        AuditActor {
            user_id: Some(self.user_id),
            organization_id: self.organization_id,
            conversation_id: Some(self.conversation_id),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolProvenance {
    Live,
    Benchmark,
    Demo,
    Calculated,
    UserData,
}
#[derive(Clone, Debug, Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub provenance: ToolProvenance,
    pub content: Value,
    pub truncated: bool,
}
impl ToolResult {
    fn success(provenance: ToolProvenance, content: Value) -> Self {
        let mut result = Self {
            ok: true,
            provenance,
            content,
            truncated: false,
        };
        let original_chars = result.content.to_string().chars().count();
        if original_chars > MAX_CONTENT_CHARS {
            result.content = summarize_content(&result.content, original_chars);
            result.truncated = true;
        }
        result
    }
    fn failure(error: ToolError, provenance: ToolProvenance) -> Self {
        Self {
            ok: false,
            provenance,
            content: json!({"error_code": error.code, "message": error.message}),
            truncated: false,
        }
    }
}
fn summarize_content(content: &Value, original_chars: usize) -> Value {
    let mut summary = json!({"summary": "Result exceeded the display limit; compact items are shown where available.", "original_chars": original_chars});
    let items = content
        .as_array()
        .or_else(|| content.get("items").and_then(Value::as_array));
    if let Some(items) = items {
        summary["item_count"] = json!(items.len());
        if let Some(total) = content.get("total") {
            summary["total"] = total.clone();
        }
        let mut compact = Vec::new();
        for item in items {
            let mut fields = serde_json::Map::new();
            for name in [
                "id",
                "side",
                "status",
                "market_product",
                "delivery_point_name",
                "availability_window",
                "quantity_mt",
                "remaining_quantity_mt",
                "price_per_mt_usd",
                "created_at",
            ] {
                if let Some(value) = item.get(name) {
                    fields.insert(name.to_owned(), value.clone());
                }
            }
            compact.push(Value::Object(fields));
            summary["items"] = json!(compact);
            if summary.to_string().chars().count() > MAX_CONTENT_CHARS {
                compact.pop();
                summary["items"] = json!(compact);
                break;
            }
        }
        summary["shown_count"] = json!(compact.len());
        // Reserve a small envelope margin for the count added above.
        while summary.to_string().chars().count() > MAX_CONTENT_CHARS && !compact.is_empty() {
            compact.pop();
            summary["items"] = json!(compact);
            summary["shown_count"] = json!(compact.len());
        }
    }
    summary
}

#[derive(Clone, Copy, Debug)]
pub struct ToolError {
    pub code: &'static str,
    pub message: &'static str,
}
impl ToolError {
    fn validation() -> Self {
        Self {
            code: "INVALID_ARGUMENTS",
            message: "Tool arguments are outside the permitted range.",
        }
    }
    fn unavailable() -> Self {
        Self {
            code: "UNAVAILABLE",
            message: "The requested data is temporarily unavailable.",
        }
    }
    fn context() -> Self {
        Self {
            code: "CONTEXT_UNAVAILABLE",
            message: "The signed-in conversation is not available.",
        }
    }
    fn audit() -> Self {
        Self {
            code: "AUDIT_UNAVAILABLE",
            message: "Tool evidence could not be recorded. Do not assume the operation failed or retry a draft without checking it.",
        }
    }
}
impl From<ApiError> for ToolError {
    fn from(error: ApiError) -> Self {
        if error.status.as_u16() == 422 {
            Self::validation()
        } else {
            Self::unavailable()
        }
    }
}
impl From<sqlx::Error> for ToolError {
    fn from(_: sqlx::Error) -> Self {
        Self::unavailable()
    }
}

impl AgentTool {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ListMarkets(_) => "list_markets",
            Self::GetMarketSlice(_) => "get_market_slice",
            Self::PlanVoyageFuel(_) => "plan_voyage_fuel",
            Self::GetMyOrders(_) => "get_my_orders",
            Self::GetMyTrades(_) => "get_my_trades",
            Self::CreateDraft(_) => "create_draft",
            Self::AskUser(_) => "ask_user",
        }
    }
    fn provenance(&self) -> ToolProvenance {
        match self {
            Self::ListMarkets(_) | Self::PlanVoyageFuel(_) | Self::AskUser(_) => {
                ToolProvenance::Calculated
            }
            Self::GetMarketSlice(_) => ToolProvenance::Benchmark,
            _ => ToolProvenance::UserData,
        }
    }
    pub fn validate(&self, policy: &MarketPolicy, now: DateTime<Utc>) -> Result<(), ToolError> {
        match self {
            Self::GetMarketSlice(args) => {
                crate::mcp::prices::validate_slice(
                    policy,
                    &args.market_product,
                    &args.delivery_point,
                    &args.availability_window,
                    now,
                )?;
            }
            Self::PlanVoyageFuel(args) => {
                crate::mcp::voyage::required_price_lookups(&args.request)
                    .map_err(|_| ToolError::validation())?;
            }
            Self::GetMyTrades(args) if !(1..=50).contains(&args.limit.unwrap_or(20)) => {
                return Err(ToolError::validation());
            }
            Self::CreateDraft(args) => {
                args.canonical_terms(policy, now)?;
            }
            Self::AskUser(args)
                if !(1..=500).contains(&args.question.chars().count())
                    || args.options.as_ref().is_some_and(|options| {
                        options.len() > 6
                            || options
                                .iter()
                                .any(|option| !(1..=80).contains(&option.chars().count()))
                    }) =>
            {
                return Err(ToolError::validation());
            }
            _ => {}
        }
        Ok(())
    }
}

fn amount(text: &str) -> Result<Decimal, ToolError> {
    // from_str_exact refuses values that would lose precision in NUMERIC binding.
    let value = Decimal::from_str_exact(text).map_err(|_| ToolError::validation())?;
    if value <= Decimal::ZERO || value > MAX_DRAFT_AMOUNT {
        return Err(ToolError::validation());
    }
    Ok(value.normalize())
}
impl CreateDraftArguments {
    /// Stable digest input: all eight draft argument fields, decimal strings
    /// normalized exactly, canonical window and optional UTC microsecond expiry.
    pub fn canonical_terms(
        &self,
        policy: &MarketPolicy,
        now: DateTime<Utc>,
    ) -> Result<Value, ToolError> {
        let (_, _, window) = crate::mcp::prices::validate_slice(
            policy,
            &self.market_product,
            &self.delivery_point,
            &self.availability_window,
            now,
        )?;
        if self.source_message_seq <= 0 {
            return Err(ToolError::validation());
        }
        let quantity = amount(&self.quantity_mt)?;
        let price = self.limit_price_usd.as_deref().map(amount).transpose()?;
        let expires = self
            .expires_at
            .as_deref()
            .map(|text| {
                let parsed = DateTime::parse_from_rfc3339(text)
                    .map_err(|_| ToolError::validation())?
                    .with_timezone(&Utc);
                let at = DateTime::from_timestamp_micros(parsed.timestamp_micros())
                    .ok_or_else(ToolError::validation)?;
                if parsed > now + Duration::days(90) || at <= now {
                    return Err(ToolError::validation());
                }
                Ok(at.to_rfc3339_opts(SecondsFormat::Micros, true))
            })
            .transpose()?;
        Ok(
            json!({"side": self.side, "market_product": self.market_product, "delivery_point": self.delivery_point,
            "availability_window": window, "quantity_mt": quantity.to_string(), "limit_price_usd": price.map(|value| value.to_string()),
            "expires_at": expires, "source_message_seq": self.source_message_seq}),
        )
    }
}

/// Every operation first commits its intent. Even validation and access refusals
/// get a receipt. A failed receipt never returns a successful operation result.
pub async fn execute(context: &ToolContext, tool: AgentTool) -> ToolResult {
    let provenance = tool.provenance();
    let envelope = match serde_json::to_value(&tool) {
        Ok(value) => value,
        Err(_) => return ToolResult::failure(ToolError::validation(), provenance),
    };
    let handle = match audit::record_intent(
        &context.state.pools.read,
        IntentKind::ToolCall,
        context.actor(),
        envelope,
    )
    .await
    {
        Ok(handle) => handle,
        Err(_) => return ToolResult::failure(ToolError::audit(), provenance),
    };
    let result = match execute_inner(context, tool).await {
        Ok((provenance, content)) => ToolResult::success(provenance, content),
        Err(error) => ToolResult::failure(error, provenance),
    };
    let serialized = result.content.to_string();
    let receipt = json!({"ok": result.ok, "error_code": result.content.get("error_code"), "result_digest": hex_digest(serialized.as_bytes()),
        "provenance": result.provenance, "chars": serialized.chars().count()});
    if audit::record_receipt(&handle, receipt).await.is_err() {
        return ToolResult::failure(ToolError::audit(), result.provenance);
    }
    result
}
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
async fn authorize(conn: &mut PgConnection, context: &ToolContext) -> Result<(), ToolError> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.agent_conversations c JOIN public.users u ON u.id=c.user_id WHERE c.id=$1 AND c.user_id=$2 AND c.organization_id IS NOT DISTINCT FROM $3 AND u.organization_id IS NOT DISTINCT FROM $3 AND c.status='open')")
        .bind(context.conversation_id).bind(context.user_id).bind(context.organization_id).fetch_one(conn).await?;
    if !allowed {
        return Err(ToolError::context());
    }
    Ok(())
}
async fn execute_inner(
    context: &ToolContext,
    tool: AgentTool,
) -> Result<(ToolProvenance, Value), ToolError> {
    let now = crate::routes::prices::captured_now(&context.state)?;
    tool.validate(&context.state.policy, now)?;
    let owned = ToolContext {
        state: context.state.clone(),
        user_id: context.user_id,
        organization_id: context.organization_id,
        conversation_id: context.conversation_id,
    };
    crate::run_admitted(&context.state, move |conn| {
        Box::pin(async move { authorize(conn, &owned).await.map_err(as_api) })
    })
    .await?;
    match tool {
        AgentTool::ListMarkets(_) => Ok((
            ToolProvenance::Calculated,
            crate::mcp::prices::list_markets()?,
        )),
        AgentTool::GetMarketSlice(args) => {
            let value = crate::mcp::prices::market_slice(
                &context.state,
                &args.market_product,
                &args.delivery_point,
                &args.availability_window,
                now,
            )
            .await?;
            let provenance = match value["demo_status"].as_str() {
                Some("DEMO_ONLY" | "MIXED") => ToolProvenance::Demo,
                Some("REAL_ONLY") => ToolProvenance::Live,
                _ => ToolProvenance::Benchmark,
            };
            Ok((provenance, value))
        }
        AgentTool::PlanVoyageFuel(args) => Ok((
            ToolProvenance::Calculated,
            crate::mcp::prices::voyage(&context.state, args.request, now).await?,
        )),
        AgentTool::AskUser(args) => Ok((
            ToolProvenance::Calculated,
            json!({"question": args.question, "options": args.options}),
        )),
        tool => {
            let state = context.state.clone();
            let actor = context.actor();
            let value = crate::run_admitted(&context.state, move |conn| {
                Box::pin(async move {
                    let context = ToolContext {
                        state: state.clone(),
                        user_id: actor.user_id.ok_or_else(ApiError::internal)?,
                        organization_id: actor.organization_id,
                        conversation_id: actor.conversation_id.ok_or_else(ApiError::internal)?,
                    };
                    // Check scope on the same connection used for the private read.
                    authorize(conn, &context).await.map_err(as_api)?;
                    match tool {
                        AgentTool::GetMyOrders(args) => crate::order_write::orders::owner_read(
                            conn,
                            context
                                .organization_id
                                .ok_or_else(|| as_api(ToolError::context()))?,
                            &crate::order_write::orders::OwnerOrderRead {
                                skip: 0,
                                limit: 50,
                                template: false,
                                user_id: Some(context.user_id),
                                status: args.status.map(|value| value.as_str().to_owned()),
                            },
                            now,
                            &state.policy,
                            &state.config,
                        )
                        .await
                        .map_err(|_| as_api(ToolError::unavailable())),
                        AgentTool::GetMyTrades(args) => crate::order_write::trades::owner_read(
                            conn,
                            crate::order_write::trades::OwnerTradeScope {
                                user_id: context.user_id,
                                organization_id: context.organization_id,
                                admin: false,
                                supported: None,
                                owner_only: true,
                            },
                            &crate::order_write::trades::TradePage {
                                limit: i64::from(args.limit.unwrap_or(20)),
                                ..Default::default()
                            },
                            false,
                            &state.config,
                            now,
                        )
                        .await
                        .map_err(|_| as_api(ToolError::unavailable())),
                        AgentTool::CreateDraft(args) => create_draft(conn, &context, args, now)
                            .await
                            .map_err(as_api),
                        _ => Err(ApiError::internal()),
                    }
                })
            })
            .await?;
            Ok((ToolProvenance::UserData, value))
        }
    }
}
fn as_api(error: ToolError) -> ApiError {
    ApiError {
        status: if error.code == "INVALID_ARGUMENTS" {
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        } else {
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        },
        detail: json!(error.code),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}
async fn create_draft(
    conn: &mut PgConnection,
    context: &ToolContext,
    args: CreateDraftArguments,
    now: DateTime<Utc>,
) -> Result<Value, ToolError> {
    let terms = args.canonical_terms(&context.state.policy, now)?;
    let digest = crate::market::terms::digest_terms(&terms).map_err(|_| ToolError::validation())?;
    let mut tx = conn
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await?;
    // Lock the account before the conversation, then check both current facts.
    // A concurrent departure or close must finish before this draft is admitted,
    // or wait until its transaction (including the audit append) commits.
    let user_organization: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT organization_id FROM public.users WHERE id=$1 FOR SHARE")
            .bind(context.user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if user_organization != Some(context.organization_id) {
        return Err(ToolError::context());
    }
    let conversation: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.agent_conversations WHERE id=$1 AND user_id=$2 AND organization_id IS NOT DISTINCT FROM $3 AND status='open' FOR SHARE",
    ).bind(context.conversation_id).bind(context.user_id).bind(context.organization_id)
        .fetch_optional(&mut *tx).await?;
    if conversation.is_none() {
        return Err(ToolError::context());
    }
    let source: Option<Value> = sqlx::query_scalar("SELECT content FROM public.agent_messages WHERE conversation_id=$1 AND seq=$2 AND role='user'")
        .bind(context.conversation_id).bind(args.source_message_seq).fetch_optional(&mut *tx).await?;
    let source = source.ok_or_else(ToolError::validation)?;
    // Messages may contain a JSON string or a structured user content value.
    // Store the exact text for strings, and compact JSON for structured content.
    let source_text = match source {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let id = Uuid::new_v4();
    let quantity = amount(&args.quantity_mt)?;
    let price = args.limit_price_usd.as_deref().map(amount).transpose()?;
    let expiry = terms["expires_at"]
        .as_str()
        .map(|text| DateTime::parse_from_rfc3339(text).map(|at| at.with_timezone(&Utc)))
        .transpose()
        .map_err(|_| ToolError::validation())?;
    sqlx::query("INSERT INTO public.order_drafts(id,conversation_id,user_id,side,market_product,delivery_point,availability_window,quantity_mt,limit_price_usd,expires_at,terms_digest,source_text,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'draft')")
        .bind(id).bind(context.conversation_id).bind(context.user_id).bind(args.side.as_str()).bind(&args.market_product).bind(&args.delivery_point).bind(terms["availability_window"].as_str().ok_or_else(ToolError::validation)?).bind(quantity).bind(price).bind(expiry).bind(&digest).bind(source_text).execute(&mut *tx).await?;
    audit::append_in(&mut tx, AuditEvent { kind: AuditKind::DraftCreated, actor: context.actor(), payload: json!({"draft_id": id, "terms_digest": digest, "source_message_seq": args.source_message_seq}) }).await.map_err(|_| ToolError::audit())?;
    tx.commit().await?;
    Ok(json!({"id": id, "status": "draft", "terms": terms, "terms_digest": digest}))
}

fn options_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"anyOf": [
        {"type": "array", "maxItems": 6, "items": {"type": "string", "minLength": 1, "maxLength": 80}},
        {"type": "null"}
    ]})
}

/// Schemas are generated from the same argument types used by deserialization.
pub fn catalog() -> Value {
    fn entry<T: JsonSchema>(name: &str, description: &str) -> Value {
        json!({"name": name, "description": description, "input_schema": schemars::schema_for!(T)})
    }
    json!([
        entry::<ListMarketsArguments>(
            "list_markets",
            "List the supported products and delivery points."
        ),
        entry::<GetMarketSliceArguments>(
            "get_market_slice",
            "Read anonymous market and benchmark evidence for one slice."
        ),
        entry::<PlanVoyageFuelArguments>(
            "plan_voyage_fuel",
            "Calculate indicative voyage fuel and compliance strategies."
        ),
        entry::<GetMyOrdersArguments>(
            "get_my_orders",
            "Read up to 50 orders owned by the signed-in user."
        ),
        entry::<GetMyTradesArguments>(
            "get_my_trades",
            "Read the signed-in user's trades, default 20 and maximum 50."
        ),
        entry::<CreateDraftArguments>(
            "create_draft",
            "Save a draft from a user message without creating an order."
        ),
        entry::<AskUserArguments>(
            "ask_user",
            "Ask one short question with up to six short answer options."
        )
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_cap_retains_complete_json_and_counts_serialized_characters() {
        for content in [
            json!({"text": "é".repeat(12_000)}),
            json!({"text": "\\".repeat(6_000)}),
        ] {
            let original_chars = content.to_string().chars().count();
            let result = ToolResult::success(ToolProvenance::UserData, content);
            assert!(result.truncated);
            assert!(result.content.to_string().chars().count() <= MAX_CONTENT_CHARS);
            assert_eq!(result.content["original_chars"], original_chars);
            assert!(result.content["summary"].is_string());
        }
        let result = ToolResult::success(
            ToolProvenance::Calculated,
            json!("x".repeat(MAX_CONTENT_CHARS - 2)),
        );
        assert!(!result.truncated);
        let items: Vec<_> = (0..50).map(|index| json!({"id":index,"quantity_mt":"10.123456789","private_terms":"x".repeat(1000)})).collect();
        let summarized = ToolResult::success(ToolProvenance::UserData, json!(items));
        assert!(summarized.truncated);
        assert_eq!(summarized.content["shown_count"], 50);
        assert_eq!(
            summarized.content["items"][0]["quantity_mt"],
            "10.123456789"
        );
        assert!(
            summarized.content["items"][0]
                .get("private_terms")
                .is_none()
        );
        assert!(summarized.content.to_string().chars().count() <= MAX_CONTENT_CHARS);
        assert_eq!(
            result.content.to_string().chars().count(),
            MAX_CONTENT_CHARS
        );
    }
}
