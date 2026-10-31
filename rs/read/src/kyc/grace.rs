//! Read-only grace identity and at-most-once notice policy.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::policy::{KycParty, KycPolicy};

#[derive(Clone, Debug)]
pub struct Grace {
    pub user_id: Uuid,
    pub organization_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

impl Grace {
    pub(super) fn belongs_to(&self, party: &KycParty) -> bool {
        self.user_id == party.user_id && Some(self.organization_id) == party.organization_id
    }
}

pub fn grace_applies(party: &KycParty, grace: &Grace, now: DateTime<Utc>) -> bool {
    grace.belongs_to(party) && now < grace.expires_at
}

/// A durable claim suppresses another send, including an uncertain outcome.
/// The operator owns claim persistence and environment/release attestation.
pub fn notice_is_due(policy: &KycPolicy, already_claimed: bool) -> bool {
    policy.execution_gate_enabled && !already_claimed
}
