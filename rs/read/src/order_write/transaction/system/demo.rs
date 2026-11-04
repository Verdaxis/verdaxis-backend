//! The demo activity writer (Phase 10 Task 17; design revision 3,
//! `outputs/research/rust-phase10/task17-system-writer-design.md`).
//!
//! Ports `app/services/demo_activity.py` and `scripts/run_demo_activity.py`.
//! The job (`cli::jobs::demo`) runs the phases in order, each one transaction,
//! and stops at the first failure: provisioning (in the job module: it writes
//! organizations, not the order book), then here coverage (one transaction per
//! product), legacy expiry, prune and tick. Every phase that changed something
//! publishes one `demo_market_changed` event, as Python does.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::super::{BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketPreview};
use super::{SystemMarketCommand, SystemWriter, refused, run_system_market_command, sealed};
use crate::cli::jobs::demo::model::{
    CoverageOrder, DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID,
    DEMO_COVERAGE_OPERATION, DEMO_SEED_ORGANIZATIONS,
};
use crate::order_write::MarketWriteError;
use crate::order_write::locks::{
    LockClass, MarketOperation, OperationLockPlan, PartyLockPreview, SliceKey,
};

/// The two demo activity organizations: tick, prune and legacy expiry write
/// only their rows.
pub(crate) const ACTIVITY_ORGANIZATIONS: [Uuid; 2] =
    [DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID];

/// Append Python's `demo_market_changed` event for one phase, in the
/// caller's transaction: participants the activity pair, no producer
/// evidence, the versioned payload with the public invalidation marker; the
/// dispatchers are woken on commit.
pub(crate) async fn append_demo_market_changed(
    conn: &mut PgConnection,
    operation: &str,
) -> Result<(), sqlx::Error> {
    let mut participants = ACTIVITY_ORGANIZATIONS.to_vec();
    participants.sort_unstable();
    let payload = serde_json::json!({
        "operation": operation,
        "schema_version": 1,
        "_public_market_invalidation": true,
    });
    sqlx::query(
        "INSERT INTO market_event_outbox \
           (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload, \
            producer_evidence, created_at) \
         VALUES ($1, 'demo_market_changed', 'demo_market', $2, $3::json, $4::json, NULL, now())",
    )
    .bind(Uuid::new_v4())
    .bind(operation)
    .bind(
        serde_json::to_value(&participants)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?,
    )
    .bind(payload)
    .execute(&mut *conn)
    .await?;
    super::super::wake_market_event_dispatchers(conn).await
}

// ---- coverage ----------------------------------------------------------------------

/// The most slices one coverage transaction may lock: one product at eight
/// ports over at most 24 windows is 192 (the side is not part of a slice key).
pub(crate) const MAX_COVERAGE_SLICES: usize = 256;
/// The most rows one coverage transaction may write: 192 slices, two sides,
/// ten levels each is 3,840.
pub(crate) const MAX_COVERAGE_ROWS: usize = 5_000;

const OPEN: &str = "OPEN";
const PARTIALLY_FILLED: &str = "PARTIALLY_FILLED";

/// The fields a refresh compares and rewrites (Python's
/// `DEMO_COVERAGE_REFRESH_FIELDS`), in their stored form. Decimal equality
/// ignores scale, as Python's does.
#[derive(Debug, Clone, PartialEq)]
struct Managed {
    creation_method: String,
    quantity_mt: Decimal,
    remaining_quantity_mt: Decimal,
    price_per_mt_usd: Decimal,
    certifications: Option<Value>,
    certification_declared: bool,
    certification_scheme: Option<String>,
    specification_standard: Option<String>,
    msds_available: bool,
    is_verdaxis_verified: bool,
    carbon_intensity_gco2_mj: Option<Decimal>,
    carbon_intensity_method: Option<String>,
    energy_density_mj_kg: Option<Decimal>,
    feedstock: Option<String>,
    origin: Option<String>,
    off_spec: bool,
    status: String,
    expires_at: Option<DateTime<Utc>>,
    idempotency_request_hash: Option<String>,
}

impl Managed {
    /// The stored form of one wanted coverage order.
    fn wanted(order: &CoverageOrder) -> Self {
        Self {
            creation_method: "SYSTEM".into(),
            quantity_mt: order.quantity_mt,
            remaining_quantity_mt: order.quantity_mt,
            price_per_mt_usd: order.price_per_mt_usd,
            certifications: Some(json!(order.certifications)),
            certification_declared: order.certification_declared,
            certification_scheme: Some(order.certification_scheme.into()),
            specification_standard: order.specification_standard.map(Into::into),
            msds_available: order.msds_available,
            is_verdaxis_verified: order.is_verdaxis_verified,
            carbon_intensity_gco2_mj: order.carbon_intensity_gco2_mj,
            carbon_intensity_method: order.carbon_intensity_method.map(Into::into),
            energy_density_mj_kg: order.energy_density_mj_kg,
            feedstock: order.feedstock.map(Into::into),
            origin: order.origin.clone(),
            off_spec: false,
            status: OPEN.into(),
            expires_at: Some(order.expires_at),
            idempotency_request_hash: Some(order.request_hash.clone()),
        }
    }

    fn live(&self) -> bool {
        self.status == OPEN || self.status == PARTIALLY_FILLED
    }
}

/// Python's `_demo_refresh_loses_priority`: a refresh that makes the order
/// more attractive or reopens it takes a new place at the back of the queue.
fn loses_priority(current: &Managed, target: &Managed) -> bool {
    current.price_per_mt_usd != target.price_per_mt_usd
        || target.quantity_mt > current.quantity_mt
        || current.certifications != target.certifications
        || current.certification_scheme != target.certification_scheme
        || (current.off_spec && !target.off_spec)
        || match (current.expires_at, target.expires_at) {
            (Some(_), None) => true,
            (Some(current), Some(target)) => target > current,
            (None, _) => false,
        }
        || (!current.live() && target.live())
}

