//! Supplier inventory records share the ordered transaction and participant effects.
//!
//! Writes run in the ordered market transaction, which locks the organization,
//! the user and then the named record. The two reads take no market lock: like
//! the Python `get_current_user` dependency, they lock only the actor's user row.
//! Every request checks its input only after the actor is authenticated and,
//! for a write, admitted, so refusals keep the Python order: account,
//! admission, input, then the supplier role.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use std::collections::BTreeSet;
use uuid::Uuid;

use super::{
    AuthenticatedActor, MarketErrorCode, MarketWriteError, Price, StockQuantity,
    authority::ActorRole,
    effects::{AuditContext, MutationEffect, MutationKind},
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview},
    outbox::{ParticipantEvent, ParticipantFields},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{
    ApiError, AppState, MarketPolicy,
    auth::{
        AuthError, AuthenticatedError, AuthenticatedRequest, CurrentActor, VerifiedAccess,
        request::RequestError, run_authenticated,
    },
    market::terms,
};

const FIELDS: &[&str] = &[
    "port_id",
    "fuel_type",
    "product_name",
    "fame_terms",
    "lng_terms",
    "current_stock_mt",
    "incoming_stock_mt",
    "price_per_mt_usd",
    "energy_density_mj_kg",
    "is_certified",
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
];
const PATCH_FIELDS: &[&str] = &[
    "product_name",
    "fame_terms",
    "lng_terms",
    "current_stock_mt",
    "incoming_stock_mt",
    "price_per_mt_usd",
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
];
fn invalid() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
fn not_available() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ExecutionNotAvailable)
}
fn not_found() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::InventoryNotFound)
}

#[derive(Clone, Debug)]
pub struct InventoryInput(Map<String, Value>);
impl InventoryInput {
    pub fn create(value: Value, policy: &MarketPolicy) -> Result<Self, MarketWriteError> {
        let mut fields = object(value, FIELDS)?;
        for (key, default) in [
            ("incoming_stock_mt", json!("0")),
            ("is_certified", json!(false)),
            ("certification_declared", json!(false)),
            ("msds_available", json!(false)),
            ("off_spec", json!(false)),
        ] {
            fields.entry(key).or_insert(default);
        }
        for key in FIELDS {
            fields.entry(*key).or_insert(Value::Null);
        }
        validate_fields(&mut fields)?;
        let product = fields["product_name"]
            .as_str()
            .and_then(|name| policy.products.iter().find(|p| p.name == name));
        let profile = product
            .map(|p| p.qualification_profile.as_str())
            .unwrap_or("scalar");
        let fuel = fields["fuel_type"].as_str().ok_or_else(invalid)?;
        if ![
            "FAME", "Methanol", "Ethanol", "Biofuel", "LNG", "Ammonia", "LSMGO",
        ]
        .contains(&fuel)
        {
            return Err(invalid());
        }
        for (term, expected) in [("fame_terms", "fame_v1"), ("lng_terms", "lng_v1")] {
            if profile != expected && !fields[term].is_null() {
                return Err(invalid());
            }
        }
        if fuel == "FAME" && profile != "fame_v1" {
            return Err(invalid());
        }
        if matches!(profile, "fame_v1" | "lng_v1") {
            if product.is_none_or(|p| p.fuel_type != fuel) || fields["is_certified"] == true {
                return Err(invalid());
            }
            let term = if profile == "fame_v1" {
                "fame_terms"
            } else {
                "lng_terms"
            };
            let declaration = &fields[term];
            if profile == "fame_v1" {
                terms::validate_fame_terms(
                    "ASK",
                    Some(declaration),
                    Utc::now()
                        .with_timezone(&chrono_tz::Asia::Singapore)
                        .date_naive(),
                )
                .map_err(|_| invalid())?;
            } else {
                terms::validate_lng_terms("ASK", Some(declaration)).map_err(|_| invalid())?;
            }
            let mut declaration = declaration.clone();
            complete_declaration(&mut declaration, profile)?;
            derive_metadata(&mut fields, &declaration, profile)?;
            fields.insert(term.into(), declaration);
        }
        Ok(Self(fields))
    }
    pub fn patch(value: Value) -> Result<Self, MarketWriteError> {
        let mut fields = object(value, PATCH_FIELDS)?;
        validate_fields(&mut fields)?;
        Ok(Self(fields))
    }
}
fn object(value: Value, allowed: &[&str]) -> Result<Map<String, Value>, MarketWriteError> {
    let Value::Object(fields) = value else {
        return Err(invalid());
    };
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid());
    }
    Ok(fields)
}
fn validate_fields(fields: &mut Map<String, Value>) -> Result<(), MarketWriteError> {
    for (key, value) in fields.iter_mut() {
        match key.as_str() {
            "current_stock_mt" | "incoming_stock_mt" => {
                let text = decimal_text(value)?;
                *value = json!(StockQuantity::parse_exact(&text)?.value().to_string());
            }
            "price_per_mt_usd" if !value.is_null() => {
                let text = decimal_text(value)?;
                *value = json!(Price::parse_exact(&text)?.value().to_string());
            }
            "energy_density_mj_kg" | "carbon_intensity_gco2_mj" if !value.is_null() => {
                let text = decimal_text(value)?;
                let amount = StockQuantity::parse_exact(&text)?.value();
                let maximum = if key == "energy_density_mj_kg" {
                    Decimal::new(99999, 2)
                } else {
                    Decimal::from(10000)
                };
                if amount > maximum {
                    return Err(invalid());
                }
                *value = json!(amount.to_string());
            }
            "is_certified" | "certification_declared" | "msds_available" | "off_spec" => {
                if !value.is_boolean() {
                    return Err(invalid());
                }
            }
            "fame_terms" | "lng_terms" => {
                if !value.is_null() && !value.is_object() {
                    return Err(invalid());
                }
            }
            _ => {
                if !value.is_null() && !value.is_string() {
                    return Err(invalid());
                }
            }
        }
    }
    if fields
        .get("port_id")
        .is_some_and(|v| v.as_str().is_none_or(str::is_empty))
    {
        return Err(invalid());
    }
    Ok(())
}
fn decimal_text(value: &Value) -> Result<String, MarketWriteError> {
    match value {
        Value::String(v) => Ok(v.clone()),
        Value::Number(v) => Ok(v.to_string()),
        _ => Err(invalid()),
    }
}
fn complete_declaration(declaration: &mut Value, profile: &str) -> Result<(), MarketWriteError> {
    let fields = declaration.as_object_mut().ok_or_else(invalid)?;
    let optional: &[&str] = if profile == "fame_v1" {
        for (key, value) in [
            ("schema_version", json!(1)),
            ("neat_fame", json!(true)),
            ("nomination_status", json!("PENDING")),
            ("document_references", json!([])),
        ] {
            fields.entry(key).or_insert(value);
        }
        &[
            "astm_grade",
            "en_climate_class",
            "production_origin",
            "feedstock_origin",
            "shipping_location",
            "cfpp_c",
            "cloud_point_c",
            "ci_gco2e_mj",
            "ci_methodology",
            "ci_boundary",
            "ci_basis",
            "lhv_mj_kg",
            "certificate_scope",
            "quality_evidence",
            "sustainability_evidence",
            "batch_reference",
            "producing_site",
        ]
    } else {
        &["feedstock_detail", "min_methane_number"]
    };
    for key in optional {
        fields.entry(*key).or_insert(Value::Null);
    }
    let decimals: &[&str] = if profile == "fame_v1" {
        &["cfpp_c", "cloud_point_c", "ci_gco2e_mj", "lhv_mj_kg"]
    } else {
        &["bio_share_pct", "wtt_ghg_gco2e_mj", "min_methane_number"]
    };
    for key in decimals {
        if !fields[*key].is_null() {
            fields.insert((*key).into(), numeric_string(&fields[*key])?);
        }
    }
    Ok(())
}
fn derive_metadata(
    fields: &mut Map<String, Value>,
    declaration: &Value,
    profile: &str,
) -> Result<(), MarketWriteError> {
    if profile == "fame_v1" {
        for (output, input) in [
            ("certification_scheme", "sustainability_scheme"),
            ("carbon_intensity_gco2_mj", "ci_gco2e_mj"),
            ("carbon_intensity_method", "ci_methodology"),
            ("energy_density_mj_kg", "lhv_mj_kg"),
            ("origin", "production_origin"),
        ] {
            fields.insert(output.into(), declaration[input].clone());
        }
        fields.insert(
            "specification_standard".into(),
            json!(format!(
                "{} {}",
                declaration["standard"].as_str().ok_or_else(invalid)?,
                declaration["standard_edition"]
                    .as_str()
                    .ok_or_else(invalid)?
            )),
        );
        fields.insert(
            "feedstock".into(),
            json!("Used cooking oil (100% of feedstock inputs)"),
        );
    } else {
        let scheme = match declaration["sustainability_scheme"].as_str() {
            Some("ISCC_EU") => "ISCC EU",
            Some("REDCERT_EU") => "REDcert-EU",
            _ => return Err(invalid()),
        };
        let feedstock = match declaration["feedstock"].as_str() {
            Some("MANURE") => "Manure",
            Some("FOOD_WASTE") => "Food waste",
            Some("SEWAGE_SLUDGE") => "Sewage sludge",
            Some("LANDFILL_GAS") => "Landfill gas",
            Some("AGRICULTURAL_RESIDUES") => "Agricultural residues",
            Some("OTHER_WASTES_AND_RESIDUES") => "Other wastes and residues",
            Some("ENERGY_CROPS") => "Energy crops",
            _ => return Err(invalid()),
        };
        for (key, value) in [
            ("certification_declared", json!(true)),
            ("certification_scheme", json!(scheme)),
            ("feedstock", json!(feedstock)),
            (
                "specification_standard",
                declaration["specification_standard"].clone(),
            ),
            ("origin", declaration["production_country"].clone()),
            ("carbon_intensity_gco2_mj", Value::Null),
            ("carbon_intensity_method", Value::Null),
        ] {
            fields.insert(key.into(), value);
        }
    }
    Ok(())
}

