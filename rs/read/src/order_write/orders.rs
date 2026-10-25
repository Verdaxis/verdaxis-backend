//! Order cancellation and organization-private owner projections.
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
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

pub(crate) fn refusal(status: axum::http::StatusCode, message: &'static str) -> MarketWriteError {
    let mut error = MarketWriteError::new(MarketErrorCode::ValidationFailed);
    error.status = status;
    error.message = message;
    error
}
fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn forbidden() -> MarketWriteError {
    refusal(
        axum::http::StatusCode::FORBIDDEN,
        "The order is not owned by this actor",
    )
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
}
struct OrderCommand {
    request: OrderRequest,
    policy: Arc<MarketPolicy>,
    config: Arc<crate::Config>,
    hash: String,
}

pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: OrderRequest,
) -> Result<CommittedCommand, MarketWriteError> {
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
        },
        class,
    )
    .await
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
        .bind(id).fetch_optional(conn).await?.ok_or_else(|| refusal(axum::http::StatusCode::NOT_FOUND,"Order not found"))?;
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
            let Some(id) = self.request.id else {
                let body = tx
                    .order_port()?
                    .owner_read(actor, &self.request, now, &self.policy, &self.config)
                    .await?;
                return Ok(CommittedCommand {
                    response_status: 200,
                    response_body: body,
                    replayed: false,
                });
            };
            let context = tx
                .order_port()?
                .authorize(actor, id, self.request.context, now)
                .await?;
            let order = tx.order_port()?.load(id).await?;
            if order.method == "MARKET_SUPPORT" {
                if self.request.input.reason.is_none() {
                    return Err(refusal(
                        axum::http::StatusCode::BAD_REQUEST,
                        "Assisted cancellation requires a reason",
                    )
                    .into());
                }
                let expected = self.request.input.expected_version(id)?;
                if expected.is_none() {
                    return Err(refusal(
                        axum::http::StatusCode::PRECONDITION_REQUIRED,
                        "If-Match is required",
                    )
                    .into());
                }
                if expected != Some(order.version) {
                    return Err(refusal(
                        axum::http::StatusCode::PRECONDITION_FAILED,
                        "Listing changed; reload and retry",
                    )
                    .into());
                }
            }
            if !matches!(order.status.as_str(), "OPEN" | "PARTIALLY_FILLED") {
                return Err(refusal(
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
            tx.order_port()?.withdraw(id).await?;
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
            false,
        )
        .await
    }
    async fn withdraw(&mut self, id: Uuid) -> Result<(), MarketWriteError> {
        let mut ledger = super::transaction::StockLedger::new(self.conn, self.items, self.orders);
        let order = ledger.order(id).await?;
        ledger.withdraw_order(&order, "CANCELLED").await?;
        sqlx::query("UPDATE orderbook_orders SET updated_at=now() WHERE id=$1")
            .bind(id)
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
        let changes = json!({"status":"CANCELLED","reason":request.input.reason,"actor_user_id":ctx.actor().user_id(),"effective_organization_id":ctx.effective_organization_id(),"original_principal_user_id":order.owner,"cancellation_principal_user_id":ctx.actor().user_id(),"support_context_id":ctx.support_context_id(),"original_support_context_id":original_context,"version":order.version.checked_add(1).ok_or_else(internal)?});
        let provenance: String = row.try_get("provenance")?;
        let activity_row = sqlx::query("SELECT o.side::text AS side,p.market_product::text AS market_product,d.name AS point_name FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$1").bind(order.id).fetch_one(&mut *self.conn).await?;
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
            market_product: activity_row.try_get("market_product")?,
            delivery_point_id: order.point.ok_or_else(internal)?,
            availability_window: order.window.clone(),
            observed_at: None,
            delivery_point: activity_row
                .try_get::<Option<String>, _>("point_name")?
                .unwrap_or_default(),
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
    async fn owner_read(
        &mut self,
        actor: &AuthenticatedActor,
        request: &OrderRequest,
        now: DateTime<Utc>,
        policy: &MarketPolicy,
        config: &crate::Config,
    ) -> Result<Value, MarketWriteError> {
        let organization = if let Some(context) = request.context {
            sqlx::query_scalar("SELECT organization_id FROM market_support_contexts WHERE id=$1 AND actor_user_id=$2").bind(context).bind(actor.user_id()).fetch_optional(&mut *self.conn).await?.ok_or_else(forbidden)?
        } else {
            actor.account().current_organization_id.ok_or_else(|| {
                refusal(
                    axum::http::StatusCode::BAD_REQUEST,
                    "User must belong to an organization",
                )
            })?
        };
        let _context = super::authority::admission::order_context(
            self.conn,
            self.locked,
            actor,
            organization,
            request.context,
            now,
            true,
        )
        .await?;
        let rows=sqlx::query("SELECT to_jsonb(o) || jsonb_build_object('quantity_mt',o.quantity_mt::text,'remaining_quantity_mt',o.remaining_quantity_mt::text,'price_per_mt_usd',o.price_per_mt_usd::text,'carbon_intensity_gco2_mj',o.carbon_intensity_gco2_mj::text,'energy_density_mj_kg',o.energy_density_mj_kg::text) AS data,p.name AS product_name,p.market_product::text AS market_product,p.fuel_type AS fuel_type,p.fuel_grade AS fuel_grade,d.name AS point_name,d.region AS region,(SELECT count(*) FROM trades t WHERE (o.side::text='ASK' AND t.ask_order_id=o.id) OR (o.side::text='BID' AND t.bid_order_id=o.id)) AS trade_count FROM orderbook_orders o JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.organization_id=$1 AND (NOT $2 OR (o.side::text='ASK' AND (o.expires_at IS NULL OR o.expires_at>$3))) ORDER BY o.created_at DESC,o.id DESC OFFSET $4 LIMIT $5")
            .bind(organization).bind(request.template).bind(now).bind(if request.template{0}else{request.skip}).bind(if request.template{1}else{request.limit}).fetch_all(&mut *self.conn).await?;
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
        let benchmarks = owner_benchmarks(self.conn, policy, config, &keys, now).await?;
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
    Ok(Value::Object(out))
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
    let version = data["version"].as_i64().ok_or_else(internal)? as i32;
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
    Ok(Value::Object(out))
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