/// One stored coverage row of the product.
struct Stored {
    id: Uuid,
    key: String,
    organization_id: Uuid,
    slice: SliceKey,
    fields: Managed,
}

/// One change, in the order Python makes them (the wanted rows' order, so
/// new acceptance ordinals follow the same sequence).
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Create {
        index: usize,
        id: Uuid,
    },
    Refresh {
        index: usize,
        id: Uuid,
        loses_priority: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
struct CoveragePlan {
    changes: Vec<Change>,
    /// Live rows whose key the run no longer wants: they expire.
    stale: Vec<Uuid>,
    organizations: BTreeSet<Uuid>,
    slices: Vec<SliceKey>,
}

impl CoveragePlan {
    /// What the drift check compares: the changes and the stale rows, by
    /// identity. A new row's id is drawn per preview, so a create is named by
    /// its key.
    fn fingerprint(&self, wanted: &[CoverageOrder]) -> Value {
        let changes: Vec<Value> = self
            .changes
            .iter()
            .map(|change| match change {
                Change::Create { index, .. } => json!(["create", wanted[*index].key]),
                Change::Refresh {
                    index,
                    id,
                    loses_priority,
                } => {
                    json!(["refresh", wanted[*index].key, id, loses_priority])
                }
            })
            .collect();
        json!({"changes": changes, "stale": self.stale})
    }
}

/// Compare the wanted rows with the stored rows. A wanted key with no stored
/// row is created; a stored row whose managed fields differ is refreshed; a
/// live stored row whose key is no longer wanted expires. When two stored
/// rows share a key, the last one read (by id) is the one compared, as in
/// Python's dictionary.
fn plan(wanted: &[CoverageOrder], stored: &[Stored]) -> CoveragePlan {
    let by_key: BTreeMap<&str, &Stored> =
        stored.iter().map(|row| (row.key.as_str(), row)).collect();
    let wanted_keys: BTreeSet<&str> = wanted.iter().map(|order| order.key.as_str()).collect();
    let mut plan = CoveragePlan::default();
    let mut slices = Vec::new();
    for (index, order) in wanted.iter().enumerate() {
        let target = Managed::wanted(order);
        let slice = SliceKey {
            product_id: order.product_id,
            delivery_point_id: Some(order.delivery_point_id),
            availability_window: order.window.clone(),
        };
        match by_key.get(order.key.as_str()) {
            None => {
                plan.changes.push(Change::Create {
                    index,
                    id: Uuid::new_v4(),
                });
                plan.organizations.insert(order.organization_id);
                slices.push(slice);
            }
            Some(current) if current.fields != target => {
                plan.changes.push(Change::Refresh {
                    index,
                    id: current.id,
                    loses_priority: loses_priority(&current.fields, &target),
                });
                plan.organizations.insert(current.organization_id);
                slices.push(slice);
            }
            Some(_) => {}
        }
    }
    for row in stored {
        if !wanted_keys.contains(row.key.as_str()) && row.fields.live() {
            plan.stale.push(row.id);
            plan.organizations.insert(row.organization_id);
            slices.push(row.slice.clone());
        }
    }
    plan.slices = slices;
    plan
}

/// Read the stored coverage rows of one product: every row whose key the
/// run wants, and every live one (Python's selection, limited to the
/// product).
async fn stored_rows(
    conn: &mut PgConnection,
    product_id: Uuid,
    keys: &[String],
) -> Result<Vec<Stored>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, idempotency_key, organization_id, delivery_point_id, availability_window, \
                creation_method::text AS creation_method, quantity_mt, remaining_quantity_mt, \
                price_per_mt_usd, certifications::jsonb AS certifications, certification_declared, \
                certification_scheme, specification_standard, msds_available, \
                is_verdaxis_verified, carbon_intensity_gco2_mj, carbon_intensity_method, \
                energy_density_mj_kg, feedstock, origin, off_spec, status::text AS status, \
                expires_at, idempotency_request_hash \
         FROM orderbook_orders \
         WHERE product_id = $1 AND provenance = 'DEMO' AND idempotency_operation = $2 \
           AND (idempotency_key = ANY($3) OR status IN ('OPEN', 'PARTIALLY_FILLED')) \
         ORDER BY id",
    )
    .bind(product_id)
    .bind(DEMO_COVERAGE_OPERATION)
    .bind(keys)
    .fetch_all(conn)
    .await?;
    rows.iter()
        .map(|row| {
            use sqlx::Row;
            Ok(Stored {
                id: row.try_get("id")?,
                key: row.try_get("idempotency_key")?,
                organization_id: row.try_get("organization_id")?,
                slice: SliceKey {
                    product_id,
                    delivery_point_id: row.try_get("delivery_point_id")?,
                    availability_window: row.try_get("availability_window")?,
                },
                fields: Managed {
                    creation_method: row.try_get("creation_method")?,
                    quantity_mt: row.try_get("quantity_mt")?,
                    remaining_quantity_mt: row.try_get("remaining_quantity_mt")?,
                    price_per_mt_usd: row.try_get("price_per_mt_usd")?,
                    certifications: row.try_get("certifications")?,
                    certification_declared: row.try_get("certification_declared")?,
                    certification_scheme: row.try_get("certification_scheme")?,
                    specification_standard: row.try_get("specification_standard")?,
                    msds_available: row.try_get("msds_available")?,
                    is_verdaxis_verified: row.try_get("is_verdaxis_verified")?,
                    carbon_intensity_gco2_mj: row.try_get("carbon_intensity_gco2_mj")?,
                    carbon_intensity_method: row.try_get("carbon_intensity_method")?,
                    energy_density_mj_kg: row.try_get("energy_density_mj_kg")?,
                    feedstock: row.try_get("feedstock")?,
                    origin: row.try_get("origin")?,
                    off_spec: row.try_get("off_spec")?,
                    status: row.try_get("status")?,
                    expires_at: row.try_get("expires_at")?,
                    idempotency_request_hash: row.try_get("idempotency_request_hash")?,
                },
            })
        })
        .collect()
}

