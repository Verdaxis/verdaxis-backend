//! Supplier inventory records share the ordered transaction and participant effects.
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    AuthenticatedActor, MarketErrorCode, MarketWriteError, Price, StockQuantity,
    authority::ActorRole,
    effects::{AuditContext, MutationEffect, MutationKind},
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview},
    outbox::{ParticipantEvent, ParticipantFields},
    transaction::{BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview, run_market_command},
};
use crate::{AppState, MarketPolicy, auth::VerifiedAccess, market::terms};

const FIELDS: &[&str] = &[
    "port_id", "fuel_type", "product_name", "fame_terms", "lng_terms", "current_stock_mt",
    "incoming_stock_mt", "price_per_mt_usd", "energy_density_mj_kg", "is_certified",
    "certification_declared", "certification_scheme", "specification_standard", "msds_available",
    "carbon_intensity_gco2_mj", "carbon_intensity_method", "feedstock", "origin", "off_spec", "off_spec_notes",
];
const PATCH_FIELDS: &[&str] = &[
    "product_name", "fame_terms", "lng_terms", "current_stock_mt", "incoming_stock_mt", "price_per_mt_usd",
    "certification_declared", "certification_scheme", "specification_standard", "msds_available",
    "carbon_intensity_gco2_mj", "carbon_intensity_method", "feedstock", "origin", "off_spec", "off_spec_notes",
];
fn invalid() -> MarketWriteError { MarketWriteError::new(MarketErrorCode::ValidationFailed) }
fn refusal(status: StatusCode, message: &'static str) -> MarketWriteError {
    let mut error = invalid(); error.status = status; error.message = message; error
}
fn not_found() -> MarketWriteError { refusal(StatusCode::NOT_FOUND, "Inventory item was not found") }

