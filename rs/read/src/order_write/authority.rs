//! The shared write authority projection (plan §11.2; CF4_04; PD89-12).
//!
//! [`WriteContext`] binds one authenticated actor to the organization a write
//! acts for, the transaction instant, and — for a lifecycle command — the one
//! recorded trade side the actor may act on. [`AuthenticatedActor`] carries the
//! verified token together with the account state read under the transaction's
//! user lock, so a reapproval changes the decision inside the same attempt.
//!
//! Every field is private and this module declares no constructor, no default
//! and no conversion: an authority value that any caller can mint is not
//! evidence. Task 3 builds the actor and the context inside the transaction
//! from verified access and the locked projection; Task 10 produces support
//! contexts. Until then these declarations are the Interface contract only.
// Until the integration owner wires this leaf into `order_write`, the boundary
// tests compile it on its own, so not every declaration has a consumer yet.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::VerifiedAccess;

/// The three states the account row stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountState {
    Pending,
    Approved,
    Rejected,
}

/// The persisted role of an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorRole {
    Buyer,
    Supplier,
    Admin,
}

/// The locked database projection of one account (CF4_04).
///
/// Task 3 fills it from the user row it already locks and refreshes it on every
/// retry. It is never a token claim and never a cached copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedActorState {
    pub user_id: Uuid,
    pub status: AccountState,
    pub role: ActorRole,
    pub current_organization_id: Option<Uuid>,
    pub email_verified: bool,
    pub must_change_password: bool,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub authentication_revoked_at: Option<DateTime<Utc>>,
}

/// A verified access token together with the account state read under lock.
#[derive(Debug)]
pub struct AuthenticatedActor {
    access: VerifiedAccess,
    account: LockedActorState,
}

impl AuthenticatedActor {
    /// The verified token claims, including its expiry.
    pub fn access(&self) -> &VerifiedAccess {
        &self.access
    }

    /// The locked account projection.
    pub fn account(&self) -> &LockedActorState {
        &self.account
    }

    /// The account whose row was locked. The token subject must equal it; that
    /// check belongs to the transaction constructor, not this declaration.
    pub fn user_id(&self) -> Uuid {
        self.account.user_id
    }
}

/// A reference to the support context a write acts under.
///
/// The reference is only an identity: the context's authority is revalidated
/// under locks by the support module, never taken from this value.
#[derive(Debug)]
pub struct SupportContextRef {
    id: Uuid,
}

impl SupportContextRef {
    /// The support context identity.
    pub fn id(&self) -> Uuid {
        self.id
    }
}

/// The recorded trade side a settlement command acts on.
///
/// The side comes from `trades.buyer_id` / `trades.seller_id`, never from the
/// caller's claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSide {
    Buyer,
    Seller,
}

/// The historical assisted-side evidence of one trade side (CF4_04).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistedSideEvidence {
    side: TradeSide,
    organization_id: Uuid,
    linked_order_id: Uuid,
    owner_user_id: Uuid,
    created_by_actor_user_id: Uuid,
}

impl AssistedSideEvidence {
    pub fn side(&self) -> TradeSide {
        self.side
    }

    pub fn organization_id(&self) -> Uuid {
        self.organization_id
    }

    pub fn linked_order_id(&self) -> Uuid {
        self.linked_order_id
    }

    pub fn owner_user_id(&self) -> Uuid {
        self.owner_user_id
    }

    pub fn created_by_actor_user_id(&self) -> Uuid {
        self.created_by_actor_user_id
    }
}

/// The typed ownership proof for one recorded trade side (PD89-CF4-9).
///
/// The proof is built only by the settlement loader from the loaded trade and
/// its immutable evidence. Its representation is private, so no caller can
/// describe an authority it does not hold, and there is no descriptive string
/// or caller-supplied side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementOwnership(SettlementProof);

#[derive(Clone, Debug, PartialEq, Eq)]
enum SettlementProof {
    RecordedUser {
        side: TradeSide,
        user_id: Uuid,
        organization_id: Uuid,
    },
    AssistedCustomerMember {
        side: TradeSide,
        actor_user_id: Uuid,
        organization_id: Uuid,
        expected_role: ActorRole,
        evidence: AssistedSideEvidence,
    },
}

impl SettlementOwnership {
    /// The recorded side this proof authorizes.
    pub fn side(&self) -> TradeSide {
        match &self.0 {
            SettlementProof::RecordedUser { side, .. }
            | SettlementProof::AssistedCustomerMember { side, .. } => *side,
        }
    }

    /// The immutable side organization, derived from the trade row.
    pub fn organization_id(&self) -> Uuid {
        match &self.0 {
            SettlementProof::RecordedUser {
                organization_id, ..
            }
            | SettlementProof::AssistedCustomerMember {
                organization_id, ..
            } => *organization_id,
        }
    }

    /// The exact recorded user, when the proof is concrete ownership.
    pub fn recorded_user_id(&self) -> Option<Uuid> {
        match &self.0 {
            SettlementProof::RecordedUser { user_id, .. } => Some(*user_id),
            SettlementProof::AssistedCustomerMember { .. } => None,
        }
    }

    /// The acting member of the assisted customer organization.
    pub fn actor_user_id(&self) -> Uuid {
        match &self.0 {
            SettlementProof::RecordedUser { user_id, .. } => *user_id,
            SettlementProof::AssistedCustomerMember { actor_user_id, .. } => *actor_user_id,
        }
    }

    /// The side-compatible role an assisted member must hold.
    pub fn expected_role(&self) -> Option<ActorRole> {
        match &self.0 {
            SettlementProof::RecordedUser { .. } => None,
            SettlementProof::AssistedCustomerMember { expected_role, .. } => Some(*expected_role),
        }
    }

    /// The immutable assisted-side evidence, when the proof is assisted.
    pub fn evidence(&self) -> Option<&AssistedSideEvidence> {
        match &self.0 {
            SettlementProof::RecordedUser { .. } => None,
            SettlementProof::AssistedCustomerMember { evidence, .. } => Some(evidence),
        }
    }
}

/// Everything a write handler needs to know about who is acting and for whom.
///
/// The fields are plan §11.2's, plus CF4_04's two settlement bindings. A
/// request-scoped command leaves both unset and uses only the actor's effective
/// organization; a lifecycle command sets both and acts on exactly one recorded
/// side of exactly one trade.
#[derive(Debug)]
pub struct WriteContext {
    actor: AuthenticatedActor,
    effective_organization_id: Uuid,
    support: Option<SupportContextRef>,
    now: DateTime<Utc>,
    settlement_trade_id: Option<Uuid>,
    settlement_ownership: Option<SettlementOwnership>,
}

impl WriteContext {
    /// The authenticated actor.
    pub fn actor(&self) -> &AuthenticatedActor {
        &self.actor
    }

    /// The organization this write acts for.
    pub fn effective_organization_id(&self) -> Uuid {
        self.effective_organization_id
    }

    /// The support context this write acts under, if any.
    pub fn support(&self) -> Option<&SupportContextRef> {
        self.support.as_ref()
    }

    /// The support context identity, if any.
    pub fn support_context_id(&self) -> Option<Uuid> {
        self.support.as_ref().map(SupportContextRef::id)
    }

    /// The transaction instant. It is read once per command.
    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }

    /// The one trade a settlement command acts on, if any.
    pub fn settlement_trade_id(&self) -> Option<Uuid> {
        self.settlement_trade_id
    }

    /// The ownership proof of that trade's recorded side, if any.
    pub fn settlement_ownership(&self) -> Option<&SettlementOwnership> {
        self.settlement_ownership.as_ref()
    }
}