/// What one coverage transaction did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverageCounts {
    pub created: usize,
    pub refreshed: usize,
    pub expired: usize,
}

impl CoverageCounts {
    pub fn changed(&self) -> bool {
        self.created + self.refreshed + self.expired > 0
    }
}

/// The coverage of one product. `wanted` holds exactly this product's rows
/// from one `build_coverage` of the run.
pub(crate) struct DemoCoverage {
    product_id: Uuid,
    wanted: Vec<CoverageOrder>,
    now: DateTime<Utc>,
    /// The plan of the latest preview. The attempt core previews twice; the
    /// second preview runs under the locks, and `execute` writes exactly it.
    planned: Mutex<CoveragePlan>,
}

impl sealed::Sealed for DemoCoverage {}

impl DemoCoverage {
    pub(crate) fn new(
        product_id: Uuid,
        wanted: Vec<CoverageOrder>,
        now: DateTime<Utc>,
    ) -> Result<Self, CoverageTooLarge> {
        if wanted.len() > MAX_COVERAGE_ROWS
            || wanted.iter().any(|order| order.product_id != product_id)
        {
            return Err(CoverageTooLarge);
        }
        Ok(Self {
            product_id,
            wanted,
            now,
            planned: Mutex::new(CoveragePlan::default()),
        })
    }
}

/// A product's coverage exceeds the named bounds of one transaction, or
/// mixes products. Nothing is locked or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoverageTooLarge;

impl SystemMarketCommand for DemoCoverage {
    type Output = CoverageCounts;

    fn allowed_organizations(&self) -> &[Uuid] {
        &DEMO_SEED_ORGANIZATIONS
    }

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let keys: Vec<String> = self.wanted.iter().map(|order| order.key.clone()).collect();
            let stored = stored_rows(conn, self.product_id, &keys).await?;
            let plan = plan(&self.wanted, &stored);
            let parties = PartyLockPreview::system(
                plan.organizations.iter().copied(),
                plan.fingerprint(&self.wanted),
            );
            let mut locks = OperationLockPlan::new(MarketOperation::DemoSystem);
            locks
                .parties(&parties)?
                .slices(plan.slices.iter().cloned())?;
            let slice_locks = locks
                .trace()
                .iter()
                .filter(|(class, _)| *class == LockClass::Slice)
                .count();
            if slice_locks > MAX_COVERAGE_SLICES {
                return Err(refused());
            }
            *self.planned.lock().map_err(|_| refused())? = plan;
            Ok(MarketPreview { parties, locks })
        })
    }

    fn execute<'a, 'b: 'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'b>,
        writer: &'a SystemWriter,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let plan = self.planned.lock().map_err(|_| refused())?.clone();
            let counts = write_coverage(tx.conn, &self.wanted, &plan, writer, self.now).await?;
            if counts.changed() {
                append_demo_market_changed(tx.conn, "coverage").await?;
            }
            Ok(counts)
        })
    }
}

/// The stored columns of one wanted row, for `jsonb_populate_recordset`.
fn row_json(order: &CoverageOrder, id: Uuid, ordinal: Option<i64>, now: DateTime<Utc>) -> Value {
    let fields = Managed::wanted(order);
    json!({
        "id": id,
        "organization_id": order.organization_id,
        "creation_method": fields.creation_method,
        "provenance": "DEMO",
        "side": order.side.as_str(),
        "product_id": order.product_id,
        "delivery_point_id": order.delivery_point_id,
        "quantity_mt": fields.quantity_mt.to_string(),
        "remaining_quantity_mt": fields.remaining_quantity_mt.to_string(),
        "price_per_mt_usd": fields.price_per_mt_usd.to_string(),
        "availability_window": order.window,
        "certifications": fields.certifications,
        "certification_declared": fields.certification_declared,
        "certification_scheme": fields.certification_scheme,
        "specification_standard": fields.specification_standard,
        "msds_available": fields.msds_available,
        "is_verdaxis_verified": fields.is_verdaxis_verified,
        "carbon_intensity_gco2_mj": fields.carbon_intensity_gco2_mj.map(|value| value.to_string()),
        "carbon_intensity_method": fields.carbon_intensity_method,
        "energy_density_mj_kg": fields.energy_density_mj_kg.map(|value| value.to_string()),
        "feedstock": fields.feedstock,
        "origin": fields.origin,
        "off_spec": fields.off_spec,
        "status": fields.status,
        "expires_at": fields.expires_at,
        "idempotency_operation": DEMO_COVERAGE_OPERATION,
        "idempotency_key": order.key,
        "idempotency_request_hash": fields.idempotency_request_hash,
        "acceptance_ordinal": ordinal,
        "created_at": order.created_at,
        "updated_at": now,
    })
}

