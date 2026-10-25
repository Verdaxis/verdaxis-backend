//! Canonical lock identity and advisory keys for the order-book write boundary.
//!
//! Port of the reviewed key derivation in `app/services/market_locks.py` and
//! `app/services/idempotency.py`. Slice identity is side-independent: a BID and
//! an ASK on the same product, delivery point and canonical availability window
//! share one executable slice and therefore one advisory lock. Every lock key is
//! the first eight bytes of a SHA-256 digest, read as a signed big-endian
//! integer, with zero replaced by one.

use std::collections::BTreeSet;

use super::error::{MarketErrorCode, MarketWriteError};
use chrono::NaiveDate;
use sha2::Digest;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::market::windows::normalize_window;
use crate::{ApiError, ApiErrorKind};

/// Transaction-local bounds the boundary sets before its first lock. They match
/// the reviewed Python constants; runtime-wide timeout policy stays a
/// deployment concern.
pub const MARKET_LOCK_TIMEOUT: &str = "500ms";
pub const MARKET_STATEMENT_TIMEOUT: &str = "5s";
pub const IDEMPOTENCY_LOCK_TIMEOUT: &str = "500ms";
pub const IDEMPOTENCY_STATEMENT_TIMEOUT: &str = "5s";

pub const IDEMPOTENCY_KEY_MIN: usize = 1;
pub const IDEMPOTENCY_KEY_MAX: usize = 255;
pub const IDEMPOTENCY_KEY_BOUNDS: &str = "Idempotency-Key must be 1-255 characters";

/// One market slice's side-independent identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceKey {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
}

/// SHA-256 in lowercase hexadecimal. The single digest primitive this module
/// and the request hashes share.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read the first eight digest bytes as a signed big-endian integer, replacing
/// the single unused value zero with one.
fn lock_key(canonical: &str) -> i64 {
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    match i64::from_be_bytes(bytes) {
        0 => 1,
        value => value,
    }
}

fn point_text(delivery_point_id: Option<Uuid>) -> String {
    delivery_point_id
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// Normalize one availability window to the canonical slice label.
///
/// The window grammar is shared with the market reads, so an invalid window is
/// refused with that grammar's own message rather than a second spelling.
pub fn normalize_slice_window(availability_window: &str) -> Result<String, ApiError> {
    normalize_window(availability_window, NaiveDate::MIN).map_err(|detail| ApiError {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        detail: serde_json::json!(detail),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    })
}

/// The canonical order used before any slice advisory lock is taken:
/// product id, then delivery point, then normalized window, each as text.
pub fn market_slice_lock_sort_key(
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    normalized_window: &str,
) -> (String, String, String) {
    (
        product_id.to_string(),
        point_text(delivery_point_id),
        normalized_window.to_owned(),
    )
}

/// One slice advisory key. `normalized_window` must already be canonical: pass
/// [`normalize_slice_window`] output, or use [`ordered_slice_lock_keys`].
pub fn market_slice_lock_key(
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    normalized_window: &str,
) -> i64 {
    lock_key(&format!(
        "{}|{}|{}",
        product_id,
        point_text(delivery_point_id),
        normalized_window
    ))
}

/// The deduplicated, canonically sorted advisory keys for one lock plan.
///
/// The reviewed Python deduplicates on the side-independent identity and sorts
/// by [`market_slice_lock_sort_key`] before acquiring anything, so two requests
/// that touch the same slices always take them in the same order.
pub fn ordered_slice_lock_keys(slices: &[SliceKey]) -> Result<Vec<i64>, ApiError> {
    let mut unique: Vec<(String, String, String, Uuid, Option<Uuid>)> = Vec::new();
    for slice in slices {
        let normalized = normalize_slice_window(&slice.availability_window)?;
        let (product, point, window) =
            market_slice_lock_sort_key(slice.product_id, slice.delivery_point_id, &normalized);
        let duplicate = unique
            .iter()
            .any(|existing| existing.0 == product && existing.1 == point && existing.2 == window);
        if !duplicate {
            unique.push((
                product,
                point,
                window,
                slice.product_id,
                slice.delivery_point_id,
            ));
        }
    }
    unique.sort();
    Ok(unique
        .iter()
        .map(|entry| market_slice_lock_key(entry.3, entry.4, &entry.2))
        .collect())
}

/// One idempotency advisory key: the actor, the operation and the key.
pub fn idempotency_lock_key(lock_scope_id: Uuid, operation: &str, key: &str) -> i64 {
    lock_key(&format!("{lock_scope_id}|{operation}|{key}"))
}

/// Trim and bound-check one request idempotency key.
///
/// Python counts code points, so a multi-byte key is bounded by its characters,
/// not its bytes.
pub fn normalize_idempotency_key(raw: &str) -> Result<String, ApiError> {
    let key = raw.trim();
    let length = key.chars().count();
    if !(IDEMPOTENCY_KEY_MIN..=IDEMPOTENCY_KEY_MAX).contains(&length) {
        return Err(ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            detail: serde_json::json!(IDEMPOTENCY_KEY_BOUNDS),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        });
    }
    Ok(key.to_owned())
}