/// One inventory request as its route received it.
///
/// Query strings, record identifiers and bodies are not checked here; the
/// component checks them after authentication and, for a write, admission.
#[derive(Clone, Debug)]
pub enum InventoryAction {
    /// `GET /api/inventory` with its raw query string.
    List { query: Option<String> },
    /// `GET /api/listings/my` with its raw query string.
    Listings { query: Option<String> },
    /// `POST /api/inventory` with its JSON body.
    Create { body: Value },
    /// `PATCH /api/inventory/{item_id}` with its JSON body.
    Patch { item_id: String, body: Value },
    /// `DELETE /api/inventory/{item_id}`.
    Delete { item_id: String },
}
impl InventoryAction {
    /// Whether the request names one existing record.
    fn names_record(&self) -> bool {
        matches!(self, Self::Patch { .. } | Self::Delete { .. })
    }
    /// The named record, when its identifier is a UUID.
    fn record_id(&self) -> Option<Uuid> {
        match self {
            Self::Patch { item_id, .. } | Self::Delete { item_id } => Uuid::parse_str(item_id).ok(),
            _ => None,
        }
    }
}

/// A write whose input passed validation.
enum InventoryWrite {
    Create(InventoryInput),
    Patch { id: Uuid, input: InventoryInput },
    Delete(Uuid),
}
impl InventoryWrite {
    fn validate(action: &InventoryAction, policy: &MarketPolicy) -> Result<Self, MarketWriteError> {
        let record = || action.record_id().ok_or_else(invalid);
        match action {
            InventoryAction::Create { body } => {
                Ok(Self::Create(InventoryInput::create(body.clone(), policy)?))
            }
            InventoryAction::Patch { body, .. } => Ok(Self::Patch {
                id: record()?,
                input: InventoryInput::patch(body.clone())?,
            }),
            InventoryAction::Delete { .. } => Ok(Self::Delete(record()?)),
            // Reads run in their own transaction and never reach a write.
            InventoryAction::List { .. } | InventoryAction::Listings { .. } => Err(invalid()),
        }
    }
}

/// Why an inventory request was refused.
#[derive(Debug)]
pub enum InventoryFailure {
    /// The read transaction's account check, as `get_current_user` applies it.
    Authentication(AuthError),
    /// A market refusal, including the write transaction's account check.
    Refusal(MarketWriteError),
    /// The read transaction could not complete.
    Database(ApiError),
}
impl IntoResponse for InventoryFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Authentication(error) => RequestError::Authentication(error).into_response(),
            Self::Refusal(error) => error.into_response(),
            Self::Database(error) => error.into_response(),
        }
    }
}

pub async fn execute_inventory(
    app: AppState,
    access: VerifiedAccess,
    action: InventoryAction,
) -> Result<Value, InventoryFailure> {
    match action {
        InventoryAction::List { query } => {
            read_inventory(app, access, InventoryRead::Items, query).await
        }
        InventoryAction::Listings { query } => {
            read_inventory(app, access, InventoryRead::Listings, query).await
        }
        write => write_inventory(app, access, write)
            .await
            .map_err(InventoryFailure::Refusal),
    }
}

#[derive(Clone, Copy, Debug)]
enum InventoryRead {
    Items,
    Listings,
}
impl InventoryRead {
    fn route(self) -> &'static str {
        match self {
            Self::Items => "/api/inventory",
            Self::Listings => "/api/listings/my",
        }
    }
    async fn run(
        self,
        conn: &mut PgConnection,
        actor: &CurrentActor,
        query: Option<&str>,
        policy: &MarketPolicy,
    ) -> Result<Value, MarketWriteError> {
        let (skip, limit) = page(query)?;
        if actor.role.as_deref() != Some("SUPPLIER") {
            return Err(not_available());
        }
        // A supplier without an organization owns no records.
        let Some(org) = actor.organization_id else {
            return Ok(json!([]));
        };
        match self {
            Self::Items => inventory_items(conn, org, skip, limit).await,
            Self::Listings => compatibility_listings(conn, org, skip, limit, policy).await,
        }
    }
}

/// Run one read in the authenticated read transaction.
///
/// Neither route is in the Python shared-lock list, so the user row is locked
/// `FOR UPDATE` whether or not a support-context header is present; no
/// organization or market lock is taken.
async fn read_inventory(
    app: AppState,
    access: VerifiedAccess,
    read: InventoryRead,
    query: Option<String>,
) -> Result<Value, InventoryFailure> {
    let request = AuthenticatedRequest::new("GET", read.route(), false);
    let policy = app.policy.clone();
    let outcome = run_authenticated(&app, access, request, move |tx| {
        Box::pin(async move {
            let actor = tx.actor().clone();
            match read
                .run(tx.connection(), &actor, query.as_deref(), &policy)
                .await
            {
                // A database failure rolls the read transaction back.
                Err(error) if error.code == MarketErrorCode::DatabaseError => {
                    Err(ApiError::database())
                }
                outcome => Ok(outcome),
            }
        })
    })
    .await;
    match outcome {
        Ok(result) => result.map_err(InventoryFailure::Refusal),
        Err(AuthenticatedError::Authentication(error)) => {
            Err(InventoryFailure::Authentication(error))
        }
        Err(AuthenticatedError::Database(error)) => Err(InventoryFailure::Database(error)),
        Err(AuthenticatedError::AmbiguousCommit) => {
            Err(InventoryFailure::Database(ApiError::database()))
        }
    }
}

/// The Python query rules: `skip` at least 0 (default 0) and `limit` from 1 to
/// 100 (default 50). Only the last value of a repeated parameter applies.
fn page(query: Option<&str>) -> Result<(i64, i64), MarketWriteError> {
    let (mut skip, mut limit) = (None, None);
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match &*key {
            "skip" => skip = Some(value),
            "limit" => limit = Some(value),
            _ => {}
        }
    }
    let skip = skip.map_or(Ok(0), |value| value.parse::<i64>());
    let limit = limit.map_or(Ok(50), |value| value.parse::<i64>());
    match (skip, limit) {
        (Ok(skip), Ok(limit)) if skip >= 0 && (1..=100).contains(&limit) => Ok((skip, limit)),
        _ => Err(invalid()),
    }
}

/// Run one write in the ordered market transaction.
///
/// The lock plan names the record that the preview saw. If another request
/// deletes that record while this one waits for its lock, the attempt ends as
/// preview drift, and the next attempt within the shared attempt bound plans
/// from the current rows. The caller then gets the same 404 as for a record
/// that never existed.
async fn write_inventory(
    app: AppState,
    access: VerifiedAccess,
    action: InventoryAction,
) -> Result<Value, MarketWriteError> {
    let pool = app.pools.read.clone();
    let permits = app.reads.clone();
    run_market_command(
        pool,
        permits,
        access,
        InventoryCommand {
            app,
            action,
            now: Utc::now(),
        },
        super::AdmissionClass::Execution,
    )
    .await
}

struct InventoryCommand {
    app: AppState,
    action: InventoryAction,
    now: DateTime<Utc>,
}
impl MarketCommand for InventoryCommand {
    type Output = Value;
    fn replans_missing_lock_rows(&self) -> bool {
        self.action.names_record()
    }
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let org: Option<Uuid> =
                sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                    .bind(actor)
                    .fetch_optional(&mut *conn)
                    .await?
                    .flatten();
            let record = self.action.record_id();
            let exists = if let (Some(id), Some(org)) = (record, org) {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM inventory_items WHERE id=$1 AND supplier_id=$2)",
                )
                .bind(id)
                .bind(org)
                .fetch_one(&mut *conn)
                .await?
            } else {
                false
            };
            let parties = PartyLockPreview::new(
                org,
                [],
                actor,
                json!({"organization_id":org,"owned_item_exists":exists}),
            );
            let mut locks = OperationLockPlan::new(if self.action.names_record() {
                MarketOperation::InventoryEdit
            } else {
                MarketOperation::InventoryCreate
            });
            locks.parties(&parties)?;
            if let Some(id) = record.filter(|_| exists) {
                locks.inventory([id])?;
            }
            Ok(MarketPreview { parties, locks })
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Value, MarketAttemptError>> {
        Box::pin(async move {
            tx.inventory()?
                .execute(&self.app, actor, &self.action, self.now)
                .await
                .map_err(Into::into)
        })
    }
}

