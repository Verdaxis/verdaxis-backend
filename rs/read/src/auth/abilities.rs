//! User-level hints composed from the enforcing market policies.
use super::{AuthenticatedError, AuthenticatedRequest, run_authenticated, verify_authorization};
use crate::market_party_policy::{
    PartyFacts, execution_denial, indicative_denial, role_denial, support_denial,
    take_provenance_allowed,
};
use crate::{ApiError, AppState};
use axum::{
    Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use serde::Serialize;
use sqlx::Row;

pub type Facts = PartyFacts;

const ACTIONS: [&str; 23] = [
    "place_bid_order",
    "place_ask_order",
    "amend_order",
    "cancel_order",
    "hit_ask_order",
    "hit_bid_order",
    "confirm_trade",
    "deliver_trade",
    "pay_trade",
    "manage_inventory",
    "publish_inventory",
    "create_rfq",
    "quote_rfq",
    "view_admin_analytics",
    "view_audit_log",
    "view_user_activity",
    "view_feedback",
    "manage_subscriptions",
    "manage_accounts",
    "review_kyc",
    "manage_commissions",
    "market_support_access",
    "market_support_assisted_orders",
];
const PATH: &str = "/api/auth/me/abilities";

#[derive(Serialize)]
pub struct Ability {
    action: &'static str,
    allowed: bool,
    reason_code: Option<&'static str>,
    reason: Option<&'static str>,
}
#[derive(Serialize)]
pub struct Abilities {
    schema_version: u8,
    abilities: Vec<Ability>,
}

fn message(code: &str) -> &'static str {
    match code {
        "ADMIN_CANNOT_EXECUTE" => "Administrator accounts cannot place or execute market actions.",
        "ROLE_NOT_ASSIGNED" => "Your account has no trading role yet.",
        "ORGANIZATION_REQUIRED" => "Your account must belong to an organization.",
        "ACCOUNT_NOT_APPROVED" => "Your account has not been approved yet.",
        "EMAIL_NOT_VERIFIED" => "Verify your email address to continue.",
        "PASSWORD_CHANGE_REQUIRED" => "Change your password to continue.",
        "KYC_REQUIRED" => "Complete KYC verification to trade.",
        "KYC_REJECTED" => "Your identity verification was rejected.",
        "KYC_ORGANIZATION_MISMATCH" => {
            "Your identity verification belongs to a different organization."
        }
        "ORGANIZATION_NOT_APPROVED" => "Your organization has not been approved yet.",
        "BUYER_ROLE_REQUIRED" => "Only buyer accounts can do this.",
        "SUPPLIER_ROLE_REQUIRED" => "Only supplier accounts can do this.",
        "ADMIN_ROLE_REQUIRED" => "Only administrator accounts can do this.",
        "ORGANIZATION_PROVENANCE_UNVERIFIED" => {
            "Your organization is not yet verified for live market trading."
        }
        "LEGACY_ADMIN_MUTATION_RETIRED" => {
            "Administrators must use an active Market Support context for this."
        }
        "MARKET_SUPPORT_DISABLED" => "Market Support is not enabled.",
        "MARKET_SUPPORT_CAPABILITY_REQUIRED" => {
            "You have not been granted the Market Support capability this needs."
        }
        "ACTION_RETIRED" => "This feature has been retired.",
        _ => "This action is not available.",
    }
}

