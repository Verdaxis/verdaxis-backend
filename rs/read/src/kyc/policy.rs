//! Execution admission shared by scalar callers and SQL projections.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KycStatus {
    Pending,
    Submitted,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Deserialize)]
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

/// The strict arm of CF4_02 `KycSqlBinding.strict_sql`: current-organization
/// approval or an unexpired organization-bound grace, after the shared
/// known-status allowlist and the rejection refusal.
const STRICT_BODY: &str = "{user}.kyc_status IN ('PENDING','SUBMITTED','APPROVED','REJECTED') AND {user}.kyc_status <> 'REJECTED' AND (({user}.kyc_status = 'APPROVED' AND {user}.kyc_organization_id = {user}.organization_id) OR EXISTS (SELECT 1 FROM kyc_execution_grace g WHERE g.user_id = {user}.id AND g.organization_id = {user}.organization_id AND g.expires_at > /*kyc_now*/::timestamptz))";

/// The advisory arm of CF4_02 `KycSqlBinding.advisory_sql`.
const ADVISORY_BODY: &str = "{user}.kyc_status IN ('PENDING','SUBMITTED','APPROVED','REJECTED') AND {user}.kyc_status <> 'REJECTED' AND ({user}.kyc_organization_id IS NULL OR {user}.kyc_organization_id = {user}.organization_id)";

/// Render the CF4_02 `KycSqlBinding.combined` form: the advisory arm when the
/// switch is false, the strict arm when it is true, both after the same known
/// status allowlist. The outer parentheses keep the two arms together when a
/// caller composes the predicate with `AND`. Static composition markers are
/// replaced by the market query constructor; request values must use
/// `bind_eligibility_sql` instead.
pub fn eligibility_sql(party: KycSqlParty) -> String {
    let alias = match party.user_alias {
        KycUserAlias::User => "u",
        KycUserAlias::Member => "m",
    };
    // Organization identity deliberately comes from current user membership,
    // not an outer organization which may be the assisted actor's tenant.
    format!(
        "(NOT /*kyc_enabled*/::boolean AND {advisory}) OR (/*kyc_enabled*/::boolean AND {strict})",
        advisory = ADVISORY_BODY.replace("{user}", alias),
        strict = STRICT_BODY.replace("{user}", alias),
    )
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
    builder.push(prefix);
    builder.push_bind(policy.execution_gate_enabled);
    // The combined form names the switch in both arms. Reuse the parameter the
    // first bind just produced instead of binding the same value twice; the
    // contract declares exactly two bound values, switch then instant.
    let switch = builder
        .sql()
        .as_str()
        .rsplit_once('$')
        .expect("bound switch placeholder")
        .1
        .to_owned();
    let (middle, rest) = rest
        .split_once("/*kyc_enabled*/")
        .expect("second gate marker");
    builder.push(middle);
    builder.push(format!("${switch}"));
    let (middle, suffix) = rest.split_once("/*kyc_now*/").expect("static clock marker");
    builder.push(middle);
    builder.push_bind(now);
    builder.push(suffix);
}
