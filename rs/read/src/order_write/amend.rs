//! Order amendment with inline rematching and acceptance-priority ordinals.
//!
//! One sealed command: preview, receipt replay, fresh admission, the row
//! update, an optional rematch through the real planner and completion bridge,
//! then one terminal effect set. The single captured clock is the only time
//! source, so an order that would expire between the source's two clock reads
//! stays live at the captured instant and the expiry sweep owns it afterwards.
use std::{collections::BTreeSet, sync::Arc};

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    AdmissionClass, AuthenticatedActor, Canonical, CommittedCommand, MarketErrorCode,
    MarketWriteError, Operation, OrderQuantity, Price, ReceiptRecord, StockQuantity, WriteContext,
    effects::{AuditContext, BeforeEffects, EffectPort, MutationEffect, MutationKind},
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview},
    matching::{MatchBound, MatchOutcome, OrderRow, OrderStatus, Side},
    outbox::{OrderActivity, ParticipantEvent, ParticipantFields},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{AppState, MarketPolicy};

fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn refusal(status: StatusCode, message: &'static str) -> MarketWriteError {
    super::orders::refusal(MarketErrorCode::ValidationFailed, status, message)
}
fn bad(message: &'static str) -> MarketWriteError {
    refusal(StatusCode::BAD_REQUEST, message)
}
fn unauthorized() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::OrderOwnerForbidden)
}

/// Every field the update body may carry. Fields outside this set are ignored,
/// exactly as the source request model ignores undeclared keys.
const FIELDS: [&str; 17] = [
    "quantity_mt",
    "price_per_mt_usd",
    "availability_window",
    "expires_at",
    "certifications",
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
    "off_spec_notes",
    "fame_terms",
    "lng_terms",
];

/// An explicit null on any of these fields is refused before any lock. Expiry
/// and the free-text notes keep their clearable meaning.
const NULL_REFUSED: [&str; 12] = [
    "quantity_mt",
    "price_per_mt_usd",
    "certifications",
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
];

/// The effective-update key set that makes a command economic, so the matcher
/// runs after the row is written: the source's supplier-metadata set plus the
/// economic scalars and typed terms.
const ECONOMIC_FIELDS: [&str; 16] = [
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
    "off_spec_notes",
    "quantity_mt",
    "price_per_mt_usd",
    "expires_at",
    "certifications",
    "fame_terms",
    "lng_terms",
];

/// The supplier metadata a BID never stores; the scheme qualifier is shared.
const ASK_ONLY: [&str; 7] = [
    "certification_declared",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
];

/// The decimal field names of the update payload.
const DECIMAL_FIELDS: [&str; 3] = [
    "quantity_mt",
    "price_per_mt_usd",
    "carbon_intensity_gco2_mj",
];

pub(crate) struct AmendRequest {
    pub order: Uuid,
    pub input: AmendBody,
    pub key: Option<String>,
    pub audit: AuditContext,
    pub settings: crate::routes::market_write::amend::AmendSettings,
    pub now: DateTime<Utc>,
}

/// The parsed update body: the supplied fields, their validated values and the
/// canonical request identity for one order.
#[derive(Clone, Debug)]
pub(crate) struct AmendBody {
    fields: Map<String, Value>,
    payload_hash: String,
}

