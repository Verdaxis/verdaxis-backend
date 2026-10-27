//! Organization-owned order creation with inline matching and order-row replay.
use std::{collections::BTreeSet, sync::Arc};

use axum::http::StatusCode;
use chrono::{DateTime, Datelike, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::{
    AdmissionClass, AuthenticatedActor, CommittedCommand, MarketErrorCode, MarketWriteError,
    authority::{AccountState, ActorRole},
    effects::{AuditContext, EffectSlice, MutationEffect, MutationKind, NewOrderEffect},
    locks::{LockClass, Locked, MarketOperation, OperationLockPlan, PartyLockPreview},
    matching::{
        CreationMethod, ExecutionTerms, MatchBound, OrderRow, OrderStatus, OwnerAdmission, Side,
    },
    outbox::{OrderActivity, ParticipantEvent, ParticipantFields},
    transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
        run_market_command,
    },
};
use crate::{
    AppState, MarketPolicy,
    routes::market_write::order_create::{CreateInput, CreateSettings},
};

fn internal() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::DatabaseError)
}
fn refusal(status: StatusCode, message: &'static str) -> MarketWriteError {
    super::orders::refusal(MarketErrorCode::ValidationFailed, status, message)
}
fn bad(message: &'static str) -> MarketWriteError {
    refusal(StatusCode::BAD_REQUEST, message)
}
fn context_error(status: StatusCode) -> MarketWriteError {
    super::orders::refusal(
        MarketErrorCode::MarketSupportContextInvalid,
        status,
        "The support context is not valid",
    )
}

pub(crate) struct CreateRequest {
    pub input: CreateInput,
    pub context: Option<Uuid>,
    pub raw_key: Option<String>,
    pub audit: AuditContext,
    pub settings: CreateSettings,
    pub now: DateTime<Utc>,
}

pub(crate) async fn dispatch(
    app: AppState,
    access: crate::auth::VerifiedAccess,
    request: CreateRequest,
) -> Result<CommittedCommand, MarketWriteError> {
    let CreateRequest {
        input,
        context,
        raw_key,
        audit,
        settings,
        now,
    } = request;
    let access = Arc::new(access);
    if let Some(context) = context {
        if !settings.support_enabled {
            return Err(context_error(StatusCode::NOT_FOUND));
        }
        // The existing context owner commits expiry separately from order work.
        let response = super::support::run_support(
            app.pools.read.clone(),
            app.reads.clone(),
            super::support::SupportCommand {
                request: super::support::SupportRequest::ContextGet {
                    context: context.to_string(),
                },
                settings: super::support::SupportSettings::new(true, ""),
                access: access.clone(),
                policy: app.policy.clone(),
                config: app.config.clone(),
                audit: audit.clone(),
            },
        )
        .await
        .map_err(|error| {
            if matches!(
                error.code,
                MarketErrorCode::SupportForbidden
                    | MarketErrorCode::SupportCapabilityRequired
                    | MarketErrorCode::NotFound
            ) {
                context_error(StatusCode::NOT_FOUND)
            } else {
                error
            }
        })?;
        if response["status"] != "ACTIVE" {
            return Err(context_error(StatusCode::CONFLICT));
        }
    }
    if context.is_some() != input.confirmation.is_some() {
        return Err(bad(
            "Final confirmation is only valid and required in an assisted workspace",
        ));
    }
    let key = raw_key
        .filter(|s| !s.is_empty())
        .map(|s| {
            super::normalize_idempotency_key(&s).map_err(|_| bad("Idempotency-Key is not valid"))
        })
        .transpose()?;
    if context.is_some() && key.is_none() {
        return Err(bad("Assisted orders require Idempotency-Key"));
    }
    let hash = input.request_hash(context.is_some())?;
    let command = CreateCommand {
        id: Uuid::new_v4(),
        input,
        context,
        key,
        hash,
        settings,
        audit,
        now,
        access: access.clone(),
        policy: app.policy,
        config: app.config,
    };
    run_market_command(
        app.pools.read,
        app.reads,
        crate::auth::VerifiedAccess {
            subject: access.subject,
            issued_at: access.issued_at,
            issuance_out_of_range: access.issuance_out_of_range,
        },
        command,
        AdmissionClass::Execution,
    )
    .await
}

