//! Administrator capability lifecycle and bounded support workspace reads.
use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{AuthenticatedActor, MarketErrorCode, MarketWriteError};
use super::effects::{AuditContext, MutationEffect, MutationKind};
use super::locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview};
use super::transaction::{BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview};
use crate::{Config, MarketPolicy};
use crate::auth::{VerifiedAccess, cutoff_refusal, TokenIssuance};

pub async fn run_support(pool:sqlx::PgPool,permits:Arc<tokio::sync::Semaphore>,command:SupportCommand)->Result<Value,MarketWriteError> {
    let access=VerifiedAccess{subject:command.access.subject,issued_at:command.access.issued_at,issuance_out_of_range:command.access.issuance_out_of_range};
    super::transaction::run_market_command(pool,permits,access,command,super::AdmissionClass::Execution).await
}

#[derive(Clone)]
pub struct SupportSettings {
    pub enabled: bool,
    bootstrap: BTreeSet<Uuid>,
}
impl SupportSettings {
    pub fn new(enabled: bool, ids: &str) -> Self {
        Self { enabled, bootstrap: ids.split(',').filter_map(|s|s.trim().parse().ok()).collect() }
    }
    pub fn is_bootstrap(&self, actor: Uuid) -> bool { self.bootstrap.contains(&actor) }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Capability {
    #[serde(rename = "MARKET_SUPPORT_LISTINGS")]
    Listings,
    #[serde(rename = "MARKET_SUPPORT_AUTHORIZATIONS")]
    Authorizations,
}
impl Capability {
    fn text(self) -> &'static str { match self { Self::Listings=>"MARKET_SUPPORT_LISTINGS",Self::Authorizations=>"MARKET_SUPPORT_AUTHORIZATIONS" } }
}
#[derive(Clone, Deserialize)]
pub struct CapabilityGrant {
    pub user_id: Uuid,
    pub capability: Capability,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
}
impl CapabilityGrant {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self,MarketWriteError> {
        let mut body: Self = serde_json::from_value(value).map_err(|_|validation())?;
        body.reason=reason(&body.reason)?;
        if body.expires_at.is_some_and(|at|at<=now) { return Err(validation()); }
        Ok(body)
    }
}
pub fn reason(raw: &str) -> Result<String,MarketWriteError> {
    let value=raw.trim();
    if !(3..=500).contains(&value.chars().count()) { return Err(validation()); }
    Ok(value.into())
}
#[derive(Clone, Debug)]
pub struct Page { pub skip:i64, pub limit:i64, pub query:Option<String> }
impl Page {
    pub fn parse(raw:Option<&str>) -> Result<Self,MarketWriteError> {
        let mut page=Self{skip:0,limit:25,query:None};
        for (key,value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
            match key.as_ref() {
                "skip"=>page.skip=value.parse().map_err(|_|validation())?,
                "limit"=>page.limit=value.parse().map_err(|_|validation())?,
                "query"=> { if value.chars().count()>100 {return Err(validation());} page.query=Some(value.trim().into()); },
                _=>{}
            }
        }
        if page.skip<0 || !(1..=100).contains(&page.limit) {return Err(validation());}
        Ok(page)
    }
}
fn validation()->MarketWriteError {MarketWriteError::new(MarketErrorCode::ValidationFailed)}
fn refusal(code:MarketErrorCode)->MarketWriteError {MarketWriteError::new(code)}

#[derive(Clone)]
pub enum SupportAction {
    Capabilities,
    Grant(CapabilityGrant),
    Revoke{assignment:Uuid,reason:String},
    Organizations(Page),
    Authorizations{organization:Uuid,page:Page},
    Listings{organization:Uuid,page:Page},
    Retired,
}
impl SupportAction {
    fn required(&self)->Option<Capability> { match self {Self::Authorizations{..}=>Some(Capability::Authorizations),Self::Listings{..}=>Some(Capability::Listings),_=>None} }
    fn bootstrap(&self)->bool {matches!(self,Self::Grant(_)|Self::Revoke{..})}
}
pub struct SupportCommand {
    pub action:SupportAction,
    pub settings:SupportSettings,
    pub access:Arc<VerifiedAccess>,
    pub policy:Arc<MarketPolicy>,
    pub config:Arc<Config>,
    pub audit:AuditContext,
}
impl SupportCommand {
    async fn authorize(&self,conn:&mut PgConnection,actor:Uuid,now:DateTime<Utc>)->Result<Vec<Uuid>,MarketWriteError> {
        let row=sqlx::query("SELECT role::text AS role,status::text AS status,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1").bind(actor).fetch_optional(&mut *conn).await?.ok_or_else(||refusal(MarketErrorCode::AuthCredentialsInvalid))?;
        let state=crate::auth::AccountState{exists:true,status:row.try_get("status")?,must_change_password:row.try_get("must_change_password")?,password_changed_at:row.try_get::<Option<DateTime<Utc>>,_>("password_changed_at")?.map(|t|t.timestamp_micros()),authentication_revoked_at:row.try_get::<Option<DateTime<Utc>>,_>("authentication_revoked_at")?.map(|t|t.timestamp_micros())};
        if let Some(error)=cutoff_refusal(TokenIssuance{issued_at:self.access.issued_at,out_of_range:self.access.issuance_out_of_range},&state) {
            return Err(refusal(match error {crate::auth::AuthError::PasswordChanged=>MarketErrorCode::AuthPasswordChanged,_=>MarketErrorCode::AuthSessionRevoked}));
        }
        if !self.settings.enabled {return Err(refusal(MarketErrorCode::SupportDisabled));}
        if row.try_get::<String,_>("role")? != "ADMIN" || state.status!="APPROVED" || state.must_change_password {return Err(refusal(MarketErrorCode::SupportForbidden));}
        if self.action.bootstrap() && !self.settings.is_bootstrap(actor) {return Err(refusal(MarketErrorCode::SupportForbidden));}
        let rows=sqlx::query("SELECT id,capability FROM staff_capability_assignments WHERE user_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2) ORDER BY id").bind(actor).bind(now).fetch_all(&mut *conn).await?;
        if self.action.required().is_some_and(|cap|!rows.iter().any(|r|r.get::<String,_>("capability")==cap.text())) || (matches!(self.action,SupportAction::Organizations(_)) && rows.is_empty()) {
            return Err(refusal(MarketErrorCode::SupportCapabilityRequired));
        }
        Ok(rows.iter().map(|r|r.get("id")).collect())
    }
}
impl MarketCommand for SupportCommand {
    type Output=Value;
    fn preview<'a>(&'a self,conn:&'a mut PgConnection,actor:Uuid)->BoxFuture<'a,Result<MarketPreview,MarketAttemptError>> {
        Box::pin(async move {
            self.authorize(conn,actor,Utc::now()).await?;
            let mut users=vec![actor];
            let mut assignments=Vec::new();
            let mut contexts=Vec::new();
            let mut fingerprint=Value::Null;
            let operation=match &self.action {
                SupportAction::Grant(body)=> {
                    users.push(body.user_id);
                    assignments=sqlx::query_scalar("SELECT id FROM staff_capability_assignments WHERE user_id=$1 AND capability=$2 ORDER BY id").bind(body.user_id).bind(body.capability.text()).fetch_all(&mut *conn).await?;
                    MarketOperation::CapabilityGrant
                },
                SupportAction::Revoke{assignment,..}=> {
                    let target:Option<Uuid>=sqlx::query_scalar("SELECT user_id FROM staff_capability_assignments WHERE id=$1").bind(assignment).fetch_optional(&mut *conn).await?;
                    if let Some(target)=target {
                        users.push(target); assignments.push(*assignment);
                        contexts=sqlx::query_scalar("SELECT id FROM market_support_contexts WHERE actor_user_id=$1 AND status='ACTIVE' ORDER BY id").bind(target).fetch_all(&mut *conn).await?;
                    }
                    fingerprint=json!({"target":target,"contexts":contexts});
                    MarketOperation::CapabilityRevoke
                },
                _=>MarketOperation::CapabilityGrant,
            };
            let parties=PartyLockPreview::new([],users,actor,fingerprint);
            let mut locks=OperationLockPlan::new(operation);
            locks.parties(&parties)?;
            locks.resources(LockClass::Capability,assignments)?;
            if operation==MarketOperation::CapabilityRevoke {locks.resources(LockClass::Context,contexts)?;}
            Ok(MarketPreview{parties,locks})
        })
    }
    fn execute<'a>(&'a self,tx:&'a mut LockedMarketTransaction<'_>,actor:&'a AuthenticatedActor)->BoxFuture<'a,Result<Value,MarketAttemptError>> {
        Box::pin(async move {
            let now=Utc::now();
            let mut port=tx.support()?;
            self.authorize(port.conn,actor.user_id(),now).await?;
            let (value,effects)=port.execute(&self.action,actor.user_id(),now,&self.audit).await?;
            drop(port);
            if !effects.is_empty() {
                let mut port=tx.effects()?;
                let mut set=port.capture_before(&self.policy,&self.config,&[],now).await?.stage();
                for effect in effects {set.push(effect)?;}
                port.finish(&self.policy,&self.config,set).await?;
            }
            Ok(value)
        })
    }
}

pub(super) struct SupportPort<'a> { conn:&'a mut PgConnection, locked:&'a Locked }
impl<'a> SupportPort<'a> {
    pub(super) fn new(conn:&'a mut PgConnection,locked:&'a Locked)->Self {Self{conn,locked}}
    async fn execute(&mut self,action:&SupportAction,actor:Uuid,now:DateTime<Utc>,audit:&AuditContext)->Result<(Value,Vec<MutationEffect>),MarketWriteError> {
        let mut effects=Vec::new();
        let result=match action {
            SupportAction::Retired=>return Err(refusal(MarketErrorCode::MarketSupportLegacyMutationRetired)),
            SupportAction::Capabilities=>json!(sqlx::query_scalar::<_,String>("SELECT capability FROM staff_capability_assignments WHERE user_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2) ORDER BY capability").bind(actor).bind(now).fetch_all(&mut *self.conn).await?),
            SupportAction::Grant(body)=> {
                if !self.locked.contains(LockClass::User,body.user_id) {return Err(validation());}
                let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role='ADMIN' AND status='APPROVED')").bind(body.user_id).fetch_one(&mut *self.conn).await?;
                if !eligible {return Err(refusal(MarketErrorCode::SupportTargetInvalid));}
                if body.expires_at.is_some_and(|at|at<=now) {return Err(validation());}
                let row=sqlx::query("INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id,granted_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (user_id,capability) DO UPDATE SET reason=EXCLUDED.reason,granted_by_user_id=EXCLUDED.granted_by_user_id,granted_at=EXCLUDED.granted_at,expires_at=EXCLUDED.expires_at,revoked_at=NULL,revoked_by_user_id=NULL,revocation_reason=NULL RETURNING to_jsonb(staff_capability_assignments) AS value")
                    .bind(Uuid::new_v4()).bind(body.user_id).bind(body.capability.text()).bind(&body.reason).bind(actor).bind(now).bind(body.expires_at).fetch_one(&mut *self.conn).await?;
                let value:Value=row.try_get("value")?;
                effects.push(descriptor(MutationKind::SupportCapabilityGranted,actor,value["id"].as_str().ok_or_else(validation)?.parse().map_err(|_|validation())?,"staff_capability_assignment",json!({"target_user_id":body.user_id,"capability":body.capability,"reason":body.reason}),now,audit));
                value
            },
            SupportAction::Revoke{assignment,reason}=> {
                if !self.locked.contains(LockClass::Capability,*assignment) {return Err(refusal(MarketErrorCode::SupportAssignmentNotFound));}
                let row=sqlx::query("UPDATE staff_capability_assignments SET revoked_at=COALESCE(revoked_at,$2),revoked_by_user_id=CASE WHEN revoked_at IS NULL THEN $3 ELSE revoked_by_user_id END,revocation_reason=CASE WHEN revoked_at IS NULL THEN $4 ELSE revocation_reason END WHERE id=$1 RETURNING to_jsonb(staff_capability_assignments) AS value").bind(assignment).bind(now).bind(actor).bind(reason).fetch_one(&mut *self.conn).await?;
                let value:Value=row.try_get("value")?;
                let target:Uuid=serde_json::from_value(value["user_id"].clone()).map_err(|_|validation())?;
                effects.push(descriptor(MutationKind::SupportCapabilityRevoked,actor,*assignment,"staff_capability_assignment",json!({"target_user_id":target,"capability":value["capability"],"reason":reason}),now,audit));
                let contexts:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM market_support_contexts WHERE actor_user_id=$1 AND status='ACTIVE' ORDER BY id").bind(target).fetch_all(&mut *self.conn).await?;
                for context in contexts {
                    if !self.locked.contains(LockClass::Context,context) {return Err(refusal(MarketErrorCode::MarketSliceChanged));}
                    sqlx::query("UPDATE market_support_contexts SET status='REVOKED',ended_at=$2,version=version+1 WHERE id=$1 AND status='ACTIVE'").bind(context).bind(now).execute(&mut *self.conn).await?;
                    effects.push(descriptor(MutationKind::SupportContextRevoked,actor,context,"market_support_context",json!({"status":"REVOKED","reason":reason,"capability_assignment_id":assignment}),now,audit));
                }
                value
            },
            SupportAction::Organizations(page)=> {
                let pattern=page.query.as_ref().filter(|q|!q.is_empty()).map(|q|format!("%{q}%"));
                let total:i64=sqlx::query_scalar("SELECT count(*) FROM organizations WHERE verification_status='APPROVED' AND provenance='REAL' AND ($1::text IS NULL OR name ILIKE $1 OR domain ILIKE $1)").bind(&pattern).fetch_one(&mut *self.conn).await?;
                let items:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'domain',domain,'type',type) FROM organizations WHERE verification_status='APPROVED' AND provenance='REAL' AND ($1::text IS NULL OR name ILIKE $1 OR domain ILIKE $1) ORDER BY name,id OFFSET $2 LIMIT $3").bind(&pattern).bind(page.skip).bind(page.limit).fetch_all(&mut *self.conn).await?;
                json!({"items":items,"total":total,"skip":page.skip,"limit":page.limit})
            },
            SupportAction::Authorizations{organization,page}|SupportAction::Listings{organization,page}=> {
                self.organization(*organization).await?;
                let listings=matches!(action,SupportAction::Listings{..});
                self.page(*organization,page,listings,now).await?
            },
        };
        Ok((result,effects))
    }
    async fn organization(&mut self,id:Uuid)->Result<(),MarketWriteError> {
        let row=sqlx::query("SELECT verification_status,provenance::text AS provenance FROM organizations WHERE id=$1").bind(id).fetch_optional(&mut *self.conn).await?.ok_or_else(||refusal(MarketErrorCode::SupportOrganizationNotFound))?;
        if row.try_get::<String,_>("verification_status")?!="APPROVED" {return Err(refusal(MarketErrorCode::SupportOrganizationNotApproved));}
        if row.try_get::<String,_>("provenance")?!="REAL" {return Err(refusal(MarketErrorCode::SupportOrganizationNotReal));}
        Ok(())
    }
    async fn page(&mut self,organization:Uuid,page:&Page,listings:bool,now:DateTime<Utc>)->Result<Value,MarketWriteError> {
        if listings { self.listings(organization,page,now).await } else {self.authorizations(organization,page).await}
    }
    async fn authorizations(&mut self,organization:Uuid,page:&Page)->Result<Value,MarketWriteError> {
        let total:i64=sqlx::query_scalar("SELECT count(*) FROM market_support_authorizations WHERE organization_id=$1").bind(organization).fetch_one(&mut *self.conn).await?;
        let rows=sqlx::query("SELECT to_jsonb(a) AS value,quantity_mt::text AS quantity,price_per_mt_usd::text AS price,carbon_intensity_gco2_mj::text AS ci FROM market_support_authorizations a WHERE organization_id=$1 ORDER BY created_at DESC,id DESC OFFSET $2 LIMIT $3").bind(organization).bind(page.skip).bind(page.limit).fetch_all(&mut *self.conn).await?;
        let mut items=Vec::new();
        for row in rows {
            let stored:Value=row.try_get("value")?;
            let mut out=serde_json::Map::new();
            for field in ["id","organization_id","accountable_user_id","status","fame_terms","lng_terms","product_id","delivery_point_id","availability_window","authorization_expires_at","order_expires_at","terms_digest","evidence_reference","evidence_sha256","commercial_consent_version","commercial_consent_reference","support_case_reference","instruction_at","acknowledge_exact_terms","acknowledge_executable_standing_order","created_by_actor_user_id","created_at","consumed_at","revoked_at","revoked_by_actor_user_id","revocation_reason"] {out.insert(field.into(),stored[field].clone());}
            out.insert("quantity_mt".into(),json!(row.try_get::<String,_>("quantity")?));
            out.insert("price_per_mt_usd".into(),json!(row.try_get::<String,_>("price")?));
            let mut order=json!({"side":stored["order_side"],"expires_at":stored["order_expires_at"],"port_id":null,"vessel_id":null,"is_anonymous":stored["is_anonymous"],"carbon_intensity_gco2_mj":row.try_get::<Option<String>,_>("ci")?});
            for field in ["product_id","delivery_point_id","availability_window","certifications","certification_declared","certification_scheme","specification_standard","msds_available","carbon_intensity_method","feedstock","origin","off_spec","off_spec_notes","fame_terms","lng_terms"] {order[field]=stored[field].clone();}
            order["quantity_mt"]=out["quantity_mt"].clone();order["price_per_mt_usd"]=out["price_per_mt_usd"].clone();
            out.insert("order".into(),order);items.push(Value::Object(out));
        }
        Ok(json!({"items":items,"total":total,"skip":page.skip,"limit":page.limit}))
    }
    async fn listings(&mut self,organization:Uuid,page:&Page,now:DateTime<Utc>)->Result<Value,MarketWriteError> {
        let total:i64=sqlx::query_scalar("SELECT count(*) FROM orderbook_orders WHERE organization_id=$1 AND creation_method='MARKET_SUPPORT'").bind(organization).fetch_one(&mut *self.conn).await?;
        let digest_sql=crate::market::visibility::private_digest_input_sql("o");
        let sql=format!("SELECT {digest_sql} AS private_digest_input,o.provenance::text AS provenance,o.side::text AS side,o.price_per_mt_usd,o.expires_at,o.created_at,COALESCE(p.name,'') AS product_name,COALESCE(p.fuel_type,'') AS product_fuel_type,COALESCE(p.fuel_grade,'Conventional') AS product_fuel_grade,COALESCE(dp.name,'') AS delivery_point_name,COALESCE(dp.region,'') AS delivery_point_region,COALESCE(org.supplier_tier,'INDEPENDENT') AS tier_label,o.owner_user_id,o.created_by_actor_user_id,o.support_authorization_id,o.creation_method,o.version,o.id,o.product_id,o.delivery_point_id,o.availability_window FROM orderbook_orders o LEFT JOIN products p ON p.id=o.product_id LEFT JOIN delivery_points dp ON dp.id=o.delivery_point_id JOIN organizations org ON org.id=o.organization_id WHERE o.organization_id=$1 AND o.creation_method='MARKET_SUPPORT' ORDER BY o.created_at DESC,o.id DESC OFFSET $2 LIMIT $3");
        let rows=sqlx::query(sqlx::AssertSqlSafe(sql)).bind(organization).bind(page.skip).bind(page.limit).fetch_all(&mut *self.conn).await?;
        let policy:MarketPolicy=serde_json::from_str(include_str!("../../../../deploy/rust/market-policy.json")).map_err(|_|validation())?;
        let mut items=Vec::new();
        for row in rows {
            let mut projection=crate::market::order_projection::PublicOrder::from_row(&row).map_err(|_|validation())?;
            let product_id:Uuid=row.try_get("product_id")?;
            projection.labels["market_product"]=json!(policy.products.iter().find(|p|p.id==product_id).map(|p|p.code.as_str()));
            let order=projection.public(false,None).map_err(|_|validation())?;
            let id:Uuid=row.try_get("id")?;
            let version:i32=row.try_get("version")?;
            let etag=format!("W/\"order-{id}-v{version}\"");
            items.push(json!({"order":order,"accountable_user_id":row.try_get::<Option<Uuid>,_>("owner_user_id")?,"created_by_actor_user_id":row.try_get::<Option<Uuid>,_>("created_by_actor_user_id")?,"creation_method":row.try_get::<String,_>("creation_method")?,"support_authorization_id":row.try_get::<Option<Uuid>,_>("support_authorization_id")?,"version":version,"etag":etag}));
        }
        let _=now;
        Ok(json!({"items":items,"total":total,"skip":page.skip,"limit":page.limit}))
    }
}
fn descriptor(kind:MutationKind,actor:Uuid,id:Uuid,resource:&str,changes:Value,at:DateTime<Utc>,audit:&AuditContext)->MutationEffect {
    MutationEffect{kind,actor_id:actor,organization_id:Uuid::nil(),resource_type:resource.into(),resource_id:id,changes,event:None,at,audit_context:audit.clone()}
}
