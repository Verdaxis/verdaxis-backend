//! User-level market admission shared by commands and ability hints.
#[derive(Clone, Debug, Default)]
pub struct PartyFacts {
    pub role: Option<String>,
    pub status: String,
    pub email_verified: bool,
    pub must_change_password: bool,
    pub has_organization: bool,
    pub organization_status: Option<String>,
    pub provenance: String,
    pub kyc_status: String,
    pub kyc_bound: bool,
    pub kyc_matches: bool,
    pub grace: bool,
    pub execution_gate_enabled: bool,
    pub support_enabled: bool,
    pub listings: bool,
    pub authorizations: bool,
}
impl PartyFacts {
    pub fn approved_buyer() -> Self {
        Self {
            role: Some("BUYER".into()),
            status: "APPROVED".into(),
            email_verified: true,
            must_change_password: false,
            has_organization: true,
            organization_status: Some("APPROVED".into()),
            provenance: "REAL".into(),
            kyc_status: "APPROVED".into(),
            kyc_bound: true,
            kyc_matches: true,
            grace: false,
            execution_gate_enabled: true,
            support_enabled: false,
            listings: false,
            authorizations: false,
        }
    }
}

pub fn role_denial(role: Option<&str>, required: &str) -> Option<&'static str> {
    if role == Some(required) {
        None
    } else {
        Some(match required {
            "BUYER" => "BUYER_ROLE_REQUIRED",
            "SUPPLIER" => "SUPPLIER_ROLE_REQUIRED",
            _ => "ADMIN_ROLE_REQUIRED",
        })
    }
}

pub fn execution_kyc_allowed(
    status: &str,
    bound: bool,
    matches: bool,
    grace: bool,
    gate: bool,
) -> bool {
    crate::kyc::policy::execution_allowed(
        status == "REJECTED",
        status == "APPROVED" && matches,
        !bound || matches,
        grace,
        gate,
    )
}

pub(crate) fn account_denial(f: &PartyFacts) -> Option<&'static str> {
    if f.role.as_deref() == Some("ADMIN") {
        Some("ADMIN_CANNOT_EXECUTE")
    } else if !matches!(f.role.as_deref(), Some("BUYER" | "SUPPLIER")) {
        Some("ROLE_NOT_ASSIGNED")
    } else if !f.has_organization || f.organization_status.is_none() {
        Some("ORGANIZATION_REQUIRED")
    } else if f.status != "APPROVED" {
        Some("ACCOUNT_NOT_APPROVED")
    } else if !f.email_verified {
        Some("EMAIL_NOT_VERIFIED")
    } else if f.must_change_password {
        Some("PASSWORD_CHANGE_REQUIRED")
    } else if f.organization_status.as_deref() != Some("APPROVED") {
        Some("ORGANIZATION_NOT_APPROVED")
    } else {
        None
    }
}

pub fn create_denial(f: &PartyFacts) -> Option<&'static str> {
    kyc_denial(f).or(account_denial(f))
}

pub fn execution_denial(f: &PartyFacts) -> Option<&'static str> {
    account_denial(f).or(kyc_denial(f))
}

pub fn kyc_denial(f: &PartyFacts) -> Option<&'static str> {
    if execution_kyc_allowed(
        &f.kyc_status,
        f.kyc_bound,
        f.kyc_matches,
        f.grace,
        f.execution_gate_enabled,
    ) {
        None
    } else if f.kyc_status == "REJECTED" {
        Some("KYC_REJECTED")
    } else {
        Some("KYC_REQUIRED")
    }
}

pub fn indicative_denial(f: &PartyFacts) -> Option<&'static str> {
    account_denial(f).or_else(|| {
        if f.kyc_status == "REJECTED" {
            Some("KYC_REJECTED")
        } else if f.kyc_bound && !f.kyc_matches {
            Some("KYC_ORGANIZATION_MISMATCH")
        } else {
            None
        }
    })
}

pub fn support_denial(f: &PartyFacts) -> Option<&'static str> {
    if f.status != "APPROVED" {
        Some("ACCOUNT_NOT_APPROVED")
    } else if f.must_change_password {
        Some("PASSWORD_CHANGE_REQUIRED")
    } else if !f.support_enabled {
        Some("MARKET_SUPPORT_DISABLED")
    } else {
        role_denial(f.role.as_deref(), "ADMIN")
    }
}

pub fn legacy_admin_denial(
    role: Option<&str>,
    support_enabled: bool,
    has_context: bool,
) -> Option<&'static str> {
    (support_enabled && !has_context && role == Some("ADMIN"))
        .then_some("LEGACY_ADMIN_MUTATION_RETIRED")
}

pub fn take_provenance_allowed(provenance: &str) -> bool {
    provenance == "REAL"
}
