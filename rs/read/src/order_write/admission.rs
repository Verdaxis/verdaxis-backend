//! Admission uses database state protected by the complete party locks.
//! This is a child of `authority`, so only this verified path can construct it.
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::super::{
    error::{MarketErrorCode, MarketWriteError},
    locks::{LockClass, Locked},
};
use super::{
    AccountState, ActorRole, AuthenticatedActor, LockedActorState, SettlementOwnership,
    SettlementProof, TradeSide, WriteContext,
};
use crate::auth::{TokenIssuance, VerifiedAccess, cutoff_refusal};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionClass {
    Execution,
    Cleanup,
}

#[derive(Clone, Debug)]
pub struct PartyRef {
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub role: ActorRole,
}

fn denied() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::TradePartyForbidden)
}
fn invalid() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::TradePartiesInvalid)
}
fn require_locked(locked: &Locked, class: LockClass, id: Uuid) -> Result<(), MarketWriteError> {
    if locked.contains(class, id) {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn role(text: &str) -> Result<ActorRole, MarketWriteError> {
    match text {
        "BUYER" => Ok(ActorRole::Buyer),
        "SUPPLIER" => Ok(ActorRole::Supplier),
        "ADMIN" => Ok(ActorRole::Admin),
        _ => Err(invalid()),
    }
}
fn role_text(role: ActorRole) -> &'static str {
    match role {
        ActorRole::Buyer => "BUYER",
        ActorRole::Supplier => "SUPPLIER",
        ActorRole::Admin => "ADMIN",
    }
}

pub async fn authenticate(
    conn: &mut PgConnection,
    locked: &Locked,
    access: VerifiedAccess,
    class: AdmissionClass,
) -> Result<AuthenticatedActor, MarketWriteError> {
    authenticate_shared(conn, locked, std::sync::Arc::new(access), class).await
}

pub(crate) async fn authenticate_shared(
    conn: &mut PgConnection,
    locked: &Locked,
    access: std::sync::Arc<VerifiedAccess>,
    class: AdmissionClass,
) -> Result<AuthenticatedActor, MarketWriteError> {
    require_locked(locked, LockClass::User, access.subject)?;
    let row=sqlx::query("SELECT id,organization_id,role::text AS role,status::text AS status,email_verified,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1")
        .bind(access.subject).fetch_optional(conn).await?.ok_or_else(||MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid))?;
    let status = match row.get::<String, _>("status").as_str() {
        "APPROVED" => AccountState::Approved,
        "PENDING" => AccountState::Pending,
        "REJECTED" => AccountState::Rejected,
        _ => return Err(invalid()),
    };
    let account = LockedActorState {
        user_id: row.get("id"),
        status,
        role: role(&row.get::<String, _>("role"))?,
        current_organization_id: row.get("organization_id"),
        email_verified: row.get("email_verified"),
        must_change_password: row.get("must_change_password"),
        password_changed_at: row.get("password_changed_at"),
        authentication_revoked_at: row.get("authentication_revoked_at"),
    };
    let mut cutoff = crate::auth::AccountState {
        exists: true,
        status: String::new(),
        must_change_password: false,
        password_changed_at: account
            .password_changed_at
            .map(|time| time.timestamp_micros()),
        authentication_revoked_at: account
            .authentication_revoked_at
            .map(|time| time.timestamp_micros()),
    };
    if class == AdmissionClass::Cleanup && status == AccountState::Rejected {
        cutoff.authentication_revoked_at = None;
    }
    // Reuse Phase 3 microsecond, missing-issuance and equality semantics.
    if let Some(error) = cutoff_refusal(
        TokenIssuance {
            issued_at: access.issued_at,
            out_of_range: access.issued_at.is_none(),
        },
        &cutoff,
    ) {
        return Err(MarketWriteError::new(match error {
            crate::auth::AuthError::PasswordChanged => MarketErrorCode::AuthPasswordChanged,
            _ => MarketErrorCode::AuthSessionRevoked,
        }));
    }
    if class == AdmissionClass::Execution
        && (status != AccountState::Approved || account.must_change_password)
    {
        return Err(MarketWriteError::new(
            MarketErrorCode::ExecutionNotAvailable,
        ));
    }
    Ok(AuthenticatedActor { access, account })
}