pub(super) struct InventoryPort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}
impl<'a> InventoryPort<'a> {
    pub(super) fn new(conn: &'a mut PgConnection, locked: &'a Locked) -> Self {
        Self { conn, locked }
    }
    async fn execute(
        &mut self,
        app: &AppState,
        actor: &AuthenticatedActor,
        action: &InventoryAction,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let org = self.require_admission(actor).await?;
        let write = InventoryWrite::validate(action, &app.policy)?;
        if actor.account().role != ActorRole::Supplier {
            return Err(not_available());
        }
        match write {
            InventoryWrite::Create(input) => self.create(app, actor, org, input, now).await,
            InventoryWrite::Patch { id, input } => {
                self.update(app, actor, org, id, input, now).await
            }
            InventoryWrite::Delete(id) => self.delete(app, actor, org, id, now).await,
        }
    }
    async fn create(
        &mut self,
        app: &AppState,
        actor: &AuthenticatedActor,
        org: Uuid,
        input: InventoryInput,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        self.require_port(input.0["port_id"].as_str().ok_or_else(invalid)?)
            .await?;
        let id = Uuid::new_v4();
        let before = super::effects::EffectPort::new(self.conn, &[], self.locked)
            .capture_before(&app.policy, &app.config, &[], now)
            .await?;
        let mut values = input.0;
        values.insert("id".into(), json!(id));
        values.insert("supplier_id".into(), json!(org));
        values.insert("owner_user_id".into(), json!(actor.user_id()));
        values.insert("reserved_stock_mt".into(), json!("0"));
        values.insert("updated_at".into(), json!(now));
        let row:Value=sqlx::query_scalar("INSERT INTO inventory_items (id,supplier_id,owner_user_id,port_id,fuel_type,product_name,fame_terms,lng_terms,current_stock_mt,incoming_stock_mt,reserved_stock_mt,price_per_mt_usd,energy_density_mj_kg,is_certified,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,updated_at) SELECT r.id,r.supplier_id,r.owner_user_id,r.port_id,r.fuel_type,r.product_name,r.fame_terms,r.lng_terms,r.current_stock_mt,r.incoming_stock_mt,r.reserved_stock_mt,r.price_per_mt_usd,r.energy_density_mj_kg,r.is_certified,r.certification_declared,r.certification_scheme,r.specification_standard,r.msds_available,r.carbon_intensity_gco2_mj,r.carbon_intensity_method,r.feedstock,r.origin,r.off_spec,r.off_spec_notes,r.updated_at FROM jsonb_populate_record(NULL::inventory_items,$1) r RETURNING to_jsonb(inventory_items)")
            .bind(Value::Object(values)).fetch_one(&mut *self.conn).await.map_err(port_error)?;
        let public = self.public_visible(id, &app.policy).await?;
        self.effect(
            app,
            actor,
            org,
            id,
            MutationKind::InventoryCreated,
            public,
            before,
            now,
        )
        .await?;
        response(row)
    }
    async fn update(
        &mut self,
        app: &AppState,
        actor: &AuthenticatedActor,
        org: Uuid,
        id: Uuid,
        input: InventoryInput,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let row = self.owned(id, org).await?;
        self.mutable(id, &row, false).await?;
        let public_before = self.public_visible(id, &app.policy).await?;
        let mut merged = Map::new();
        for key in FIELDS {
            merged.insert((*key).into(), row[*key].clone());
        }
        for (key, value) in input.0 {
            merged.insert(key, value);
        }
        let merged = InventoryInput::create(Value::Object(merged), &app.policy)?;
        let current = response(row.clone())?;
        let changed = FIELDS
            .iter()
            .any(|key| !same_inventory_value(key, &current[*key], &merged.0[*key]));
        let before = super::effects::EffectPort::new(self.conn, &[], self.locked)
            .capture_before(&app.policy, &app.config, &[], now)
            .await?;
        let mut values = merged.0;
        values.insert("updated_at".into(), json!(now));
        let updated:Value=sqlx::query_scalar("UPDATE inventory_items i SET product_name=r.product_name,fame_terms=r.fame_terms,lng_terms=r.lng_terms,current_stock_mt=r.current_stock_mt,incoming_stock_mt=r.incoming_stock_mt,price_per_mt_usd=r.price_per_mt_usd,energy_density_mj_kg=r.energy_density_mj_kg,is_certified=r.is_certified,certification_declared=r.certification_declared,certification_scheme=r.certification_scheme,specification_standard=r.specification_standard,msds_available=r.msds_available,carbon_intensity_gco2_mj=r.carbon_intensity_gco2_mj,carbon_intensity_method=r.carbon_intensity_method,feedstock=r.feedstock,origin=r.origin,off_spec=r.off_spec,off_spec_notes=r.off_spec_notes,updated_at=r.updated_at FROM jsonb_populate_record(NULL::inventory_items,$2) r WHERE i.id=$1 RETURNING to_jsonb(i)").bind(id).bind(Value::Object(values)).fetch_one(&mut *self.conn).await?;
        let public_after = self.public_visible(id, &app.policy).await?;
        self.effect(
            app,
            actor,
            org,
            id,
            MutationKind::InventoryUpdated,
            changed && (public_before || public_after),
            before,
            now,
        )
        .await?;
        response(updated)
    }
    async fn delete(
        &mut self,
        app: &AppState,
        actor: &AuthenticatedActor,
        org: Uuid,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let row = self.owned(id, org).await?;
        self.mutable(id, &row, true).await?;
        let public = self.public_visible(id, &app.policy).await?;
        let before = super::effects::EffectPort::new(self.conn, &[], self.locked)
            .capture_before(&app.policy, &app.config, &[], now)
            .await?;
        sqlx::query("DELETE FROM inventory_items WHERE id=$1")
            .bind(id)
            .execute(&mut *self.conn)
            .await
            .map_err(|error| {
                if error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23503") {
                    MarketWriteError::new(MarketErrorCode::InventoryReferenced)
                } else {
                    error.into()
                }
            })?;
        self.effect(
            app,
            actor,
            org,
            id,
            MutationKind::InventoryDeleted,
            public,
            before,
            now,
        )
        .await?;
        Ok(Value::Null)
    }
    /// Indicative market admission, checked again under the organization and
    /// user locks: the Python `require_security_market_admission` dependency
    /// (`market_party_is_admitted` with the legacy KYC rule). An account
    /// without an organization is not admitted. Returns the organization.
    async fn require_admission(
        &mut self,
        actor: &AuthenticatedActor,
    ) -> Result<Uuid, MarketWriteError> {
        let org = actor
            .account()
            .current_organization_id
            .ok_or_else(not_available)?;
        if !self.locked.contains(LockClass::Organization, org) || !actor.account().email_verified {
            return Err(not_available());
        }
        let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1 AND u.organization_id=$2 AND u.role::text IN ('BUYER','SUPPLIER') AND u.status::text='APPROVED' AND u.email_verified AND NOT u.must_change_password AND u.kyc_status<>'REJECTED' AND (u.kyc_organization_id IS NULL OR u.kyc_organization_id=u.organization_id) AND o.verification_status::text='APPROVED')").bind(actor.user_id()).bind(org).fetch_one(&mut *self.conn).await?;
        if !admitted {
            return Err(not_available());
        }
        Ok(org)
    }
    async fn owned(&mut self, id: Uuid, org: Uuid) -> Result<Value, MarketWriteError> {
        if !self.locked.contains(LockClass::Inventory, id) {
            return Err(not_found());
        }
        sqlx::query_scalar(
            "SELECT to_jsonb(i) FROM inventory_items i WHERE id=$1 AND supplier_id=$2",
        )
        .bind(id)
        .bind(org)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or_else(not_found)
    }
    async fn mutable(
        &mut self,
        id: Uuid,
        row: &Value,
        deleting: bool,
    ) -> Result<(), MarketWriteError> {
        let reserved = decimal_from_json(&row["reserved_stock_mt"])?;
        if reserved > Decimal::ZERO {
            return Err(MarketWriteError::new(MarketErrorCode::InventoryReserved));
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders o WHERE o.inventory_item_id=$1 AND (o.status::text IN ('OPEN','PARTIALLY_FILLED') OR EXISTS(SELECT 1 FROM trades t WHERE (t.bid_order_id=o.id OR t.ask_order_id=o.id) AND t.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED'))))").bind(id).fetch_one(&mut *self.conn).await?;
        if active {
            return Err(MarketWriteError::new(MarketErrorCode::InventoryInUse));
        }
        if deleting {
            let history: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE inventory_item_id=$1)",
            )
            .bind(id)
            .fetch_one(&mut *self.conn)
            .await?;
            if history {
                return Err(MarketWriteError::new(MarketErrorCode::InventoryReferenced));
            }
        }
        Ok(())
    }
    /// A record needs an existing port, as in the Python handler. A port outside
    /// the canonical delivery points is refused only when the record is
    /// published.
    async fn require_port(&mut self, port: &str) -> Result<(), MarketWriteError> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ports WHERE id=$1)")
            .bind(port)
            .fetch_one(&mut *self.conn)
            .await?;
        if !exists {
            return Err(MarketWriteError::new(MarketErrorCode::PortNotFound));
        }
        Ok(())
    }
    async fn public_visible(
        &mut self,
        id: Uuid,
        policy: &MarketPolicy,
    ) -> Result<bool, MarketWriteError> {
        let row=sqlx::query("SELECT to_jsonb(i) AS item,p.id AS product_id,p.name AS product_name,p.fuel_type AS product_fuel,p.fuel_grade AS product_grade,p.is_active AS product_active,d.id AS point_id,d.name AS point_name,d.region AS point_region,d.is_active AS point_active,port.is_active AS port_active,o.provenance,o.verification_status::text AS org_status,u.status::text AS owner_status,u.kyc_status,u.organization_id AS owner_org FROM inventory_items i JOIN ports port ON port.id=i.port_id JOIN delivery_points d ON d.name=port.name JOIN products p ON p.name=i.product_name JOIN organizations o ON o.id=i.supplier_id JOIN users u ON u.id=i.owner_user_id WHERE i.id=$1").bind(id).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else { return Ok(false) };
        let item: Value = row.get("item");
        let product = policy.products.iter().find(|p| {
            p.id == row.get::<Uuid, _>("product_id")
                && p.name == row.get::<String, _>("product_name")
                && p.fuel_type == row.get::<String, _>("product_fuel")
                && p.fuel_grade == row.get::<String, _>("product_grade")
        });
        let canonical_point = policy.points.iter().any(|p| {
            p.id == row.get::<Uuid, _>("point_id")
                && p.name == row.get::<String, _>("point_name")
                && p.region == row.get::<String, _>("point_region")
        });
        let Some(product) = product else {
            return Ok(false);
        };
        let provenance: String = row.get("provenance");
        let demo_ids: &[&str] = &[
            "4da7b285-34ee-5443-9406-f96b4ed1a251",
            "0dbce576-2026-5925-ab66-674d505e98ad",
            "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a",
            "277491df-cb0d-5f2d-a2cf-5746829c6da6",
            "3b302066-d65c-5c3e-8fcc-70b3da3bcafd",
            "79609f48-0a3e-560e-a1e1-63d90601d84a",
            "2c4e387e-de22-5adb-ad88-9274ba84ebe1",
            "612953c7-567a-58b3-bc42-ee817d2bbe74",
            "93ccda09-54b3-53ee-afc0-759d3048161f",
            "82426590-0963-5486-9b05-f81e97afe6ef",
            "acc3f20a-fe94-4463-9029-a55e35634eb7",
            "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
            "7cc77115-0a9f-4ec4-8c74-05aa10050111",
            "d1e43e55-3fb0-4b5e-9f0b-93aa10050222",
        ];
        let org = item["supplier_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok());
        let evidence = provenance == "REAL"
            || (provenance == "DEMO"
                && org.is_some_and(|id| demo_ids.contains(&id.to_string().as_str())));
        if !canonical_point
            || product.execution_mode != "ORDERBOOK"
            || !product.lanes.contains(&row.get::<Uuid, _>("point_id"))
            || !row.get::<bool, _>("product_active")
            || !row.get::<bool, _>("point_active")
            || !row.get::<bool, _>("port_active")
            || row.get::<String, _>("org_status") != "APPROVED"
            || row.get::<String, _>("owner_status") != "APPROVED"
            || row.get::<String, _>("kyc_status") != "APPROVED"
            || row.get::<Option<Uuid>, _>("owner_org") != org
            || !evidence
        {
            return Ok(false);
        }
        if let Some(standard) = &product.specification_standard {
            let present = |field| {
                item[field].as_str().is_some_and(|s| {
                    !s.trim_matches([' ', '\t', '\r', '\n', '\x0c', '\x0b'])
                        .is_empty()
                })
            };
            return Ok(item["off_spec"] == false
                && item["specification_standard"]
                    .as_str()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case(standard))
                && present("carbon_intensity_method")
                && item["certification_declared"] == true
                && present("certification_scheme")
                && item["msds_available"] == true
                && !item["carbon_intensity_gco2_mj"].is_null()
                && present("feedstock")
                && present("origin"));
        }
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    async fn effect(
        &mut self,
        app: &AppState,
        actor: &AuthenticatedActor,
        org: Uuid,
        id: Uuid,
        kind: MutationKind,
        public: bool,
        before: super::effects::BeforeEffects,
        now: DateTime<Utc>,
    ) -> Result<(), MarketWriteError> {
        let event = ParticipantEvent {
            event_type: match kind {
                MutationKind::InventoryCreated => "inventory_created",
                MutationKind::InventoryUpdated => "inventory_updated",
                _ => "inventory_deleted",
            }
            .into(),
            aggregate_type: "inventory".into(),
            aggregate_id: id.to_string(),
            participant_org_ids: vec![org],
            fields: ParticipantFields::Inventory { id },
            public_invalidated: public,
        };
        let descriptor = MutationEffect {
            kind,
            actor_id: actor.user_id(),
            organization_id: org,
            resource_type: "inventory".into(),
            resource_id: id,
            changes: Value::Null,
            event: Some(event),
            at: now,
            audit_context: AuditContext::default(),
        };
        super::effects::EffectPort::new(self.conn, &[], self.locked)
            .inventory_changed(&app.policy, &app.config, before, descriptor)
            .await?;
        Ok(())
    }
}