#[derive(Clone, Debug)]
pub struct InventoryInput(Map<String, Value>);
impl InventoryInput {
    pub fn create(value: Value, policy: &MarketPolicy) -> Result<Self, MarketWriteError> {
        let mut fields = object(value, FIELDS)?;
        for (key,default) in [("incoming_stock_mt",json!("0")),("is_certified",json!(false)),("certification_declared",json!(false)),("msds_available",json!(false)),("off_spec",json!(false))] { fields.entry(key).or_insert(default); }
        for key in FIELDS { fields.entry(*key).or_insert(Value::Null); }
        validate_fields(&mut fields)?;
        let product = fields["product_name"].as_str().and_then(|name| policy.products.iter().find(|p| p.name == name));
        let profile = product.map(|p| p.qualification_profile.as_str()).unwrap_or("scalar");
        let fuel = fields["fuel_type"].as_str().ok_or_else(invalid)?;
        if !["FAME","Methanol","Ethanol","Biofuel","LNG","Ammonia","LSMGO"].contains(&fuel) { return Err(invalid()); }
        for (term,expected) in [("fame_terms","fame_v1"),("lng_terms","lng_v1")] {
            if profile != expected && !fields[term].is_null() { return Err(invalid()); }
        }
        if fuel == "FAME" && profile != "fame_v1" { return Err(invalid()); }
        if matches!(profile,"fame_v1"|"lng_v1") {
            if product.is_none_or(|p| p.fuel_type != fuel) || fields["is_certified"] == true { return Err(invalid()); }
            let term = if profile == "fame_v1" { "fame_terms" } else { "lng_terms" };
            let declaration = &fields[term];
            if profile == "fame_v1" {
                terms::validate_fame_terms("ASK",Some(declaration),Utc::now().with_timezone(&chrono_tz::Asia::Singapore).date_naive()).map_err(|_| invalid())?;
            } else { terms::validate_lng_terms("ASK",Some(declaration)).map_err(|_| invalid())?; }
            let declaration = declaration.clone();
            derive_metadata(&mut fields,&declaration,profile)?;
        }
        Ok(Self(fields))
    }
    pub fn patch(value: Value) -> Result<Self, MarketWriteError> {
        let mut fields = object(value,PATCH_FIELDS)?;
        validate_fields(&mut fields)?;
        Ok(Self(fields))
    }
}
fn object(value:Value, allowed:&[&str]) -> Result<Map<String,Value>,MarketWriteError> {
    let Value::Object(fields)=value else { return Err(invalid()); };
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) { return Err(invalid()); }
    Ok(fields)
}
fn validate_fields(fields:&mut Map<String,Value>) -> Result<(),MarketWriteError> {
    for (key,value) in fields.iter_mut() {
        match key.as_str() {
            "current_stock_mt"|"incoming_stock_mt" => {
                let text = decimal_text(value)?; *value=json!(StockQuantity::parse_exact(&text)?.value().to_string());
            }
            "price_per_mt_usd" if !value.is_null() => {
                let text=decimal_text(value)?; *value=json!(Price::parse_exact(&text)?.value().to_string());
            }
            "energy_density_mj_kg"|"carbon_intensity_gco2_mj" if !value.is_null() => {
                let text=decimal_text(value)?;
                let amount=StockQuantity::parse_exact(&text)?.value();
                if amount>Decimal::from(10000) { return Err(invalid()); }
                *value=json!(amount.to_string());
            }
            "is_certified"|"certification_declared"|"msds_available"|"off_spec" => if !value.is_boolean() { return Err(invalid()); },
            "fame_terms"|"lng_terms" => if !value.is_null() && !value.is_object() { return Err(invalid()); },
            _ => if !value.is_null() && !value.is_string() { return Err(invalid()); },
        }
    }
    if fields.get("port_id").is_some_and(|v| v.as_str().is_none_or(str::is_empty)) { return Err(invalid()); }
    Ok(())
}
fn decimal_text(value:&Value) -> Result<String,MarketWriteError> {
    match value { Value::String(v)=>Ok(v.clone()),Value::Number(v)=>Ok(v.to_string()),_=>Err(invalid()) }
}
fn derive_metadata(fields:&mut Map<String,Value>, declaration:&Value, profile:&str) -> Result<(),MarketWriteError> {
    if profile=="fame_v1" {
        for (output,input) in [("certification_scheme","sustainability_scheme"),("carbon_intensity_gco2_mj","ci_gco2e_mj"),("carbon_intensity_method","ci_methodology"),("energy_density_mj_kg","lhv_mj_kg"),("origin","production_origin")] { fields.insert(output.into(),declaration[input].clone()); }
        fields.insert("specification_standard".into(),json!(format!("{} {}",declaration["standard"].as_str().ok_or_else(invalid)?,declaration["standard_edition"].as_str().ok_or_else(invalid)?)));
        fields.insert("feedstock".into(),json!("Used cooking oil (100% of feedstock inputs)"));
    } else {
        let scheme=match declaration["sustainability_scheme"].as_str() { Some("ISCC_EU")=>"ISCC EU",Some("REDCERT_EU")=>"REDcert-EU",_=>return Err(invalid()) };
        let feedstock=match declaration["feedstock"].as_str() {
            Some("MANURE")=>"Manure",Some("FOOD_WASTE")=>"Food waste",Some("SEWAGE_SLUDGE")=>"Sewage sludge",Some("LANDFILL_GAS")=>"Landfill gas",Some("AGRICULTURAL_RESIDUES")=>"Agricultural residues",Some("OTHER_WASTES_AND_RESIDUES")=>"Other wastes and residues",Some("ENERGY_CROPS")=>"Energy crops",_=>return Err(invalid()) };
        for (key,value) in [("certification_declared",json!(true)),("certification_scheme",json!(scheme)),("feedstock",json!(feedstock)),("specification_standard",declaration["specification_standard"].clone()),("origin",declaration["production_country"].clone()),("carbon_intensity_gco2_mj",Value::Null),("carbon_intensity_method",Value::Null)] { fields.insert(key.into(),value); }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum InventoryAction { List{skip:i64,limit:i64}, Listings{skip:i64,limit:i64}, Create(InventoryInput), Patch{ id:Uuid,input:InventoryInput }, Delete(Uuid) }
impl InventoryAction {
    fn id(&self)->Option<Uuid> { match self {Self::Patch{id,..}|Self::Delete(id)=>Some(*id),_=>None} }
    fn writes(&self)->bool { matches!(self,Self::Create(_)|Self::Patch{..}|Self::Delete(_)) }
}
struct InventoryCommand { app:AppState, action:InventoryAction, now:DateTime<Utc> }

pub async fn execute_inventory(app:AppState, access:VerifiedAccess, action:InventoryAction)->Result<Value,MarketWriteError> {
    let pool=app.pools.read.clone();
    let permits=app.reads.clone();
    run_market_command(pool,permits,access,InventoryCommand{app,action,now:Utc::now()},super::AdmissionClass::Execution).await
}
impl MarketCommand for InventoryCommand {
    type Output=Value;
    fn preview<'a>(&'a self,conn:&'a mut PgConnection,actor:Uuid)->BoxFuture<'a,Result<MarketPreview,MarketAttemptError>> {
        Box::pin(async move {
            let org:Option<Uuid>=sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1").bind(actor).fetch_optional(&mut *conn).await?.flatten();
            let parties=PartyLockPreview::new(org,[],actor,json!({"organization_id":org}));
            let mut locks=OperationLockPlan::new(if self.action.id().is_some(){MarketOperation::InventoryEdit}else{MarketOperation::InventoryCreate});
            locks.parties(&parties)?;
            if let Some(id)=self.action.id() { locks.inventory([id])?; }
            Ok(MarketPreview{parties,locks})
        })
    }
    fn execute<'a>(&'a self,tx:&'a mut LockedMarketTransaction<'_>,actor:&'a AuthenticatedActor)->BoxFuture<'a,Result<Value,MarketAttemptError>> {
        Box::pin(async move { tx.inventory()?.execute(&self.app,actor,&self.action,self.now).await.map_err(Into::into) })
    }
}

pub(super) struct InventoryPort<'a> { conn:&'a mut PgConnection, locked:&'a Locked }
impl<'a> InventoryPort<'a> {
    pub(super) fn new(conn:&'a mut PgConnection,locked:&'a Locked)->Self { Self{conn,locked} }
    async fn execute(&mut self,app:&AppState,actor:&AuthenticatedActor,action:&InventoryAction,now:DateTime<Utc>)->Result<Value,MarketWriteError> {
        if actor.account().role!=ActorRole::Supplier { return Err(MarketWriteError::new(MarketErrorCode::ExecutionNotAvailable)); }
        let org=actor.account().current_organization_id;
        if action.writes() {
            let org=org.ok_or_else(||refusal(StatusCode::BAD_REQUEST,"The account has no organization"))?;
            self.require_supplier(actor,org).await?;
        }
        match action {
            InventoryAction::List{skip,limit} => {
                let Some(org)=org else { return Ok(json!([])); };
                let rows=sqlx::query("SELECT to_jsonb(i) AS item FROM inventory_items i WHERE supplier_id=$1 ORDER BY updated_at DESC,id OFFSET $2 LIMIT $3").bind(org).bind(skip).bind(limit).fetch_all(&mut *self.conn).await?;
                Ok(Value::Array(rows.into_iter().map(|r| response(r.get("item"))).collect::<Result<_,_>>()?))
            }
            InventoryAction::Listings{skip,limit} => self.listings(org,*skip,*limit,&app.policy).await,
            InventoryAction::Create(input) => {
                let org=org.ok_or_else(invalid)?;
                self.require_port(input.0["port_id"].as_str().ok_or_else(invalid)?,&app.policy).await?;
                let id=Uuid::new_v4();
                let before=super::effects::EffectPort::new(self.conn,&[],self.locked).capture_before(&app.policy,&app.config,&[],now).await?;
                let mut values=input.0.clone();
                values.insert("id".into(),json!(id)); values.insert("supplier_id".into(),json!(org)); values.insert("owner_user_id".into(),json!(actor.user_id())); values.insert("reserved_stock_mt".into(),json!("0")); values.insert("updated_at".into(),json!(now));
                let row:Value=sqlx::query_scalar("INSERT INTO inventory_items (id,supplier_id,owner_user_id,port_id,fuel_type,product_name,fame_terms,lng_terms,current_stock_mt,incoming_stock_mt,reserved_stock_mt,price_per_mt_usd,energy_density_mj_kg,is_certified,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,updated_at) SELECT r.id,r.supplier_id,r.owner_user_id,r.port_id,r.fuel_type,r.product_name,r.fame_terms,r.lng_terms,r.current_stock_mt,r.incoming_stock_mt,r.reserved_stock_mt,r.price_per_mt_usd,r.energy_density_mj_kg,r.is_certified,r.certification_declared,r.certification_scheme,r.specification_standard,r.msds_available,r.carbon_intensity_gco2_mj,r.carbon_intensity_method,r.feedstock,r.origin,r.off_spec,r.off_spec_notes,r.updated_at FROM jsonb_populate_record(NULL::inventory_items,$1) r RETURNING to_jsonb(inventory_items)")
                    .bind(Value::Object(values)).fetch_one(&mut *self.conn).await.map_err(port_error)?;
                let public=self.public_visible(id,&app.policy).await?;
                self.effect(app,actor,org,id,MutationKind::InventoryCreated,public,before,now).await?;
                response(row)
            }
            InventoryAction::Patch{id,input} => {
                let org=org.ok_or_else(invalid)?;
                let row=self.owned(*id,org).await?;
                self.mutable(*id,&row,false).await?;
                let public_before=self.public_visible(*id,&app.policy).await?;
                let mut merged=Map::new(); for key in FIELDS { merged.insert((*key).into(),row[*key].clone()); }
                for (key,value) in &input.0 { merged.insert(key.clone(),value.clone()); }
                let merged=InventoryInput::create(Value::Object(merged),&app.policy)?;
                let current=response(row.clone())?;
                let changed=FIELDS.iter().any(|key| current[*key]!=merged.0[*key]);
                let before=super::effects::EffectPort::new(self.conn,&[],self.locked).capture_before(&app.policy,&app.config,&[],now).await?;
                let mut values=merged.0; values.insert("updated_at".into(),json!(now));
                let updated:Value=sqlx::query_scalar("UPDATE inventory_items i SET product_name=r.product_name,fame_terms=r.fame_terms,lng_terms=r.lng_terms,current_stock_mt=r.current_stock_mt,incoming_stock_mt=r.incoming_stock_mt,price_per_mt_usd=r.price_per_mt_usd,energy_density_mj_kg=r.energy_density_mj_kg,is_certified=r.is_certified,certification_declared=r.certification_declared,certification_scheme=r.certification_scheme,specification_standard=r.specification_standard,msds_available=r.msds_available,carbon_intensity_gco2_mj=r.carbon_intensity_gco2_mj,carbon_intensity_method=r.carbon_intensity_method,feedstock=r.feedstock,origin=r.origin,off_spec=r.off_spec,off_spec_notes=r.off_spec_notes,updated_at=r.updated_at FROM jsonb_populate_record(NULL::inventory_items,$2) r WHERE i.id=$1 RETURNING to_jsonb(i)").bind(id).bind(Value::Object(values)).fetch_one(&mut *self.conn).await?;
                let public_after=self.public_visible(*id,&app.policy).await?;
                self.effect(app,actor,org,*id,MutationKind::InventoryUpdated,changed&&(public_before||public_after),before,now).await?;
                response(updated)
            }
            InventoryAction::Delete(id) => {
                let org=org.ok_or_else(invalid)?;
                let row=self.owned(*id,org).await?; self.mutable(*id,&row,true).await?;
                let public=self.public_visible(*id,&app.policy).await?;
                let before=super::effects::EffectPort::new(self.conn,&[],self.locked).capture_before(&app.policy,&app.config,&[],now).await?;
                sqlx::query("DELETE FROM inventory_items WHERE id=$1").bind(id).execute(&mut *self.conn).await.map_err(|error| {
                    if error.as_database_error().and_then(|e|e.code()).as_deref()==Some("23503") { MarketWriteError::new(MarketErrorCode::InventoryReferenced) } else {error.into()}
                })?;
                self.effect(app,actor,org,*id,MutationKind::InventoryDeleted,public,before,now).await?;
                Ok(Value::Null)
            }
        }
    }
    async fn require_supplier(&mut self,actor:&AuthenticatedActor,org:Uuid)->Result<(),MarketWriteError> {
        if !self.locked.contains(LockClass::Organization,org) || !actor.account().email_verified { return Err(MarketWriteError::new(MarketErrorCode::ExecutionNotAvailable)); }
        let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users u JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1 AND u.organization_id=$2 AND u.role::text='SUPPLIER' AND u.status::text='APPROVED' AND u.email_verified AND NOT u.must_change_password AND u.kyc_status<>'REJECTED' AND (u.kyc_organization_id IS NULL OR u.kyc_organization_id=u.organization_id) AND o.verification_status::text='APPROVED')").bind(actor.user_id()).bind(org).fetch_one(&mut *self.conn).await?;
        if !admitted { return Err(MarketWriteError::new(MarketErrorCode::ExecutionNotAvailable)); }
        Ok(())
    }
    async fn owned(&mut self,id:Uuid,org:Uuid)->Result<Value,MarketWriteError> {
        if !self.locked.contains(LockClass::Inventory,id) { return Err(MarketWriteError::new(MarketErrorCode::MarketSliceChanged)); }
        sqlx::query_scalar("SELECT to_jsonb(i) FROM inventory_items i WHERE id=$1 AND supplier_id=$2").bind(id).bind(org).fetch_optional(&mut *self.conn).await?.ok_or_else(not_found)
    }
    async fn mutable(&mut self,id:Uuid,row:&Value,deleting:bool)->Result<(),MarketWriteError> {
        let reserved=decimal_from_json(&row["reserved_stock_mt"])?;
        if reserved>Decimal::ZERO { return Err(MarketWriteError::new(MarketErrorCode::InventoryReserved)); }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders o WHERE o.inventory_item_id=$1 AND (o.status::text IN ('OPEN','PARTIALLY_FILLED') OR EXISTS(SELECT 1 FROM trades t WHERE (t.bid_order_id=o.id OR t.ask_order_id=o.id) AND t.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED'))))").bind(id).fetch_one(&mut *self.conn).await?;
        if active { return Err(MarketWriteError::new(MarketErrorCode::InventoryInUse)); }
        if deleting {
            let history:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE inventory_item_id=$1)").bind(id).fetch_one(&mut *self.conn).await?;
            if history { return Err(MarketWriteError::new(MarketErrorCode::InventoryReferenced)); }
        }
        Ok(())
    }
    async fn require_port(&mut self,port:&str,policy:&MarketPolicy)->Result<(),MarketWriteError> {
        let name:Option<String>=sqlx::query_scalar("SELECT name FROM ports WHERE id=$1").bind(port).fetch_optional(&mut *self.conn).await?;
        if name.is_none_or(|name| !policy.points.iter().any(|p|p.name==name)) { return Err(refusal(StatusCode::NOT_FOUND,"Port was not found")); }
        Ok(())
    }
    async fn public_visible(&mut self,id:Uuid,policy:&MarketPolicy)->Result<bool,MarketWriteError> {
        let row=sqlx::query("SELECT to_jsonb(i) AS item,p.id AS product_id,p.name AS product_name,p.fuel_type AS product_fuel,p.fuel_grade AS product_grade,p.is_active AS product_active,d.id AS point_id,d.name AS point_name,d.region AS point_region,d.is_active AS point_active,port.is_active AS port_active,o.provenance,o.verification_status::text AS org_status,u.status::text AS owner_status,u.kyc_status,u.organization_id AS owner_org FROM inventory_items i JOIN ports port ON port.id=i.port_id JOIN delivery_points d ON d.name=port.name JOIN products p ON p.name=i.product_name JOIN organizations o ON o.id=i.supplier_id JOIN users u ON u.id=i.owner_user_id WHERE i.id=$1").bind(id).fetch_optional(&mut *self.conn).await?;
        let Some(row)=row else {return Ok(false)};
        let item:Value=row.get("item");
        let product=policy.products.iter().find(|p|p.id==row.get::<Uuid,_>("product_id")&&p.name==row.get::<String,_>("product_name")&&p.fuel_type==row.get::<String,_>("product_fuel")&&p.fuel_grade==row.get::<String,_>("product_grade"));
        let canonical_point=policy.points.iter().any(|p|p.id==row.get::<Uuid,_>("point_id")&&p.name==row.get::<String,_>("point_name")&&p.region==row.get::<String,_>("point_region"));
        let Some(product)=product else {return Ok(false)};
        let provenance:String=row.get("provenance");
        let demo_ids: &[&str] = &[
            "4da7b285-34ee-5443-9406-f96b4ed1a251", "0dbce576-2026-5925-ab66-674d505e98ad",
            "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a", "277491df-cb0d-5f2d-a2cf-5746829c6da6",
            "3b302066-d65c-5c3e-8fcc-70b3da3bcafd", "79609f48-0a3e-560e-a1e1-63d90601d84a",
            "2c4e387e-de22-5adb-ad88-9274ba84ebe1", "612953c7-567a-58b3-bc42-ee817d2bbe74",
            "93ccda09-54b3-53ee-afc0-759d3048161f", "82426590-0963-5486-9b05-f81e97afe6ef",
            "acc3f20a-fe94-4463-9029-a55e35634eb7", "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
            "7cc77115-0a9f-4ec4-8c74-05aa10050111", "d1e43e55-3fb0-4b5e-9f0b-93aa10050222",
        ];
        let org=item["supplier_id"].as_str().and_then(|s|s.parse::<Uuid>().ok());
        let evidence=provenance=="REAL" || (provenance=="DEMO"&&org.is_some_and(|id|demo_ids.contains(&id.to_string().as_str())));
        if !canonical_point || !row.get::<bool,_>("product_active") || !row.get::<bool,_>("point_active") || !row.get::<bool,_>("port_active") || row.get::<String,_>("org_status")!="APPROVED" || row.get::<String,_>("owner_status")!="APPROVED" || row.get::<String,_>("kyc_status")!="APPROVED" || row.get::<Option<Uuid>,_>("owner_org")!=org || !evidence {return Ok(false)}
        if let Some(standard)=&product.specification_standard {
            let present=|field|item[field].as_str().is_some_and(|s|!s.trim_matches([' ','\t','\r','\n','\x0c','\x0b']).is_empty());
            return Ok(item["off_spec"]==false && item["specification_standard"].as_str().is_some_and(|s|s.trim().eq_ignore_ascii_case(standard)) && present("carbon_intensity_method") && item["certification_declared"]==true && present("certification_scheme") && item["msds_available"]==true && !item["carbon_intensity_gco2_mj"].is_null() && present("feedstock") && present("origin"));
        }
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    async fn effect(&mut self,app:&AppState,actor:&AuthenticatedActor,org:Uuid,id:Uuid,kind:MutationKind,public:bool,before:super::effects::BeforeEffects,now:DateTime<Utc>)->Result<(),MarketWriteError> {
        let event=ParticipantEvent { event_type:match kind {MutationKind::InventoryCreated=>"inventory_created",MutationKind::InventoryUpdated=>"inventory_updated",_=>"inventory_deleted"}.into(),aggregate_type:"inventory".into(),aggregate_id:id.to_string(),participant_org_ids:vec![org],fields:ParticipantFields::Inventory{id},public_invalidated:public };
        let descriptor=MutationEffect{kind,actor_id:actor.user_id(),organization_id:org,resource_type:"inventory".into(),resource_id:id,changes:Value::Null,event:Some(event),at:now,audit_context:AuditContext::default()};
        super::effects::EffectPort::new(self.conn,&[],self.locked).inventory_changed(&app.policy,&app.config,before,descriptor).await?;
        Ok(())
    }
    async fn listings(&mut self,org:Option<Uuid>,skip:i64,limit:i64,policy:&MarketPolicy)->Result<Value,MarketWriteError> {
        let Some(org)=org else {return Ok(json!([]))};
        let rows=sqlx::query("SELECT to_jsonb(o) AS item,p.name,p.fuel_type,p.fuel_grade,p.id AS product_id,d.region,(SELECT count(*) FROM trades t WHERE t.ask_order_id=o.id) AS match_count FROM orderbook_orders o LEFT JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.organization_id=$1 AND o.side::text='ASK' ORDER BY o.created_at DESC,o.id OFFSET $2 LIMIT $3").bind(org).bind(skip).bind(limit).fetch_all(&mut *self.conn).await?;
        let mut items=Vec::new();
        for row in rows {
            let item:Value=row.get("item");
            let mut out=Map::new();
            for key in ["id","availability_window","certification_declared","certification_scheme","specification_standard","msds_available","carbon_intensity_method","feedstock","origin","off_spec","off_spec_notes","lng_terms","fame_terms","status","expires_at","delivery_point_id"] {out.insert(key.into(),item[key].clone());}
            for key in ["quantity_mt","price_per_mt_usd","carbon_intensity_gco2_mj"] {out.insert(key.into(),numeric_string(&item[key])?);}
            for (key,column) in [("product_name","name"),("fuel_type","fuel_type"),("fuel_grade","fuel_grade"),("region","region")] {out.insert(key.into(),json!(row.get::<Option<String>,_>(column).unwrap_or_default()));}
            out.insert("market_product".into(),json!(policy.products.iter().find(|p|Some(p.id)==row.get::<Option<Uuid>,_>("product_id")).map(|p|&p.code)));
            out.insert("certifications".into(),item["certifications"].as_array().map(|v|json!(v)).unwrap_or(json!([])));
            out.insert("observed_at".into(),item["created_at"].clone()); out.insert("match_count".into(),json!(row.get::<i64,_>("match_count")));
            let provenance=item["provenance"].as_str().unwrap_or("UNKNOWN");
            let (source,demo,unknown)=match provenance {"REAL"=>("LIVE_ORDER","REAL_ONLY",0),"DEMO"=>("DEMO_SEED","DEMO_ONLY",0),_=>("UNKNOWN","UNKNOWN",1)};
            out.insert("source_kind".into(),json!(source)); out.insert("scope".into(),json!(if item["delivery_point_id"].is_null(){"UNKNOWN"}else{"DELIVERY_POINT"})); out.insert("demo_status".into(),json!(demo)); out.insert("is_demo_listing".into(),json!(provenance=="DEMO")); out.insert("unknown_count".into(),json!(unknown));
            items.push(Value::Object(out));
        }
        Ok(Value::Array(items))
    }
}
fn decimal_from_json(value:&Value)->Result<Decimal,MarketWriteError> { Decimal::from_str_exact(&decimal_text(value)?).map_err(|_|invalid()) }
fn numeric_string(value:&Value)->Result<Value,MarketWriteError> { if value.is_null(){Ok(Value::Null)}else{Ok(json!(decimal_from_json(value)?.to_string()))} }
fn response(row:Value)->Result<Value,MarketWriteError> {
    let mut out=Map::new();
    for key in FIELDS.iter().copied().chain(["id","supplier_id","reserved_stock_mt","updated_at"]) {out.insert(key.into(),row[key].clone());}
    for key in ["current_stock_mt","incoming_stock_mt","reserved_stock_mt","price_per_mt_usd","energy_density_mj_kg","carbon_intensity_gco2_mj"] {out.insert(key.into(),numeric_string(&row[key])?);}
    Ok(Value::Object(out))
}
fn port_error(error:sqlx::Error)->MarketWriteError {
    if error.as_database_error().is_some_and(|e|e.code().as_deref()==Some("23503")&&e.constraint()==Some("inventory_items_port_id_fkey")) {refusal(StatusCode::NOT_FOUND,"Port was not found")} else {error.into()}
}