/// L-numbers name lock classes, not their acquisition position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockClass {
    Organization, User, Idempotency, Slice, Capability, Context, Authorization,
    Trade, Order, Inventory, Pin, Commission, FreezeRow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketOperation {
    Create, Amend, Cancel, InventoryCreate, InventoryEdit, Publish, Take,
    Confirm, Decline, Deliver, Pay, CapabilityGrant, CapabilityRevoke,
    ContextStart, ContextExit, Commission, Freeze,
}

impl MarketOperation {
    pub fn graph(self) -> Vec<LockClass> {
        use LockClass::*;
        match self {
            Self::Create | Self::Cancel => vec![Organization,User,Idempotency,Slice,Capability,Context,Authorization,Order,Inventory,Pin],
            Self::Amend | Self::Publish | Self::Take => vec![Organization,User,Idempotency,Slice,Order,Inventory,Pin],
            Self::InventoryCreate => vec![Organization,User,Idempotency],
            Self::InventoryEdit => vec![Organization,User,Inventory],
            Self::Confirm | Self::Decline => vec![Organization,User,Idempotency,Slice,Trade,Order,Inventory,Pin],
            Self::Deliver | Self::Pay => vec![Organization,User,Idempotency,Trade],
            Self::CapabilityGrant => vec![User,Capability],
            Self::CapabilityRevoke => vec![User,Capability,Context],
            Self::ContextStart | Self::ContextExit => vec![Organization,User,Capability,Context],
            Self::Commission => vec![User,Commission],
            Self::Freeze => vec![User,Trade,FreezeRow],
        }
    }
}

/// Read-only discovery result. Equality includes membership and resource evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct PartyLockPreview {
    organizations: Vec<Uuid>,
    users: Vec<Uuid>,
    fingerprint: serde_json::Value,
}
fn sorted_ids(ids: impl IntoIterator<Item=Uuid>) -> Vec<Uuid> {
    ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}