struct CreateCommand {
    id: Uuid,
    input: CreateInput,
    context: Option<Uuid>,
    key: Option<String>,
    hash: String,
    settings: CreateSettings,
    audit: AuditContext,
    now: DateTime<Utc>,
    access: Arc<crate::auth::VerifiedAccess>,
    policy: Arc<MarketPolicy>,
    config: Arc<crate::Config>,
}
#[derive(Clone)]
struct Party {
    organization: Uuid,
    provenance: String,
    context_expiry: Option<DateTime<Utc>>,
    reference: Option<String>,
    capabilities: Vec<Uuid>,
}
impl CreateCommand {
    async fn party(
        &self,
        conn: &mut PgConnection,
        actor: &AuthenticatedActor,
    ) -> Result<Party, MarketWriteError> {
        if let Some(context) = self.context {
            if !self.settings.support_enabled
                || actor.account().role != ActorRole::Admin
                || actor.account().status != AccountState::Approved
            {
                return Err(context_error(StatusCode::NOT_FOUND));
            }
            let capabilities:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM staff_capability_assignments WHERE user_id=$1 AND capability::text IN ('MARKET_SUPPORT_LISTINGS','MARKET_SUPPORT_AUTHORIZATIONS') AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$2) ORDER BY id").bind(actor.user_id()).bind(self.now).fetch_all(&mut *conn).await?;
            if capabilities.len() != 2 {
                // The route resolved both capabilities before the command;
                // a miss here is revocation discovered under the locks, which
                // the source answers with the support-context conflict.
                return Err(context_error(StatusCode::CONFLICT));
            }
            let row=sqlx::query("SELECT c.organization_id,c.status::text AS status,c.scope::text AS scope,c.expires_at,c.support_reference,o.provenance::text AS provenance,o.verification_status::text AS organization_status FROM market_support_contexts c JOIN organizations o ON o.id=c.organization_id WHERE c.id=$1 AND c.actor_user_id=$2").bind(context).bind(actor.user_id()).fetch_optional(&mut *conn).await?.ok_or_else(||context_error(StatusCode::NOT_FOUND))?;
            if row.get::<String, _>("status") != "ACTIVE"
                || row.get::<String, _>("scope") != "ASSISTED_ORDER_ENTRY"
                || row.get::<DateTime<Utc>, _>("expires_at") <= self.now
                || row.get::<String, _>("organization_status") != "APPROVED"
                || row.get::<String, _>("provenance") != "REAL"
            {
                return Err(context_error(StatusCode::CONFLICT));
            }
            Ok(Party {
                organization: row.get("organization_id"),
                provenance: row.get("provenance"),
                context_expiry: Some(row.get("expires_at")),
                reference: Some(row.get("support_reference")),
                capabilities,
            })
        } else {
            let organization = actor
                .account()
                .current_organization_id
                .ok_or_else(|| bad("User must belong to an organization"))?;
            let provenance =
                sqlx::query_scalar("SELECT provenance::text FROM organizations WHERE id=$1")
                    .bind(organization)
                    .fetch_optional(conn)
                    .await?
                    .ok_or_else(internal)?;
            Ok(Party {
                organization,
                provenance,
                context_expiry: None,
                reference: None,
                capabilities: vec![],
            })
        }
    }
    fn candidate(&self, party: &Party, actor: Uuid, input: &CreateInput) -> OrderRow {
        let f = &input.fields;
        let text = |name: &str| f[name].as_str().map(str::to_owned);
        OrderRow {
            id: self.id,
            organization_id: party.organization,
            owner_user_id: Some(actor),
            created_by_actor_user_id: Some(actor),
            creation_method: if self.context.is_some() {
                CreationMethod::MarketSupport
            } else {
                CreationMethod::SelfService
            },
            product_id: input.product,
            delivery_point_id: Some(input.point),
            availability_window: f["availability_window"].as_str().unwrap().into(),
            side: if f["side"] == "BID" {
                Side::Bid
            } else {
                Side::Ask
            },
            provenance: party.provenance.clone(),
            organization_approved: true,
            price_per_mt_usd: input.price,
            quantity_mt: input.quantity,
            remaining_quantity_mt: input.quantity,
            acceptance_ordinal: 1,
            version: 1,
            status: OrderStatus::Open,
            expires_at: input.expiry,
            created_at: self.now,
            inventory_item_id: None,
            terms: ExecutionTerms {
                off_spec: f["off_spec"] == true,
                msds_available: f["msds_available"] == true,
                certification_declared: f["certification_declared"] == true,
                certification_scheme: text("certification_scheme"),
                certifications: f["certifications"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
                fame_terms: (!f["fame_terms"].is_null()).then(|| f["fame_terms"].clone()),
                lng_terms: (!f["lng_terms"].is_null()).then(|| f["lng_terms"].clone()),
                specification_standard: text("specification_standard"),
                carbon_intensity_method: text("carbon_intensity_method"),
                carbon_intensity_gco2_mj: text("carbon_intensity_gco2_mj")
                    .and_then(|v| Decimal::from_str_exact(&v).ok()),
                feedstock: text("feedstock"),
                origin: text("origin"),
            },
            owner_admission: OwnerAdmission::Admitted,
        }
    }
    fn bound(&self) -> MatchBound<'_> {
        MatchBound::new(self.now)
            .with_policy(&self.policy)
            .with_kyc_gate(self.settings.kyc_execution_gate_enabled)
    }
}
impl MarketCommand for CreateCommand {
    type Output = CommittedCommand;
    fn uses_order_create_replay(&self) -> bool {
        self.key.is_some()
    }
    fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
        serde_json::to_vec(&value.response_body).map_err(|_| internal())?;
        Ok(())
    }
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
            let party = self.party(conn, &authenticated).await?;
            let mut preview_input = self.input.clone();
            // Discovery sees the stored presentation: FAME fields derive for
            // both sides, Bio-LNG fields only for ASK.
            for field in ["fame_terms", "lng_terms"] {
                if !preview_input.fields[field].is_null()
                    && (field == "fame_terms" || preview_input.fields["side"] == "ASK")
                {
                    derive_metadata(&mut preview_input.fields, field)?;
                }
            }
            let candidate = self.candidate(&party, actor, &preview_input);
            let discovered = if self.context.is_some() {
                super::post_only::post_only_parties(
                    conn,
                    &candidate,
                    party.organization,
                    self.bound(),
                )
                .await?
            } else if self.settings.auto_matching_enabled {
                super::matching::match_parties(conn, &candidate, self.bound()).await?
            } else {
                super::matching::discover_parties(conn, [&candidate], []).await?
            };
            let members: Vec<Uuid> = if self.context.is_some() {
                sqlx::query_scalar("SELECT id FROM users WHERE organization_id=$1 AND role::text IN ('BUYER','SUPPLIER') ORDER BY id").bind(party.organization).fetch_all(&mut *conn).await?
            } else {
                vec![]
            };
            let parties = PartyLockPreview::new(
                discovered
                    .organizations
                    .into_iter()
                    .chain([party.organization]),
                discovered.users.into_iter().chain(members.iter().copied()),
                actor,
                json!({"organization":party.organization,"provenance":party.provenance,"capabilities":party.capabilities,"context":self.context,"members":members,"candidates":discovered.fingerprint}),
            );
            let mut locks = OperationLockPlan::new(MarketOperation::Create);
            locks.parties(&parties)?;
            if let Some(key) = &self.key {
                locks.idempotency(party.organization, "order.create", key)?;
            }
            locks.slices([candidate.slice()])?;
            locks
                .resources(LockClass::Capability, party.capabilities)?
                .resources(LockClass::Context, self.context)?;
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
            tx.create_port()?
                .replay(self, actor, key)
                .await
                .map_err(Into::into)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let (party, declared, input, mut candidate) = {
                let mut port = tx.create_port()?;
                let party = self.party(port.conn, actor).await?;
                // The source admits the acting account and its side role, then
                // validates the request body, and only then admits the
                // customer organization's execution and rechecks the minimum
                // lot, expiry and assisted instruction.
                port.admit_self(self, actor, &party).await?;
                let (declared, input) = port.validate(self).await?;
                port.admit(self, actor, &party).await?;
                port.revalidate(self, &input).await?;
                // Matching sees the construction presentation the stored row
                // will carry, not the request as declared.
                let mut stored = input.clone();
                for field in ["fame_terms", "lng_terms"] {
                    if !stored.fields[field].is_null() {
                        derive_metadata(&mut stored.fields, field)?;
                    }
                }
                let candidate = self.candidate(&party, actor.user_id(), &stored);
                (party, declared, input, candidate)
            };
            if self.context.is_some() {
                let assessment = tx
                    .assess_post_only(&candidate, party.organization, self.bound())
                    .await?;
                if assessment.indeterminate {
                    return Err(
                        MarketWriteError::new(MarketErrorCode::PostOnlyCheckIndeterminate).into(),
                    );
                }
                if assessment.would_cross {
                    return Err(MarketWriteError::post_only_would_cross(
                        assessment.best_executable_price,
                    )
                    .into());
                }
            }
            let new = NewOrderEffect {
                order_id: self.id,
                organization_id: party.organization,
                slice: EffectSlice {
                    product_id: input.product,
                    delivery_point_id: input.point,
                    availability_window: input.fields["availability_window"]
                        .as_str()
                        .unwrap()
                        .into(),
                },
            };
            let mut before = tx
                .effects()?
                .capture_mutation(&self.policy, &self.config, &[], &[], &[new], self.now)
                .await?;
            let authorization = tx
                .create_port()?
                .insert(self, actor, &party, &declared, &input)
                .await?;
            let outcome = if self.context.is_none() && self.settings.auto_matching_enabled {
                candidate = tx.matching_order(self.id).await?;
                let plan = tx.prepare_match(&candidate, self.bound()).await?;
                let ids: Vec<_> = plan
                    .locked_order_ids()
                    .into_iter()
                    .filter(|id| *id != self.id)
                    .collect();
                tx.effects()?
                    .include_existing(&mut before, &self.policy, &self.config, &ids)
                    .await?;
                Some(
                    tx.complete_match(
                        &plan,
                        &self.settings.fees,
                        input.fields["is_anonymous"] == true,
                    )
                    .await?,
                )
            } else {
                None
            };
            let mut set = before.stage();
            if let Some(authorization) = authorization {
                set.push(
                    tx.create_port()?
                        .authorization_effect(self, actor, &party, authorization, &input)
                        .await?,
                )?;
            }
            let participants: BTreeSet<_> = outcome
                .as_ref()
                .into_iter()
                .flat_map(|o| o.trades.iter())
                .flat_map(|t| [t.buyer_id, t.seller_id])
                .chain([party.organization])
                .collect();
            let descriptor = tx
                .create_port()?
                .order_effect(
                    self,
                    actor,
                    &party,
                    &input,
                    participants.into_iter().collect(),
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
                        self.audit.clone(),
                    )
                    .await?;
            }
            let response = tx
                .create_port()?
                .projection(self.id, &self.policy, &self.config, self.now)
                .await?;
            tx.effects()?
                .finish(&self.policy, &self.config, set)
                .await?;
            #[cfg(feature = "contract-tests")]
            if let Some(barrier) = &self.settings.barrier {
                barrier.reached.notify_one();
                barrier.resume.notified().await;
            }
            Ok(CommittedCommand {
                response_status: 201,
                response_body: response,
                replayed: false,
            })
        })
    }
}