pub async fn require_execution(
    conn: &mut PgConnection,
    locked: &Locked,
    party: &PartyRef,
    now: DateTime<Utc>,
    gate_enabled: bool,
) -> Result<(), MarketWriteError> {
    require_locked(locked, LockClass::User, party.user_id)?;
    require_locked(locked, LockClass::Organization, party.organization_id)?;
    if party.role == ActorRole::Admin {
        return Err(MarketWriteError::new(
            MarketErrorCode::ExecutionNotAvailable,
        ));
    }
    let row=sqlx::query("SELECT u.organization_id,u.role::text AS role,u.status::text AS status,u.email_verified,u.must_change_password,u.kyc_status,u.kyc_organization_id,o.verification_status::text AS organization_status,EXISTS(SELECT 1 FROM kyc_execution_grace g WHERE g.user_id=u.id AND g.organization_id=u.organization_id AND g.expires_at>$2) AS grace FROM users u LEFT JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1")
        .bind(party.user_id).bind(now).fetch_optional(conn).await?.ok_or_else(invalid)?;
    if row.get::<Option<Uuid>, _>("organization_id") != Some(party.organization_id)
        || row.get::<String, _>("role") != role_text(party.role)
        || row.get::<String, _>("status") != "APPROVED"
        || !row.get::<bool, _>("email_verified")
        || row.get::<bool, _>("must_change_password")
        || row
            .get::<Option<String>, _>("organization_status")
            .as_deref()
            != Some("APPROVED")
    {
        return Err(MarketWriteError::new(
            MarketErrorCode::ExecutionNotAvailable,
        ));
    }
    let status = row.get::<String, _>("kyc_status");
    let bound = row.get::<Option<Uuid>, _>("kyc_organization_id");
    let eligible = if gate_enabled {
        status != "REJECTED"
            && ((status == "APPROVED" && bound == Some(party.organization_id))
                || row.get::<bool, _>("grace"))
    } else {
        status != "REJECTED" && bound.is_none_or(|id| id == party.organization_id)
    };
    if !eligible {
        return Err(MarketWriteError::new(MarketErrorCode::KycRequired));
    }
    Ok(())
}

/// The side is an action restriction, not a caller-selected organization.
/// Concrete ownership survives movement; assisted membership never inherits it.
pub async fn cleanup_context_for_trade(
    conn: &mut PgConnection,
    locked: &Locked,
    access: VerifiedAccess,
    trade_id: Uuid,
    side: TradeSide,
) -> Result<WriteContext, MarketWriteError> {
    let actor = authenticate(conn, locked, access, AdmissionClass::Cleanup).await?;
    require_locked(locked, LockClass::Trade, trade_id)?;
    let row=sqlx::query("SELECT buyer_id,seller_id,buyer_user_id,seller_user_id,bid_order_id,ask_order_id FROM trades WHERE id=$1")
        .bind(trade_id).fetch_optional(&mut *conn).await?.ok_or_else(||MarketWriteError::new(MarketErrorCode::TradeNotFound))?;
    let (organization_id, user_id, order_id) = match side {
        TradeSide::Buyer => (
            row.get::<Uuid, _>("buyer_id"),
            row.get::<Option<Uuid>, _>("buyer_user_id"),
            row.get::<Option<Uuid>, _>("bid_order_id"),
        ),
        TradeSide::Seller => (
            row.get::<Uuid, _>("seller_id"),
            row.get::<Option<Uuid>, _>("seller_user_id"),
            row.get::<Option<Uuid>, _>("ask_order_id"),
        ),
    };
    require_locked(locked, LockClass::Organization, organization_id)?;
    let assisted = if let Some(order_id) = order_id {
        // The trade lock protects immutable linkage; only projected evidence is read.
        sqlx::query("SELECT id,organization_id,owner_user_id,created_by_actor_user_id,creation_method::text AS creation_method,side::text AS side FROM orderbook_orders WHERE id=$1")
            .bind(order_id).fetch_optional(&mut *conn).await?
    } else {
        None
    };
    if order_id.is_some() && assisted.is_none() {
        return Err(invalid());
    }
    let proof = if let Some(order) =
        assisted.filter(|order| order.get::<String, _>("creation_method") == "MARKET_SUPPORT")
    {
        let owner = order
            .get::<Option<Uuid>, _>("owner_user_id")
            .ok_or_else(invalid)?;
        let creator = order
            .get::<Option<Uuid>, _>("created_by_actor_user_id")
            .ok_or_else(invalid)?;
        let expected = match side {
            TradeSide::Buyer => ActorRole::Buyer,
            TradeSide::Seller => ActorRole::Supplier,
        };
        if user_id != Some(owner)
            || creator != owner
            || order.get::<Uuid, _>("organization_id") != organization_id
            || order.get::<String, _>("side")
                != match side {
                    TradeSide::Buyer => "BID",
                    TradeSide::Seller => "ASK",
                }
        {
            return Err(invalid());
        }
        if actor.user_id() == owner
            || actor.account.current_organization_id != Some(organization_id)
            || actor.account.role != expected
        {
            return Err(denied());
        }
        SettlementProof::AssistedCustomerMember {
            side,
            actor_user_id: actor.user_id(),
            organization_id,
            expected_role: expected,
            evidence: super::AssistedSideEvidence {
                side,
                organization_id,
                linked_order_id: order.get("id"),
                owner_user_id: owner,
                created_by_actor_user_id: creator,
            },
        }
    } else {
        if user_id != Some(actor.user_id()) {
            return Err(denied());
        }
        SettlementProof::RecordedUser {
            side,
            user_id: actor.user_id(),
            organization_id,
        }
    };
    Ok(WriteContext {
        actor,
        effective_organization_id: organization_id,
        support: None,
        now: Utc::now(),
        settlement_trade_id: Some(trade_id),
        settlement_ownership: Some(SettlementOwnership(proof)),
    })
}