/// The ids a statement wrote must be exactly the ids it was planned to
/// write; anything else refuses the transaction before commit.
fn written_exactly(written: Vec<Uuid>, planned: &[Uuid]) -> Result<(), MarketAttemptError> {
    let written: BTreeSet<Uuid> = written.into_iter().collect();
    let planned: BTreeSet<Uuid> = planned.iter().copied().collect();
    if written == planned {
        Ok(())
    } else {
        Err(refused())
    }
}

async fn write_coverage(
    conn: &mut PgConnection,
    wanted: &[CoverageOrder],
    plan: &CoveragePlan,
    writer: &SystemWriter,
    now: DateTime<Utc>,
) -> Result<CoverageCounts, MarketAttemptError> {
    if plan.changes.is_empty() && plan.stale.is_empty() {
        return Ok(CoverageCounts::default());
    }
    // New acceptance ordinals, in the order Python draws them.
    let needed = plan
        .changes
        .iter()
        .filter(|change| {
            matches!(
                change,
                Change::Create { .. }
                    | Change::Refresh {
                        loses_priority: true,
                        ..
                    }
            )
        })
        .count();
    let mut ordinals: Vec<i64> = sqlx::query_scalar(
        "SELECT nextval('orderbook_acceptance_ordinal_seq') FROM generate_series(1, $1)",
    )
    .bind(needed as i64)
    .fetch_all(&mut *conn)
    .await?;
    ordinals.sort_unstable();
    let mut ordinals = ordinals.into_iter();

    let (mut created, mut created_ids) = (Vec::new(), Vec::new());
    let (mut refreshed, mut refreshed_ids) = (Vec::new(), Vec::new());
    for change in &plan.changes {
        match change {
            Change::Create { index, id } => {
                let ordinal = ordinals.next().ok_or_else(refused)?;
                created.push(row_json(&wanted[*index], *id, Some(ordinal), now));
                created_ids.push(*id);
            }
            Change::Refresh {
                index,
                id,
                loses_priority,
            } => {
                let ordinal = if *loses_priority {
                    Some(ordinals.next().ok_or_else(refused)?)
                } else {
                    None
                };
                refreshed.push(row_json(&wanted[*index], *id, ordinal, now));
                refreshed_ids.push(*id);
            }
        }
    }
    let organizations = writer.organizations();

    if !created.is_empty() {
        let written: Vec<Uuid> = sqlx::query_scalar(
            "INSERT INTO orderbook_orders \
               (id, organization_id, creation_method, provenance, side, product_id, \
                delivery_point_id, quantity_mt, remaining_quantity_mt, price_per_mt_usd, \
                availability_window, certifications, certification_declared, \
                certification_scheme, specification_standard, msds_available, \
                is_verdaxis_verified, carbon_intensity_gco2_mj, carbon_intensity_method, \
                energy_density_mj_kg, feedstock, origin, off_spec, status, expires_at, \
                idempotency_operation, idempotency_key, idempotency_request_hash, \
                acceptance_ordinal, created_at, updated_at) \
             SELECT id, organization_id, creation_method, provenance, side, product_id, \
                delivery_point_id, quantity_mt, remaining_quantity_mt, price_per_mt_usd, \
                availability_window, certifications, certification_declared, \
                certification_scheme, specification_standard, msds_available, \
                is_verdaxis_verified, carbon_intensity_gco2_mj, carbon_intensity_method, \
                energy_density_mj_kg, feedstock, origin, off_spec, status, expires_at, \
                idempotency_operation, idempotency_key, idempotency_request_hash, \
                acceptance_ordinal, created_at, updated_at \
             FROM jsonb_populate_recordset(NULL::orderbook_orders, $1::jsonb) \
             WHERE organization_id = ANY($2) \
             RETURNING id",
        )
        .bind(Value::Array(created))
        .bind(organizations)
        .fetch_all(&mut *conn)
        .await?;
        written_exactly(written, &created_ids)?;
    }

    if !refreshed.is_empty() {
        let written: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE orderbook_orders AS o SET \
               creation_method = r.creation_method, quantity_mt = r.quantity_mt, \
               remaining_quantity_mt = r.remaining_quantity_mt, \
               price_per_mt_usd = r.price_per_mt_usd, certifications = r.certifications, \
               certification_declared = r.certification_declared, \
               certification_scheme = r.certification_scheme, \
               specification_standard = r.specification_standard, \
               msds_available = r.msds_available, is_verdaxis_verified = r.is_verdaxis_verified, \
               carbon_intensity_gco2_mj = r.carbon_intensity_gco2_mj, \
               carbon_intensity_method = r.carbon_intensity_method, \
               energy_density_mj_kg = r.energy_density_mj_kg, feedstock = r.feedstock, \
               origin = r.origin, off_spec = r.off_spec, status = r.status, \
               expires_at = r.expires_at, idempotency_request_hash = r.idempotency_request_hash, \
               acceptance_ordinal = COALESCE(r.acceptance_ordinal, o.acceptance_ordinal), \
               updated_at = r.updated_at \
             FROM jsonb_populate_recordset(NULL::orderbook_orders, $1::jsonb) AS r \
             WHERE o.id = r.id AND o.organization_id = ANY($2) AND o.provenance = 'DEMO' \
               AND o.owner_user_id IS NULL AND o.idempotency_operation = $3 \
             RETURNING o.id",
        )
        .bind(Value::Array(refreshed))
        .bind(organizations)
        .bind(DEMO_COVERAGE_OPERATION)
        .fetch_all(&mut *conn)
        .await?;
        written_exactly(written, &refreshed_ids)?;
    }

    if !plan.stale.is_empty() {
        let written: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE orderbook_orders SET status = 'EXPIRED', updated_at = $2 \
             WHERE id = ANY($1) AND organization_id = ANY($3) AND provenance = 'DEMO' \
               AND owner_user_id IS NULL AND idempotency_operation = $4 \
               AND status IN ('OPEN', 'PARTIALLY_FILLED') \
             RETURNING id",
        )
        .bind(&plan.stale)
        .bind(now)
        .bind(organizations)
        .bind(DEMO_COVERAGE_OPERATION)
        .fetch_all(&mut *conn)
        .await?;
        written_exactly(written, &plan.stale)?;
    }

    Ok(CoverageCounts {
        created: created_ids.len(),
        refreshed: refreshed_ids.len(),
        expired: plan.stale.len(),
    })
}