impl PartyLockPreview {
    pub fn new(organizations: impl IntoIterator<Item=Uuid>, users: impl IntoIterator<Item=Uuid>, actor: Uuid, fingerprint: serde_json::Value) -> Self {
        Self { organizations: sorted_ids(organizations), users: sorted_ids(users.into_iter().chain([actor])), fingerprint }
    }
    pub fn organizations(&self) -> &[Uuid] { &self.organizations }
    pub fn users(&self) -> &[Uuid] { &self.users }
    pub fn matches(&self, current: &Self) -> bool { self == current }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LockStep { class: LockClass, identity: String }

/// A plan cannot move backwards or acquire a new party after a resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationLockPlan {
    operation: MarketOperation,
    steps: Vec<LockStep>,
    parties_declared: bool,
    orders: Vec<Uuid>,
    inventory: Vec<Uuid>,
}
impl OperationLockPlan {
    pub fn new(operation: MarketOperation) -> Self {
        Self { operation, steps: Vec::new(), parties_declared: false, orders: Vec::new(), inventory: Vec::new() }
    }
    fn append(&mut self, class: LockClass, identities: impl IntoIterator<Item=String>) -> Result<&mut Self, MarketWriteError> {
        let graph=self.operation.graph();
        let position=graph.iter().position(|item| *item==class).ok_or_else(invalid_plan)?;
        if self.steps.last().is_some_and(|last| graph.iter().position(|item| *item==last.class).unwrap() > position) { return Err(invalid_plan()); }
        for identity in identities { self.steps.push(LockStep {class,identity}); }
        Ok(self)
    }
    pub fn parties(&mut self, preview: &PartyLockPreview) -> Result<&mut Self,MarketWriteError> {
        if self.parties_declared || !self.steps.is_empty() { return Err(invalid_plan()); }
        if self.operation.graph().contains(&LockClass::Organization) {
            self.append(LockClass::Organization,preview.organizations.iter().map(ToString::to_string))?;
        } else if !preview.organizations.is_empty() { return Err(invalid_plan()); }
        self.append(LockClass::User,preview.users.iter().map(ToString::to_string))?;
        self.parties_declared=true;
        Ok(self)
    }
    pub fn users(&mut self, ids: impl IntoIterator<Item=Uuid>) -> Result<&mut Self,MarketWriteError> {
        if self.parties_declared { return Err(invalid_plan()); }
        self.append(LockClass::User,sorted_ids(ids).iter().map(ToString::to_string))
    }
    pub fn idempotency(&mut self, scope: Uuid, operation: &str, key: &str) -> Result<&mut Self,MarketWriteError> {
        if !self.parties_declared { return Err(invalid_plan()); }
        let key=normalize_idempotency_key(key).map_err(|_|invalid_plan())?;
        self.append(LockClass::Idempotency,[idempotency_lock_key(scope,operation,&key).to_string()])
    }
    pub fn slices(&mut self, slices: impl IntoIterator<Item=SliceKey>) -> Result<&mut Self,MarketWriteError> {
        if !self.parties_declared { return Err(invalid_plan()); }
        let keys=ordered_slice_lock_keys(&slices.into_iter().collect::<Vec<_>>()).map_err(|_|invalid_plan())?;
        self.append(LockClass::Slice,keys.iter().map(ToString::to_string))
    }
    pub fn trade(&mut self,id: Uuid) -> Result<&mut Self,MarketWriteError> { self.resources(LockClass::Trade,[id]) }
    pub fn orders(&mut self,ids: impl IntoIterator<Item=Uuid>) -> Result<&mut Self,MarketWriteError> {
        self.orders=sorted_ids(ids); self.resources(LockClass::Order,self.orders.clone())
    }
    pub fn inventory(&mut self,ids: impl IntoIterator<Item=Uuid>) -> Result<&mut Self,MarketWriteError> {
        self.inventory=sorted_ids(ids); self.resources(LockClass::Inventory,self.inventory.clone())
    }
    pub fn resources(&mut self,class: LockClass,ids: impl IntoIterator<Item=Uuid>) -> Result<&mut Self,MarketWriteError> {
        if !self.parties_declared || matches!(class,LockClass::Organization|LockClass::User|LockClass::Slice|LockClass::Idempotency) { return Err(invalid_plan()); }
        if self.steps.iter().any(|step| step.class==class) { return Err(invalid_plan()); }
        self.append(class,sorted_ids(ids).iter().map(ToString::to_string))
    }
    pub fn order_ids(&self)->&[Uuid] { &self.orders }
    pub fn inventory_ids(&self)->&[Uuid] { &self.inventory }
    pub fn trace(&self)->Vec<(LockClass,String)> { self.steps.iter().map(|step|(step.class,step.identity.clone())).collect() }
    pub async fn acquire(&self,conn: &mut PgConnection)->Result<Locked,MarketWriteError> {
        let mut locked=Locked {steps:Vec::new(),statements:Vec::new()};
        for statement in ["SET LOCAL lock_timeout = '500ms'","SET LOCAL statement_timeout = '5s'"] {
            sqlx::raw_sql(statement).execute(&mut *conn).await.map_err(MarketWriteError::from)?;
            locked.statements.push(statement);
        }
        self.acquire_steps(conn,&mut locked).await?;
        Ok(locked)
    }
    pub(super) async fn acquire_steps(&self,conn:&mut PgConnection,locked:&mut Locked)->Result<(),MarketWriteError> {
        if !self.parties_declared { return Err(invalid_plan()); }
        for step in &self.steps {
            let result = match step.class {
                LockClass::Idempotency|LockClass::Slice => sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(step.identity.parse::<i64>().map_err(|_|invalid_plan())?).fetch_optional(&mut *conn).await,
                class => {
                    let sql=match class {
                        LockClass::Organization=>"SELECT id FROM organizations WHERE id=$1 FOR UPDATE",
                        LockClass::User=>"SELECT id FROM users WHERE id=$1 FOR UPDATE",
                        LockClass::Trade=>"SELECT id FROM trades WHERE id=$1 FOR UPDATE",
                        LockClass::Order=>"SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE",
                        LockClass::Inventory=>"SELECT id FROM inventory_items WHERE id=$1 FOR UPDATE",
                        LockClass::Capability=>"SELECT id FROM staff_capability_assignments WHERE id=$1 FOR UPDATE",
                        LockClass::Context=>"SELECT id FROM market_support_contexts WHERE id=$1 FOR UPDATE",
                        LockClass::Authorization=>"SELECT id FROM market_support_authorizations WHERE id=$1 FOR UPDATE",
                        LockClass::Pin=>"SELECT id FROM pinned_orders WHERE id=$1 FOR UPDATE",
                        LockClass::Commission=>"SELECT id FROM commissions WHERE id=$1 FOR UPDATE",
                        LockClass::FreezeRow=>"SELECT id FROM trade_freezes WHERE id=$1 FOR UPDATE",
                        _=>unreachable!(),
                    };
                    sqlx::query(sql).bind(Uuid::parse_str(&step.identity).map_err(|_|invalid_plan())?).fetch_optional(&mut *conn).await
                }
            };
            let row=result.map_err(|error|MarketWriteError::from_lock(error,step.class==LockClass::Idempotency))?;
            if row.is_none() { return Err(invalid_plan()); }
            locked.steps.push(step.clone());
        }
        Ok(())
    }
}
fn invalid_plan()->MarketWriteError { MarketWriteError::new(MarketErrorCode::MarketSliceChanged) }

/// Evidence of successful acquisition, never caller-constructed.
#[derive(Debug)]
pub struct Locked { steps: Vec<LockStep>, statements: Vec<&'static str> }
impl Locked {
    pub fn statement_trace(&self)->&[&'static str] { &self.statements }
    pub(super) fn contains(&self,class:LockClass,id:Uuid)->bool { self.steps.iter().any(|step|step.class==class && step.identity==id.to_string()) }
}

#[cfg(test)]
pub fn lock_failure_for_test(state:&str,class:LockClass)->MarketWriteError {
    MarketWriteError::lock_code_for_test(state,class==LockClass::Idempotency)
}
