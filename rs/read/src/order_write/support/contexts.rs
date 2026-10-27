//! Actor-bound context lifecycle and the organization support view.
use super::*;

#[derive(Clone, Deserialize)]
pub struct ContextCreate {
    pub organization_id: Uuid,
    pub support_reference: String,
    #[serde(default)]
    pub confirm_replacement: bool,
}
impl ContextCreate {
    pub fn parse(value: Value) -> Result<Self, MarketWriteError> {
        let mut body: Self = serde_json::from_value(value).map_err(|_| validation())?;
        if !(3..=200).contains(&body.support_reference.chars().count()) {
            return Err(validation());
        }
        body.support_reference = body.support_reference.trim_matches(is_python_whitespace).into();
        if body.support_reference.is_empty() { return Err(validation()); }
        Ok(body)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContextStatus { Active, Exited, Expired, Revoked }
impl ContextStatus {
    fn text(self) -> &'static str {
        match self { Self::Active => "ACTIVE", Self::Exited => "EXITED", Self::Expired => "EXPIRED", Self::Revoked => "REVOKED" }
    }
}

#[derive(Serialize)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub domain: Option<String>,
    #[serde(rename = "type")]
    pub organization_type: String,
}
#[derive(Serialize)]
pub struct Principal { pub id: Uuid, pub email: String, pub name: String }
impl Principal {
    pub fn new(id: Uuid, email: String, first: Option<String>, last: Option<String>, trim: bool) -> Self {
        let name = [first,last].into_iter().flatten().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
        let name = if trim { name.trim_matches(is_python_whitespace) } else { &name };
        Self { id, name: if name.is_empty() { email.clone() } else { name.into() }, email }
    }
}

pub struct ContextRecord {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub organization_id: Uuid,
    pub support_reference: String,
    pub scope: String,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: ContextStatus,
    pub version: i32,
}
impl ContextRecord {
    fn from_row(row: &PgRow) -> Result<Self, MarketWriteError> {
        let status: String = row.try_get("status")?;
        Ok(Self {
            id: row.try_get("id")?, actor_user_id: row.try_get("actor_user_id")?, organization_id: row.try_get("organization_id")?,
            support_reference: row.try_get("support_reference")?, scope: row.try_get("scope")?, started_at: row.try_get("started_at")?, expires_at: row.try_get("expires_at")?, ended_at: row.try_get("ended_at")?,
            status: serde_json::from_value(json!(status)).map_err(|_| projection_fault())?, version: row.try_get("version")?,
        })
    }
    pub fn response(&self, organization: Option<&Organization>, actor: Option<&Principal>) -> Result<Value, MarketWriteError> {
        let (Some(organization),Some(actor)) = (organization,actor) else {
            return Err(refusal(MarketErrorCode::SupportIdentitiesUnavailable));
        };
        if self.scope != "ASSISTED_ORDER_ENTRY" || self.version < 1 { return Err(projection_fault()); }
        Ok(json!({"id":self.id,"actor_user_id":self.actor_user_id,"organization_id":self.organization_id,"organization":organization,"actor":actor,"support_reference":self.support_reference,"scope":self.scope,"started_at":pydantic_datetime(self.started_at),"expires_at":pydantic_datetime(self.expires_at),"ended_at":self.ended_at.map(pydantic_datetime),"status":self.status,"version":self.version}))
    }
}

pub(super) async fn preview(conn: &mut PgConnection, actor: Uuid, action: &SupportAction) -> Result<MarketPreview, MarketWriteError> {
    // The actor prefix serializes all assignment and context writers. Include
    // assignment identities even when a grant was renewed since preview.
    let assignments: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM staff_capability_assignments WHERE user_id=$1 AND capability IN ('MARKET_SUPPORT_AUTHORIZATIONS','MARKET_SUPPORT_LISTINGS') ORDER BY id")
        .bind(actor).fetch_all(&mut *conn).await?;
    let contexts: Vec<Uuid> = match action {
        SupportAction::ContextStart(_) | SupportAction::ContextActive => sqlx::query_scalar(ACTIVE_CONTEXTS).bind(actor).fetch_all(&mut *conn).await?,
        SupportAction::ContextGet(id) | SupportAction::ContextExit(id) => sqlx::query_scalar("SELECT id FROM market_support_contexts WHERE id=$1 AND actor_user_id=$2").bind(id).bind(actor).fetch_all(&mut *conn).await?,
        _ => Vec::new(),
    };
    let operation = match action { SupportAction::ContextStart(_) => MarketOperation::ContextStart, SupportAction::ContextActive | SupportAction::ContextGet(_) | SupportAction::ContextExit(_) => MarketOperation::ContextExit, _ => SUPPORT_READ_LOCKS };
    let parties = PartyLockPreview::new([],[],actor,json!({"assignments":assignments,"contexts":contexts}));
    let mut locks = OperationLockPlan::new(operation);
    locks.parties(&parties)?.resources(LockClass::Capability,assignments)?;
    if matches!(action,SupportAction::ContextStart(_) | SupportAction::ContextActive | SupportAction::ContextGet(_) | SupportAction::ContextExit(_)) {
        locks.resources(LockClass::Context,contexts)?;
    }
    Ok(MarketPreview {parties,locks})
}