// ---- legacy expiry and prune -------------------------------------------------------
/// Python's `MAX_GENERATED_TRADES`: the soft cap on activity trades.
pub(crate) const MAX_GENERATED_TRADES: i64 = 80;
/// Python's `MAX_GENERATED_ORDERS`: three orders per kept trade.
pub(crate) const MAX_GENERATED_ORDERS: i64 = MAX_GENERATED_TRADES * 3;
/// Python's `RETENTION_DAYS`: unreferenced activity older than this goes.
pub(crate) const RETENTION_DAYS: i64 = 7;

/// An activity order nothing refers to (Python's
/// `_unreferenced_demo_order_filter`), as SQL on the alias `o`; `$1` is the
/// admitted activity organizations.
///
/// Unlike Python, a live (open or partly filled) order is never pruned:
/// prune holds no slice lock, so it must not take an order out of a book a
/// user command may be matching against. Activity orders fill at once and
/// legacy quotes expire in the phase before, so no live one is expected.
macro_rules! unreferenced_order {
    () => {
        "o.organization_id = ANY($1) AND o.provenance = 'DEMO' \
     AND o.owner_user_id IS NULL AND o.status NOT IN ('OPEN', 'PARTIALLY_FILLED') \
     AND NOT EXISTS (SELECT 1 FROM trades t WHERE t.bid_order_id = o.id OR t.ask_order_id = o.id) \
     AND NOT EXISTS (SELECT 1 FROM watchlist_targets w WHERE w.order_id = o.id) \
     AND NOT EXISTS (SELECT 1 FROM match_suggestions m \
                     WHERE m.bid_order_id = o.id OR m.ask_order_id = o.id) \
     AND NOT EXISTS (SELECT 1 FROM negotiations n \
                     WHERE n.bid_order_id = o.id OR n.ask_order_id = o.id)"
    };
}

/// An activity trade nothing refers to (Python's
/// `_unreferenced_demo_trade_filter`), as SQL on the alias `t`.
macro_rules! unreferenced_trade {
    () => {
        "t.buyer_id = ANY($1) AND t.seller_id = ANY($1) \
     AND t.buyer_provenance = 'DEMO' AND t.seller_provenance = 'DEMO' \
     AND NOT EXISTS (SELECT 1 FROM commissions c WHERE c.trade_id = t.id) \
     AND NOT EXISTS (SELECT 1 FROM rfqs r WHERE r.trade_id = t.id) \
     AND NOT EXISTS (SELECT 1 FROM negotiations n WHERE n.trade_id = t.id)"
    };
}

/// Delete the given activity trades that are still unreferenced, then the
/// orders they linked that are now unreferenced (Python's
/// `_delete_trades_and_linked_orders`). Returns (trades, orders) deleted.
async fn delete_trades_and_linked_orders(
    conn: &mut PgConnection,
    organizations: &[Uuid],
    trades: &[Uuid],
) -> Result<(usize, usize), MarketAttemptError> {
    if trades.is_empty() {
        return Ok((0, 0));
    }
    let deleted: Vec<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(concat!(
        "DELETE FROM trades AS t WHERE t.id = ANY($2) AND ",
        unreferenced_trade!(),
        " RETURNING t.id, t.bid_order_id, t.ask_order_id"
    ))
    .bind(organizations)
    .bind(trades)
    .fetch_all(&mut *conn)
    .await?;
    written_exactly(deleted.iter().map(|row| row.0).collect(), trades)?;
    let linked: Vec<Uuid> = deleted
        .iter()
        .flat_map(|(_, bid, ask)| [*bid, *ask])
        .flatten()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let orders = delete_unreferenced_orders(conn, organizations, &linked).await?;
    Ok((deleted.len(), orders))
}

/// Delete those of `candidates` that are unreferenced activity orders. A
/// linked order another record still refers to stays, as in Python, so the
/// check is only that nothing outside the candidates went.
async fn delete_unreferenced_orders(
    conn: &mut PgConnection,
    organizations: &[Uuid],
    candidates: &[Uuid],
) -> Result<usize, MarketAttemptError> {
    if candidates.is_empty() {
        return Ok(0);
    }
    let deleted: Vec<Uuid> = sqlx::query_scalar(concat!(
        "DELETE FROM orderbook_orders AS o WHERE o.id = ANY($2) AND ",
        unreferenced_order!(),
        " RETURNING o.id"
    ))
    .bind(organizations)
    .bind(candidates)
    .fetch_all(&mut *conn)
    .await?;
    if deleted.iter().any(|id| !candidates.contains(id)) {
        return Err(refused());
    }
    Ok(deleted.len())
}

/// What one prune transaction removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneCounts {
    pub trades: usize,
    pub orders: usize,
}