pub async fn require_cleanup(
    conn: &mut PgConnection,
    locked: &Locked,
    ctx: &WriteContext,
    trade_id: Uuid,
    side: TradeSide,
) -> Result<(), MarketWriteError> {
    require_locked(locked, LockClass::Trade, trade_id)?;
    let proof = ctx.settlement_ownership().ok_or_else(denied)?;
    if ctx.settlement_trade_id() != Some(trade_id)
        || proof.side() != side
        || proof.actor_user_id() != ctx.actor().user_id()
        || proof.organization_id() != ctx.effective_organization_id()
        || ctx.support().is_some()
    {
        return Err(denied());
    }
    let row = sqlx::query(
        "SELECT buyer_id,seller_id,buyer_user_id,seller_user_id FROM trades WHERE id=$1",
    )
    .bind(trade_id)
    .fetch_one(conn)
    .await?;
    let (org, user) = match side {
        TradeSide::Buyer => (
            row.get::<Uuid, _>("buyer_id"),
            row.get::<Option<Uuid>, _>("buyer_user_id"),
        ),
        TradeSide::Seller => (
            row.get::<Uuid, _>("seller_id"),
            row.get::<Option<Uuid>, _>("seller_user_id"),
        ),
    };
    if org != proof.organization_id() || proof.recorded_user_id().is_some_and(|id| Some(id) != user)
    {
        return Err(denied());
    }
    Ok(())
}

