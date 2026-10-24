//! Execution admission shared by scalar callers and SQL projections.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use super::grace::Grace;

#[derive(Clone, Copy, Debug)]
pub struct KycPolicy {
    pub execution_gate_enabled: bool,
}

impl Default for KycPolicy {
    fn default() -> Self {
        Self {
            execution_gate_enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KycStatus {
    Pending,
    Submitted,
    Approved,
    Rejected,
}

#[derive(Clone, Debug)]
pub struct KycParty {
    pub user_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub kyc_organization_id: Option<Uuid>,
    pub kyc_status: KycStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum KycReason {
    #[serde(rename = "KYC_NOT_ENFORCED")]
    NotEnforced,
    #[serde(rename = "KYC_APPROVED")]
    Approved,
    #[serde(rename = "KYC_REJECTED")]
    Rejected,
    #[serde(rename = "KYC_GRACE")]
    Grace,
    #[serde(rename = "KYC_REQUIRED")]
    Required,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KycEligibility {
    pub eligible: bool,
    pub reason: KycReason,
    pub grace_expires_at: Option<DateTime<Utc>>,
}

pub fn eligibility(
    policy: &KycPolicy,
    party: &KycParty,
    grace: Option<&Grace>,
    now: DateTime<Utc>,
) -> KycEligibility {
    let rejected = party.kyc_status == KycStatus::Rejected;
    if !policy.execution_gate_enabled {
        let eligible = !rejected
            && (party.kyc_organization_id.is_none()
                || party.kyc_organization_id == party.organization_id);
        return KycEligibility {
            eligible,
            reason: if rejected {
                KycReason::Rejected
            } else if eligible {
                KycReason::NotEnforced
            } else {
                KycReason::Required
            },
            grace_expires_at: None,
        };
    }
    if party.kyc_status == KycStatus::Approved
        && party.organization_id.is_some()
        && party.kyc_organization_id == party.organization_id
    {
        return KycEligibility {
            eligible: true,
            reason: KycReason::Approved,
            grace_expires_at: None,
        };
    }
    let grace = grace.filter(|grace| grace.belongs_to(party));
    let reason = if rejected {
        KycReason::Rejected
    } else if grace.is_some_and(|grace| now < grace.expires_at) {
        KycReason::Grace
    } else {
        KycReason::Required
    };
    KycEligibility {
        eligible: reason == KycReason::Grace,
        reason,
        grace_expires_at: grace.map(|grace| grace.expires_at),
    }
}

/// Only identifiers owned by the query constructors can enter the predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KycUserAlias {
    User,
    Member,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KycOrganizationAlias {
    Organization,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KycSqlParty {
    pub user_alias: KycUserAlias,
    pub organization_alias: KycOrganizationAlias,
}

impl KycSqlParty {
    pub const USER: Self = Self {
        user_alias: KycUserAlias::User,
        organization_alias: KycOrganizationAlias::Organization,
    };
    pub const MEMBER: Self = Self {
        user_alias: KycUserAlias::Member,
        organization_alias: KycOrganizationAlias::Organization,
    };
}

/// Static composition markers are replaced by the market query constructor;
/// request values must use `bind_eligibility_sql` instead.
pub fn eligibility_sql(party: KycSqlParty) -> String {
    let alias = match party.user_alias {
        KycUserAlias::User => "u",
        KycUserAlias::Member => "m",
    };
    // Organization identity deliberately comes from current user membership,
    // not an outer organization which may be the assisted actor's tenant.
    "({user}.kyc_status <> 'REJECTED' AND CASE WHEN /*kyc_enabled*/::boolean THEN (
        ({user}.kyc_status = 'APPROVED' AND {user}.kyc_organization_id = {user}.organization_id)
        OR EXISTS (SELECT 1 FROM kyc_execution_grace g
            WHERE g.user_id = {user}.id AND g.organization_id = {user}.organization_id
              AND g.expires_at > /*kyc_now*/::timestamptz)
      ) ELSE ({user}.kyc_organization_id IS NULL OR {user}.kyc_organization_id = {user}.organization_id) END)"
        .replace("{user}", alias)
}

pub fn bind_eligibility_sql(
    builder: &mut QueryBuilder<Postgres>,
    policy: &KycPolicy,
    party: KycSqlParty,
    now: DateTime<Utc>,
) {
    let sql = eligibility_sql(party);
    let (prefix, rest) = sql
        .split_once("/*kyc_enabled*/")
        .expect("static gate marker");
    let (middle, suffix) = rest.split_once("/*kyc_now*/").expect("static clock marker");
    builder.push(prefix);
    builder.push_bind(policy.execution_gate_enabled);
    builder.push(middle);
    builder.push_bind(now);
    builder.push(suffix);
}