async fn inventory_items(
    conn: &mut PgConnection,
    org: Uuid,
    skip: i64,
    limit: i64,
) -> Result<Value, MarketWriteError> {
    let rows=sqlx::query("SELECT to_jsonb(i) AS item FROM inventory_items i WHERE supplier_id=$1 ORDER BY updated_at DESC,id OFFSET $2 LIMIT $3").bind(org).bind(skip).bind(limit).fetch_all(&mut *conn).await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|r| response(r.get("item")))
            .collect::<Result<_, _>>()?,
    ))
}

/// The owner's ASK listings in the compatibility shape of the Python
/// `_listing_payload(..., include_private_terms=True)`.
async fn compatibility_listings(
    conn: &mut PgConnection,
    org: Uuid,
    skip: i64,
    limit: i64,
    policy: &MarketPolicy,
) -> Result<Value, MarketWriteError> {
    let rows=sqlx::query("SELECT to_jsonb(o) AS item,p.name,p.fuel_type,p.fuel_grade,p.id AS product_id,d.region,(SELECT count(*) FROM trades t WHERE t.ask_order_id=o.id) AS match_count FROM orderbook_orders o LEFT JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.organization_id=$1 AND o.side::text='ASK' ORDER BY o.created_at DESC,o.id OFFSET $2 LIMIT $3").bind(org).bind(skip).bind(limit).fetch_all(&mut *conn).await?;
    let mut items = Vec::new();
    for row in rows {
        let item: Value = row.get("item");
        let mut out = Map::new();
        for key in [
            "id",
            "availability_window",
            "certification_declared",
            "certification_scheme",
            "specification_standard",
            "msds_available",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec",
            "off_spec_notes",
            "lng_terms",
            "fame_terms",
            "status",
            "expires_at",
            "delivery_point_id",
        ] {
            out.insert(key.into(), item[key].clone());
        }
        for key in [
            "quantity_mt",
            "price_per_mt_usd",
            "carbon_intensity_gco2_mj",
        ] {
            out.insert(key.into(), numeric_string(&item[key])?);
        }
        for (key, column) in [
            ("product_name", "name"),
            ("fuel_type", "fuel_type"),
            ("fuel_grade", "fuel_grade"),
            ("region", "region"),
        ] {
            out.insert(
                key.into(),
                json!(row.get::<Option<String>, _>(column).unwrap_or_default()),
            );
        }
        out.insert(
            "market_product".into(),
            json!(
                policy
                    .products
                    .iter()
                    .find(|p| Some(p.id) == row.get::<Option<Uuid>, _>("product_id"))
                    .map(|p| &p.code)
            ),
        );
        out.insert(
            "certifications".into(),
            item["certifications"]
                .as_array()
                .map(|v| json!(v))
                .unwrap_or(json!([])),
        );
        out.insert("observed_at".into(), item["created_at"].clone());
        out.insert(
            "match_count".into(),
            json!(row.get::<i64, _>("match_count")),
        );
        let provenance = item["provenance"].as_str().unwrap_or("UNKNOWN");
        let (source, demo, unknown) = match provenance {
            "REAL" => ("LIVE_ORDER", "REAL_ONLY", 0),
            "DEMO" => ("DEMO_SEED", "DEMO_ONLY", 0),
            _ => ("UNKNOWN", "UNKNOWN", 1),
        };
        out.insert("source_kind".into(), json!(source));
        out.insert(
            "scope".into(),
            json!(if item["delivery_point_id"].is_null() {
                "UNKNOWN"
            } else {
                "DELIVERY_POINT"
            }),
        );
        out.insert("demo_status".into(), json!(demo));
        out.insert("is_demo_listing".into(), json!(provenance == "DEMO"));
        out.insert("unknown_count".into(), json!(unknown));
        items.push(Value::Object(out));
    }
    Ok(Value::Array(items))
}
fn same_inventory_value(field: &str, before: &Value, after: &Value) -> bool {
    if matches!(
        field,
        "current_stock_mt"
            | "incoming_stock_mt"
            | "price_per_mt_usd"
            | "energy_density_mj_kg"
            | "carbon_intensity_gco2_mj"
    ) && !before.is_null()
        && !after.is_null()
    {
        return matches!((decimal_from_json(before), decimal_from_json(after)), (Ok(left), Ok(right)) if left == right);
    }
    before == after
}
fn decimal_from_json(value: &Value) -> Result<Decimal, MarketWriteError> {
    Decimal::from_str_exact(&decimal_text(value)?).map_err(|_| invalid())
}
fn numeric_string(value: &Value) -> Result<Value, MarketWriteError> {
    if value.is_null() {
        Ok(Value::Null)
    } else {
        Ok(json!(decimal_from_json(value)?.to_string()))
    }
}
fn response(row: Value) -> Result<Value, MarketWriteError> {
    let mut out = Map::new();
    for key in
        FIELDS
            .iter()
            .copied()
            .chain(["id", "supplier_id", "reserved_stock_mt", "updated_at"])
    {
        out.insert(key.into(), row[key].clone());
    }
    for key in [
        "current_stock_mt",
        "incoming_stock_mt",
        "reserved_stock_mt",
        "price_per_mt_usd",
        "energy_density_mj_kg",
        "carbon_intensity_gco2_mj",
    ] {
        out.insert(key.into(), numeric_string(&row[key])?);
    }
    Ok(Value::Object(out))
}
/// Publication uses the same configuration inputs as ordinary order entry.
pub(crate) struct PublishRequest {
    pub item: Uuid,
    pub raw_key: Option<String>,
    pub audit: AuditContext,
    pub settings: crate::routes::market_write::order_create::CreateSettings,
    pub now: DateTime<Utc>,
}