/// Construct execution authority only from the verified actor and held evidence.
pub async fn execution_context(
    conn: &mut PgConnection,
    locked: &Locked,
    actor: AuthenticatedActor,
    organization_id: Uuid,
    support_context_id: Option<Uuid>,
    now: DateTime<Utc>,
    gate_enabled: bool,
) -> Result<WriteContext, MarketWriteError> {
    require_locked(locked, LockClass::User, actor.user_id())?;
    require_locked(locked, LockClass::Organization, organization_id)?;
    if actor.account.status != AccountState::Approved
        || actor.account.must_change_password
        || !actor.account.email_verified
    {
        return Err(MarketWriteError::new(
            MarketErrorCode::ExecutionNotAvailable,
        ));
    }
    let support = if let Some(context_id) = support_context_id {
        if actor.account.role != ActorRole::Admin {
            return Err(denied());
        }
        require_locked(locked, LockClass::Context, context_id)?;
        let context=sqlx::query("SELECT actor_user_id,organization_id,scope::text AS scope,status::text AS status,expires_at FROM market_support_contexts WHERE id=$1")
            .bind(context_id).fetch_optional(&mut *conn).await?.ok_or_else(invalid)?;
        if context.get::<Uuid, _>("actor_user_id") != actor.user_id()
            || context.get::<Uuid, _>("organization_id") != organization_id
            || context.get::<String, _>("scope") != "ASSISTED_ORDER_ENTRY"
            || context.get::<String, _>("status") != "ACTIVE"
            || context.get::<DateTime<Utc>, _>("expires_at") <= now
        {
            return Err(denied());
        }
        let org=sqlx::query("SELECT verification_status::text AS status,provenance::text AS provenance FROM organizations WHERE id=$1").bind(organization_id).fetch_one(&mut *conn).await?;
        if org.get::<String, _>("status") != "APPROVED"
            || org.get::<String, _>("provenance") != "REAL"
        {
            return Err(denied());
        }
        for capability in ["MARKET_SUPPORT_AUTHORIZATIONS", "MARKET_SUPPORT_LISTINGS"] {
            let row=sqlx::query("SELECT id,revoked_at,expires_at FROM staff_capability_assignments WHERE user_id=$1 AND capability::text=$2")
                .bind(actor.user_id()).bind(capability).fetch_optional(&mut *conn).await?.ok_or_else(denied)?;
            require_locked(locked, LockClass::Capability, row.get("id"))?;
            if row.get::<Option<DateTime<Utc>>, _>("revoked_at").is_some()
                || row
                    .get::<Option<DateTime<Utc>>, _>("expires_at")
                    .is_some_and(|expiry| expiry <= now)
            {
                return Err(denied());
            }
        }
        let members=sqlx::query("SELECT id,role::text AS role FROM users WHERE organization_id=$1 AND role::text IN ('BUYER','SUPPLIER') ORDER BY id")
            .bind(organization_id).fetch_all(&mut *conn).await?;
        let mut eligible = false;
        for member in members {
            let user_id = member.get("id");
            // Discovery must have included every possible authorizing member.
            require_locked(locked, LockClass::User, user_id)?;
            let party = PartyRef {
                user_id,
                organization_id,
                role: role(&member.get::<String, _>("role"))?,
            };
            match require_execution(conn, locked, &party, now, gate_enabled).await {
                Ok(()) => eligible = true,
                Err(error) if error.is_retryable() => return Err(error),
                Err(error)
                    if matches!(
                        error.code,
                        MarketErrorCode::ExecutionNotAvailable | MarketErrorCode::KycRequired
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        if !eligible {
            return Err(MarketWriteError::new(MarketErrorCode::KycRequired));
        }
        Some(super::SupportContextRef { id: context_id })
    } else {
        require_execution(
            conn,
            locked,
            &PartyRef {
                user_id: actor.user_id(),
                organization_id,
                role: actor.account.role,
            },
            now,
            gate_enabled,
        )
        .await?;
        None
    };
    Ok(WriteContext {
        actor,
        effective_organization_id: organization_id,
        support,
        now,
        settlement_trade_id: None,
        settlement_ownership: None,
    })
}

#[cfg(any(test, feature = "contract-tests"))]
#[allow(dead_code)]
pub async fn test_seed_assisted(conn: &mut PgConnection, change: &str) {
    sqlx::raw_sql(include_str!("../../tests/fixtures/phase8_assisted.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let sql = match change {
        "member_rejected" => {
            "UPDATE users SET kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000000004'"
        }
        "member_absent" => {
            "UPDATE users SET organization_id=NULL WHERE id='00000000-0000-0000-0000-000000000004'"
        }
        "wrong_org" => {
            "UPDATE market_support_contexts SET organization_id='00000000-0000-0000-0000-000000000014'"
        }
        "unknown_provenance" => "UPDATE organizations SET provenance='UNKNOWN'",
        "revoked_capability" => "UPDATE staff_capability_assignments SET revoked_at=now()",
        "expired_context" => {
            "UPDATE market_support_contexts SET expires_at=now()-interval '1 minute'"
        }
        _ => "SELECT 1",
    };
    sqlx::raw_sql(sql).execute(conn).await.unwrap();
}
