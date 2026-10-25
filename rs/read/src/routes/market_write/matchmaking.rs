//! Advisory suggestions and organization-scoped dismissal. Not mounted for serving.
use std::{sync::Arc, time::Instant};

use axum::{
    Extension, Router,
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{
    ApiError, AppState, MarketPolicy,
    auth::{
        AuthenticatedError, AuthenticatedRequest, VerificationSettings,
        limits::SharedLimits,
        request::{RequestAdmission, RequestError, validate_and_admit},
        run_authenticated, verify_authorization,
    },
    market::{
        order_projection::PublicOrder,
        terms::{
            QualificationInput, normalize_certification_scheme, order_is_execution_qualified_at,
        },
        visibility::{PublicOrderFilters, VisibilityClock, bind_order_query, order_query_sql},
    },
    market_commands::dismiss_suggestion,
};

const LIST: &str = "/api/matchmaking/suggestions";
const DISMISS: &str = "/api/matchmaking/suggestions/{order_id}/dismiss";
const MAX_SUGGESTIONS: usize = 20;

pub struct MatchmakingAuthority {
    settings: VerificationSettings,
    limits: SharedLimits,
    started: Instant,
}
impl MatchmakingAuthority {
    pub fn new(settings: VerificationSettings, limits: SharedLimits) -> Self {
        Self {
            settings,
            limits,
            started: Instant::now(),
        }
    }
    fn admission(&self, path: &str, request: &Request) -> RequestAdmission {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip());
        let forwarded = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok());
        RequestAdmission {
            client: crate::runtime::http::client_ip(peer, forwarded),
            path: path.into(),
            now_seconds: self.started.elapsed().as_secs(),
            limits: self.limits.clone(),
        }
    }
}
pub fn routes(authority: Arc<MatchmakingAuthority>) -> Router<AppState> {
    Router::new()
        .route(LIST, get(list))
        .route(DISMISS, patch(dismiss))
        .layer(Extension(authority))
}
fn error(code: &str, message: &str, status: axum::http::StatusCode) -> ApiError {
    ApiError {
        kind: crate::ApiErrorKind::Expected,
        status,
        detail: json!({"code":code,"message":message}),
        retry_after: None,
    }
}
fn auth_error(failure: AuthenticatedError) -> Response {
    match failure {
        AuthenticatedError::Authentication(e) => RequestError::Authentication(e).into_response(),
        AuthenticatedError::Database(e) => e.into_response(),
        AuthenticatedError::AmbiguousCommit => error(
            "COMMIT_OUTCOME_UNKNOWN",
            "The transaction outcome is unknown",
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
        )
        .into_response(),
    }
}
async fn list(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<MatchmakingAuthority>>,
    request: Request,
) -> Response {
    let access = match verify_authorization(
        request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        &authority.settings,
        Utc::now(),
    ) {
        Ok(v) => v,
        Err(e) => return RequestError::Authentication(e).into_response(),
    };
    let admission = authority.admission(LIST, &request);
    let policy = state.policy.clone();
    let config = state.config.clone();
    let result = run_authenticated(
        &state,
        access,
        AuthenticatedRequest::new("GET", LIST, false),
        move |tx| {
            Box::pin(async move {
                validate_and_admit(Value::Null, &admission, Ok)?;
                let actor = tx.actor();
                let side = match actor.role.as_deref() {
                    Some("BUYER") => "BID",
                    Some("SUPPLIER") => "ASK",
                    _ => return Ok(json!([])),
                };
                let Some(org) = actor.organization_id else {
                    return Ok(json!([]));
                };
                suggestions(tx.connection(), &policy, &config, org, side).await
            })
        },
    )
    .await;
    match result {
        Ok(body) => axum::Json(body).into_response(),
        Err(e) => auth_error(e),
    }
}
async fn dismiss(
    State(state): State<AppState>,
    Extension(authority): Extension<Arc<MatchmakingAuthority>>,
    Path(order): Path<String>,
    request: Request,
) -> Response {
    let access = match verify_authorization(
        request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        &authority.settings,
        Utc::now(),
    ) {
        Ok(v) => v,
        Err(e) => return RequestError::Authentication(e).into_response(),
    };
    let admission = authority.admission(DISMISS, &request);
    match dismiss_suggestion(&state, access, order, admission).await {
        Ok(body) => axum::Json(body).into_response(),
        Err(e) => e.into_response(),
    }
}