/// Python's `prune_demo_activity`: seven-day retention, then the soft caps,
/// for the activity organizations' unreferenced trades and orders.
async fn prune(
    conn: &mut PgConnection,
    organizations: &[Uuid],
    now: DateTime<Utc>,
) -> Result<PruneCounts, MarketAttemptError> {
    let cutoff = now - chrono::Duration::days(RETENTION_DAYS);
    let mut counts = PruneCounts::default();

    let old_trades: Vec<Uuid> = sqlx::query_scalar(concat!(
        "SELECT t.id FROM trades AS t WHERE ",
        unreferenced_trade!(),
        " AND t.created_at < $2 ORDER BY t.id"
    ))
    .bind(organizations)
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;
    let (trades, orders) =
        delete_trades_and_linked_orders(conn, organizations, &old_trades).await?;
    counts.trades += trades;
    counts.orders += orders;

    let old_orders: Vec<Uuid> = sqlx::query_scalar(concat!(
        "SELECT o.id FROM orderbook_orders AS o WHERE ",
        unreferenced_order!(),
        " AND o.created_at < $2 ORDER BY o.id"
    ))
    .bind(organizations)
    .bind(cutoff)
    .fetch_all(&mut *conn)
    .await?;
    let deleted = delete_unreferenced_orders(conn, organizations, &old_orders).await?;
    if deleted != old_orders.len() {
        return Err(refused());
    }
    counts.orders += deleted;

    // The soft caps: referenced records stay even above the cap.
    let total_trades: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM trades WHERE buyer_id = ANY($1) AND seller_id = ANY($1)",
    )
    .bind(organizations)
    .fetch_one(&mut *conn)
    .await?;
    if total_trades > MAX_GENERATED_TRADES {
        let oldest: Vec<Uuid> = sqlx::query_scalar(concat!(
            "SELECT t.id FROM trades AS t WHERE ",
            unreferenced_trade!(),
            " ORDER BY t.created_at, t.id LIMIT $2"
        ))
        .bind(organizations)
        .bind(total_trades - MAX_GENERATED_TRADES)
        .fetch_all(&mut *conn)
        .await?;
        let (trades, orders) =
            delete_trades_and_linked_orders(conn, organizations, &oldest).await?;
        counts.trades += trades;
        counts.orders += orders;
    }

    let total_orders: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE organization_id = ANY($1)")
            .bind(organizations)
            .fetch_one(&mut *conn)
            .await?;
    if total_orders > MAX_GENERATED_ORDERS {
        let oldest: Vec<Uuid> = sqlx::query_scalar(concat!(
            "SELECT o.id FROM orderbook_orders AS o WHERE ",
            unreferenced_order!(),
            " ORDER BY o.created_at, o.id LIMIT $2"
        ))
        .bind(organizations)
        .bind(total_orders - MAX_GENERATED_ORDERS)
        .fetch_all(&mut *conn)
        .await?;
        counts.orders += delete_unreferenced_orders(conn, organizations, &oldest).await?;
    }
    Ok(counts)
}

/// The prune phase: the two activity organizations, no slices (it removes
/// only unreferenced history, never a live book order).
pub(crate) struct DemoPrune {
    now: DateTime<Utc>,
}

impl sealed::Sealed for DemoPrune {}

impl DemoPrune {
    pub(crate) fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }
}

impl SystemMarketCommand for DemoPrune {
    type Output = PruneCounts;

    fn allowed_organizations(&self) -> &[Uuid] {
        &ACTIVITY_ORGANIZATIONS
    }

    fn preview<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move { activity_preview(Value::Null, &[]) })
    }

    fn execute<'a, 'b: 'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'b>,
        writer: &'a SystemWriter,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let counts = prune(tx.conn, writer.organizations(), self.now).await?;
            if counts.trades + counts.orders > 0 {
                append_demo_market_changed(tx.conn, "prune").await?;
            }
            Ok(counts)
        })
    }
}

/// The lock plan of an activity phase: both activity organizations, then
/// the given slices.
fn activity_preview(
    fingerprint: Value,
    slices: &[SliceKey],
) -> Result<MarketPreview, MarketAttemptError> {
    let parties = PartyLockPreview::system(ACTIVITY_ORGANIZATIONS, fingerprint);
    let mut locks = OperationLockPlan::new(MarketOperation::DemoSystem);
    locks.parties(&parties)?.slices(slices.iter().cloned())?;
    Ok(MarketPreview { parties, locks })
}

/// Python's `_expire_legacy_activity_quotes`: old flat-price activity quotes
/// (no creation method, no idempotency, untouched and unreferenced) expire.
/// Records and pins stay.
macro_rules! legacy_quote {
    () => {
        "o.organization_id = ANY($1) AND o.provenance = 'DEMO' \
     AND o.creation_method = 'LEGACY_UNKNOWN' AND o.owner_user_id IS NULL \
     AND o.created_by_actor_user_id IS NULL AND o.support_authorization_id IS NULL \
     AND o.idempotency_operation IS NULL AND o.idempotency_key IS NULL \
     AND o.idempotency_request_hash IS NULL AND o.status = 'OPEN' \
     AND o.remaining_quantity_mt = o.quantity_mt \
     AND NOT EXISTS (SELECT 1 FROM trades t WHERE t.bid_order_id = o.id OR t.ask_order_id = o.id) \
     AND NOT EXISTS (SELECT 1 FROM negotiations n \
                     WHERE n.bid_order_id = o.id OR n.ask_order_id = o.id) \
     AND NOT EXISTS (SELECT 1 FROM match_suggestions m \
                     WHERE m.bid_order_id = o.id OR m.ask_order_id = o.id)"
    };
}

/// The legacy expiry phase: the activity organizations and the slices of
/// the quotes it expires.
pub(crate) struct DemoLegacyExpiry {
    now: DateTime<Utc>,
    planned: Mutex<Vec<Uuid>>,
}

impl sealed::Sealed for DemoLegacyExpiry {}