pub(crate) async fn publish(
    app: AppState,
    access: VerifiedAccess,
    request: PublishRequest,
) -> Result<super::CommittedCommand, MarketWriteError> {
    let verified = std::sync::Arc::new(VerifiedAccess {
        subject: access.subject,
        issued_at: access.issued_at,
        issuance_out_of_range: access.issuance_out_of_range,
    });
    run_market_command(
        app.pools.read.clone(),
        app.reads.clone(),
        access,
        PublishCommand {
            app,
            request,
            id: Uuid::new_v4(),
            access: verified,
        },
        super::AdmissionClass::Cleanup,
    )
    .await
}

struct PublishCommand {
    app: AppState,
    request: PublishRequest,
    id: Uuid,
    access: std::sync::Arc<VerifiedAccess>,
}
fn publish_bad(message: &'static str) -> MarketWriteError {
    super::orders::refusal(
        MarketErrorCode::ValidationFailed,
        StatusCode::BAD_REQUEST,
        message,
    )
}
fn slice_changed() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::MarketSliceChanged)
}
fn publication_result(id: Uuid, replayed: bool, matches: usize) -> super::CommittedCommand {
    super::CommittedCommand {
        response_status: 200,
        response_body: json!({"status":"published","listing_id":id}),
        replayed,
        matched_trades: matches,
    }
}
impl PublishCommand {
    fn key(&self) -> Result<Option<String>, MarketWriteError> {
        self.request
            .raw_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .map(|k| {
                super::normalize_idempotency_key(k)
                    .map_err(|_| publish_bad("Idempotency-Key must have 1-255 characters"))
            })
            .transpose()
    }
    fn hash(&self) -> Result<String, MarketWriteError> {
        super::idempotency_request_hash(
            &super::Canonical::from_json(&json!({"inventory_item_id":self.request.item}))
                .map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())
    }
    fn bound(&self) -> super::matching::MatchBound<'_> {
        super::matching::MatchBound::new(self.request.now)
            .with_policy(&self.app.policy)
            .with_kyc_gate(self.request.settings.kyc_execution_gate_enabled)
    }
    async fn actor_org(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
    ) -> Result<Uuid, MarketWriteError> {
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT organization_id FROM users WHERE id=$1")
            .bind(actor)
            .fetch_optional(conn)
            .await?
            .flatten()
            .ok_or_else(|| publish_bad("User has no organization"))
    }
    async fn candidate(
        &self,
        conn: &mut PgConnection,
        actor: Uuid,
        org: Uuid,
    ) -> Result<Option<super::matching::OrderRow>, MarketWriteError> {
        let Some(item) = load_publish_item(conn, self.request.item, org).await? else {
            return Ok(None);
        };
        let Some((product, point)) = resolve_publish_slice(conn, &self.app.policy, &item).await?
        else {
            return Ok(None);
        };
        let provenance: String =
            sqlx::query_scalar("SELECT provenance::text FROM organizations WHERE id=$1")
                .bind(org)
                .fetch_one(&mut *conn)
                .await?;
        let quantity = decimal_from_json(&item["current_stock_mt"])?;
        let price = if item["price_per_mt_usd"].is_null() {
            Decimal::ONE
        } else {
            decimal_from_json(&item["price_per_mt_usd"])?
        };
        let text = |key: &str| item[key].as_str().map(str::to_owned);
        let mut row = super::matching::OrderRow {
            id: self.id,
            organization_id: org,
            owner_user_id: Some(actor),
            created_by_actor_user_id: Some(actor),
            creation_method: super::matching::CreationMethod::SelfService,
            product_id: product,
            delivery_point_id: Some(point),
            availability_window: "SPOT".into(),
            side: super::matching::Side::Ask,
            provenance: provenance.clone(),
            organization_approved: true,
            price_per_mt_usd: price,
            quantity_mt: quantity.max(Decimal::ONE),
            remaining_quantity_mt: quantity.max(Decimal::ONE),
            acceptance_ordinal: 1,
            version: 1,
            status: super::matching::OrderStatus::Open,
            expires_at: (provenance == "DEMO")
                .then(|| self.request.now + chrono::Duration::hours(24)),
            created_at: self.request.now,
            inventory_item_id: Some(self.request.item),
            terms: super::matching::ExecutionTerms {
                off_spec: item["off_spec"] == true,
                msds_available: item["msds_available"] == true,
                certification_declared: item["certification_declared"] == true,
                certification_scheme: text("certification_scheme"),
                certifications: if item["is_certified"] == true {
                    vec!["INVENTORY_CERTIFIED".into()]
                } else {
                    vec![]
                },
                fame_terms: (!item["fame_terms"].is_null()).then(|| item["fame_terms"].clone()),
                lng_terms: (!item["lng_terms"].is_null()).then(|| item["lng_terms"].clone()),
                specification_standard: text("specification_standard"),
                carbon_intensity_method: text("carbon_intensity_method"),
                carbon_intensity_gco2_mj: if item["carbon_intensity_gco2_mj"].is_null() {
                    None
                } else {
                    Some(decimal_from_json(&item["carbon_intensity_gco2_mj"])?)
                },
                feedstock: text("feedstock"),
                origin: text("origin"),
            },
            owner_admission: super::matching::OwnerAdmission::Admitted,
        };
        if let Some(terms) = &row.terms.fame_terms {
            let mut metadata = item.as_object().cloned().ok_or_else(invalid)?;
            derive_metadata(&mut metadata, terms, "fame_v1")?;
            row.terms.certification_scheme =
                metadata["certification_scheme"].as_str().map(str::to_owned);
            row.terms.specification_standard = metadata["specification_standard"]
                .as_str()
                .map(str::to_owned);
            row.terms.carbon_intensity_method = metadata["carbon_intensity_method"]
                .as_str()
                .map(str::to_owned);
            row.terms.carbon_intensity_gco2_mj = if metadata["carbon_intensity_gco2_mj"].is_null() {
                None
            } else {
                Some(decimal_from_json(&metadata["carbon_intensity_gco2_mj"])?)
            };
            row.terms.feedstock = metadata["feedstock"].as_str().map(str::to_owned);
            row.terms.origin = metadata["origin"].as_str().map(str::to_owned);
            row.terms.certifications.clear();
        }
        Ok(Some(row))
    }
}
async fn load_publish_item(
    conn: &mut PgConnection,
    id: Uuid,
    org: Uuid,
) -> Result<Option<Value>, MarketWriteError> {
    Ok(sqlx::query_scalar(
        "SELECT to_jsonb(i) FROM inventory_items i WHERE id=$1 AND supplier_id=$2",
    )
    .bind(id)
    .bind(org)
    .fetch_optional(conn)
    .await?)
}
async fn resolve_publish_slice(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    item: &Value,
) -> Result<Option<(Uuid, Uuid)>, MarketWriteError> {
    let Some(name) = item["product_name"].as_str().map(str::trim) else {
        return Ok(None);
    };
    let Some(product) = policy.products.iter().find(|p| p.name == name) else {
        return Ok(None);
    };
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products WHERE id=$1 AND name=$2 AND fuel_type::text=$3 AND fuel_grade=$4 AND is_active)").bind(product.id).bind(&product.name).bind(&product.fuel_type).bind(&product.fuel_grade).fetch_one(&mut *conn).await?;
    if !active {
        return Ok(None);
    };
    let port: Option<String> = sqlx::query_scalar("SELECT name FROM ports WHERE id=$1")
        .bind(item["port_id"].as_str())
        .fetch_optional(&mut *conn)
        .await?;
    let Some(point) = port
        .as_deref()
        .and_then(|name| policy.points.iter().find(|p| p.name == name.trim()))
    else {
        return Ok(None);
    };
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM delivery_points WHERE id=$1 AND name=$2 AND region=$3 AND is_active)").bind(point.id).bind(&point.name).bind(&point.region).fetch_one(conn).await?;
    Ok(active.then_some((product.id, point.id)))
}
async fn live_publication(
    conn: &mut PgConnection,
    item: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, MarketWriteError> {
    Ok(sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE inventory_item_id=$1 AND status::text IN ('OPEN','PARTIALLY_FILLED') AND (expires_at IS NULL OR expires_at>$2) ORDER BY id LIMIT 1").bind(item).bind(now).fetch_optional(conn).await?)
}
async fn expiry_rows(
    conn: &mut PgConnection,
    candidate: &super::matching::OrderRow,
    now: DateTime<Utc>,
) -> Result<Vec<super::matching::OrderRow>, MarketWriteError> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM orderbook_orders WHERE product_id=$1 AND delivery_point_id=$2 AND availability_window='SPOT' AND side::text='ASK' AND status::text IN ('OPEN','PARTIALLY_FILLED') AND expires_at<=$3 ORDER BY id LIMIT 100").bind(candidate.product_id).bind(candidate.delivery_point_id).bind(now).fetch_all(&mut *conn).await?;
    let mut rows = Vec::new();
    for id in ids {
        rows.push(super::matching::load_order(conn, id).await?);
    }
    Ok(rows)
}
async fn publication_candidates(
    conn: &mut PgConnection,
    candidate: &super::matching::OrderRow,
    bound: super::matching::MatchBound<'_>,
    enabled: bool,
) -> Result<Vec<super::matching::OrderRow>, MarketWriteError> {
    if !enabled
        || candidate.provenance != "REAL"
        || !super::matching::order_is_execution_qualified(candidate, bound)
    {
        return Ok(vec![]);
    };
    super::matching::crossing_rows(
        conn,
        &super::matching::CrossingQuery::post_only(candidate),
        &bound,
    )
    .await
}

impl MarketCommand for PublishCommand {
    type Output = super::CommittedCommand;
    fn preview_drift_error(&self) -> MarketAttemptError {
        slice_changed().into()
    }
    fn uses_order_create_replay(&self) -> bool {
        self.request.raw_key.as_ref().is_some_and(|k| !k.is_empty())
    }
    fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
        serde_json::to_vec(&value.response_body).map_err(|_| invalid())?;
        Ok(())
    }
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            super::authority::admission::authenticate_preview(
                conn,
                self.access.clone(),
                super::AdmissionClass::Cleanup,
            )
            .await?;
            let org = self.actor_org(conn, actor).await?;
            let mut organizations = vec![org];
            let mut users = vec![];
            // Recorded replay and a live listing do not need fresh catalog,
            // expiry or crossing discovery.
            let replay_exists = if let Some(key) = self.key()? {
                sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE organization_id=$1 AND idempotency_operation='inventory.publish' AND idempotency_key=$2)").bind(org).bind(key).fetch_one(&mut *conn).await?
            } else {
                false
            };
            let candidate = if replay_exists
                || live_publication(conn, self.request.item, self.request.now)
                    .await?
                    .is_some()
            {
                None
            } else {
                self.candidate(conn, actor, org).await?
            };
            let mut fingerprint = json!({"organization":org});
            if let Some(candidate) = &candidate {
                let expired = expiry_rows(conn, candidate, self.request.now).await?;
                let candidates = publication_candidates(
                    conn,
                    candidate,
                    self.bound(),
                    self.request.settings.auto_matching_enabled,
                )
                .await?;
                let found = super::matching::discover_parties(
                    conn,
                    expired.iter().chain(candidates.iter()).chain([candidate]),
                    [],
                )
                .await?;
                organizations.extend(found.organizations);
                users.extend(found.users);
                fingerprint["slice"] = json!([candidate.product_id, candidate.delivery_point_id]);
                fingerprint["parties"] = found.fingerprint;
            }
            let parties = PartyLockPreview::new(organizations, users, actor, fingerprint);
            let mut locks = OperationLockPlan::new(MarketOperation::Publish);
            locks.parties(&parties)?;
            if let Some(key) = self.key()? {
                locks.idempotency(org, "inventory.publish", &key)?;
            }
            if let Some(candidate) = candidate {
                locks.slices([candidate.slice()])?;
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
            let Some(key) = self.key()? else {
                return Ok(None);
            };
            let port = tx.inventory()?;
            let org = self.actor_org(port.conn, actor.user_id()).await?;
            Ok(super::transaction::load_inventory_replay(
                port.conn,
                actor.user_id(),
                org,
                &key,
                &self.hash()?,
            )
            .await?
            .map(|id| publication_result(id, true, 0)))
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let (org, mut candidate, expired) = {
                let mut port = tx.inventory()?;
                let org = self.actor_org(port.conn, actor.user_id()).await?;
                port.admit_publication(self, actor, org).await?;
                if load_publish_item(port.conn, self.request.item, org)
                    .await?
                    .is_none()
                {
                    return Err(not_found().into());
                }
                if let Some(id) =
                    live_publication(port.conn, self.request.item, self.request.now).await?
                {
                    return Ok(publication_result(id, true, 0));
                }
                let item = load_publish_item(port.conn, self.request.item, org)
                    .await?
                    .ok_or_else(not_found)?;
                port.validate_publication_catalog(self, &item).await?;
                let candidate = self
                    .candidate(port.conn, actor.user_id(), org)
                    .await?
                    .ok_or_else(|| publish_bad("Inventory port or product is not canonical"))?;
                let expired = expiry_rows(port.conn, &candidate, self.request.now).await?;
                (org, candidate, expired)
            };
            let candidates = tx
                .publication_resources(
                    &candidate,
                    self.bound(),
                    self.request.settings.auto_matching_enabled,
                )
                .await?;
            let expired_ids: Vec<_> = expired.iter().map(|r| r.id).collect();
            let ids: Vec<_> = expired_ids
                .iter()
                .copied()
                .chain(candidates.iter().take(100).map(|r| r.id))
                .collect();
            let new = super::effects::NewOrderEffect {
                order_id: self.id,
                organization_id: org,
                slice: super::effects::EffectSlice {
                    product_id: candidate.product_id,
                    delivery_point_id: candidate.delivery_point_id.ok_or_else(slice_changed)?,
                    availability_window: "SPOT".into(),
                },
            };
            let mut before = tx
                .effects()?
                .capture_mutation(
                    &self.app.policy,
                    &self.app.config,
                    &ids,
                    &[],
                    &[new],
                    self.request.now,
                )
                .await?;
            let expiries: Vec<_> = expired
                .iter()
                .filter_map(|row| row.expires_at.map(|at| (row.id, at)))
                .collect();
            tx.effects()?
                .capture_expiry_visibility(
                    &mut before,
                    &self.app.policy,
                    &self.app.config,
                    &expiries,
                )
                .await?;
            // All expiry and match resources were declared before the first stock movement.
            let facts = tx
                .stock_ledger()?
                .expire_slice(
                    &super::ExpirySlice {
                        product_id: candidate.product_id,
                        delivery_point_id: candidate.delivery_point_id,
                        availability_window: "SPOT",
                        side: "ASK",
                    },
                    self.request.now,
                )
                .await?;
            {
                let mut port = tx.inventory()?;
                port.revalidate_sweep(self, actor, org, &expired).await?;
                let item = load_publish_item(port.conn, self.request.item, org)
                    .await?
                    .ok_or_else(not_found)?;
                if resolve_publish_slice(port.conn, &self.app.policy, &item).await?
                    != Some((
                        candidate.product_id,
                        candidate.delivery_point_id.ok_or_else(slice_changed)?,
                    ))
                {
                    return Err(slice_changed().into());
                }
                // This early return has staged no durable effect. The complete locks
                // make a newly appearing listing impossible for Rust peers.
                if live_publication(port.conn, self.request.item, self.request.now)
                    .await?
                    .is_some()
                {
                    return Err(slice_changed().into());
                }
                let backlog:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE inventory_item_id=$1 AND status::text IN ('OPEN','PARTIALLY_FILLED') AND expires_at<=$2)").bind(self.request.item).bind(self.request.now).fetch_one(&mut *port.conn).await?;
                if backlog {
                    return Err(super::orders::refusal(
                        MarketErrorCode::InventoryInconsistent,
                        StatusCode::CONFLICT,
                        "Inventory expiry backlog must be reconciled",
                    )
                    .into());
                }
                candidate = self
                    .candidate(port.conn, actor.user_id(), org)
                    .await?
                    .ok_or_else(slice_changed)?;
                port.validate_publication_quantity(self, &item, &mut candidate)
                    .await?;
                port.insert_publication(self, &candidate, &item).await?;
            }
            // The inventory is already locked; only the new row's evidence is
            // added here, before any terminal target phase.
            tx.register_publication(self.id)?;
            tx.stock_ledger()?
                .reserve(
                    self.request.item,
                    StockQuantity::checked(candidate.quantity_mt)?,
                )
                .await?;
            let outcome = if self.request.settings.auto_matching_enabled {
                let mut owners = super::matching::OwnerRevalidation::default();
                let mut rows = candidates;
                {
                    let port = tx.inventory()?;
                    candidate.owner_admission = owners
                        .revalidate(port.conn, port.locked, &candidate, &self.bound())
                        .await?;
                    for row in rows.iter_mut().take(100) {
                        row.owner_admission = owners
                            .revalidate(port.conn, port.locked, row, &self.bound())
                            .await?;
                    }
                }
                let plan = super::matching::plan_match(&rows, &candidate, self.bound())?;
                Some(
                    tx.complete_match(&plan, &self.request.settings.fees, true)
                        .await?,
                )
            } else {
                None
            };
            let mut set = before.stage();
            let descriptors=facts.iter().map(|fact|super::effects::MutationEffect{kind:MutationKind::OrderExpired,actor_id:actor.user_id(),organization_id:fact.organization_id,resource_type:"order".into(),resource_id:fact.order_id,changes:json!({"status":"EXPIRED","released_quantity_mt":fact.released_quantity_mt.value().to_string()}),event:Some(ParticipantEvent{event_type:"order_expired".into(),aggregate_type:"order".into(),aggregate_id:fact.order_id.to_string(),participant_org_ids:vec![fact.organization_id],fields:ParticipantFields::OrderExpired{id:fact.order_id},public_invalidated:true}),at:self.request.now,audit_context:AuditContext::default()}).collect();
            set.expired(facts, descriptors)?;
            let participants = outcome
                .as_ref()
                .into_iter()
                .flat_map(|o| o.trades.iter())
                .flat_map(|t| [t.buyer_id, t.seller_id])
                .chain([org])
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let descriptor = tx
                .inventory()?
                .publication_effect(
                    self,
                    actor,
                    &candidate,
                    participants,
                    outcome.as_ref().is_some_and(|o| !o.trades.is_empty()),
                )
                .await?;
            set.push(descriptor)?;
            if let Some(outcome) = &outcome {
                tx.effects()?
                    .matched(
                        &mut set,
                        outcome,
                        &candidate,
                        actor.user_id(),
                        self.request.audit.clone(),
                    )
                    .await?;
            }
            tx.effects()?
                .finish(&self.app.policy, &self.app.config, set)
                .await?;
            #[cfg(feature = "contract-tests")]
            if let Some(barrier) = &self.request.settings.barrier {
                barrier.reached.notify_one();
                barrier.resume.notified().await;
            }
            Ok(publication_result(
                self.id,
                false,
                outcome.as_ref().map_or(0, |o| o.trades.len()),
            ))
        })
    }
}