const CONTEXT_COLUMNS: &str = "id,actor_user_id,organization_id,support_reference,scope,started_at,expires_at,ended_at,status,version";

impl SupportPort<'_> {
    async fn organization_identity(&mut self, id: Uuid) -> Result<Option<Organization>, MarketWriteError> {
        let row = sqlx::query("SELECT id,name,domain,type::text AS type FROM organizations WHERE id=$1").bind(id).fetch_optional(&mut *self.conn).await?;
        row.map(|row| Ok(Organization { id: row.try_get("id")?,name:row.try_get("name")?,domain:row.try_get("domain")?,organization_type:row.try_get("type")? })).transpose()
    }
    async fn context_response(&mut self, row: &ContextRecord) -> Result<Value, MarketWriteError> {
        let organization = self.organization_identity(row.organization_id).await?;
        let actor = sqlx::query("SELECT id,email,first_name,last_name FROM users WHERE id=$1").bind(row.actor_user_id).fetch_optional(&mut *self.conn).await?;
        let actor = actor.map(|row| -> Result<Principal,MarketWriteError> {Ok(Principal::new(row.try_get("id")?,row.try_get("email")?,row.try_get("first_name")?,row.try_get("last_name")?,false))}).transpose()?;
        row.response(organization.as_ref(),actor.as_ref())
    }
    async fn owned_context(&mut self, id: Uuid, actor: Uuid) -> Result<ContextRecord,MarketWriteError> {
        let sql = format!("SELECT {CONTEXT_COLUMNS} FROM market_support_contexts WHERE id=$1 AND actor_user_id=$2");
        let row = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).bind(actor).fetch_optional(&mut *self.conn).await?.ok_or_else(||refusal(MarketErrorCode::SupportDisabled))?;
        if !self.locked.contains(LockClass::Context,id) {return Err(refusal(MarketErrorCode::MarketSliceChanged));}
        ContextRecord::from_row(&row)
    }
    async fn active_rows(&mut self, actor: Uuid) -> Result<Vec<ContextRecord>,MarketWriteError> {
        let sql = format!("SELECT {CONTEXT_COLUMNS} FROM market_support_contexts WHERE actor_user_id=$1 AND status='ACTIVE' ORDER BY id");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(actor).fetch_all(&mut *self.conn).await?;
        rows.iter().map(|row| {
            let record = ContextRecord::from_row(row)?;
            if !self.locked.contains(LockClass::Context,record.id) {return Err(refusal(MarketErrorCode::MarketSliceChanged));}
            Ok(record)
        }).collect()
    }
    async fn end_context(&mut self, row: &mut ContextRecord, status: ContextStatus, context: &ActionContext<'_>) -> Result<(),MarketWriteError> {
        if !self.locked.contains(LockClass::Context,row.id) {return Err(refusal(MarketErrorCode::MarketSliceChanged));}
        let updated: Option<i32> = sqlx::query_scalar("UPDATE market_support_contexts SET status=$2,ended_at=$3,version=version+1 WHERE id=$1 AND actor_user_id=$4 AND status='ACTIVE' AND version=$5 RETURNING version")
            .bind(row.id).bind(status.text()).bind(context.now).bind(context.actor).bind(row.version).fetch_optional(&mut *self.conn).await?;
        row.version = updated.ok_or_else(||refusal(MarketErrorCode::MarketSliceChanged))?;
        row.status = status;
        row.ended_at = Some(context.now);
        Ok(())
    }
    pub(super) async fn entry(&mut self, id: Uuid) -> Result<Outcome,MarketWriteError> {
        self.require_real_organization(id).await?;
        let organization = self.organization_identity(id).await?.ok_or_else(||refusal(MarketErrorCode::SupportOrganizationNotFound))?;
        Ok(Outcome::read(json!({"organization":organization,"eligible":true,"reason":null})))
    }
    pub(super) async fn start_context(&mut self, body: &ContextCreate, context: &ActionContext<'_>) -> Result<Outcome,MarketWriteError> {
        self.require_real_organization(body.organization_id).await?;
        let mut effects = Vec::new();
        for mut active in self.active_rows(context.actor).await? {
            if active.expires_at <= context.now {
                self.end_context(&mut active,ContextStatus::Expired,context).await?;
            } else if active.organization_id == body.organization_id {
                return Ok(Outcome::read(self.context_response(&active).await?));
            } else {
                if !body.confirm_replacement { return Err(refusal(MarketErrorCode::ContextReplacementRequired)); }
                self.end_context(&mut active,ContextStatus::Exited,context).await?;
                effects.push(descriptor(MutationKind::SupportContextExited,active.id,"market_support_context",json!({"status":"EXITED","replacement":true}),context));
            }
        }
        let id = Uuid::new_v4();
        let expires = context.now + chrono::Duration::minutes(context.settings.context_ttl_minutes());
        let sql = format!("INSERT INTO market_support_contexts (id,actor_user_id,organization_id,accountable_user_id,support_reference,scope,started_at,expires_at,status,version) VALUES ($1,$2,$3,$2,$4,'ASSISTED_ORDER_ENTRY',$5,$6,'ACTIVE',1) RETURNING {CONTEXT_COLUMNS}");
        let stored = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).bind(context.actor).bind(body.organization_id).bind(&body.support_reference).bind(context.now).bind(expires).fetch_one(&mut *self.conn).await?;
        let row = ContextRecord::from_row(&stored)?;
        let response = self.context_response(&row).await?;
        effects.push(descriptor(MutationKind::SupportContextStarted,id,"market_support_context",json!({"organization_id":body.organization_id,"support_reference":body.support_reference,"scope":"ASSISTED_ORDER_ENTRY","expires_at":pydantic_datetime(expires)}),context));
        Ok(Outcome {response,effects})
    }
    pub(super) async fn active_context(&mut self, context: &ActionContext<'_>) -> Result<Outcome,MarketWriteError> {
        let Some(mut row) = self.active_rows(context.actor).await?.into_iter().next() else {return Ok(Outcome::read(Value::Null));};
        if row.expires_at <= context.now {
            self.end_context(&mut row,ContextStatus::Expired,context).await?;
            return Ok(Outcome::read(Value::Null));
        }
        Ok(Outcome::read(self.context_response(&row).await?))
    }
    pub(super) async fn get_context(&mut self, id: Uuid, context: &ActionContext<'_>) -> Result<Outcome,MarketWriteError> {
        let mut row = self.owned_context(id,context.actor).await?;
        if row.status == ContextStatus::Active && row.expires_at <= context.now { self.end_context(&mut row,ContextStatus::Expired,context).await?; }
        Ok(Outcome::read(self.context_response(&row).await?))
    }
    pub(super) async fn exit_context(&mut self, id: Uuid, context: &ActionContext<'_>) -> Result<Outcome,MarketWriteError> {
        let mut row = self.owned_context(id,context.actor).await?;
        let mut effects = Vec::new();
        if row.status == ContextStatus::Active {
            let status = if row.expires_at <= context.now {ContextStatus::Expired} else {ContextStatus::Exited};
            self.end_context(&mut row,status,context).await?;
            effects.push(descriptor(MutationKind::SupportContextExited,id,"market_support_context",json!({"status":status}),context));
        }
        Ok(Outcome { response:self.context_response(&row).await?,effects })
    }
    pub(super) async fn composite(&mut self, id: Uuid, context: &ActionContext<'_>) -> Result<Outcome,MarketWriteError> {
        self.require_real_organization(id).await?;
        let organization = self.organization_identity(id).await?.ok_or_else(||refusal(MarketErrorCode::SupportOrganizationNotFound))?;
        let eligibility = crate::market::visibility::eligibility_sets_sql("$3");
        let sql = format!("WITH support_organization AS (SELECT $1::uuid AS id) {eligibility} SELECT u.id,u.email,u.first_name,u.last_name FROM users u JOIN eligible_owners e ON e.user_id=u.id AND e.organization_id=u.organization_id WHERE u.organization_id=$1 AND u.role='SUPPLIER' ORDER BY u.email");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).bind(context.now).bind(context.config.kyc_execution_gate_enabled).fetch_all(&mut *self.conn).await?;
        let principals = rows.iter().map(|row| -> Result<Principal,MarketWriteError> {Ok(Principal::new(row.try_get("id")?,row.try_get("email")?,row.try_get("first_name")?,row.try_get("last_name")?,true))}).collect::<Result<Vec<_>,_>>()?;
        let page = Page {skip:0,limit:100,query:None};
        let authorizations = self.authorizations(id,&page,context.now).await?;
        let listings = self.listings(id,&page,context).await?;
        Ok(Outcome::read(json!({"organization":organization,"eligible_principals":principals,"authorizations":authorizations["items"],"listings":listings["items"]})))
    }
}