impl AmendBody {
    fn decimal(value: &Value) -> Result<Option<Decimal>, MarketWriteError> {
        let Some(text) = value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_number().map(|n| n.to_string()))
        else {
            return Ok(None);
        };
        Decimal::from_str_exact(text.trim()).map(Some).map_err(|_| {
            refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "A decimal field is not valid",
            )
        })
    }
    fn text(value: &Value) -> Result<(), MarketWriteError> {
        if !value.is_null() && !value.is_string() {
            return Err(refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "A text field carries a non-text value",
            ));
        }
        Ok(())
    }
    /// Parse one update body against the source schema's own bounds. Unknown
    /// fields are dropped, not refused; the source request model ignores them.
    pub(crate) fn parse(value: &Value, now: DateTime<Utc>) -> Result<Self, MarketWriteError> {
        let object = value.as_object().ok_or_else(|| {
            refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The update body must be a JSON object",
            )
        })?;
        let mut fields = Map::new();
        for name in FIELDS {
            if let Some(item) = object.get(name) {
                fields.insert(name.into(), item.clone());
            }
        }
        for name in NULL_REFUSED {
            if fields.get(name) == Some(&Value::Null) {
                return Err(MarketWriteError::new(MarketErrorCode::OrderFieldNull));
            }
        }
        if let Some(quantity) = fields.get("quantity_mt").cloned() {
            let value = Self::decimal(&quantity)?
                .ok_or_else(|| MarketWriteError::new(MarketErrorCode::OrderFieldNull))?;
            OrderQuantity::parse_exact(&value.to_string())?;
            fields.insert("quantity_mt".into(), json!(value.to_string()));
        }
        if let Some(price) = fields.get("price_per_mt_usd").cloned() {
            let value = Self::decimal(&price)?
                .ok_or_else(|| MarketWriteError::new(MarketErrorCode::OrderFieldNull))?;
            Price::parse_exact(&value.to_string())?;
            fields.insert("price_per_mt_usd".into(), json!(value.to_string()));
        }
        if let Some(intensity) = fields.get("carbon_intensity_gco2_mj").cloned() {
            let value = Self::decimal(&intensity)?
                .ok_or_else(|| MarketWriteError::new(MarketErrorCode::OrderFieldNull))?;
            if value < Decimal::ZERO {
                return Err(MarketWriteError::new(MarketErrorCode::ValueOutOfRange));
            }
            fields.insert("carbon_intensity_gco2_mj".into(), json!(value.to_string()));
        }
        if let Some(expiry) = fields.get("expires_at").cloned()
            && !expiry.is_null()
        {
            let parsed: DateTime<Utc> = expiry
                .as_str()
                .ok_or_else(|| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "expires_at must be a timestamp",
                    )
                })?
                .parse()
                .map_err(|_| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "expires_at must carry a timezone",
                    )
                })?;
            if parsed <= now {
                return Err(refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "expires_at must be in the future",
                ));
            }
            fields.insert(
                "expires_at".into(),
                json!(crate::routes::ports::pydantic_datetime(parsed)),
            );
        }
        // The update schema carries certifications as declared; the route
        // normalizes them against the locked row, and the request hash must
        // cover the declared list exactly as the source model dumps it.
        if let Some(certifications) = fields.get("certifications")
            && !certifications.is_null()
        {
            let list = certifications.as_array().ok_or_else(|| {
                refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "certifications must be a list",
                )
            })?;
            for item in list {
                if !item.is_string() {
                    return Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "certifications must be strings",
                    ));
                }
            }
        }
        if let Some(scheme) = fields.get("certification_scheme").cloned()
            && !scheme.is_null()
        {
            let text = scheme.as_str().ok_or_else(|| {
                refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "certification_scheme must be a string",
                )
            })?;
            let normalized = crate::market::terms::normalize_certification_scheme(Some(text))
                .ok_or_else(|| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "certification_scheme is empty",
                    )
                })?;
            fields.insert("certification_scheme".into(), json!(normalized));
        }
        for name in ["certification_declared", "msds_available", "off_spec"] {
            if let Some(flag) = fields.get(name)
                && !flag.is_boolean()
            {
                return Err(refusal(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A boolean field carries a non-boolean value",
                ));
            }
        }
        for name in [
            "specification_standard",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec_notes",
        ] {
            if let Some(item) = fields.get(name) {
                Self::text(item)?;
            }
        }
        for field in ["fame_terms", "lng_terms"] {
            if let Some(terms) = fields.get(field).cloned()
                && !terms.is_null()
            {
                fields.insert(
                    field.into(),
                    crate::market::terms::owner_response_terms(
                        if field == "fame_terms" {
                            "fame_v1"
                        } else {
                            "lng_v1"
                        },
                        &terms,
                    )
                    .map_err(|_| MarketWriteError::new(MarketErrorCode::ValidationFailed))?,
                );
            }
        }
        let payload_hash = Self::hash(Uuid::nil(), &fields)?;
        Ok(Self {
            fields,
            payload_hash,
        })
    }
    /// The canonical command-result request hash of one update payload. The
    /// source's canonical form prints `Decimal` values normalized and aware
    /// datetimes as UTC `Z` instants; the stored exact text is normalized here
    /// so both stacks hash the same bytes.
    fn hash(order: Uuid, fields: &Map<String, Value>) -> Result<String, MarketWriteError> {
        let payload = json!({
            "operation": "order.amend",
            "order_id": order.to_string(),
            "update": Value::Object(fields.clone()),
        });
        let canonical = Canonical::from_json(&payload).map_err(|_| internal())?;
        let Canonical::Object(map) = &canonical else {
            return Ok(super::command_request_hash(&canonical));
        };
        let mut out = std::collections::BTreeMap::new();
        for (key, item) in map {
            if key == "update"
                && let Canonical::Object(update) = item
            {
                let mut normalized = std::collections::BTreeMap::new();
                for (name, value) in update {
                    let value = if DECIMAL_FIELDS.contains(&name.as_str())
                        && let Canonical::Text(text) = value
                        && let Ok(number) = Decimal::from_str_exact(text)
                    {
                        Canonical::Text(number.normalize().to_string())
                    } else if name == "expires_at"
                        && let Canonical::Text(text) = value
                        && let Ok(at) = text.parse::<DateTime<Utc>>()
                    {
                        Canonical::Timestamp(at)
                    } else {
                        value.clone()
                    };
                    normalized.insert(name.clone(), value);
                }
                out.insert(key.clone(), Canonical::Object(normalized));
            } else {
                out.insert(key.clone(), item.clone());
            }
        }
        Ok(super::command_request_hash(&Canonical::Object(out)))
    }
    /// Bind the parsed body to one order so the hash covers the resource.
    pub(crate) fn with_order(&self, order: Uuid) -> Self {
        let payload_hash =
            Self::hash(order, &self.fields).unwrap_or_else(|_| self.payload_hash.clone());
        Self {
            fields: self.fields.clone(),
            payload_hash,
        }
    }
    pub(crate) fn payload_hash(&self) -> &str {
        &self.payload_hash
    }
    pub(crate) fn value(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }
    pub(crate) fn supplied(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }
    fn supplied_any(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.fields.contains_key(*name))
    }
}

pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: AmendRequest,
) -> Result<CommittedCommand, MarketWriteError> {
    let AmendRequest {
        order,
        input,
        key,
        audit,
        settings,
        now,
    } = request;
    let key = key
        .filter(|k| !k.is_empty())
        .map(|k| {
            super::normalize_idempotency_key(&k)
                .map_err(|_| bad("Idempotency-Key must have 1-255 characters"))
        })
        .transpose()?;
    let input = input.with_order(order);
    let command = AmendCommand {
        order,
        input,
        key,
        audit,
        settings,
        now,
        access: Arc::new(crate::auth::VerifiedAccess {
            subject: access.subject,
            issued_at: access.issued_at,
            issuance_out_of_range: access.issuance_out_of_range,
        }),
        policy: app.policy.clone(),
        config: app.config.clone(),
    };
    run_market_command(
        app.pools.read.clone(),
        app.reads.clone(),
        access,
        command,
        AdmissionClass::Execution,
    )
    .await
}