/// Acquire the single bounded order set, then every inventory row in UUID
/// order. The returned look-ahead remains in the planner input but is not locked.
pub(super) async fn lock_publication_resources(
    conn: &mut PgConnection,
    locked: &Locked,
    candidate: &super::matching::OrderRow,
    bound: super::matching::MatchBound<'_>,
    enabled: bool,
) -> Result<(Vec<super::matching::OrderRow>, Vec<Uuid>, Vec<Uuid>), MarketWriteError> {
    locked.require_economic_phase()?;
    locked.require_effect_slice(&candidate.slice())?;
    if locked.has_acquired_economic_resources() {
        return Err(slice_changed());
    }
    let expired = expiry_rows(conn, candidate, bound.now()).await?;
    let rows = publication_candidates(conn, candidate, bound, enabled).await?;
    super::matching::require_parties_locked(
        locked,
        expired.iter().chain(rows.iter()).chain([candidate]),
    )?;
    let orders: Vec<_> = expired
        .iter()
        .map(|r| r.id)
        .chain(rows.iter().take(100).map(|r| r.id))
        .collect();
    for id in &orders {
        sqlx::query("SELECT id FROM orderbook_orders WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
        locked.record_matching_resource(LockClass::Order, *id)?;
    }
    let items: Vec<_> = expired
        .iter()
        .chain(rows.iter().take(100))
        .chain([candidate])
        .filter_map(|r| r.inventory_item_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for id in &items {
        sqlx::query("SELECT id FROM inventory_items WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| MarketWriteError::new(MarketErrorCode::InventoryInconsistent))?;
        locked.record_matching_resource(LockClass::Inventory, *id)?;
    }
    let current_expired = expiry_rows(conn, candidate, bound.now()).await?;
    if current_expired.iter().map(|r| r.id).collect::<Vec<_>>()
        != expired.iter().map(|r| r.id).collect::<Vec<_>>()
    {
        return Err(slice_changed());
    }
    let current = publication_candidates(conn, candidate, bound, enabled).await?;
    // Re-selection retains the original SQL count. Any change aborts instead
    // of substituting rows beyond the first hundred.
    if current.iter().map(|r| r.id).collect::<Vec<_>>()
        != rows.iter().map(|r| r.id).collect::<Vec<_>>()
    {
        return Err(slice_changed());
    }
    let mut declared_orders = orders;
    declared_orders.push(candidate.id);
    Ok((current, declared_orders, items))
}

impl InventoryPort<'_> {
    async fn admit_publication(
        &mut self,
        command: &PublishCommand,
        actor: &AuthenticatedActor,
        org: Uuid,
    ) -> Result<(), MarketWriteError> {
        let row=sqlx::query("SELECT u.kyc_status,u.kyc_organization_id,g.expires_at FROM users u LEFT JOIN kyc_execution_grace g ON g.user_id=u.id AND g.organization_id=u.organization_id WHERE u.id=$1").bind(actor.user_id()).fetch_one(&mut *self.conn).await?;
        let status: String = row.get("kyc_status");
        let grace = row
            .get::<Option<DateTime<Utc>>, _>("expires_at")
            .map(|expires_at| crate::kyc::grace::Grace {
                user_id: actor.user_id(),
                organization_id: org,
                expires_at,
            });
        let eligibility = crate::kyc::policy::eligibility(
            &crate::kyc::policy::KycPolicy {
                execution_gate_enabled: command.request.settings.kyc_execution_gate_enabled,
            },
            &crate::kyc::policy::KycParty {
                user_id: actor.user_id(),
                organization_id: Some(org),
                kyc_organization_id: row.get("kyc_organization_id"),
                kyc_status: serde_json::from_value(json!(status)).map_err(|_| invalid())?,
            },
            grace.as_ref(),
            command.request.now,
        );
        if !eligibility.eligible {
            return Err(MarketWriteError::kyc_required(
                status,
                eligibility.grace_expires_at,
            ));
        }
        // Fresh publication always revalidates the exact supplier under the prefix locks.
        super::authority::admission::require_execution(
            self.conn,
            self.locked,
            &super::PartyRef {
                user_id: actor.user_id(),
                organization_id: org,
                role: ActorRole::Supplier,
            },
            command.request.now,
            command.request.settings.kyc_execution_gate_enabled,
        )
        .await
    }
    async fn revalidate_sweep(
        &mut self,
        command: &PublishCommand,
        actor: &AuthenticatedActor,
        org: Uuid,
        expired: &[super::matching::OrderRow],
    ) -> Result<(), MarketWriteError> {
        self.admit_publication(command, actor, org)
            .await
            .map_err(|_| slice_changed())?;
        for row in expired {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM organizations WHERE id=$1)")
                    .bind(row.organization_id)
                    .fetch_one(&mut *self.conn)
                    .await?;
            if !exists {
                return Err(slice_changed());
            }
        }
        Ok(())
    }
    async fn validate_publication_catalog(
        &mut self,
        command: &PublishCommand,
        item: &Value,
    ) -> Result<(), MarketWriteError> {
        if item["price_per_mt_usd"].is_null() {
            return Err(publish_bad("Inventory item is missing a price"));
        }
        let name = item["product_name"].as_str().unwrap_or_default().trim();
        let product = command
            .app
            .policy
            .products
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| publish_bad("Inventory product is not mapped"))?;
        if product.qualification_profile == "lng_v1" {
            return Err(publish_bad(
                "Inventory publication is not supported; post an ASK with lng_terms",
            ));
        }
        if product.execution_mode != "ORDERBOOK" {
            return Err(publish_bad("Product requires an RFQ"));
        }
        let (product_id, point) = resolve_publish_slice(self.conn, &command.app.policy, item)
            .await?
            .ok_or_else(|| {
                publish_bad("Inventory product or delivery point is not active and canonical")
            })?;
        if product_id != product.id || !product.lanes.contains(&point) {
            return Err(publish_bad(
                "Product is not available at this delivery point",
            ));
        }
        Ok(())
    }
    async fn validate_publication_quantity(
        &mut self,
        command: &PublishCommand,
        item: &Value,
        candidate: &mut super::matching::OrderRow,
    ) -> Result<(), MarketWriteError> {
        let quantity = decimal_from_json(&item["current_stock_mt"])?;
        if quantity <= Decimal::ZERO {
            return Err(publish_bad("Inventory stock is unavailable"));
        }
        let minimum: Option<Decimal> =
            sqlx::query_scalar("SELECT min_lot_size FROM products WHERE id=$1")
                .bind(candidate.product_id)
                .fetch_one(&mut *self.conn)
                .await?;
        let minimum = minimum.filter(|q| *q > Decimal::ZERO).ok_or_else(|| {
            super::orders::refusal(
                MarketErrorCode::ValidationFailed,
                StatusCode::CONFLICT,
                "Product minimum lot is unavailable",
            )
        })?;
        if quantity < minimum {
            return Err(publish_bad(
                "Inventory quantity is below the product minimum",
            ));
        }
        candidate.quantity_mt = super::OrderQuantity::checked(quantity)?.value();
        candidate.remaining_quantity_mt = quantity;
        let product = command
            .app
            .policy
            .products
            .iter()
            .find(|p| p.id == candidate.product_id)
            .ok_or_else(invalid)?;
        if product.qualification_profile == "fame_v1" {
            if item["fuel_type"] != "FAME" || item["is_certified"] == true {
                return Err(invalid());
            }
            crate::market::terms::validate_fame_terms(
                "ASK",
                candidate.terms.fame_terms.as_ref(),
                command.bound().today(),
            )
            .map_err(|_| invalid())?;
            let normalized = crate::market::terms::owner_response_terms(
                "fame_v1",
                candidate.terms.fame_terms.as_ref().ok_or_else(invalid)?,
            )
            .map_err(|_| invalid())?;
            candidate.terms.fame_terms = Some(normalized);
        } else if candidate.terms.fame_terms.is_some() || candidate.terms.lng_terms.is_some() {
            return Err(invalid());
        }
        if !super::matching::order_is_execution_qualified(candidate, command.bound()) {
            return Err(publish_bad(
                "Inventory is not execution-qualified for publication",
            ));
        }
        Ok(())
    }
    async fn insert_publication(
        &mut self,
        command: &PublishCommand,
        row: &super::matching::OrderRow,
        item: &Value,
    ) -> Result<(), MarketAttemptError> {
        let ordinal: i64 = sqlx::query_scalar("SELECT nextval('orderbook_acceptance_ordinal_seq')")
            .fetch_one(&mut *self.conn)
            .await?;
        let t = &row.terms;
        let key = command.key()?;
        let payload = json!({"id":row.id,"organization_id":row.organization_id,"owner_user_id":row.owner_user_id,"created_by_actor_user_id":row.created_by_actor_user_id,"creation_method":"SELF_SERVICE","provenance":row.provenance,"inventory_item_id":command.request.item,"side":"ASK","product_id":row.product_id,"delivery_point_id":row.delivery_point_id,"port_id":item["port_id"],"quantity_mt":row.quantity_mt.to_string(),"remaining_quantity_mt":row.quantity_mt.to_string(),"price_per_mt_usd":row.price_per_mt_usd.to_string(),"availability_window":"SPOT","certifications":t.certifications,"fame_terms":t.fame_terms,"lng_terms":t.lng_terms,"energy_density_mj_kg":t.fame_terms.as_ref().map(|f|&f["lhv_mj_kg"]),"status":"OPEN","version":1,"acceptance_ordinal":ordinal,"expires_at":row.expires_at,"idempotency_key":key,"idempotency_operation":key.as_ref().map(|_|"inventory.publish"),"idempotency_request_hash":key.as_ref().map(|_|command.hash()).transpose()?,"certification_declared":t.certification_declared,"certification_scheme":t.certification_scheme,"specification_standard":t.specification_standard,"msds_available":t.msds_available,"carbon_intensity_gco2_mj":t.carbon_intensity_gco2_mj.map(|v|v.to_string()),"carbon_intensity_method":t.carbon_intensity_method,"feedstock":t.feedstock,"origin":t.origin,"off_spec":t.off_spec,"off_spec_notes":item["off_spec_notes"],"created_at":command.request.now,"updated_at":command.request.now,"is_verdaxis_verified":false});
        sqlx::query("INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,inventory_item_id,side,product_id,delivery_point_id,port_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,certifications,fame_terms,lng_terms,energy_density_mj_kg,status,version,acceptance_ordinal,expires_at,idempotency_key,idempotency_operation,idempotency_request_hash,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,created_at,updated_at,is_verdaxis_verified) SELECT id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,inventory_item_id,side,product_id,delivery_point_id,port_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,certifications,fame_terms,lng_terms,energy_density_mj_kg,status,version,acceptance_ordinal,expires_at,idempotency_key,idempotency_operation,idempotency_request_hash,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,created_at,updated_at,is_verdaxis_verified FROM jsonb_populate_record(NULL::orderbook_orders,$1::jsonb)").bind(payload).execute(&mut *self.conn).await?;
        Ok(())
    }
    async fn publication_effect(
        &mut self,
        command: &PublishCommand,
        actor: &AuthenticatedActor,
        row: &super::matching::OrderRow,
        participants: Vec<Uuid>,
        matched: bool,
    ) -> Result<MutationEffect, MarketWriteError> {
        let projection = super::order_create::CreatePort::new(self.conn, self.locked)?
            .projection(
                row.id,
                &command.app.policy,
                &command.app.config,
                command.request.now,
            )
            .await?;
        let status: String =
            sqlx::query_scalar("SELECT status::text FROM orderbook_orders WHERE id=$1")
                .bind(row.id)
                .fetch_one(&mut *self.conn)
                .await?;
        let visible = super::effects::order_visible(
            self.conn,
            &command.app.policy,
            &command.app.config,
            row.id,
            command.request.now,
        )
        .await?;
        let text = |key: &str| projection[key].as_str().unwrap_or_default().to_owned();
        let activity = super::outbox::OrderActivity {
            id: row.id,
            side: "ASK".into(),
            product_name: text("product_name"),
            fuel_type: text("fuel_type"),
            region: text("region"),
            source_kind: text("source_kind"),
            scope: text("scope"),
            demo_status: text("demo_status"),
            is_demo_listing: row.provenance == "DEMO",
            unknown_count: u32::from(row.provenance == "UNKNOWN"),
            market_product: projection["market_product"].as_str().map(str::to_owned),
            delivery_point_id: row.delivery_point_id,
            availability_window: "SPOT".into(),
            observed_at: Some(command.request.now),
            delivery_point: projection["delivery_point_name"]
                .as_str()
                .map(str::to_owned),
        };
        Ok(MutationEffect {
            kind: MutationKind::InventoryPublished,
            actor_id: actor.user_id(),
            organization_id: row.organization_id,
            resource_type: "inventory".into(),
            resource_id: command.request.item,
            changes: json!({"inventory_item_id":command.request.item,"listing_id":row.id,"quantity_mt":row.quantity_mt.to_string(),"price_per_mt_usd":row.price_per_mt_usd.to_string(),"status":status}),
            event: Some(ParticipantEvent {
                event_type: "order_created".into(),
                aggregate_type: "order".into(),
                aggregate_id: row.id.to_string(),
                participant_org_ids: participants,
                fields: ParticipantFields::OrderCreated {
                    activity,
                    price: row.price_per_mt_usd.to_string(),
                    quantity: text("remaining_quantity_mt"),
                },
                public_invalidated: visible || matched,
            }),
            at: command.request.now,
            audit_context: command.request.audit.clone(),
        })
    }
}

fn port_error(error: sqlx::Error) -> MarketWriteError {
    if error.as_database_error().is_some_and(|e| {
        e.code().as_deref() == Some("23503")
            && e.constraint() == Some("inventory_items_port_id_fkey")
    }) {
        MarketWriteError::new(MarketErrorCode::PortNotFound)
    } else {
        error.into()
    }
}