pub fn build_abilities(f: &Facts) -> Abilities {
    // ADMIN cannot execute at all: show the role restriction, not a KYC task.
    // This display precedence does not change the enforcing routes' gate order.
    let administrator = (f.role.as_deref() == Some("ADMIN")).then_some("ADMIN_CANNOT_EXECUTE");
    let execution = administrator.or(execution_denial(f));
    let create = administrator.or(crate::market_party_policy::create_denial(f));
    let indicative = indicative_denial(f);
    let legacy = crate::market_party_policy::legacy_admin_denial(
        f.role.as_deref(),
        f.support_enabled,
        false,
    );
    let provenance =
        (!take_provenance_allowed(&f.provenance)).then_some("ORGANIZATION_PROVENANCE_UNVERIFIED");
    let mut capabilities = Vec::new();
    if f.listings {
        capabilities.push(crate::market_support::Capability::Listings);
    }
    if f.authorizations {
        capabilities.push(crate::market_support::Capability::Authorizations);
    }
    let abilities = ACTIONS
        .into_iter()
        .map(|action| {
            let denial = match action {
                "place_bid_order" => create.or(role_denial(f.role.as_deref(), "BUYER")),
                "place_ask_order" => create.or(role_denial(f.role.as_deref(), "SUPPLIER")),
                "publish_inventory" => create.or(role_denial(f.role.as_deref(), "SUPPLIER")),
                "amend_order" => legacy.or(execution),
                "cancel_order" => legacy
                    .or((f.role.as_deref() == Some("ADMIN")).then_some("ADMIN_CANNOT_EXECUTE")),
                "hit_ask_order" => execution
                    .or(role_denial(f.role.as_deref(), "BUYER"))
                    .or(provenance),
                "hit_bid_order" => execution
                    .or(role_denial(f.role.as_deref(), "SUPPLIER"))
                    .or(provenance),
                "confirm_trade" => create,
                "deliver_trade" => None,
                "pay_trade" => role_denial(f.role.as_deref(), "SUPPLIER"),
                "manage_inventory" => indicative.or(role_denial(f.role.as_deref(), "SUPPLIER")),
                "create_rfq" | "quote_rfq" => Some("ACTION_RETIRED"),
                "market_support_access" => support_denial(f).or(
                    (!crate::market_support::capabilities_allow_access(&capabilities))
                        .then_some("MARKET_SUPPORT_CAPABILITY_REQUIRED"),
                ),
                "market_support_assisted_orders" => support_denial(f).or(
                    (!crate::market_support::capabilities_allow_assisted_orders(&capabilities))
                        .then_some("MARKET_SUPPORT_CAPABILITY_REQUIRED"),
                ),
                _ => role_denial(f.role.as_deref(), "ADMIN"),
            };
            Ability {
                action,
                allowed: denial.is_none(),
                reason_code: denial,
                reason: denial.map(message),
            }
        })
        .collect();
    Abilities {
        schema_version: 1,
        abilities,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(read))
}

async fn read(State(state): State<AppState>, request: Request) -> Response {
    let Some(settings) = state.phase2.as_ref() else {
        return ApiError::database().into_response();
    };
    let now = Utc::now();
    let access = match verify_authorization(
        request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        &settings.verification,
        now,
    ) {
        Ok(access) => access,
        Err(error) => return crate::business::error::authentication(error).into_response(),
    };
    let gate = state.config.kyc_execution_gate_enabled;
    let support = state.config.phase2.market_write.support_enabled;
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("GET", PATH, false),
        move |transaction| Box::pin(async move {
            const SELECT_FACTS: &str = "SELECT u.role::text AS role, u.status::text AS status, \
                u.email_verified, u.must_change_password, u.organization_id, u.kyc_status, \
                u.kyc_organization_id, o.verification_status::text AS organization_status, \
                o.provenance, EXISTS(SELECT 1 FROM kyc_execution_grace g \
                WHERE g.user_id=u.id AND g.organization_id=u.organization_id AND g.expires_at>$2) AS grace \
                FROM users u LEFT JOIN organizations o ON o.id=u.organization_id WHERE u.id=$1";
            let user = transaction.actor().id;
            let row = sqlx::query(SELECT_FACTS)
                .bind(user).bind(now)
                .fetch_one(transaction.connection()).await
                .map_err(|_| ApiError::database())?;
            let organization: Option<uuid::Uuid> = row.get("organization_id");
            let bound: Option<uuid::Uuid> = row.get("kyc_organization_id");
            let mut facts = Facts {
                role: row.get("role"),
                status: row.get("status"),
                email_verified: row.get("email_verified"),
                must_change_password: row.get("must_change_password"),
                has_organization: organization.is_some(),
                organization_status: row.get("organization_status"),
                provenance: row.get::<Option<String>, _>("provenance").unwrap_or_default(),
                kyc_status: row.get("kyc_status"),
                kyc_bound: bound.is_some(),
                kyc_matches: bound == organization,
                grace: row.get("grace"),
                execution_gate_enabled: gate,
                support_enabled: support,
                listings: false,
                authorizations: false,
            };
            if support_denial(&facts).is_none() {
                let capabilities = crate::market_support::current_capabilities(
                    transaction.connection(), user, now,
                ).await.map_err(|_| ApiError::database())?;
                facts.listings = capabilities.contains(&crate::market_support::Capability::Listings);
                facts.authorizations = capabilities.contains(&crate::market_support::Capability::Authorizations);
            }
            Ok(build_abilities(&facts))
        }),
    ).await;
    let mut response = match result {
        Ok(body) => axum::Json(body).into_response(),
        Err(AuthenticatedError::Authentication(error)) => {
            crate::business::error::authentication(error).into_response()
        }
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    };
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