impl DemoLegacyExpiry {
    pub(crate) fn new(now: DateTime<Utc>) -> Self {
        Self {
            now,
            planned: Mutex::new(Vec::new()),
        }
    }
}

impl SystemMarketCommand for DemoLegacyExpiry {
    type Output = usize;

    fn allowed_organizations(&self) -> &[Uuid] {
        &ACTIVITY_ORGANIZATIONS
    }

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let rows: Vec<(Uuid, Uuid, Option<Uuid>, String)> = sqlx::query_as(concat!(
                "SELECT o.id, o.product_id, o.delivery_point_id, o.availability_window \
                 FROM orderbook_orders AS o WHERE ",
                legacy_quote!(),
                " ORDER BY o.id"
            ))
            .bind(&ACTIVITY_ORGANIZATIONS[..])
            .fetch_all(&mut *conn)
            .await?;
            let ids: Vec<Uuid> = rows.iter().map(|row| row.0).collect();
            let slices: Vec<SliceKey> = rows
                .into_iter()
                .map(
                    |(_, product_id, delivery_point_id, availability_window)| SliceKey {
                        product_id,
                        delivery_point_id,
                        availability_window,
                    },
                )
                .collect();
            let preview = activity_preview(json!(ids), &slices)?;
            *self.planned.lock().map_err(|_| refused())? = ids;
            Ok(preview)
        })
    }

    fn execute<'a, 'b: 'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'b>,
        writer: &'a SystemWriter,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let planned = self.planned.lock().map_err(|_| refused())?.clone();
            if planned.is_empty() {
                return Ok(0);
            }
            let expired: Vec<Uuid> = sqlx::query_scalar(concat!(
                "UPDATE orderbook_orders AS o SET status = 'EXPIRED', updated_at = $3 \
                 WHERE o.id = ANY($2) AND ",
                legacy_quote!(),
                " RETURNING o.id"
            ))
            .bind(writer.organizations())
            .bind(&planned)
            .bind(self.now)
            .fetch_all(&mut *tx.conn)
            .await?;
            written_exactly(expired, &planned)?;
            append_demo_market_changed(tx.conn, "coverage").await?;
            Ok(planned.len())
        })
    }
}

// ---- the market phases of one run ---------------------------------------------------

/// What the market phases of one demo run did, by count.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DemoMarketReport {
    /// Products whose coverage transaction committed.
    pub coverage_products: usize,
    pub coverage: CoverageCounts,
    pub legacy_expired: usize,
    pub prune: PruneCounts,
}

/// The market phase that stopped a run. Every earlier transaction
/// committed; this one rolled back and no later one ran.
#[derive(Debug)]
pub enum DemoMarketFailure {
    /// Reading which products hold live coverage failed.
    CoverageScope,
    /// One product's wanted rows exceed one transaction's bound.
    CoverageTooLarge,
    Coverage(MarketWriteError),
    LegacyExpiry(MarketWriteError),
    Prune(MarketWriteError),
}

impl DemoMarketFailure {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::CoverageScope | Self::CoverageTooLarge | Self::Coverage(_) => "coverage",
            Self::LegacyExpiry(_) => "legacy_expiry",
            Self::Prune(_) => "prune",
        }
    }
}

/// The run's wanted coverage, one entry per product in the book's order,
/// then every other product that still holds live coverage (its rows all
/// expire).
async fn coverage_by_product(
    pool: &PgPool,
    wanted: Vec<CoverageOrder>,
) -> Result<Vec<(Uuid, Vec<CoverageOrder>)>, DemoMarketFailure> {
    let mut order: Vec<Uuid> = Vec::new();
    let mut grouped: BTreeMap<Uuid, Vec<CoverageOrder>> = BTreeMap::new();
    for row in wanted {
        if !grouped.contains_key(&row.product_id) {
            order.push(row.product_id);
        }
        grouped.entry(row.product_id).or_default().push(row);
    }
    let live: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT product_id FROM orderbook_orders \
         WHERE provenance = 'DEMO' AND idempotency_operation = $1 \
           AND status IN ('OPEN', 'PARTIALLY_FILLED') \
         ORDER BY product_id",
    )
    .bind(DEMO_COVERAGE_OPERATION)
    .fetch_all(pool)
    .await
    .map_err(|_| DemoMarketFailure::CoverageScope)?;
    for product in live {
        if let std::collections::btree_map::Entry::Vacant(entry) = grouped.entry(product) {
            order.push(product);
            entry.insert(Vec::new());
        }
    }
    Ok(order
        .into_iter()
        .map(|product| {
            let rows = grouped.remove(&product).unwrap_or_default();
            (product, rows)
        })
        .collect())
}