pub(super) struct CreatePort<'a> {
    conn: &'a mut PgConnection,
    locked: &'a Locked,
}
impl<'a> CreatePort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, locked })
    }
    /// Fresh self-service admission: KYC, account admission and the side
    /// role, in the source's order, before request-body validation.
    async fn admit_self(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
    ) -> Result<(), MarketWriteError> {
        if command.context.is_some() {
            return Ok(());
        }
        let row=sqlx::query("SELECT u.kyc_status,u.kyc_organization_id,g.expires_at FROM users u LEFT JOIN kyc_execution_grace g ON g.user_id=u.id AND g.organization_id=u.organization_id WHERE u.id=$1").bind(actor.user_id()).fetch_one(&mut *self.conn).await?;
        let status: String = row.get("kyc_status");
        let parsed = serde_json::from_value(json!(status)).map_err(|_| internal())?;
        let grace = row
            .get::<Option<DateTime<Utc>>, _>("expires_at")
            .map(|expires_at| crate::kyc::grace::Grace {
                user_id: actor.user_id(),
                organization_id: party.organization,
                expires_at,
            });
        let result = crate::kyc::policy::eligibility(
            &crate::kyc::policy::KycPolicy {
                execution_gate_enabled: command.settings.kyc_execution_gate_enabled,
            },
            &crate::kyc::policy::KycParty {
                user_id: actor.user_id(),
                organization_id: Some(party.organization),
                kyc_organization_id: row.get("kyc_organization_id"),
                kyc_status: parsed,
            },
            grace.as_ref(),
            command.now,
        );
        if !result.eligible {
            return Err(MarketWriteError::kyc_required(
                status,
                result.grace_expires_at,
            ));
        }
        let access = crate::auth::VerifiedAccess {
            subject: actor.user_id(),
            issued_at: actor.access().issued_at,
            issuance_out_of_range: actor.access().issuance_out_of_range,
        };
        let authenticated = super::authority::admission::authenticate(
            self.conn,
            self.locked,
            access,
            AdmissionClass::Execution,
        )
        .await?;
        super::authority::admission::execution_context(
            self.conn,
            self.locked,
            authenticated,
            party.organization,
            command.context,
            command.now,
            command.settings.kyc_execution_gate_enabled,
        )
        .await?;
        if (command.input.fields["side"] == "BID" && actor.account().role != ActorRole::Buyer)
            || (command.input.fields["side"] == "ASK"
                && actor.account().role != ActorRole::Supplier)
        {
            return Err(refusal(
                StatusCode::FORBIDDEN,
                "The account role cannot place this order side",
            ));
        }
        Ok(())
    }
    /// The customer organization's execution admission the source evaluates
    /// after request-body validation. Self-service admission is complete by
    /// then; an assisted request admits its customer organization here.
    async fn admit(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
    ) -> Result<(), MarketWriteError> {
        if command.context.is_none() {
            return Ok(());
        }
        let access = crate::auth::VerifiedAccess {
            subject: actor.user_id(),
            issued_at: actor.access().issued_at,
            issuance_out_of_range: actor.access().issuance_out_of_range,
        };
        let authenticated = super::authority::admission::authenticate(
            self.conn,
            self.locked,
            access,
            AdmissionClass::Execution,
        )
        .await?;
        super::authority::admission::execution_context(
            self.conn,
            self.locked,
            authenticated,
            party.organization,
            command.context,
            command.now,
            command.settings.kyc_execution_gate_enabled,
        )
        .await
        .map_err(|e| {
            if e.code == MarketErrorCode::KycRequired {
                MarketWriteError::kyc_required("PENDING".into(), None)
            } else {
                e
            }
        })?;
        Ok(())
    }
    /// Request-body validation, in the source's order. Returns the declared
    /// input (the request as parsed, before display derivation) alongside the
    /// derived input the stored order carries.
    async fn validate(
        &mut self,
        command: &CreateCommand,
    ) -> Result<(CreateInput, CreateInput), MarketWriteError> {
        let mut input = command.input.clone();
        let f = &mut input.fields;
        let window = f["availability_window"].as_str().unwrap();
        let today = command
            .now
            .with_timezone(&chrono_tz::Asia::Singapore)
            .date_naive();
        let current = if window == "SPOT" {
            true
        } else if let Some((year, suffix)) = window.split_once('-') {
            let year: i32 = year
                .parse()
                .map_err(|_| bad("Invalid availability window"))?;
            if suffix == "CAL" {
                year >= today.year()
            } else if let Some(q) = suffix.strip_prefix('Q') {
                (year, q.parse::<u32>().unwrap_or(0)) >= (today.year(), (today.month() - 1) / 3 + 1)
            } else {
                (year, suffix.parse::<u32>().unwrap_or(0)) >= (today.year(), today.month())
            }
        } else {
            false
        };
        if !current {
            return Err(bad("Availability window is no longer open"));
        }
        let policy_product = command
            .policy
            .products
            .iter()
            .find(|p| p.id == input.product);
        let profile = policy_product
            .map(|p| p.qualification_profile.as_str())
            .unwrap_or("scalar");
        for (field, expected) in [("fame_terms", "fame_v1"), ("lng_terms", "lng_v1")] {
            if profile != expected && !f[field].is_null() {
                return Err(MarketWriteError::new(MarketErrorCode::ValidationFailed));
            }
            if profile == expected {
                if f[field].is_null() || input.quantity < Decimal::ONE {
                    return Err(MarketWriteError::new(MarketErrorCode::ValidationFailed));
                }
                if field == "fame_terms" {
                    crate::market::terms::validate_fame_terms(
                        f["side"].as_str().unwrap(),
                        Some(&f[field]),
                        today,
                    )
                    .map_err(|_| MarketWriteError::new(MarketErrorCode::ValidationFailed))?;
                } else if f["side"] == "ASK" && f[field]["seller_is_physical_supplier"] != true {
                    return Err(MarketWriteError::new(
                        MarketErrorCode::ResellerAllocationRequired,
                    ));
                }
            }
        }
        // The source derives FAME display fields for both sides but Bio-LNG
        // display fields only for ASK, each after its typed admission checks.
        for field in ["fame_terms", "lng_terms"] {
            if !f[field].is_null() && (field == "fame_terms" || f["side"] == "ASK") {
                derive_metadata(f, field)?;
            }
        }
        if f["side"] == "BID" {
            if input.supplied.iter().any(|name| {
                [
                    "certification_declared",
                    "specification_standard",
                    "msds_available",
                    "carbon_intensity_gco2_mj",
                    "carbon_intensity_method",
                    "feedstock",
                    "origin",
                    "off_spec",
                    "off_spec_notes",
                ]
                .contains(&name.as_str())
            }) {
                return Err(bad("Supplier metadata is only allowed for ASK orders"));
            }
        } else {
            if f["certification_declared"] != true
                || f["certification_scheme"]
                    .as_str()
                    .is_none_or(|s| s.trim().is_empty())
            {
                return Err(bad(
                    "ASK orders require certification declaration and scheme",
                ));
            }
            if profile != "scalar" {
                if f["msds_available"] != true {
                    return Err(MarketWriteError::new(MarketErrorCode::ValidationFailed));
                }
            } else {
                for key in ["specification_standard", "feedstock", "origin"] {
                    if f[key].as_str().is_none_or(|s| s.trim().is_empty()) {
                        return Err(bad("ASK orders require supplier metadata"));
                    }
                }
                if f["msds_available"] != true || f["carbon_intensity_gco2_mj"].is_null() {
                    return Err(bad("ASK orders require supplier metadata"));
                }
                if let Some(required) =
                    policy_product.and_then(|p| p.specification_standard.as_deref())
                    && (!f["specification_standard"]
                        .as_str()
                        .unwrap()
                        .trim()
                        .eq_ignore_ascii_case(required)
                        || f["carbon_intensity_method"]
                            .as_str()
                            .is_none_or(|s| s.trim().is_empty()))
                {
                    return Err(bad(
                        "Supplier specification and lifecycle method are required",
                    ));
                }
            }
        }
        let p = policy_product.ok_or_else(|| bad("Invalid product_id"))?;
        let active: bool = sqlx::query_scalar("SELECT is_active FROM products WHERE id=$1")
            .bind(input.product)
            .fetch_optional(&mut *self.conn)
            .await?
            .unwrap_or(false);
        if !active || p.execution_mode != "ORDERBOOK" {
            return Err(bad("Invalid product_id"));
        }
        if !p.lanes.contains(&input.point) {
            return Err(bad("Product is not available at this delivery point"));
        }
        let catalog_point = command
            .policy
            .points
            .iter()
            .find(|point| point.id == input.point)
            .ok_or_else(|| bad("Invalid delivery_point_id"))?;
        let row = sqlx::query("SELECT is_active,name,region FROM delivery_points WHERE id=$1")
            .bind(input.point)
            .fetch_optional(&mut *self.conn)
            .await?;
        let Some(row) = row else {
            return Err(bad("Invalid delivery_point_id"));
        };
        if !row.get::<bool, _>("is_active")
            || row.get::<String, _>("name") != catalog_point.name
            || row.get::<String, _>("region") != catalog_point.region
        {
            return Err(bad("Invalid delivery_point_id"));
        }
        Ok((command.input.clone(), input))
    }
    /// Post-admission revalidation: the minimum lot, the fresh expiry clock
    /// and the assisted confirmation checks the source keeps after its
    /// admission locks.
    async fn revalidate(
        &mut self,
        command: &CreateCommand,
        input: &CreateInput,
    ) -> Result<(), MarketWriteError> {
        let minimum: Option<Decimal> =
            sqlx::query_scalar("SELECT min_lot_size FROM products WHERE id=$1")
                .bind(input.product)
                .fetch_one(&mut *self.conn)
                .await?;
        let minimum = minimum
            .filter(|v| *v > Decimal::ZERO)
            .ok_or_else(|| refusal(StatusCode::CONFLICT, "Product minimum lot is unavailable"))?;
        if input.quantity < minimum {
            return Err(bad("Order quantity is below the product minimum lot"));
        }
        if input.expiry.is_some_and(|at| at <= command.now) {
            return Err(bad("Order expiry must be in the future"));
        }
        if let Some(confirmation) = &input.confirmation {
            if !input.fields["port_id"].is_null() || !input.fields["vessel_id"].is_null() {
                return Err(bad("Assisted orders use the canonical delivery point only"));
            }
            let instruction: DateTime<Utc> = confirmation["instruction_at"]
                .as_str()
                .unwrap()
                .parse()
                .map_err(|_| internal())?;
            if instruction > command.now {
                return Err(MarketWriteError::new(
                    MarketErrorCode::SupportInstructionTimeInvalid,
                ));
            }
            if input.expiry.is_some_and(|at| at <= instruction) {
                return Err(bad("Order expiry must follow instruction time"));
            }
        }
        Ok(())
    }
    async fn replay(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        key: &str,
    ) -> Result<Option<CommittedCommand>, MarketWriteError> {
        let party = command.party(self.conn, actor).await?;
        let row=sqlx::query("SELECT id,idempotency_request_hash,creation_method::text AS method,owner_user_id,created_by_actor_user_id,support_authorization_id FROM orderbook_orders WHERE organization_id=$1 AND idempotency_operation='order.create' AND idempotency_key=$2").bind(party.organization).bind(key).fetch_optional(&mut *self.conn).await?;
        let Some(row) = row else { return Ok(None) };
        let expected = if command.context.is_some() {
            "MARKET_SUPPORT"
        } else {
            "SELF_SERVICE"
        };
        let mut valid = row
            .get::<Option<String>, _>("idempotency_request_hash")
            .as_deref()
            == Some(&command.hash)
            && row.get::<String, _>("method") == expected
            && row
                .get::<Option<Uuid>, _>("owner_user_id")
                .is_none_or(|id| id == actor.user_id())
            && row
                .get::<Option<Uuid>, _>("created_by_actor_user_id")
                .is_none_or(|id| id == actor.user_id());
        if let Some(context) = command.context {
            valid &= row.get::<Option<Uuid>, _>("owner_user_id") == Some(actor.user_id());
            let authorization: Option<Uuid> = row.get("support_authorization_id");
            valid &= sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM market_support_authorizations WHERE id=$1 AND accountable_user_id=$2 AND market_support_context_id=$3)").bind(authorization).bind(actor.user_id()).bind(context).fetch_one(&mut *self.conn).await?;
        }
        if !valid {
            return Err(MarketWriteError::new(MarketErrorCode::IdempotencyConflict));
        }
        Ok(Some(CommittedCommand {
            response_status: 201,
            response_body: self
                .projection(row.get("id"), &command.policy, &command.config, command.now)
                .await?,
            replayed: true,
        }))
    }
    async fn insert(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
        declared: &CreateInput,
        input: &CreateInput,
    ) -> Result<Option<Uuid>, MarketAttemptError> {
        let ordinal: i64 = sqlx::query_scalar("SELECT nextval('orderbook_acceptance_ordinal_seq')")
            .fetch_one(&mut *self.conn)
            .await?;
        let authorization = if command.context.is_some() {
            Some(
                self.insert_authorization(command, actor, party, declared)
                    .await?,
            )
        } else {
            None
        };
        // The stored row carries the construction-time presentation: FAME
        // display fields for both sides, Bio-LNG display fields for both
        // sides as well, regardless of what validation derived.
        let mut stored = input.clone();
        for field in ["fame_terms", "lng_terms"] {
            if !stored.fields[field].is_null() {
                derive_metadata(&mut stored.fields, field)?;
            }
        }
        let input = &stored;
        let f = &input.fields;
        let payload = json!({"id":command.id,"organization_id":party.organization,"owner_user_id":actor.user_id(),"created_by_actor_user_id":actor.user_id(),"creation_method":if command.context.is_some(){"MARKET_SUPPORT"}else{"SELF_SERVICE"},"provenance":party.provenance,"side":f["side"],"product_id":input.product,"delivery_point_id":input.point,"port_id":f["port_id"],"vessel_id":f["vessel_id"],"quantity_mt":input.quantity.to_string(),"remaining_quantity_mt":input.quantity.to_string(),"price_per_mt_usd":input.price.to_string(),"availability_window":f["availability_window"],"expires_at":input.expiry,"certifications":f["certifications"],"certification_declared":f["certification_declared"],"certification_scheme":f["certification_scheme"],"specification_standard":f["specification_standard"],"msds_available":f["msds_available"],"carbon_intensity_gco2_mj":f["carbon_intensity_gco2_mj"],"carbon_intensity_method":f["carbon_intensity_method"],"feedstock":f["feedstock"],"origin":f["origin"],"off_spec":f["off_spec"],"off_spec_notes":f["off_spec_notes"],"fame_terms":f["fame_terms"],"lng_terms":f["lng_terms"],"idempotency_key":command.key,"idempotency_operation":command.key.as_ref().map(|_|"order.create"),"idempotency_request_hash":command.key.as_ref().map(|_|&command.hash),"acceptance_ordinal":ordinal,"support_authorization_id":authorization,"created_at":command.now,"updated_at":command.now,"status":"OPEN","version":1,"is_verdaxis_verified":false});
        sqlx::query("INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,port_id,vessel_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,expires_at,certifications,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,fame_terms,lng_terms,idempotency_key,idempotency_operation,idempotency_request_hash,acceptance_ordinal,support_authorization_id,created_at,updated_at,status,version,is_verdaxis_verified) SELECT id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,port_id,vessel_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,expires_at,certifications,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,fame_terms,lng_terms,idempotency_key,idempotency_operation,idempotency_request_hash,acceptance_ordinal,support_authorization_id,created_at,updated_at,status,version,is_verdaxis_verified FROM jsonb_populate_record(NULL::orderbook_orders,$1::jsonb)").bind(payload).execute(&mut *self.conn).await?;
        Ok(authorization)
    }
    async fn insert_authorization(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
        declared: &CreateInput,
    ) -> Result<Uuid, MarketWriteError> {
        let id = Uuid::new_v4();
        let confirmation = declared.confirmation.as_ref().ok_or_else(internal)?;
        // The authority records the request as declared: display derivation
        // never reaches the assisted authorization's terms.
        let mut payload = declared.economic_payload()?;
        let map = payload.as_object_mut().ok_or_else(internal)?;
        let expiry = party.context_expiry.ok_or_else(internal)?;
        let expiry = declared.expiry.map_or(expiry, |at| at.min(expiry));
        let digest = declared.authorization_digest()?;
        for (key, value) in [
            ("id", json!(id)),
            ("organization_id", json!(party.organization)),
            ("accountable_user_id", json!(actor.user_id())),
            ("market_support_context_id", json!(command.context)),
            ("order_side", declared.fields["side"].clone()),
            ("authorization_expires_at", json!(expiry)),
            ("order_expires_at", json!(declared.expiry)),
            ("terms_digest", json!(digest)),
            (
                "evidence_reference",
                confirmation["external_instruction_reference"].clone(),
            ),
            (
                "evidence_sha256",
                json!(
                    confirmation["evidence_excerpt"]
                        .as_str()
                        .map(|s| super::locks::sha256_hex(s.as_bytes()))
                ),
            ),
            ("instruction_at", confirmation["instruction_at"].clone()),
            ("acknowledge_exact_terms", json!(true)),
            ("acknowledge_executable_standing_order", json!(true)),
            (
                "commercial_consent_version",
                json!(command.settings.consent_version),
            ),
            (
                "commercial_consent_reference",
                json!(command.settings.consent_reference),
            ),
            ("support_case_reference", json!(party.reference)),
            ("idempotency_key", json!(command.key)),
            ("idempotency_request_hash", json!(command.hash)),
            ("created_by_actor_user_id", json!(actor.user_id())),
            ("status", json!("CONSUMED")),
            ("consumed_at", json!(command.now)),
            ("created_at", json!(command.now)),
            ("version", json!(1)),
        ] {
            map.insert(key.into(), value);
        }
        sqlx::query("INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,market_support_context_id,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,is_anonymous,certifications,certification_declared,certification_scheme,fame_terms,lng_terms,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,terms_digest,evidence_reference,evidence_sha256,instruction_at,acknowledge_exact_terms,acknowledge_executable_standing_order,commercial_consent_version,commercial_consent_reference,support_case_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,status,consumed_at,created_at) SELECT id,organization_id,accountable_user_id,market_support_context_id,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,is_anonymous,certifications,certification_declared,certification_scheme,fame_terms,lng_terms,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,off_spec_notes,terms_digest,evidence_reference,evidence_sha256,instruction_at,acknowledge_exact_terms,acknowledge_executable_standing_order,commercial_consent_version,commercial_consent_reference,support_case_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,status,consumed_at,created_at FROM jsonb_populate_record(NULL::market_support_authorizations,$1::jsonb)").bind(payload).execute(&mut *self.conn).await?;
        Ok(id)
    }
    async fn authorization_effect(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
        id: Uuid,
        input: &CreateInput,
    ) -> Result<MutationEffect, MarketWriteError> {
        let row=sqlx::query("SELECT terms_digest,evidence_reference,instruction_at,commercial_consent_version FROM market_support_authorizations WHERE id=$1").bind(id).fetch_one(&mut *self.conn).await?;
        let confirmation = input.confirmation.as_ref().ok_or_else(internal)?;
        Ok(MutationEffect {
            kind: MutationKind::AssistedOrderAuthorizationCreated,
            actor_id: actor.user_id(),
            organization_id: party.organization,
            resource_type: "market_support_authorization".into(),
            resource_id: id,
            changes: json!({"organization_id":party.organization,"accountable_user_id":actor.user_id(),"support_context_id":command.context,"terms_digest":row.get::<String,_>("terms_digest"),"evidence_reference":row.get::<String,_>("evidence_reference"),"instruction_at":confirmation["instruction_at"],"acknowledge_exact_terms":true,"acknowledge_executable_standing_order":true,"commercial_consent_version":row.get::<String,_>("commercial_consent_version"),"status":"CONSUMED"}),
            event: None,
            at: command.now,
            audit_context: command.audit.clone(),
        })
    }
    async fn order_effect(
        &mut self,
        command: &CreateCommand,
        actor: &AuthenticatedActor,
        party: &Party,
        input: &CreateInput,
        participants: Vec<Uuid>,
        matched: bool,
    ) -> Result<MutationEffect, MarketWriteError> {
        let projection = self
            .projection(command.id, &command.policy, &command.config, command.now)
            .await?;
        let visible = super::effects::order_visible(
            self.conn,
            &command.policy,
            &command.config,
            command.id,
            command.now,
        )
        .await?;
        let f = &input.fields;
        let activity = OrderActivity {
            id: command.id,
            side: f["side"].as_str().unwrap().into(),
            product_name: projection["product_name"]
                .as_str()
                .unwrap_or_default()
                .into(),
            fuel_type: projection["fuel_type"].as_str().unwrap_or_default().into(),
            region: projection["region"].as_str().unwrap_or_default().into(),
            source_kind: projection["source_kind"]
                .as_str()
                .unwrap_or("UNKNOWN")
                .into(),
            scope: projection["scope"].as_str().unwrap_or("UNKNOWN").into(),
            demo_status: projection["demo_status"]
                .as_str()
                .unwrap_or("UNKNOWN")
                .into(),
            is_demo_listing: party.provenance == "DEMO",
            unknown_count: u32::from(party.provenance == "UNKNOWN"),
            market_product: projection["market_product"].as_str().map(str::to_owned),
            delivery_point_id: Some(input.point),
            availability_window: f["availability_window"].as_str().unwrap().into(),
            observed_at: Some(command.now),
            delivery_point: projection["delivery_point_name"]
                .as_str()
                .map(str::to_owned),
        };
        Ok(MutationEffect {
            kind: MutationKind::OrderCreated,
            actor_id: actor.user_id(),
            organization_id: party.organization,
            resource_type: "order".into(),
            resource_id: command.id,
            changes: json!({"side":f["side"],"product_id":input.product,"market_product":projection["market_product"],"delivery_point_id":input.point,"availability_window":f["availability_window"],"quantity_mt":input.quantity.to_string(),"price_per_mt_usd":input.price.to_string(),"status":"OPEN","fame_terms":f["fame_terms"],"actor_user_id":actor.user_id(),"effective_organization_id":party.organization,"accountable_user_id":actor.user_id(),"support_context_id":command.context,"creation_method":if command.context.is_some(){"MARKET_SUPPORT"}else{"SELF_SERVICE"}}),
            event: Some(ParticipantEvent {
                event_type: "order_created".into(),
                aggregate_type: "order".into(),
                aggregate_id: command.id.to_string(),
                participant_org_ids: participants,
                fields: ParticipantFields::OrderCreated {
                    activity,
                    price: input.price.to_string(),
                    quantity: projection["remaining_quantity_mt"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                },
                public_invalidated: visible || matched,
            }),
            at: command.now,
            audit_context: command.audit.clone(),
        })
    }
    async fn projection(
        &mut self,
        id: Uuid,
        policy: &MarketPolicy,
        config: &crate::Config,
        now: DateTime<Utc>,
    ) -> Result<Value, MarketWriteError> {
        let row=sqlx::query("SELECT to_jsonb(o)||jsonb_build_object('quantity_mt',o.quantity_mt::text,'remaining_quantity_mt',o.remaining_quantity_mt::text,'price_per_mt_usd',o.price_per_mt_usd::text,'carbon_intensity_gco2_mj',o.carbon_intensity_gco2_mj::text) AS private_digest_input,p.name AS product_name,p.fuel_type AS product_fuel_type,p.fuel_grade AS product_fuel_grade,d.name AS delivery_point_name,d.region AS delivery_point_region,''::text AS market_product,COALESCE(g.supplier_tier::text,'INDEPENDENT') AS tier_label,o.expires_at,o.created_at,o.price_per_mt_usd,o.side::text AS side,o.provenance::text AS provenance FROM orderbook_orders o JOIN products p ON p.id=o.product_id JOIN organizations g ON g.id=o.organization_id LEFT JOIN delivery_points d ON d.id=o.delivery_point_id WHERE o.id=$1").bind(id).fetch_one(&mut *self.conn).await?;
        let mut projection =
            crate::market::order_projection::PublicOrder::from_row(&row).map_err(|_| internal())?;
        projection.labels["market_product"] = json!(
            policy
                .products
                .iter()
                .find(|p| p.id.to_string()
                    == projection.private["product_id"]
                        .as_str()
                        .unwrap_or_default())
                .map(|p| &p.code)
        );
        let key = projection.benchmark_key(now).map_err(|_| internal())?;
        let benchmarks = crate::market::live_benchmarks::live_benchmarks(
            self.conn,
            policy,
            config,
            &key.into_iter().collect::<Vec<_>>(),
            now,
        )
        .await
        .map_err(|_| internal())?;
        let mut response = projection
            .public(false, benchmarks.values().next())
            .map_err(|_| internal())?;
        let provenance = projection.provenance.as_str();
        response["is_demo_listing"] = json!(provenance == "DEMO");
        response["source_kind"] = json!(match provenance {
            "REAL" => "LIVE_ORDER",
            "DEMO" => "DEMO_SEED",
            _ => "UNKNOWN",
        });
        response["demo_status"] = json!(match provenance {
            "REAL" => "REAL_ONLY",
            "DEMO" => "DEMO_ONLY",
            _ => "UNKNOWN",
        });
        response["unknown_count"] = json!(u32::from(provenance == "UNKNOWN"));
        Ok(response)
    }
}

