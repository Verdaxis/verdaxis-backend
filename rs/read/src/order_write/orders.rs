//! Order cancellation and organization-private owner projections.
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    AdmissionClass, AuthenticatedActor, Canonical, CommittedCommand, MarketErrorCode,
    MarketWriteError, Operation, ReceiptRecord, WriteContext,
    effects::{AuditContext, MutationEffect, MutationKind},
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview, SliceKey},
    outbox::{OrderActivity, ParticipantEvent, ParticipantFields},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{AppState, MarketPolicy};

/// A refusal with an explicit code, and the status and text these routes keep
/// for their clients where they differ from the code's own row.
pub(crate) fn refusal(
    code: MarketErrorCode,
    status: axum::http::StatusCode,
    message: &'static str,
) -> MarketWriteError {
    let mut error = MarketWriteError::new(code);
    error.status = status;
    error.message = message;
    error
}
fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn forbidden() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::OrderOwnerForbidden)
}

#[derive(Clone)]
pub(crate) struct OrderRequest {
    pub id: Option<Uuid>,
    pub context: Option<Uuid>,
    pub input: crate::routes::market_write::orders::CancelInput,
    pub skip: i64,
    pub limit: i64,
    pub template: bool,
    pub audit: AuditContext,
    pub support_enabled: bool,
}
struct OrderCommand {
    request: OrderRequest,
    policy: Arc<MarketPolicy>,
    config: Arc<crate::Config>,
    hash: String,
    access: Arc<crate::auth::VerifiedAccess>,
}

pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: OrderRequest,
) -> Result<CommittedCommand, MarketWriteError> {
    if request.id.is_none() {
        return dispatch_read(app, access, request).await;
    }
    let command_access = Arc::new(crate::auth::VerifiedAccess {
        subject: access.subject,
        issued_at: access.issued_at,
        issuance_out_of_range: access.issuance_out_of_range,
    });
    let payload = json!({"operation":"order.cancel","order_id":request.id,"reason":request.input.reason,"if_match":request.input.if_match});
    let canonical = Canonical::from_json(&payload).map_err(|_| internal())?;
    let hash = super::command_request_hash(&canonical);
    let class = if request.id.is_some() {
        AdmissionClass::Cleanup
    } else {
        AdmissionClass::Execution
    };
    run_market_command(
        app.pools.read.clone(),
        app.reads.clone(),
        access,
        OrderCommand {
            request,
            policy: app.policy,
            config: app.config,
            hash,
            access: command_access,
        },
        class,
    )
    .await
}