/// The market phases of one run, in Python's order: coverage (one
/// transaction per product), legacy expiry, prune. Each transaction commits
/// on its own and the first failure stops the run; a later run converges.
/// `report` holds what committed before a failure.
pub async fn run_demo_market_phases(
    pool: &PgPool,
    now: DateTime<Utc>,
    wanted: Vec<CoverageOrder>,
    report: &mut DemoMarketReport,
) -> Result<(), DemoMarketFailure> {
    // One phase runs at a time; the permit only bounds this job's own use.
    let permits = Arc::new(Semaphore::new(1));
    for (product, rows) in coverage_by_product(pool, wanted).await? {
        let command = DemoCoverage::new(product, rows, now)
            .map_err(|_| DemoMarketFailure::CoverageTooLarge)?;
        let counts = run_system_market_command(pool.clone(), permits.clone(), command)
            .await
            .map_err(DemoMarketFailure::Coverage)?;
        report.coverage_products += 1;
        report.coverage.created += counts.created;
        report.coverage.refreshed += counts.refreshed;
        report.coverage.expired += counts.expired;
    }
    report.legacy_expired =
        run_system_market_command(pool.clone(), permits.clone(), DemoLegacyExpiry::new(now))
            .await
            .map_err(DemoMarketFailure::LegacyExpiry)?;
    report.prune = run_system_market_command(pool.clone(), permits, DemoPrune::new(now))
        .await
        .map_err(DemoMarketFailure::Prune)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::jobs::demo::model::{
        Catalog, DEMO_SEED_BUYERS, DEMO_SEED_SUPPLIERS, build_coverage,
    };
    use chrono::TimeZone;

    /// A catalog that names each product and port by a digest of its name.
    struct Names;
    fn named(kind: &str, name: &str) -> Uuid {
        let digest = crate::cli::jobs::demo::model::sha256_hex(&format!("{kind}|{name}"));
        Uuid::parse_str(&digest[..32]).unwrap()
    }
    impl Catalog for Names {
        fn product_id(&self, name: &str) -> Uuid {
            named("product", name)
        }
        fn delivery_point_id(&self, name: &str) -> Uuid {
            named("point", name)
        }
    }

    fn wanted(count: usize) -> Vec<CoverageOrder> {
        let now = Utc.with_ymd_and_hms(2026, 11, 4, 4, 0, 0).unwrap();
        build_coverage(now, &Names, &DEMO_SEED_BUYERS, &DEMO_SEED_SUPPLIERS)
            .into_iter()
            .take(count)
            .collect()
    }

    fn stored(order: &CoverageOrder) -> Stored {
        Stored {
            id: Uuid::new_v4(),
            key: order.key.clone(),
            organization_id: order.organization_id,
            slice: SliceKey {
                product_id: order.product_id,
                delivery_point_id: Some(order.delivery_point_id),
                availability_window: order.window.clone(),
            },
            fields: Managed::wanted(order),
        }
    }

    #[test]
    fn a_refresh_that_improves_or_reopens_the_order_loses_priority() {
        let order = &wanted(1)[0];
        let target = Managed::wanted(order);
        let unchanged = target.clone();
        assert!(!loses_priority(&unchanged, &target));

        let mut smaller = target.clone();
        smaller.quantity_mt += Decimal::ONE;
        assert!(
            !loses_priority(&smaller, &target),
            "a quantity cut keeps its place"
        );
        let mut larger = target.clone();
        larger.quantity_mt -= Decimal::ONE;
        assert!(loses_priority(&larger, &target), "more quantity re-queues");

        let mut priced = target.clone();
        priced.price_per_mt_usd += Decimal::ONE;
        assert!(loses_priority(&priced, &target));

        let mut rescaled = target.clone();
        rescaled.price_per_mt_usd = rescaled.price_per_mt_usd.round_dp(2);
        rescaled.price_per_mt_usd.rescale(6);
        assert!(
            !loses_priority(&rescaled, &target),
            "equal values at another scale"
        );
        assert_eq!(rescaled, target, "the stored scale is not a change");

        let mut off_spec = target.clone();
        off_spec.off_spec = true;
        assert!(loses_priority(&off_spec, &target));

        let mut earlier = target.clone();
        earlier.expires_at = target.expires_at.map(|at| at - chrono::Duration::hours(1));
        assert!(
            loses_priority(&earlier, &target),
            "a later expiry re-queues"
        );
        let mut later = target.clone();
        later.expires_at = target.expires_at.map(|at| at + chrono::Duration::hours(1));
        assert!(
            !loses_priority(&later, &target),
            "an earlier expiry keeps its place"
        );

        let mut filled = target.clone();
        filled.status = "FILLED".into();
        assert!(loses_priority(&filled, &target), "reopening re-queues");
        let mut partial = target.clone();
        partial.status = PARTIALLY_FILLED.into();
        assert!(
            !loses_priority(&partial, &target),
            "a live order stays live"
        );
    }

    #[test]
    fn the_plan_creates_refreshes_and_expires_in_the_wanted_order() {
        let rows = wanted(4);
        let mut kept = stored(&rows[0]);
        kept.fields.price_per_mt_usd.rescale(4);
        let mut changed = stored(&rows[2]);
        changed.fields.status = "FILLED".into();
        let mut gone = stored(&rows[3]);
        gone.key = "retired|key".into();
        let mut closed = stored(&rows[3]);
        closed.key = "closed|key".into();
        closed.fields.status = "EXPIRED".into();
        let stored_rows = vec![kept, changed, gone, closed];

        let plan = plan(&rows[..3], &stored_rows);
        assert_eq!(plan.changes.len(), 2, "{plan:?}");
        assert!(matches!(plan.changes[0], Change::Create { index: 1, .. }));
        assert_eq!(
            plan.changes[1],
            Change::Refresh {
                index: 2,
                id: stored_rows[1].id,
                loses_priority: true
            }
        );
        assert_eq!(
            plan.stale,
            vec![stored_rows[2].id],
            "only a live unwanted row expires"
        );
        assert_eq!(plan.slices.len(), 3, "one slice per change and per expiry");
        let fingerprint = plan.fingerprint(&rows);
        let again = super::plan(&rows[..3], &stored_rows);
        assert_eq!(
            again.fingerprint(&rows),
            fingerprint,
            "a new row is named by its key"
        );
    }

    #[test]
    fn an_unchanged_book_plans_nothing() {
        let rows = wanted(20);
        let stored_rows: Vec<Stored> = rows.iter().map(stored).collect();
        let plan = plan(&rows, &stored_rows);
        assert_eq!(plan, CoveragePlan::default());
    }
}