pub(super) struct AmendCommand {
    order: Uuid,
    input: AmendBody,
    key: Option<String>,
    audit: AuditContext,
    settings: crate::routes::market_write::amend::AmendSettings,
    now: DateTime<Utc>,
    access: Arc<crate::auth::VerifiedAccess>,
    policy: Arc<MarketPolicy>,
    config: Arc<crate::Config>,
}

impl AmendCommand {
    fn bound(&self) -> MatchBound<'_> {
        MatchBound::new(self.now)
            .with_policy(&self.policy)
            .with_kyc_gate(self.config.kyc_execution_gate_enabled)
    }
    fn product(&self, id: Uuid) -> Option<&crate::ProductPolicy> {
        self.policy.products.iter().find(|p| p.id == id)
    }
}

impl MarketCommand for AmendCommand {
    type Output = CommittedCommand;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let authenticated = super::authority::admission::authenticate_preview(
                conn,
                self.access.clone(),
                AdmissionClass::Execution,
            )
            .await?;
            // The legacy administrator workspace is closed for self-service
            // amendment, as the source refuses it before preparing a command.
            if self.settings.support_enabled
                && authenticated.account().role == super::authority::ActorRole::Admin
            {
                return Err(MarketWriteError::new(
                    MarketErrorCode::MarketSupportLegacyMutationRetired,
                )
                .into());
            }
            let actor_org = authenticated.account().current_organization_id;
            let row = load_optional(conn, self.order)
                .await?
                .ok_or_else(|| MarketWriteError::new(MarketErrorCode::OrderNotFound))?;
            let mut organizations: Vec<Uuid> = vec![row.organization_id];
            organizations.extend(actor_org);
            let mut users: Vec<Uuid> = row.owner_user_id.into_iter().collect();
            // Discovery covers every party a rematch could touch when the
            // amendment is economic; otherwise the order's own parties only.
            let parties = if self.settings.auto_matching_enabled
                && self.input.supplied_any(&ECONOMIC_FIELDS)
            {
                let discovered = super::matching::match_parties(conn, &row, self.bound()).await?;
                organizations.extend(discovered.organizations);
                users.extend(discovered.users);
                PartyLockPreview::new(
                    organizations,
                    users,
                    actor,
                    json!({"order": self.order, "candidates": discovered.fingerprint}),
                )
            } else {
                PartyLockPreview::new(organizations, users, actor, json!({"order": self.order}))
            };
            let mut locks = OperationLockPlan::new(MarketOperation::Amend);
            locks.parties(&parties)?;
            if let (Some(org), Some(key)) = (actor_org, self.key.as_deref()) {
                locks.idempotency(org, Operation::OrderAmend.as_str(), key)?;
            }
            locks.slices([row.slice()])?;
            // The amended row and any linked inventory row are acquired by the
            // match planner's candidate pass, never declared before it: the
            // plan closes candidate acquisition once it names an economic row.
            Ok(MarketPreview { parties, locks })
        })
    }
    fn replay<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async move {
            let Some(key) = &self.key else {
                return Ok(None);
            };
            let order = tx.matching_order(self.order).await?;
            authorize(actor, &order)?;
            let context = tx
                .execution_context(
                    actor,
                    order.organization_id,
                    self.now,
                    self.config.kyc_execution_gate_enabled,
                )
                .await?;
            Ok(tx
                .replay_receipt(
                    &context,
                    Operation::OrderAmend,
                    self.order,
                    key,
                    self.input.payload_hash(),
                )
                .await?)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let order = tx.matching_order(self.order).await?;
            authorize(actor, &order)?;
            let context = tx
                .execution_context(
                    actor,
                    order.organization_id,
                    self.now,
                    self.config.kyc_execution_gate_enabled,
                )
                .await?;
            let (staged, outcome) = {
                let mut staged = tx.amend_port(self, &context)?.run(&order).await?;
                let outcome = self.rematch(tx, &order, &mut staged.before).await?;
                (staged, outcome)
            };
            let response = tx
                .create_port()?
                .projection(self.order, &self.policy, &self.config, self.now)
                .await?;
            let after_public = tx
                .amend_visibility(self.order, &self.policy, &self.config, self.now)
                .await?;
            let key = self.key.clone();
            let receipt = key.map(|key| ReceiptRecord {
                id: Uuid::new_v4(),
                actor_user_id: actor.user_id(),
                effective_organization_id: context.effective_organization_id(),
                support_context_id: None,
                operation: Operation::OrderAmend,
                idempotency_key: key,
                request_hash: self.input.payload_hash().to_owned(),
                resource_id: self.order,
                response_status: 200,
                response_body: response.clone(),
            });
            // The audit descriptor and the participant event join the staged
            // match effects; one finish composes the terminal effect set.
            let StagedAmend {
                before,
                public_before,
                update,
            } = staged;
            let matched = outcome.as_ref().map_or(0, |o| o.trades.len());
            let participants: Vec<Uuid> = outcome
                .as_ref()
                .map(|outcome| {
                    let mut organizations: BTreeSet<Uuid> = outcome
                        .trades
                        .iter()
                        .flat_map(|trade| [trade.buyer_id, trade.seller_id])
                        .collect();
                    organizations.insert(order.organization_id);
                    organizations.into_iter().collect()
                })
                .unwrap_or_else(|| vec![order.organization_id]);
            let mut set = before.stage();
            if let Some(outcome) = &outcome {
                tx.effects()?
                    .matched(
                        &mut set,
                        outcome,
                        &order,
                        actor.user_id(),
                        self.audit.clone(),
                    )
                    .await?;
            }
            set.push(
                self.order_descriptor_parts(
                    tx.amend_conn(),
                    &update,
                    &participants,
                    &order,
                    public_before,
                    after_public,
                    matched,
                )
                .await?,
            )?;
            if let Some(receipt) = receipt {
                tx.record_receipt(&context, &receipt).await?;
            }
            tx.effects()?
                .finish(&self.policy, &self.config, set)
                .await?;
            Ok(CommittedCommand {
                response_status: 200,
                response_body: response,
                replayed: false,
                matched_trades: matched,
            })
        })
    }
}