fn check_reason(
    order: &OrderIdentity,
    input: &crate::routes::market_write::orders::CancelInput,
) -> Result<(), MarketWriteError> {
    if (input.post || order.method == "MARKET_SUPPORT") && input.reason.is_none() {
        return Err(refusal(
            MarketErrorCode::ValidationFailed,
            axum::http::StatusCode::BAD_REQUEST,
            "A cancellation reason is required",
        ));
    }
    Ok(())
}
fn bumps_version(order: &OrderIdentity, policy: &MarketPolicy) -> bool {
    order.method == "MARKET_SUPPORT"
        || policy
            .products
            .iter()
            .any(|p| p.id == order.product && p.qualification_profile == "fame_v1")
}
fn api_error(error: MarketWriteError) -> crate::ApiError {
    crate::ApiError {
        status: error.status,
        detail: json!({"code":error.code.as_str(),"message":error.message}),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
}
async fn dispatch_read(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: OrderRequest,
) -> Result<CommittedCommand, MarketWriteError> {
    let policy = app.policy.clone();
    let config = app.config.clone();
    let path = if request.template {
        "/api/orderbook/my/latest-ask-template"
    } else {
        "/api/orderbook/my"
    };
    let auth_request =
        crate::auth::AuthenticatedRequest::new("GET", path, request.context.is_some());
    let result = crate::auth::run_authenticated(&app, access, auth_request, move |tx| {
        Box::pin(async move {
            let actor = tx.actor().clone();
            let now = Utc::now();
            let context_org = resolve_support(tx.connection(), actor.id, request.context, now)
                .await
                .map_err(api_error)?;
            let organization = context_org.or(actor.organization_id).ok_or_else(|| {
                api_error(refusal(
                    MarketErrorCode::ValidationFailed,
                    axum::http::StatusCode::BAD_REQUEST,
                    "User must belong to an organization",
                ))
            })?;
            owner_read(
                tx.connection(),
                organization,
                &request,
                now,
                &policy,
                &config,
            )
            .await
            .map_err(api_error)
        })
    })
    .await;
    match result {
        Ok(body) => Ok(CommittedCommand {
            response_status: 200,
            response_body: body,
            replayed: false,
        }),
        Err(crate::auth::AuthenticatedError::Authentication(error)) => Err(refusal(
            match error {
                crate::auth::AuthError::PasswordChanged => MarketErrorCode::AuthPasswordChanged,
                crate::auth::AuthError::SessionRevoked => MarketErrorCode::AuthSessionRevoked,
                _ => MarketErrorCode::AuthCredentialsInvalid,
            },
            axum::http::StatusCode::from_u16(error.status()).unwrap(),
            "Account is not available",
        )),
        Err(crate::auth::AuthenticatedError::Database(error)) => {
            let code = if error.detail["code"] == "MARKET_SUPPORT_CONTEXT_INVALID" {
                MarketErrorCode::MarketSupportContextInvalid
            } else {
                MarketErrorCode::ValidationFailed
            };
            Err(refusal(
                code,
                error.status,
                "The owner read is not available",
            ))
        }
        Err(_) => Err(internal()),
    }
}

/// Resolve support authority before resource lookup. The ordered write path
/// rechecks these facts under its capability and context locks.
async fn resolve_support(
    conn: &mut PgConnection,
    actor: Uuid,
    context: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, MarketWriteError> {
    let Some(context) = context else {
        return Ok(None);
    };
    let missing = || {
        refusal(
            MarketErrorCode::MarketSupportContextInvalid,
            axum::http::StatusCode::NOT_FOUND,
            "Support context not found",
        )
    };
    let invalid = || MarketWriteError::new(MarketErrorCode::MarketSupportContextInvalid);
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role::text='ADMIN' AND status::text='APPROVED')")
        .bind(actor).fetch_one(&mut *conn).await?;
    if !allowed {
        return Err(missing());
    }
    let capabilities: i64 = sqlx::query_scalar("SELECT count(DISTINCT capability) FROM staff_capability_assignments WHERE user_id=$1 AND capability::text IN ('MARKET_SUPPORT_AUTHORIZATIONS','MARKET_SUPPORT_LISTINGS') AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2)")
        .bind(actor).bind(now).fetch_one(&mut *conn).await?;
    if capabilities != 2 {
        return Err(MarketWriteError::new(MarketErrorCode::NotFound));
    }
    let row = sqlx::query("SELECT c.organization_id,c.status::text AS status,c.scope::text AS scope,c.expires_at,o.verification_status::text AS organization_status,o.provenance::text AS provenance FROM market_support_contexts c JOIN organizations o ON o.id=c.organization_id WHERE c.id=$1 AND c.actor_user_id=$2")
        .bind(context).bind(actor).fetch_optional(&mut *conn).await?.ok_or_else(missing)?;
    if row.get::<String, _>("status") != "ACTIVE"
        || row.get::<String, _>("scope") != "ASSISTED_ORDER_ENTRY"
        || row.get::<DateTime<Utc>, _>("expires_at") <= now
        || row.get::<String, _>("organization_status") != "APPROVED"
        || row.get::<String, _>("provenance") != "REAL"
    {
        return Err(invalid());
    }
    Ok(Some(row.get("organization_id")))
}

#[derive(Clone)]
struct OrderIdentity {
    id: Uuid,
    organization: Uuid,
    owner: Option<Uuid>,
    product: Uuid,
    point: Option<Uuid>,
    window: String,
    item: Option<Uuid>,
    method: String,
    authorization: Option<Uuid>,
    version: i32,
    status: String,
}
async fn identity(conn: &mut PgConnection, id: Uuid) -> Result<OrderIdentity, MarketWriteError> {
    let row = sqlx::query("SELECT id,organization_id,owner_user_id,product_id,delivery_point_id,availability_window,inventory_item_id,creation_method::text AS creation_method,support_authorization_id,version,status::text AS status FROM orderbook_orders WHERE id=$1")
        .bind(id).fetch_optional(conn).await?.ok_or(MarketWriteError::new(MarketErrorCode::OrderNotFound))?;
    Ok(OrderIdentity {
        id,
        organization: row.try_get("organization_id")?,
        owner: row.try_get("owner_user_id")?,
        product: row.try_get("product_id")?,
        point: row.try_get("delivery_point_id")?,
        window: row.try_get("availability_window")?,
        item: row.try_get("inventory_item_id")?,
        method: row.try_get("creation_method")?,
        authorization: row.try_get("support_authorization_id")?,
        version: row.try_get("version")?,
        status: row.try_get("status")?,
    })
}

impl MarketCommand for OrderCommand {
    type Output = CommittedCommand;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let checked = super::authority::admission::authenticate_preview(
                conn,
                self.access.clone(),
                AdmissionClass::Cleanup,
            )
            .await?;
            if self.request.context.is_none()
                && self.request.support_enabled
                && checked.account().role == super::authority::ActorRole::Admin
            {
                return Err(MarketWriteError::new(
                    MarketErrorCode::MarketSupportLegacyMutationRetired,
                )
                .into());
            }
            if self.request.context.is_some() {
                resolve_support(conn, actor, self.request.context, Utc::now()).await?;
            }
            let actor_org: Option<Uuid> =
                sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                    .bind(actor)
                    .fetch_optional(&mut *conn)
                    .await?
                    .flatten();
            let order = match self.request.id {
                Some(id) => Some(identity(conn, id).await?),
                None => None,
            };
            if let Some(order) = &order {
                check_reason(order, &self.request.input)?;
            }
            let context_org = if let Some(id) = self.request.context {
                sqlx::query_scalar::<_,Uuid>("SELECT organization_id FROM market_support_contexts WHERE id=$1 AND actor_user_id=$2").bind(id).bind(actor).fetch_optional(&mut *conn).await?
            } else {
                None
            };
            let organization = context_org
                .or_else(|| order.as_ref().map(|o| o.organization))
                .or(actor_org);
            let mut organizations: Vec<_> = organization.into_iter().chain(actor_org).collect();
            organizations.sort_unstable();
            organizations.dedup();
            let users = order.as_ref().and_then(|o| o.owner);
            let capabilities: Vec<Uuid> = if self.request.context.is_some() {
                sqlx::query_scalar("SELECT id FROM staff_capability_assignments WHERE user_id=$1 AND capability::text IN ('MARKET_SUPPORT_AUTHORIZATIONS','MARKET_SUPPORT_LISTINGS') ORDER BY id").bind(actor).fetch_all(&mut *conn).await?
            } else {
                vec![]
            };
            let facts = json!({"actor_organization":actor_org,"organization":organization,"context":self.request.context,"context_organization":context_org,"capabilities":capabilities,"order":order.as_ref().map(|o|json!({"id":o.id,"organization":o.organization,"owner":o.owner,"product":o.product,"point":o.point,"window":o.window,"item":o.item,"method":o.method,"authorization":o.authorization}))});
            let parties = PartyLockPreview::new(organizations, users, actor, facts);
            let mut locks = OperationLockPlan::new(MarketOperation::Cancel);
            locks.parties(&parties)?;
            if let Some(key) = &self.request.input.key {
                locks.idempotency(actor, Operation::OrderCancel.as_str(), key)?;
            }
            if let Some(order) = &order {
                locks.slices([SliceKey {
                    product_id: order.product,
                    delivery_point_id: order.point,
                    availability_window: order.window.clone(),
                }])?;
            }
            locks.resources(LockClass::Capability, capabilities)?;
            locks.resources(LockClass::Context, self.request.context)?;
            if let Some(order) = order {
                locks.orders([order.id])?;
                locks.inventory(order.item)?;
            }
            Ok(MarketPreview { parties, locks })
        })
    }
    fn replay<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async move {
            let Some(id) = self.request.id else {
                return Ok(None);
            };
            let order = tx.order_port()?.load(id).await?;
            check_reason(&order, &self.request.input)?;
            let context = tx
                .order_port()?
                .authorize(actor, id, self.request.context, Utc::now())
                .await?;
            let Some(key) = &self.request.input.key else {
                return Ok(None);
            };
            Ok(tx
                .replay_receipt(&context, Operation::OrderCancel, id, key, &self.hash)
                .await?)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let now = Utc::now();
            let id = self.request.id.ok_or_else(internal)?;
            let order = tx.order_port()?.load(id).await?;
            check_reason(&order, &self.request.input)?;
            let context = tx
                .order_port()?
                .authorize(actor, id, self.request.context, now)
                .await?;
            if order.method == "MARKET_SUPPORT" {
                self.request.input.check_etag(id, order.version)?;
            }
            if !matches!(order.status.as_str(), "OPEN" | "PARTIALLY_FILLED") {
                // Cancellation keeps its 400 refusal for an order that is no longer live.
                return Err(refusal(
                    MarketErrorCode::OrderTerminal,
                    axum::http::StatusCode::BAD_REQUEST,
                    "Only a live order can be cancelled",
                )
                .into());
            }
            let before = tx
                .effects()?
                .capture_before(&self.policy, &self.config, &[id], now)
                .await?;
            let descriptor = tx
                .order_port()?
                .descriptor(
                    &context,
                    &order,
                    &self.request,
                    now,
                    &self.policy,
                    &self.config,
                )
                .await?;
            tx.order_port()?.withdraw(id, &self.policy, now).await?;
            if let Some(key) = &self.request.input.key {
                tx.record_receipt(
                    &context,
                    &ReceiptRecord {
                        id: Uuid::new_v4(),
                        actor_user_id: actor.user_id(),
                        effective_organization_id: context.effective_organization_id(),
                        support_context_id: context.support_context_id(),
                        operation: Operation::OrderCancel,
                        idempotency_key: key.clone(),
                        request_hash: self.hash.clone(),
                        resource_id: id,
                        response_status: 204,
                        response_body: Value::Null,
                    },
                )
                .await?;
            }
            let mut effects = before.stage();
            effects.push(descriptor)?;
            tx.effects()?
                .finish(&self.policy, &self.config, effects)
                .await?;
            Ok(CommittedCommand {
                response_status: 204,
                response_body: Value::Null,
                replayed: false,
            })
        })
    }
}

