//! Unmounted order-amendment component. Economic work stays in the sealed owner.
use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::put,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    AppState,
    analytics::{
        events::{
            AllowedProperty, AllowedPropertyValue, AllowedServerEvent, DomainCatalog,
            RegisteredEvent, ServerEvent,
        },
        sender::RequestMetadata,
    },
    auth::VerificationSettings,
    market_amend::{self, AmendBody, AuditContext, MarketErrorCode, MarketWriteError},
};

fn invalid() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
fn bad_request() -> MarketWriteError {
    let mut e = invalid();
    e.status = StatusCode::BAD_REQUEST;
    e
}

/// The parsed update body as the component surface: the parsed fields, the
/// supplied names and the canonical request identity.
#[derive(Clone, Debug)]
pub struct AmendInput(AmendBody);
impl AmendInput {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self, MarketWriteError> {
        AmendBody::parse(&value, now).map(Self)
    }
    /// Whether any of the named fields was supplied, including as null.
    pub fn supplied(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.0.supplied(name))
    }
    pub fn value(&self, name: &str) -> Value {
        self.0.value(name).cloned().unwrap_or(Value::Null)
    }
    pub fn payload_hash(&self) -> &str {
        self.0.payload_hash()
    }
}

#[derive(Clone, Debug)]
pub struct AmendSettings {
    pub auto_matching_enabled: bool,
    pub kyc_execution_gate_enabled: bool,
    pub support_enabled: bool,
    pub fees: crate::subscriptions::FeeConfig,
    /// Post-commit conversion delivery; absent until serving composition.
    pub analytics: Option<crate::analytics::sender::AnalyticsSender>,
}
impl Default for AmendSettings {
    fn default() -> Self {
        Self {
            auto_matching_enabled: true,
            kyc_execution_gate_enabled: true,
            support_enabled: true,
            fees: Default::default(),
            analytics: None,
        }
    }
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    settings: AmendSettings,
}
/// Add amendment entry without replacing any existing route. This component
/// composition does not activate any serving route.
pub fn router(
    app: AppState,
    verification: VerificationSettings,
    settings: AmendSettings,
) -> Router {
    Router::new()
        .route("/api/orderbook/{id}", put(amend))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            settings,
        })
}
async fn amend(
    State(state): State<RouteState>,
    path: axum::extract::Path<Uuid>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let now = Utc::now();
    let result = async {
        let access = crate::auth::verify_authorization(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            &state.verification,
            now,
        )
        .map_err(|_| MarketWriteError::new(MarketErrorCode::AuthCredentialsInvalid))?;
        if body.len() > 64 * 1024 {
            let mut e = invalid();
            e.status = StatusCode::PAYLOAD_TOO_LARGE;
            return Err(e);
        }
        let input = AmendBody::parse(&serde_json::from_slice(&body).map_err(|_| invalid())?, now)?;
        let header = |name| {
            headers
                .get(name)
                .map(|v| v.to_str().map_err(|_| bad_request()))
                .transpose()
        };
        let ip = crate::runtime::http::client_ip(
            peer.map(|peer| peer.0.0.ip()),
            header("x-forwarded-for")?,
        );
        let audit = AuditContext::new(
            Some(ip),
            Some(crate::runtime::logging::request_id(header("x-request-id")?)),
        )?;
        market_amend::dispatch(
            state.app.clone(),
            access,
            market_amend::AmendRequest {
                order: path.0,
                input,
                key: header("idempotency-key")?.map(str::to_owned),
                audit,
                settings: state.settings.clone(),
                now,
            },
        )
        .await
    }
    .await;
    match result {
        Ok(reply) => {
            // A known fresh commit schedules one conversion event per matched
            // trade; replay, refusal and rollback never reach this branch, and
            // the sender drops the work without touching the committed result.
            if !reply.replayed
                && reply.response_status == 200
                && let Some(sender) = &state.settings.analytics
            {
                for _ in 0..reply.matched_trades {
                    schedule_conversion(sender, &state.app, &reply.response_body, &headers);
                }
            }
            (StatusCode::OK, axum::Json(reply.response_body)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// Build one registered conversion event from the committed amendment and hand
/// it to the shared sender with this request's metadata. The committed order
/// projection supplies the side and market labels; values outside the closed
/// taxonomy are dropped by the event builder, never by this route.
fn schedule_conversion(
    sender: &crate::analytics::sender::AnalyticsSender,
    app: &AppState,
    order: &Value,
    headers: &HeaderMap,
) {
    let event = AllowedServerEvent::TradeCreated;
    let registered = RegisteredEvent::Server(event);
    let side = order["side"].as_str().unwrap_or_default();
    let role = match side {
        "BID" => Some("BUYER"),
        "ASK" => Some("SUPPLIER"),
        _ => None,
    };
    let catalog = DomainCatalog::from_policy(&app.policy, Vec::<String>::new());
    let mut properties = Vec::new();
    for (property, value) in [
        (AllowedProperty::Role, role),
        (AllowedProperty::Side, Some(side)),
        (
            AllowedProperty::CanonicalProduct,
            order["market_product"].as_str(),
        ),
        (
            AllowedProperty::DeliveryPoint,
            order["delivery_point_name"].as_str(),
        ),
        (
            AllowedProperty::AvailabilityWindow,
            order["availability_window"].as_str(),
        ),
    ] {
        if let Some(value) =
            value.and_then(|v| AllowedPropertyValue::parse(registered, property, v, &catalog))
        {
            properties.push((property, value));
        }
    }
    if let Ok(delivery) = ServerEvent::new(event, properties) {
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        sender.schedule(
            delivery,
            RequestMetadata::new(header("user-agent"), header("host")),
            header("x-monitor-token"),
        );
    }
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str, settings: AmendSettings) -> Router {
    let config = serde_json::from_value(serde_json::json!({
        "environment": "test",
        "release_sha": "test",
        "migration_revision": "p89_20261022_trade_freeze",
        "database_url": "",
        "db_session_time_zone": "UTC",
        "bind": "127.0.0.1:0",
        "read_admission_slots": 8,
        "pool_max": 9,
        "python_budget": 0,
        "configured_max_connections": 100,
        "kyc_execution_gate_enabled": settings.kyc_execution_gate_enabled
    }))
    .unwrap();
    let app = AppState {
        config: Arc::new(config),
        pools: crate::runtime::db::Pools {
            read: pool.clone(),
            health: pool,
        },
        policy: Arc::new(
            serde_json::from_str(include_str!(
                "../../../../../deploy/rust/market-policy.json"
            ))
            .unwrap(),
        ),
        reads: Arc::new(tokio::sync::Semaphore::new(8)),
        reference_cache: Arc::new(crate::runtime::cache::ReferenceCache::default()),
    };
    router(
        app,
        VerificationSettings::new(key.into(), None, "amend-tests".into(), "amend-tests".into()),
        settings,
    )
}

/// The same component router with a conversion sender attached, for the
/// post-commit delivery cases; serving composition stays integration-owned.
#[cfg(feature = "contract-tests")]
pub fn contract_router_with_sender(
    pool: sqlx::PgPool,
    key: &str,
    mut settings: AmendSettings,
    sender: crate::analytics::sender::AnalyticsSender,
) -> Router {
    settings.analytics = Some(sender);
    contract_router(pool, key, settings)
}