impl AmendCommand {
    /// The sealed transaction's amend seam: construct the port on the
    /// transaction's own connection, plan and locks. The seam exists so no
    /// connection escapes the boundary.
    pub(super) fn port_on<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        plan: &'a OperationLockPlan,
        locked: &'a Locked,
        context: &'a WriteContext,
    ) -> Result<AmendPort<'a>, MarketWriteError> {
        AmendPort::new(conn, plan, locked, self, context)
    }

    /// The optional inline rematch on the sealed transaction. The planner's
    /// candidate pass acquires the resting rows and the linked inventory; the
    /// completion bridge consumes their reservations on the same connection.
    async fn rematch(
        &self,
        tx: &mut LockedMarketTransaction<'_>,
        order: &OrderRow,
        before: &mut BeforeEffects,
    ) -> Result<Option<MatchOutcome>, MarketAttemptError> {
        if !self.settings.auto_matching_enabled || !self.input.supplied_any(&ECONOMIC_FIELDS) {
            return Ok(None);
        }
        let current = tx.matching_order(self.order).await?;
        let plan = tx.prepare_match(&current, self.bound()).await?;
        let resting: Vec<Uuid> = plan
            .locked_order_ids()
            .into_iter()
            .filter(|id| *id != self.order)
            .collect();
        if !resting.is_empty() {
            tx.effects()?
                .include_existing(before, &self.policy, &self.config, &resting)
                .await?;
        }
        let outcome = tx.complete_match(&plan, &self.settings.fees, true).await?;
        Ok((!outcome.trades.is_empty()).then_some(outcome))
    }

    /// The route's own effect: one `order.updated` audit when an economic value
    /// changed, and one `order_updated` participant event whatever changed.
    #[allow(clippy::too_many_arguments)]
    async fn order_descriptor_parts(
        &self,
        conn: &mut PgConnection,
        update: &Map<String, Value>,
        participants: &[Uuid],
        before: &OrderRow,
        public_before: bool,
        public_after: bool,
        matched: usize,
    ) -> Result<MutationEffect, MarketWriteError> {
        let row = sqlx::query("SELECT o.side::text AS side,p.name AS product_name,p.fuel_type,d.name AS point_name,d.region,o.provenance::text AS provenance,o.updated_at,o.price_per_mt_usd,o.remaining_quantity_mt,o.acceptance_ordinal FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$1")
            .bind(self.order)
            .fetch_one(conn)
            .await?;
        let product = self
            .policy
            .products
            .iter()
            .find(|p| p.id == before.product_id);
        let provenance: String = row.try_get("provenance")?;
        let activity = OrderActivity {
            id: self.order,
            side: row.try_get("side")?,
            product_name: row.try_get("product_name")?,
            fuel_type: row.try_get("fuel_type")?,
            region: row
                .try_get::<Option<String>, _>("region")?
                .unwrap_or_default(),
            source_kind: match provenance.as_str() {
                "REAL" => "LIVE_ORDER",
                "DEMO" => "DEMO_SEED",
                _ => "UNKNOWN",
            }
            .into(),
            scope: if before.delivery_point_id.is_some() {
                "DELIVERY_POINT"
            } else {
                "UNKNOWN"
            }
            .into(),
            demo_status: match provenance.as_str() {
                "REAL" => "REAL_ONLY",
                "DEMO" => "DEMO_ONLY",
                _ => "UNKNOWN",
            }
            .into(),
            is_demo_listing: provenance == "DEMO",
            unknown_count: u32::from(provenance == "UNKNOWN"),
            market_product: product.map(|p| p.code.clone()),
            delivery_point_id: before.delivery_point_id,
            availability_window: before.availability_window.clone(),
            observed_at: Some(self.now),
            delivery_point: row.try_get("point_name")?,
        };
        // The audit carries only fields whose effective value changed.
        let _ = &self.input;
        let mut changes = Map::new();
        for field in [
            "quantity_mt",
            "price_per_mt_usd",
            "remaining_quantity_mt",
            "status",
            "expires_at",
            "certifications",
            "certification_declared",
            "certification_scheme",
            "specification_standard",
            "msds_available",
            "carbon_intensity_gco2_mj",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec",
            "off_spec_notes",
            "fame_terms",
            "lng_terms",
        ] {
            if let Some(value) = update.get(field) {
                changes.insert(field.into(), json!({"from": null, "to": value}));
            }
        }
        let versioned = product.is_some_and(|p| p.qualification_profile == "fame_v1");
        if versioned {
            changes.insert(
                "version".into(),
                json!({"from": before.version, "to": before.version + 1}),
            );
        }
        Ok(MutationEffect {
            kind: MutationKind::OrderUpdated,
            actor_id: self.access.subject,
            organization_id: before.organization_id,
            resource_type: "order".into(),
            resource_id: self.order,
            changes: Value::Object(changes),
            event: Some(ParticipantEvent {
                event_type: "order_updated".into(),
                aggregate_type: "order".into(),
                aggregate_id: self.order.to_string(),
                participant_org_ids: participants.to_vec(),
                fields: ParticipantFields::OrderUpdated(activity),
                public_invalidated: (public_before || public_after) || matched > 0,
            }),
            at: self.now,
            audit_context: self.audit.clone(),
        })
    }
}