pub(super) struct OrderPort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
    orders: &'a [Uuid],
    items: &'a [Uuid],
}
impl<'a> OrderPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &'a Locked,
        orders: &'a [Uuid],
        items: &'a [Uuid],
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self {
            conn,
            locked,
            orders,
            items,
        })
    }
    async fn load(&mut self, id: Uuid) -> Result<OrderIdentity, MarketWriteError> {
        if !self.orders.contains(&id) {
            return Err(internal());
        }
        identity(self.conn, id).await
    }
    async fn authorize(
        &mut self,
        actor: &AuthenticatedActor,
        id: Uuid,
        support: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<WriteContext, MarketWriteError> {
        let order = self.load(id).await?;
        if let Some(context) = support
            && resolve_support(self.conn, actor.user_id(), Some(context), now).await?
                != Some(order.organization)
        {
            return Err(forbidden());
        }
        if support.is_none() && actor.account().role == super::authority::ActorRole::Admin {
            return Err(forbidden());
        }
        if support.is_some() {
            if order.method != "MARKET_SUPPORT" || order.authorization.is_none() {
                return Err(forbidden());
            }
        } else if order.method == "MARKET_SUPPORT" {
            if actor.account().current_organization_id != Some(order.organization)
                || actor.account().role == super::authority::ActorRole::Admin
            {
                return Err(forbidden());
            }
        } else if order.owner.map_or(
            actor.account().current_organization_id != Some(order.organization),
            |owner| owner != actor.user_id(),
        ) {
            return Err(forbidden());
        }
        super::authority::admission::order_context(
            self.conn,
            self.locked,
            actor,
            order.organization,
            support,
            now,
        )
        .await
    }
    async fn withdraw(
        &mut self,
        id: Uuid,
        policy: &MarketPolicy,
        at: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        let identity = identity(self.conn, id).await?;
        let mut ledger = super::transaction::StockLedger::new(self.conn, self.items, self.orders);
        let order = ledger.order(id).await?;
        ledger
            .withdraw_order(&order, "CANCELLED", bumps_version(&identity, policy))
            .await?;
        sqlx::query("UPDATE orderbook_orders SET updated_at=$2 WHERE id=$1")
            .bind(id)
            .bind(at)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
    async fn descriptor(
        &mut self,
        ctx: &WriteContext,
        order: &OrderIdentity,
        request: &OrderRequest,
        at: DateTime<Utc>,
        policy: &MarketPolicy,
        config: &crate::Config,
    ) -> Result<MutationEffect, MarketWriteError> {
        let row = sqlx::query("SELECT p.name,p.fuel_type,d.region,o.provenance::text AS provenance FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$1").bind(order.id).fetch_one(&mut *self.conn).await?;
        let original_context: Option<Uuid> = if ctx.support().is_some() {
            sqlx::query_scalar(
                "SELECT market_support_context_id FROM market_support_authorizations WHERE id=$1",
            )
            .bind(order.authorization)
            .fetch_optional(&mut *self.conn)
            .await?
            .flatten()
        } else {
            None
        };
        let changes = json!({"status":"CANCELLED","reason":request.input.reason,"actor_user_id":ctx.actor().user_id(),"effective_organization_id":ctx.effective_organization_id(),"original_principal_user_id":order.owner,"cancellation_principal_user_id":ctx.actor().user_id(),"support_context_id":ctx.support_context_id(),"original_support_context_id":original_context,"version":if bumps_version(order, policy) { order.version.checked_add(1).ok_or_else(internal)? } else { order.version }});
        let provenance: String = row.try_get("provenance")?;
        let activity_row = sqlx::query("SELECT o.side::text AS side,p.id AS product_id,d.name AS point_name FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$1").bind(order.id).fetch_one(&mut *self.conn).await?;
        let activity = OrderActivity {
            id: order.id,
            side: activity_row.try_get("side")?,
            product_name: row.try_get("name")?,
            fuel_type: row.try_get("fuel_type")?,
            region: row
                .try_get::<Option<String>, _>("region")?
                .unwrap_or_default(),
            source_kind: if provenance == "REAL" {
                "LIVE_ORDER"
            } else if provenance == "DEMO" {
                "DEMO_SEED"
            } else {
                "UNKNOWN"
            }
            .into(),
            scope: if order.point.is_some() {
                "DELIVERY_POINT"
            } else {
                "UNKNOWN"
            }
            .into(),
            demo_status: if provenance == "REAL" {
                "REAL_ONLY"
            } else if provenance == "DEMO" {
                "DEMO_ONLY"
            } else {
                "UNKNOWN"
            }
            .into(),
            is_demo_listing: provenance == "DEMO",
            unknown_count: u32::from(provenance == "UNKNOWN"),
            market_product: policy
                .products
                .iter()
                .find(|p| p.id == order.product)
                .map(|p| p.code.clone()),
            delivery_point_id: order.point,
            availability_window: order.window.clone(),
            observed_at: Some(at),
            delivery_point: activity_row.try_get("point_name")?,
        };
        let query = crate::market::visibility::order_query_sql(
            "SELECT id FROM eligible_orders WHERE id=$14",
        );
        let public_before = crate::market::visibility::bind_order_query(
            &query,
            policy,
            &crate::VisibilityClock::at(at),
            config,
            &crate::PublicOrderFilters::default(),
        )?
        .bind(order.id)
        .fetch_optional(&mut *self.conn)
        .await?
        .is_some();
        let event = ParticipantEvent {
            event_type: "order_cancelled".into(),
            aggregate_type: "order".into(),
            aggregate_id: order.id.to_string(),
            participant_org_ids: vec![order.organization],
            fields: ParticipantFields::OrderCancelled(activity),
            public_invalidated: public_before,
        };

        Ok(MutationEffect {
            kind: MutationKind::OrderCancelled,
            actor_id: ctx.actor().user_id(),
            organization_id: order.organization,
            resource_type: "order".into(),
            resource_id: order.id,
            changes,
            event: Some(event),
            at,
            audit_context: request.audit.clone(),
        })
    }
}

async fn owner_read(
    conn: &mut PgConnection,
    organization: Uuid,
    request: &OrderRequest,
    now: DateTime<Utc>,
    policy: &MarketPolicy,
    config: &crate::Config,
) -> Result<Value, MarketWriteError> {
    let rows=sqlx::query("SELECT to_jsonb(o) || jsonb_build_object('quantity_mt',o.quantity_mt::text,'remaining_quantity_mt',o.remaining_quantity_mt::text,'price_per_mt_usd',o.price_per_mt_usd::text,'carbon_intensity_gco2_mj',o.carbon_intensity_gco2_mj::text,'energy_density_mj_kg',o.energy_density_mj_kg::text) AS data,p.name AS product_name,(SELECT value->>'code' FROM jsonb_array_elements($6::jsonb->'products') WHERE value->>'id'=p.id::text) AS market_product,p.fuel_type AS fuel_type,p.fuel_grade AS fuel_grade,d.name AS point_name,d.region AS region,COALESCE(org.supplier_tier::text,'INDEPENDENT') AS tier_label,(SELECT count(*) FROM trades t WHERE (o.side::text='ASK' AND t.ask_order_id=o.id) OR (o.side::text='BID' AND t.bid_order_id=o.id)) AS trade_count FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN organizations org ON org.id=o.organization_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.organization_id=$1 AND (NOT $2 OR (o.side::text='ASK' AND (o.expires_at IS NULL OR o.expires_at>$3))) ORDER BY o.created_at DESC,o.id DESC OFFSET $4 LIMIT $5")
            .bind(organization).bind(request.template).bind(now).bind(if request.template{0}else{request.skip}).bind(if request.template{1}else{request.limit}).bind(serde_json::to_value(policy).map_err(|_|internal())?).fetch_all(&mut *conn).await?;
    if request.template {
        return rows
            .first()
            .map(template)
            .transpose()
            .map(|v| v.unwrap_or(Value::Null));
    }
    let keys:Vec<_>=rows.iter().filter_map(|row|{
            let data:Value=row.try_get("data").ok()?;
            Some(json!({"side":data["side"],"product":row.try_get::<String,_>("market_product").ok()?,"point":data["delivery_point_id"],"window":data["availability_window"]}))
        }).collect();
    let benchmarks = owner_benchmarks(conn, policy, config, &keys, now).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut value = owner_projection(&row)?;
        let key = json!({"side":value["side"],"product":value["market_product"],"point":value["delivery_point_id"],"window":value["availability_window"]});
        if let Some(price) = benchmarks.get(&key.to_string()) {
            let price_text = value["price_per_mt_usd"].as_str().ok_or_else(internal)?;
            let price_value = Decimal::from_str_exact(price_text).map_err(|_| internal())?;
            let premium = price_value.checked_sub(*price).ok_or_else(internal)?;
            value["benchmark_price_per_mt_usd"] = json!(price.to_string());
            value["premium_discount_per_mt_usd"] = json!(premium.to_string());
            value["benchmark_source"] = json!(format!(
                "live_slice_{}_vwap",
                value["side"].as_str().ok_or_else(internal)?.to_lowercase()
            ));
        }
        out.push(value);
    }
    Ok(Value::Array(out))
}
fn template(row: &sqlx::postgres::PgRow) -> Result<Value, MarketWriteError> {
    let data: Value = row.try_get("data")?;
    let mut out = serde_json::Map::new();
    for key in [
        "product_id",
        "delivery_point_id",
        "quantity_mt",
        "price_per_mt_usd",
        "availability_window",
        "certifications",
        "certification_declared",
        "certification_scheme",
        "specification_standard",
        "msds_available",
        "carbon_intensity_gco2_mj",
        "carbon_intensity_method",
        "feedstock",
        "origin",
        "fame_terms",
        "lng_terms",
    ] {
        out.insert(key.into(), data[key].clone());
    }
    out.insert(
        "certifications".into(),
        data["certifications"]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])),
    );
    out.insert("off_spec".into(), json!(false));
    out.insert("off_spec_notes".into(), Value::Null);
    normalize_owner_fields(&mut out)?;
    Ok(Value::Object(out))
}
#[derive(Deserialize, Serialize)]
struct OwnerOrderResponse {
    id: Uuid,
    side: String,
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    port_id: Option<String>,
    quantity_mt: String,
    remaining_quantity_mt: String,
    price_per_mt_usd: String,
    availability_window: String,
    certifications: Vec<String>,
    certification_declared: bool,
    certification_scheme: Option<String>,
    specification_standard: Option<String>,
    msds_available: bool,
    carbon_intensity_gco2_mj: Option<String>,
    carbon_intensity_method: Option<String>,
    feedstock: Option<String>,
    origin: Option<String>,
    off_spec: bool,
    off_spec_notes: Option<String>,
    fame_terms: Value,
    lng_terms: Value,
    is_verdaxis_verified: bool,
    tier_label: String,
    status: String,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    organization_id: Uuid,
    vessel_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
    creation_method: String,
    version: i32,
    product_name: String,
    market_product: Option<String>,
    fuel_type: String,
    fuel_grade: String,
    delivery_point_name: Option<String>,
    region: String,
    terms_digest: String,
    etag: String,
    trade_count: i64,
    benchmark_price_per_mt_usd: Option<String>,
    premium_discount_per_mt_usd: Option<String>,
    benchmark_source: Option<String>,
    is_crossed: bool,
    is_demo_listing: bool,
    source_kind: String,
    scope: String,
    demo_status: String,
    unknown_count: u32,
}
fn owner_projection(row: &sqlx::postgres::PgRow) -> Result<Value, MarketWriteError> {
    let data: Value = row.try_get("data")?;
    let digest = crate::canonical_terms_from_row(&data)
        .and_then(|v| crate::digest_terms(&v))
        .map_err(|_| internal())?;
    let mut out = serde_json::Map::new();
    for key in [
        "id",
        "side",
        "product_id",
        "delivery_point_id",
        "port_id",
        "quantity_mt",
        "remaining_quantity_mt",
        "price_per_mt_usd",
        "availability_window",
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
        "is_verdaxis_verified",
        "tier_label",
        "status",
        "expires_at",
        "created_at",
        "organization_id",
        "vessel_id",
        "updated_at",
        "creation_method",
        "version",
    ] {
        out.insert(key.into(), data[key].clone());
    }
    out.insert(
        "certifications".into(),
        data["certifications"]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])),
    );
    for (target, source) in [
        ("product_name", "product_name"),
        ("market_product", "market_product"),
        ("fuel_type", "fuel_type"),
        ("fuel_grade", "fuel_grade"),
        ("delivery_point_name", "point_name"),
        ("region", "region"),
    ] {
        let value: Option<String> = row.try_get(source)?;
        out.insert(
            target.into(),
            if target == "delivery_point_name" || target == "market_product" {
                json!(value)
            } else {
                json!(value.unwrap_or_default())
            },
        );
    }
    let id = data["id"]
        .as_str()
        .ok_or_else(internal)?
        .parse()
        .map_err(|_| internal())?;
    let version =
        i32::try_from(data["version"].as_i64().ok_or_else(internal)?).map_err(|_| internal())?;
    out.insert("terms_digest".into(), json!(digest));
    out.insert(
        "etag".into(),
        json!(crate::routes::market_write::orders::order_etag(id, version)),
    );
    out.insert(
        "trade_count".into(),
        json!(row.try_get::<i64, _>("trade_count")?),
    );
    for key in [
        "benchmark_price_per_mt_usd",
        "premium_discount_per_mt_usd",
        "benchmark_source",
    ] {
        out.insert(key.into(), Value::Null);
    }
    for (key, value) in [
        ("is_crossed", json!(false)),
        ("is_demo_listing", json!(false)),
        ("source_kind", json!("UNKNOWN")),
        ("scope", json!("UNKNOWN")),
        ("demo_status", json!("UNKNOWN")),
        ("unknown_count", json!(0)),
    ] {
        out.insert(key.into(), value);
    }
    out.insert(
        "tier_label".into(),
        json!(row.try_get::<String, _>("tier_label")?),
    );
    normalize_owner_fields(&mut out)?;
    let response: OwnerOrderResponse =
        serde_json::from_value(Value::Object(out)).map_err(|_| internal())?;
    serde_json::to_value(response).map_err(|_| internal())
}