struct Order {
    public: PublicOrder,
    id: Uuid,
    organization: Uuid,
    remaining: Decimal,
    created: chrono::DateTime<Utc>,
}
impl Order {
    fn qualified(&self, policy: &MarketPolicy, clock: &VisibilityClock) -> bool {
        let v = &self.public.private;
        let product = v["product_id"].as_str().and_then(|v| v.parse().ok());
        let ci = v["carbon_intensity_gco2_mj"].as_str();
        order_is_execution_qualified_at(
            policy,
            &QualificationInput {
                product_id: product,
                market_product: self.public.labels["market_product"].as_str(),
                side: &self.public.side,
                delivery_point_id: v["delivery_point_id"].as_str(),
                availability_window: v["availability_window"].as_str().unwrap_or_default(),
                off_spec: v["off_spec"].as_bool().unwrap_or(false),
                msds_available: v["msds_available"].as_bool().unwrap_or(false),
                certification_scheme: v["certification_scheme"].as_str(),
                certification_declared: v["certification_declared"].as_bool().unwrap_or(false),
                fame_terms: Some(&v["fame_terms"]),
                lng_terms: Some(&v["lng_terms"]),
                specification_standard: v["specification_standard"].as_str(),
                carbon_intensity_method: v["carbon_intensity_method"].as_str(),
                carbon_intensity_gco2_mj: ci,
                feedstock: v["feedstock"].as_str(),
                origin: v["origin"].as_str(),
            },
            clock.singapore_date,
        )
    }
}
async fn suggestions(
    conn: &mut PgConnection,
    policy: &MarketPolicy,
    config: &crate::Config,
    org: Uuid,
    side: &str,
) -> Result<Value, ApiError> {
    let clock = VisibilityClock::at(Utc::now());
    // The complete relation applies before candidate selection or economic projection.
    let sql = order_query_sql(
        "SELECT e.*, /* private_digest_input(e) */ FROM eligible_orders e WHERE (e.organization_id=$14 AND e.side=$15) OR (e.organization_id<>$14 AND e.side<>$15 AND NOT EXISTS (SELECT 1 FROM match_suggestions s WHERE s.recipient_org_id=$14 AND s.status='DISMISSED' AND (s.bid_order_id=e.id OR s.ask_order_id=e.id))) ORDER BY e.created_at,e.id",
    );
    let rows = bind_order_query(&sql, policy, &clock, config, &PublicOrderFilters::default())
        .map_err(|_| ApiError::database())?
        .bind(org)
        .bind(side)
        .fetch_all(conn)
        .await
        .map_err(|_| ApiError::database())?;
    let mut sources = Vec::new();
    let mut candidates = Vec::new();
    for row in rows {
        let order = Order {
            public: PublicOrder::from_row(&row)?,
            id: row.try_get("id").map_err(|_| ApiError::internal())?,
            organization: row
                .try_get("organization_id")
                .map_err(|_| ApiError::internal())?,
            remaining: row
                .try_get("remaining_quantity_mt")
                .map_err(|_| ApiError::internal())?,
            created: row
                .try_get("created_at")
                .map_err(|_| ApiError::internal())?,
        };
        if !order.qualified(policy, &clock) {
            continue;
        }
        if order.organization == org {
            sources.push(order)
        } else {
            candidates.push(order)
        }
    }
    let mut ranked = Vec::new();
    for candidate in &candidates {
        let mut best = None;
        for source in &sources {
            if !compatible(policy, source, candidate) {
                continue;
            }
            let (score, reasons) = score(source, candidate)?;
            if best.as_ref().is_none_or(|(_, old, _)| score > *old) {
                best = Some((source, score, reasons));
            }
        }
        if let Some((source, score, reasons)) = best {
            ranked.push((source, candidate, score, reasons));
        }
    }
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.id.cmp(&b.1.id)));
    let mut out = Vec::new();
    for (source, candidate, score, reasons) in ranked.into_iter().take(MAX_SUGGESTIONS) {
        let (bid, ask) = if side == "BID" {
            (source, candidate)
        } else {
            (candidate, source)
        };
        out.push(json!({"id":candidate.id,"bid_order_id":bid.id,"ask_order_id":ask.id,"score":score,"match_reasons":reasons,"status":"SUGGESTED","recipient_org_id":org,"created_at":candidate.created,"bid_order":bid.public.public(false,None)?,"ask_order":ask.public.public(false,None)?}));
    }
    Ok(json!(out))
}
fn compatible(policy: &MarketPolicy, source: &Order, candidate: &Order) -> bool {
    let a = &source.public.private;
    let b = &candidate.public.private;
    if a["product_id"] != b["product_id"]
        || a["delivery_point_id"] != b["delivery_point_id"]
        || a["availability_window"] != b["availability_window"]
    {
        return false;
    }
    let (bid, ask) = if source.public.side == "BID" {
        (a, b)
    } else {
        (b, a)
    };
    let profile = policy
        .products
        .iter()
        .find(|p| Some(p.id.to_string().as_str()) == a["product_id"].as_str())
        .map(|p| p.qualification_profile.as_str())
        .unwrap_or("scalar");
    if profile == "lng_v1" {
        return lng_compatible(&bid["lng_terms"], &ask["lng_terms"]);
    }
    if profile == "fame_v1" {
        return fame_compatible(&bid["fame_terms"], &ask["fame_terms"]);
    }
    let scheme = normalize_certification_scheme(ask["certification_scheme"].as_str());
    let required: Vec<_> = bid["certifications"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| normalize_certification_scheme(v.as_str()))
        .collect();
    if !required.is_empty() {
        return scheme.is_some_and(|s| required.contains(&s));
    }
    let target = normalize_certification_scheme(bid["certification_scheme"].as_str());
    target.is_none() || target == scheme
}
fn number(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}
fn minimum(bid: &Value, ask: &Value, key: &str, actual: &str) -> bool {
    number(&bid[key]).is_none_or(|min| number(&ask[actual]).is_some_and(|v| v >= min))
}
fn maximum(bid: &Value, ask: &Value, key: &str, actual: &str) -> bool {
    number(&bid[key]).is_none_or(|max| number(&ask[actual]).is_some_and(|v| v <= max))
}
fn contains(bid: &Value, key: &str, actual: &Value) -> bool {
    bid[key].as_array().is_some_and(|v| v.contains(actual))
}
fn lng_compatible(bid: &Value, ask: &Value) -> bool {
    contains(bid, "accepted_schemes", &ask["sustainability_scheme"])
        && contains(bid, "accepted_custody", &ask["chain_of_custody"])
        && (!bid["require_union_database"].as_bool().unwrap_or(false)
            || ask["union_database_registered"] == true)
        && minimum(bid, ask, "min_bio_share_pct", "bio_share_pct")
        && maximum(bid, ask, "max_wtt_ghg_gco2e_mj", "wtt_ghg_gco2e_mj")
        && contains(bid, "accepted_delivery_methods", &ask["delivery_method"])
        && minimum(bid, ask, "min_methane_number", "min_methane_number")
        && (bid["accept_resellers"] == true || ask["seller_is_physical_supplier"] == true)
}
fn fame_compatible(bid: &Value, ask: &Value) -> bool {
    [
        "standard",
        "standard_edition",
        "sustainability_scheme",
        "evidence_due",
    ]
    .into_iter()
    .all(|k| bid[k] == ask[k])
        && ["astm_grade", "en_climate_class"]
            .into_iter()
            .all(|k| bid[k].is_null() || bid[k] == ask[k])
        && maximum(bid, ask, "max_cfpp_c", "cfpp_c")
        && maximum(bid, ask, "max_cloud_point_c", "cloud_point_c")
        && maximum(bid, ask, "max_ci_gco2e_mj", "ci_gco2e_mj")
        && (bid["max_ci_gco2e_mj"].is_null()
            || ["ci_methodology", "ci_boundary", "ci_basis"]
                .into_iter()
                .all(|k| bid[k] == ask[k]))
        && (bid["require_quality_evidence"] != true
            || ask["quality_evidence"]["status"] == "AVAILABLE")
        && (bid["require_sustainability_evidence"] != true
            || ask["sustainability_evidence"]["status"] == "AVAILABLE")
        && (ask["sustainability_evidence"].is_null()
            || ask["sustainability_evidence"]["due"] == bid["evidence_due"])
}
fn score(source: &Order, candidate: &Order) -> Result<(u32, Vec<&'static str>), ApiError> {
    let a = &source.public.private;
    let b = &candidate.public.private;
    let target = normalize_certification_scheme(a["certification_scheme"].as_str());
    if target.is_some()
        && target != normalize_certification_scheme(b["certification_scheme"].as_str())
    {
        return Ok((0, vec![]));
    }
    let mut score = 70;
    let mut reasons = vec![
        "market_product_match",
        "delivery_point_match",
        "availability_match",
    ];
    let price = source.public.price;
    let other = candidate.public.price;
    if price >= other {
        score += 20;
        reasons.push("price_overlap");
    } else {
        let gap = other
            .checked_sub(price)
            .and_then(|v| v.checked_mul(Decimal::from(100)))
            .ok_or_else(ApiError::internal)?;
        if other
            .checked_mul(Decimal::from(5))
            .is_some_and(|limit| gap <= limit)
        {
            score += 10;
            reasons.push("price_close");
        } else if other
            .checked_mul(Decimal::from(10))
            .is_some_and(|limit| gap <= limit)
        {
            score += 5;
            reasons.push("price_negotiable");
        }
    }
    let min = source.remaining.min(candidate.remaining);
    let max = source.remaining.max(candidate.remaining);
    if max > Decimal::ZERO {
        if min.checked_mul(Decimal::from(2)).is_some_and(|v| v >= max) {
            score += 10;
            reasons.push("quantity_fit");
        } else if min.checked_mul(Decimal::from(5)).is_some_and(|v| v >= max) {
            score += 5;
            reasons.push("quantity_partial");
        }
    }
    if b["certification_declared"] == true {
        let fields = ["certification_scheme", "specification_standard"]
            .map(|k| b[k].as_str().is_some_and(|s| !s.is_empty()));
        if fields.into_iter().all(|v| v) && b["msds_available"] == true {
            score += 10;
            reasons.push("documentation_complete");
        } else if fields.into_iter().any(|v| v) || b["msds_available"] == true {
            score += 5;
            reasons.push("documentation_partial");
        }
    }
    Ok((score.min(100), reasons))
}