/// Load a row that may be absent; the missing case is the caller's 404.
async fn load_optional(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Option<OrderRow>, MarketWriteError> {
    match super::matching::load_order(conn, id).await {
        Ok(row) => Ok(Some(row)),
        Err(error) if error.code == MarketErrorCode::MarketSliceChanged => Ok(None),
        Err(error) => Err(error),
    }
}

/// The source's ownership rule: the concrete owner, or the same-organization
/// fallback only where the row carries no concrete owner. An assisted listing
/// never amends; it is cancelled and replaced.
fn authorize(actor: &AuthenticatedActor, order: &OrderRow) -> Result<(), MarketWriteError> {
    if actor.account().role == super::authority::ActorRole::Admin {
        return Err(unauthorized());
    }
    if order.creation_method == super::matching::CreationMethod::MarketSupport {
        return Err(super::orders::refusal(
            MarketErrorCode::ValidationFailed,
            StatusCode::CONFLICT,
            "Assisted listings cannot be edited; cancel and create a replacement",
        ));
    }
    let owns = match order.owner_user_id {
        Some(owner) => owner == actor.user_id(),
        None => actor
            .account()
            .current_organization_id
            .is_some_and(|org| org == order.organization_id),
    };
    if !owns {
        return Err(unauthorized());
    }
    Ok(())
}

/// The priority-loss predicates of the source helper: every amendment that
/// admits new execution opportunity allocates a fresh ordinal.
fn loses_priority(order: &OrderRow, update: &Map<String, Value>) -> bool {
    let new_price = update
        .get("price_per_mt_usd")
        .and_then(Value::as_str)
        .and_then(|text| Decimal::from_str_exact(text).ok())
        .unwrap_or(order.price_per_mt_usd);
    if new_price != order.price_per_mt_usd {
        return true;
    }
    let new_quantity = update
        .get("quantity_mt")
        .and_then(Value::as_str)
        .and_then(|text| Decimal::from_str_exact(text).ok())
        .unwrap_or(order.quantity_mt);
    if new_quantity > order.quantity_mt {
        return true;
    }
    if let Some(expiry) = update.get("expires_at") {
        let new_expiry = expiry
            .as_str()
            .and_then(|text| text.parse::<DateTime<Utc>>().ok());
        if new_expiry != order.expires_at
            && order.expires_at.is_some()
            && (new_expiry.is_none() || new_expiry > order.expires_at)
        {
            return true;
        }
    }
    if let Some(certifications) = update.get("certifications") {
        let normalize = |value: &Value| -> BTreeSet<Option<String>> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str())
                .map(|text| crate::market::terms::normalize_certification_scheme(Some(text)))
                .collect()
        };
        let old = json!(order.terms.certifications);
        if normalize(&old) != normalize(certifications) {
            return true;
        }
    }
    for field in ["certification_scheme", "fame_terms", "lng_terms"] {
        if let Some(value) = update.get(field) {
            let stored: Value = match field {
                "certification_scheme" => order.terms.certification_scheme.clone().into(),
                "fame_terms" => order.terms.fame_terms.clone().unwrap_or(Value::Null),
                _ => order.terms.lng_terms.clone().unwrap_or(Value::Null),
            };
            if *value != stored {
                return true;
            }
        }
    }
    update.get("off_spec") == Some(&json!(false)) && order.terms.off_spec
}

/// Whether the amendment moves a previously unqualified row to executable.
/// Every scalar supplier field can complete a qualification requirement, so
/// the whole effective declaration is re-checked against the locked row. The
/// fresh ordinal rule for this transition is an approved extension of the
/// source's priority helper, decided in the release plan.
fn newly_qualified(
    policy: &MarketPolicy,
    order: &OrderRow,
    update: &Map<String, Value>,
    today: chrono::NaiveDate,
) -> bool {
    let effective = |name: &str, stored: Option<&str>| -> Option<String> {
        match update.get(name) {
            None | Some(Value::Null) => stored.map(str::to_owned),
            Some(value) => value.as_str().map(str::to_owned),
        }
    };
    let intensity = match update.get("carbon_intensity_gco2_mj") {
        None => order.terms.carbon_intensity_gco2_mj.map(|v| v.to_string()),
        Some(value) => value.as_str().map(str::to_owned),
    };
    // Every projection field borrows, so each effective value is a named local.
    let delivery_point = order.delivery_point_id.map(|id| id.to_string());
    let scheme = effective(
        "certification_scheme",
        order.terms.certification_scheme.as_deref(),
    );
    let standard = effective(
        "specification_standard",
        order.terms.specification_standard.as_deref(),
    );
    let method = effective(
        "carbon_intensity_method",
        order.terms.carbon_intensity_method.as_deref(),
    );
    let feedstock = effective("feedstock", order.terms.feedstock.as_deref());
    let origin = effective("origin", order.terms.origin.as_deref());
    let projection = crate::market::terms::QualificationInput {
        product_id: Some(order.product_id),
        market_product: None,
        side: order.side.as_str(),
        delivery_point_id: delivery_point.as_deref(),
        availability_window: &order.availability_window,
        off_spec: update
            .get("off_spec")
            .and_then(Value::as_bool)
            .unwrap_or(order.terms.off_spec),
        msds_available: update
            .get("msds_available")
            .and_then(Value::as_bool)
            .unwrap_or(order.terms.msds_available),
        certification_scheme: scheme.as_deref(),
        certification_declared: update
            .get("certification_declared")
            .and_then(Value::as_bool)
            .unwrap_or(order.terms.certification_declared),
        fame_terms: update.get("fame_terms").or(order.terms.fame_terms.as_ref()),
        lng_terms: update.get("lng_terms").or(order.terms.lng_terms.as_ref()),
        specification_standard: standard.as_deref(),
        carbon_intensity_method: method.as_deref(),
        carbon_intensity_gco2_mj: intensity.as_deref(),
        feedstock: feedstock.as_deref(),
        origin: origin.as_deref(),
    };
    crate::market::terms::order_is_execution_qualified_at(policy, &projection, today)
        && !super::matching::order_is_execution_qualified(
            order,
            MatchBound::new(order.created_at).with_policy(policy),
        )
}