fn normalize_owner_fields(
    out: &mut serde_json::Map<String, Value>,
) -> Result<(), MarketWriteError> {
    if let Some(window) = out.get_mut("availability_window") {
        *window = json!(
            crate::market::windows::normalize_window(
                window.as_str().ok_or_else(internal)?,
                Utc::now().date_naive()
            )
            .map_err(|_| internal())?
        );
    }
    for (key, profile) in [("fame_terms", "fame_v1"), ("lng_terms", "lng_v1")] {
        if let Some(terms) = out.get_mut(key) {
            *terms = crate::market::terms::owner_response_terms(profile, terms)
                .map_err(|_| internal())?;
        }
    }
    Ok(())
}

async fn owner_benchmarks(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &crate::Config,
    keys: &[Value],
    now: DateTime<Utc>,
) -> Result<std::collections::HashMap<String, Decimal>, MarketWriteError> {
    use crate::market::{
        live_benchmarks::{candidate_is_benchmark_qualified, qualified_vwap},
        visibility::{OrderSurface, benchmark_candidate_sql, bind_order_query},
    };
    if keys.is_empty() {
        return Ok(Default::default());
    }
    let sql = benchmark_candidate_sql(include_str!("../../sql/live_benchmarks.sql"));
    let clock = crate::VisibilityClock::at(now);
    let filters = crate::PublicOrderFilters {
        surface: OrderSurface::Candidates,
        ..Default::default()
    };
    let rows = bind_order_query(&sql, policy, &clock, config, &filters)?
        .bind(json!(keys).to_string())
        .fetch_all(conn)
        .await?;
    let mut groups: std::collections::HashMap<String, Vec<(Decimal, Decimal)>> = Default::default();
    for row in rows {
        let input: Value = row.try_get("private_digest_input")?;
        if !candidate_is_benchmark_qualified(policy, &input, &clock) {
            continue;
        }
        let key=json!({"side":row.try_get::<String,_>("side")?,"product":row.try_get::<String,_>("market_product")?,"point":row.try_get::<Uuid,_>("delivery_point_id")?,"window":row.try_get::<String,_>("availability_window")?}).to_string();
        groups.entry(key).or_default().push((
            row.try_get("remaining_quantity_mt")?,
            row.try_get("price_per_mt_usd")?,
        ));
    }
    Ok(groups
        .into_iter()
        .filter_map(|(key, rows)| qualified_vwap(&rows).map(|price| (key, price)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_version_rule_uses_catalog_qualification() {
        let mut policy: MarketPolicy =
            serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json"))
                .unwrap();
        let product = policy
            .products
            .iter_mut()
            .find(|product| product.qualification_profile == "fame_v1")
            .unwrap();
        product.code = "RENAMED_CATALOG_ENTRY".into();
        let mut order = OrderIdentity {
            id: Uuid::new_v4(),
            organization: Uuid::new_v4(),
            owner: None,
            product: product.id,
            point: None,
            window: "SPOT".into(),
            item: None,
            method: "SELF_SERVICE".into(),
            authorization: None,
            version: 1,
            status: "OPEN".into(),
        };
        assert!(bumps_version(&order, &policy));
        for product in &policy.products {
            order.product = product.id;
            assert_eq!(
                bumps_version(&order, &policy),
                product.qualification_profile == "fame_v1"
            );
        }
        order.product = Uuid::new_v4();
        assert!(!bumps_version(&order, &policy));
        order.method = "MARKET_SUPPORT".into();
        assert!(bumps_version(&order, &policy));
    }
}