fn derive_metadata(
    fields: &mut serde_json::Map<String, Value>,
    field: &str,
) -> Result<(), MarketWriteError> {
    let terms = fields[field].clone();
    if field == "fame_terms" {
        fields.insert(
            "certification_scheme".into(),
            terms["sustainability_scheme"].clone(),
        );
        if fields["side"] == "ASK" {
            for (out, source) in [
                ("carbon_intensity_gco2_mj", "ci_gco2e_mj"),
                ("carbon_intensity_method", "ci_methodology"),
                ("origin", "production_origin"),
            ] {
                fields.insert(out.into(), terms[source].clone());
            }
            fields.insert(
                "specification_standard".into(),
                json!(format!(
                    "{} {}",
                    terms["standard"].as_str().unwrap_or_default(),
                    terms["standard_edition"].as_str().unwrap_or_default()
                )),
            );
            fields.insert(
                "feedstock".into(),
                json!("Used cooking oil (100% of feedstock inputs)"),
            );
        }
    } else {
        let label = |scheme: &str| match scheme {
            "ISCC_EU" => "ISCC EU",
            "REDCERT_EU" => "REDcert-EU",
            _ => "",
        };
        fields.insert("carbon_intensity_gco2_mj".into(), Value::Null);
        fields.insert("carbon_intensity_method".into(), Value::Null);
        if fields["side"] == "ASK" {
            let scheme = label(terms["sustainability_scheme"].as_str().unwrap_or_default());
            fields.insert("certification_declared".into(), json!(true));
            fields.insert("certification_scheme".into(), json!(scheme));
            fields.insert("certifications".into(), json!([scheme]));
            for (out, source) in [
                ("specification_standard", "specification_standard"),
                ("origin", "production_country"),
            ] {
                fields.insert(out.into(), terms[source].clone());
            }
            let feedstock = match terms["feedstock"].as_str().unwrap_or_default() {
                "MANURE" => "Manure",
                "FOOD_WASTE" => "Food waste",
                "SEWAGE_SLUDGE" => "Sewage sludge",
                "LANDFILL_GAS" => "Landfill gas",
                "AGRICULTURAL_RESIDUES" => "Agricultural residues",
                "OTHER_WASTES_AND_RESIDUES" => "Other wastes and residues",
                "ENERGY_CROPS" => "Energy crops",
                _ => return Err(internal()),
            };
            fields.insert("feedstock".into(), json!(feedstock));
        } else {
            fields.insert("certification_scheme".into(), Value::Null);
            fields.insert(
                "certifications".into(),
                json!(
                    terms["accepted_schemes"]
                        .as_array()
                        .ok_or_else(internal)?
                        .iter()
                        .map(|v| label(v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    Ok(())
}