/// Allocate the next acceptance ordinal under the held slice lock.
async fn next_ordinal(conn: &mut PgConnection) -> Result<i64, MarketWriteError> {
    Ok(
        sqlx::query_scalar("SELECT nextval('orderbook_acceptance_ordinal_seq')")
            .fetch_one(conn)
            .await?,
    )
}

/// One amendment inside the sealed transaction, from the locked row to the
/// composed terminal effect set.
pub(super) struct AmendPort<'a> {
    command: &'a AmendCommand,
    #[allow(dead_code)]
    context: &'a WriteContext,
    plan: &'a OperationLockPlan,
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}

// The port is constructed only through the sealed transaction's accessor; its
// fields stay private to this module.

/// Everything the amendment staged before its terminal effect set: the captured
/// before-state, the pre-mutation public visibility, an optional match outcome,
/// the effective update that was written, and the participant organizations the
/// event must reach.
pub(super) struct StagedAmend {
    before: BeforeEffects,
    public_before: bool,
    update: Map<String, Value>,
}

impl<'a> AmendPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        plan: &'a OperationLockPlan,
        locked: &'a Locked,
        command: &'a AmendCommand,
        context: &'a WriteContext,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self {
            command,
            context,
            plan,
            conn,
            locked,
        })
    }
    fn conn(&mut self) -> &mut PgConnection {
        self.conn
    }

    /// The complete effective update after the source's validation order:
    /// window immutability, typed terms with derived metadata, BID stripping,
    /// ASK certification and metadata requirements.
    async fn effective_update(
        &mut self,
        order: &OrderRow,
    ) -> Result<Map<String, Value>, MarketWriteError> {
        let command = self.command;
        let mut update = command.input.fields.clone();
        // A non-null changed window is an identity change; a supplied equal
        // window is accepted then removed, exactly as the source removes it.
        if let Some(window) = update.remove("availability_window")
            && !window.is_null()
            && window.as_str().unwrap_or_default() != order.availability_window
        {
            return Err(super::orders::refusal(
                MarketErrorCode::OrderIdentityImmutable,
                StatusCode::CONFLICT,
                "Executable order market identity is immutable; cancel and create a new order",
            ));
        }
        let profile = command
            .product(order.product_id)
            .map(|p| p.qualification_profile.as_str())
            .unwrap_or("scalar");
        // Typed terms: an absent update field falls back to the stored row, so
        // a partial update re-derives its metadata from the stored declaration.
        let stored = |field: &str| -> Result<Option<Value>, MarketWriteError> {
            let terms = if field == "fame_terms" {
                &order.terms.fame_terms
            } else {
                &order.terms.lng_terms
            };
            terms
                .clone()
                .map(|terms| {
                    crate::market::terms::owner_response_terms(
                        if field == "fame_terms" {
                            "fame_v1"
                        } else {
                            "lng_v1"
                        },
                        &terms,
                    )
                })
                .transpose()
                .map_err(|_| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "The stored terms are not valid",
                    )
                })
        };
        let fame = match update.get("fame_terms") {
            None | Some(Value::Null) => (profile == "fame_v1")
                .then(|| stored("fame_terms"))
                .transpose()?
                .flatten(),
            Some(terms) => {
                if profile != "fame_v1" {
                    return Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "The structured terms do not belong to this product",
                    ));
                }
                crate::market::terms::validate_fame_terms(
                    order.side.as_str(),
                    Some(terms),
                    command.bound().today(),
                )
                .map_err(|_| {
                    refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Valid B100 requirements or declarations are required",
                    )
                })?;
                Some(
                    crate::market::terms::owner_response_terms("fame_v1", terms).map_err(|_| {
                        refusal(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Valid B100 requirements or declarations are required",
                        )
                    })?,
                )
            }
        };
        let lng = match update.get("lng_terms") {
            None | Some(Value::Null) => (profile == "lng_v1")
                .then(|| stored("lng_terms"))
                .transpose()?
                .flatten(),
            Some(terms) => {
                if profile != "lng_v1" {
                    return Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "The gas-fuel terms do not belong to this product",
                    ));
                }
                crate::market::terms::validate_lng_terms(order.side.as_str(), Some(terms))
                    .map_err(|_| {
                        refusal(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Valid Bio-LNG requirements or declarations are required",
                        )
                    })?;
                Some(
                    crate::market::terms::owner_response_terms("lng_v1", terms).map_err(|_| {
                        refusal(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "Valid Bio-LNG requirements or declarations are required",
                        )
                    })?,
                )
            }
        };
        // The structured-fuel minimum applies to the effective quantity.
        for field in ["fame_terms", "lng_terms"] {
            let terms = if field == "fame_terms" { &fame } else { &lng };
            if let Some(terms) = terms {
                let effective = update
                    .get("quantity_mt")
                    .and_then(Value::as_str)
                    .and_then(|text| Decimal::from_str_exact(text).ok())
                    .unwrap_or(order.quantity_mt);
                if effective < Decimal::ONE {
                    return Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Structured-fuel orders require at least 1 MT",
                    ));
                }
                update.insert(field.into(), terms.clone());
                // Derive the display fields exactly as the source does after
                // re-inserting the declaration.
                let mut carrier = Map::new();
                carrier.insert(field.into(), terms.clone());
                carrier.insert("side".into(), json!(order.side.as_str()));
                super::order_create::derive_metadata(&mut carrier, field)?;
                for (name, value) in carrier {
                    if name != "side" {
                        update.insert(name, value);
                    }
                }
            }
        }
        // A BID stores no supplier metadata beyond the scheme qualifier.
        if order.side == Side::Bid {
            for field in ASK_ONLY {
                update.remove(field);
            }
        } else {
            let declared = update
                .get("certification_declared")
                .and_then(Value::as_bool)
                .unwrap_or(order.terms.certification_declared);
            let scheme = match update.get("certification_scheme") {
                None | Some(Value::Null) => order.terms.certification_scheme.clone(),
                Some(value) => value.as_str().map(str::to_owned),
            };
            if !declared || scheme.as_deref().unwrap_or_default().is_empty() {
                return Err(bad(
                    "ASK orders require an explicit certification declaration and scheme",
                ));
            }
            if fame.is_some() || lng.is_some() {
                let msds = update
                    .get("msds_available")
                    .and_then(Value::as_bool)
                    .unwrap_or(order.terms.msds_available);
                if !msds {
                    return Err(refusal(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "ASK orders require an available safety data sheet",
                    ));
                }
            } else if profile == "scalar" {
                let effective = |name: &str, fallback: Option<String>| match update.get(name) {
                    None | Some(Value::Null) => fallback,
                    Some(value) => value.as_str().map(str::to_owned),
                };
                let standard = effective(
                    "specification_standard",
                    order.terms.specification_standard.clone(),
                );
                let method = effective(
                    "carbon_intensity_method",
                    order.terms.carbon_intensity_method.clone(),
                );
                let feedstock = effective("feedstock", order.terms.feedstock.clone());
                let origin = effective("origin", order.terms.origin.clone());
                let msds = update
                    .get("msds_available")
                    .and_then(Value::as_bool)
                    .unwrap_or(order.terms.msds_available);
                let intensity = match update.get("carbon_intensity_gco2_mj") {
                    None => order.terms.carbon_intensity_gco2_mj.map(|v| v.to_string()),
                    Some(value) => value.as_str().map(str::to_owned),
                };
                let product = command
                    .product(order.product_id)
                    .ok_or_else(|| bad("The product is not in the market policy"))?;
                let missing = standard.as_deref().unwrap_or_default().trim().is_empty()
                    || !msds
                    || intensity.is_none()
                    || feedstock.as_deref().unwrap_or_default().trim().is_empty()
                    || origin.as_deref().unwrap_or_default().trim().is_empty()
                    || method.as_deref().unwrap_or_default().trim().is_empty()
                    || product
                        .specification_standard
                        .as_deref()
                        .is_some_and(|required| {
                            !standard
                                .as_deref()
                                .unwrap_or_default()
                                .trim()
                                .eq_ignore_ascii_case(required.trim())
                        });
                if missing {
                    return Err(bad("ASK orders require complete supplier metadata"));
                }
            }
        }
        Ok(update)
    }

    /// Write the effective update to the locked row. The version bumps only
    /// when an effective field changed; matching may bump it again.
    async fn write(
        &mut self,
        order: &OrderRow,
        update: &Map<String, Value>,
    ) -> Result<(), MarketWriteError> {
        let command = self.command;
        let terms_changed = update.iter().any(|(field, value)| {
            let stored: Value = match field.as_str() {
                "quantity_mt" => json!(order.quantity_mt.to_string()),
                "price_per_mt_usd" => json!(order.price_per_mt_usd.to_string()),
                "expires_at" => match order.expires_at {
                    Some(at) => json!(crate::routes::ports::pydantic_datetime(at)),
                    None => Value::Null,
                },
                "certifications" => json!(order.terms.certifications),
                "certification_declared" => json!(order.terms.certification_declared),
                "certification_scheme" => match &order.terms.certification_scheme {
                    Some(scheme) => json!(scheme),
                    None => Value::Null,
                },
                "specification_standard" => match &order.terms.specification_standard {
                    Some(text) => json!(text),
                    None => Value::Null,
                },
                "msds_available" => json!(order.terms.msds_available),
                "carbon_intensity_gco2_mj" => match order.terms.carbon_intensity_gco2_mj {
                    Some(value) => json!(value.to_string()),
                    None => Value::Null,
                },
                "carbon_intensity_method" => match &order.terms.carbon_intensity_method {
                    Some(text) => json!(text),
                    None => Value::Null,
                },
                "feedstock" => match &order.terms.feedstock {
                    Some(text) => json!(text),
                    None => Value::Null,
                },
                "origin" => match &order.terms.origin {
                    Some(text) => json!(text),
                    None => Value::Null,
                },
                "off_spec" => json!(order.terms.off_spec),
                "fame_terms" => order.terms.fame_terms.clone().unwrap_or(Value::Null),
                "lng_terms" => order.terms.lng_terms.clone().unwrap_or(Value::Null),
                "remaining_quantity_mt" => json!(order.remaining_quantity_mt.to_string()),
                "status" => json!(order.status.as_str()),
                "acceptance_ordinal" => json!(order.acceptance_ordinal),
                _ => return false,
            };
            *value != stored
        });
        // One statement with typed binds: the row, then every effective field
        // in a fixed order. Only fields the update carries are assigned, so an
        // omitted field keeps its stored value.
        let mut payload = Map::new();
        payload.insert("id".into(), json!(order.id.to_string()));
        payload.insert("updated_at".into(), json!(command.now.to_rfc3339()));
        // The clock is bound directly, not through the payload text.
        if terms_changed {
            payload.insert("version".into(), json!(order.version + 1));
        }
        for (field, value) in update {
            payload.insert(field.clone(), value.clone());
        }
        let columns = [
            "quantity_mt",
            "price_per_mt_usd",
            "remaining_quantity_mt",
            "status",
            "acceptance_ordinal",
            "version",
            "expires_at",
            "certifications",
            "certification_declared",
            "certification_scheme",
            "specification_standard",
            "msds_available",
            "carbon_intensity_gco2_mj",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec",
            "off_spec_notes",
            "fame_terms",
            "lng_terms",
            "updated_at",
        ];
        let assigned: Vec<&str> = columns
            .into_iter()
            .filter(|column| payload.contains_key(*column))
            .collect();
        // JSON columns carry serialized text and need an explicit cast; the
        // statement names the cast so the parameter type stays unambiguous.
        let json_columns = ["certifications", "fame_terms", "lng_terms"];
        let assigns = assigned
            .iter()
            .enumerate()
            .map(|(index, column)| {
                if json_columns.contains(column) {
                    format!("{column}=${}::jsonb", index + 1)
                } else {
                    format!("{column}=${}", index + 1)
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        let statement = format!(
            "UPDATE orderbook_orders SET {assigns} WHERE id=${}",
            assigned.len() + 1
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(statement));
        for column in &assigned {
            let value = &payload[*column];
            query = match *column {
                "quantity_mt"
                | "price_per_mt_usd"
                | "remaining_quantity_mt"
                | "carbon_intensity_gco2_mj" => {
                    query.bind(value.as_str().and_then(|t| t.parse::<Decimal>().ok()))
                }
                "acceptance_ordinal" | "version" => query.bind(value.as_i64()),
                "expires_at" => {
                    query.bind(value.as_str().and_then(|t| t.parse::<DateTime<Utc>>().ok()))
                }
                "updated_at" => query.bind(command.now),
                // JSON columns bind their serialized text; the statement casts.
                "certifications" | "fame_terms" | "lng_terms" => {
                    query.bind(serde_json::to_string(value).ok())
                }
                // Boolean columns bind natively; the payload carries booleans.
                "certification_declared" | "msds_available" | "off_spec" => {
                    query.bind(value.as_bool())
                }
                // Text columns carry their scalar text.
                _ => query.bind(value.as_str().map(str::to_owned)),
            };
        }
        let query = query.bind(order.id);
        query.execute(&mut *self.conn).await?;
        Ok(())
    }

    /// Run the complete amendment: revalidate, apply the update and its exact
    /// stock delta, optionally rematch, then stage one terminal effect set.
    async fn run(&mut self, order: &OrderRow) -> Result<StagedAmend, MarketAttemptError> {
        let command = self.command;
        // The locked-row state checks the source performs after its locks.
        if order.expires_at.is_some_and(|at| at <= command.now)
            && matches!(
                order.status,
                OrderStatus::Open | OrderStatus::PartiallyFilled
            )
        {
            return Err(bad("Order has expired").into());
        }
        if !matches!(
            order.status,
            OrderStatus::Open | OrderStatus::PartiallyFilled
        ) {
            return Err(bad("Can only update orders with OPEN or PARTIALLY_FILLED status").into());
        }
        let update = self.effective_update(order).await?;
        // Quantity is recomputed against the locked row: the preview may have
        // waited behind another update on the same slice.
        let mut update = update;
        let mut new_status: Option<&str> = None;
        if let Some(quantity) = update
            .get("quantity_mt")
            .and_then(Value::as_str)
            .and_then(|text| Decimal::from_str_exact(text).ok())
        {
            let filled = order.quantity_mt - order.remaining_quantity_mt;
            let new_remaining = quantity - filled;
            if new_remaining < Decimal::ZERO {
                return Err(bad("New quantity cannot be less than already filled amount").into());
            }
            let minimum: Option<Decimal> =
                sqlx::query_scalar("SELECT min_lot_size FROM products WHERE id=$1")
                    .bind(order.product_id)
                    .fetch_one(self.conn())
                    .await?;
            let minimum = minimum.filter(|v| *v > Decimal::ZERO).ok_or_else(|| {
                refusal(StatusCode::CONFLICT, "Product minimum lot is unavailable")
            })?;
            if quantity < minimum {
                return Err(bad("Order quantity is below the product minimum lot").into());
            }
            update.insert(
                "remaining_quantity_mt".into(),
                json!(new_remaining.to_string()),
            );
            if new_remaining == Decimal::ZERO {
                new_status = Some("FILLED");
            }
            // Reserve or release exactly the remainder delta against the
            // linked inventory row, which the plan already locked.
            if let Some(item) = order.inventory_item_id {
                let delta = new_remaining - order.remaining_quantity_mt;
                let mut ledger = super::transaction::StockLedger::new(self.conn, &[item], &[]);
                if delta > Decimal::ZERO {
                    ledger.reserve(item, StockQuantity::checked(delta)?).await?;
                } else if delta < Decimal::ZERO {
                    ledger
                        .release(item, StockQuantity::checked(-delta)?, true)
                        .await?;
                }
            }
        }
        if let Some(status) = new_status {
            update.insert("status".into(), json!(status));
        }
        let ordinal = if loses_priority(order, &update)
            || newly_qualified(&command.policy, order, &update, command.bound().today())
        {
            Some(next_ordinal(self.conn()).await?)
        } else {
            None
        };
        if let Some(ordinal) = ordinal {
            update.insert("acceptance_ordinal".into(), json!(ordinal));
        }
        // Capture the before-state before the first mutation; the matcher's
        // resting rows join it before completion. The amended row is the one
        // economic row this port names: the plan keeps candidates open.
        let owned = [command.order];
        let mut before = EffectPort::new(self.conn, &owned, self.locked)
            .capture_before(
                &command.policy,
                &command.config,
                &owned,
                command.now,
            )
            .await?;
        let public_before = super::effects::order_visible(
            self.conn,
            &command.policy,
            &command.config,
            command.order,
            command.now,
        )
        .await?;
        self.write(order, &update).await?;
        // The rematch is composed by the sealed transaction after the port
        // returns: the planner acquires the amended row and any candidates in
        // the open candidate pass, which this plan keeps undeclared.
        Ok(StagedAmend {
            before,
            public_before,
            update,
        })
    }

}
