//! Unmounted order-entry component. Economic work stays in the sealed owner.
use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    auth::VerificationSettings,
    order_write::{
        Canonical, MarketErrorCode, MarketWriteError, OrderQuantity, Price, effects::AuditContext,
        idempotency_request_hash,
    },
};

fn invalid() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
fn bad_request() -> MarketWriteError {
    let mut e = invalid();
    e.status = StatusCode::BAD_REQUEST;
    e
}
fn text_decimal(value: &Value) -> Result<String, MarketWriteError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(invalid()),
    }
}
fn timestamp(value: &Value) -> Result<Option<DateTime<Utc>>, MarketWriteError> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .ok_or_else(invalid)?
        .parse()
        .map(Some)
        .map_err(|_| invalid())
}

#[derive(Clone, Debug)]
pub struct CreateInput {
    pub(crate) fields: Map<String, Value>,
    pub(crate) supplied: Vec<String>,
    pub(crate) confirmation: Option<Value>,
    pub(crate) quantity: Decimal,
    pub(crate) price: Decimal,
    pub(crate) expiry: Option<DateTime<Utc>>,
    pub(crate) product: Uuid,
    pub(crate) point: Uuid,
}
impl CreateInput {
    pub fn parse(value: Value, now: DateTime<Utc>) -> Result<Self, MarketWriteError> {
        let mut fields = value.as_object().cloned().ok_or_else(invalid)?;
        let supplied = fields.keys().cloned().collect();
        if !matches!(
            fields.get("side").and_then(Value::as_str),
            Some("BID" | "ASK")
        ) {
            return Err(invalid());
        }
        let product = fields
            .get("product_id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let point = fields
            .get("delivery_point_id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
        let quantity = OrderQuantity::parse_exact(&text_decimal(
            fields.get("quantity_mt").ok_or_else(invalid)?,
        )?)?
        .value();
        let price = Price::parse_exact(&text_decimal(
            fields.get("price_per_mt_usd").ok_or_else(invalid)?,
        )?)?
        .value();
        fields.insert("quantity_mt".into(), json!(quantity.to_string()));
        fields.insert("price_per_mt_usd".into(), json!(price.to_string()));
        let window = fields
            .get("availability_window")
            .map(|v| v.as_str().ok_or_else(invalid))
            .transpose()?
            .unwrap_or("SPOT");
        let window = crate::market::windows::normalize_window(window, now.date_naive())
            .map_err(|_| invalid())?;
        fields.insert("availability_window".into(), json!(window));
        for (name, default) in [
            ("certifications", json!([])),
            ("is_anonymous", json!(true)),
            ("certification_declared", json!(false)),
            ("msds_available", json!(false)),
            ("off_spec", json!(false)),
        ] {
            fields.entry(name).or_insert(default);
        }
        if fields["is_anonymous"] != true {
            return Err(invalid());
        }
        for name in ["certification_declared", "msds_available", "off_spec"] {
            if !fields[name].is_boolean() {
                return Err(invalid());
            }
        }
        for name in [
            "port_id",
            "vessel_id",
            "expires_at",
            "certification_scheme",
            "specification_standard",
            "carbon_intensity_gco2_mj",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec_notes",
            "fame_terms",
            "lng_terms",
        ] {
            fields.entry(name).or_insert(Value::Null);
        }
        for name in [
            "port_id",
            "certification_scheme",
            "specification_standard",
            "carbon_intensity_method",
            "feedstock",
            "origin",
            "off_spec_notes",
        ] {
            if !fields[name].is_null() && !fields[name].is_string() {
                return Err(invalid());
            }
        }
        if !fields["vessel_id"].is_null() {
            fields["vessel_id"]
                .as_str()
                .ok_or_else(invalid)?
                .parse::<Uuid>()
                .map_err(|_| invalid())?;
        }
        if !fields["carbon_intensity_gco2_mj"].is_null() {
            let amount =
                Decimal::from_str_exact(&text_decimal(&fields["carbon_intensity_gco2_mj"])?)
                    .map_err(|_| invalid())?;
            if amount < Decimal::ZERO {
                return Err(invalid());
            }
            fields.insert("carbon_intensity_gco2_mj".into(), json!(amount.to_string()));
        }
        let certs = fields["certifications"].as_array().ok_or_else(invalid)?;
        let mut normalized = Vec::new();
        for cert in certs {
            let text = cert
                .as_str()
                .ok_or_else(invalid)?
                .trim_matches(crate::market::whitespace::is_python_whitespace);
            if !text.is_empty() && !normalized.contains(&text.to_owned()) {
                normalized.push(text.to_owned());
            }
        }
        fields.insert("certifications".into(), json!(normalized));
        if let Some(scheme) = fields["certification_scheme"].as_str() {
            let scheme = scheme.trim_matches(crate::market::whitespace::is_python_whitespace);
            fields.insert(
                "certification_scheme".into(),
                if scheme.is_empty() {
                    Value::Null
                } else {
                    json!(scheme)
                },
            );
        }
        let expiry = timestamp(&fields["expires_at"])?;
        if let Some(expiry) = expiry {
            fields.insert(
                "expires_at".into(),
                json!(crate::routes::ports::pydantic_datetime(expiry)),
            );
        }
        for (field, profile) in [("fame_terms", "fame_v1"), ("lng_terms", "lng_v1")] {
            if !fields[field].is_null() {
                if field == "fame_terms" {
                    crate::market::terms::validate_fame_schema(
                        fields["side"].as_str().unwrap(),
                        Some(&fields[field]),
                    )
                    .map_err(|_| invalid())?;
                } else {
                    crate::market::terms::validate_lng_terms(
                        fields["side"].as_str().unwrap(),
                        Some(&fields[field]),
                    )
                    .map_err(|_| invalid())?;
                }
                let normalized =
                    crate::market::terms::owner_response_terms(profile, &fields[field])
                        .map_err(|_| invalid())?;
                fields.insert(field.into(), normalized);
            }
        }
        let confirmation = fields
            .remove("support_confirmation")
            .filter(|v| !v.is_null())
            .map(|mut v| -> Result<Value, MarketWriteError> {
                let map = v.as_object_mut().ok_or_else(invalid)?;
                let reference = map
                    .get("external_instruction_reference")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                if !(3..=500).contains(&reference.chars().count()) || reference.trim().is_empty() {
                    return Err(invalid());
                }
                let reference = reference
                    .trim_matches(crate::market::whitespace::is_python_whitespace)
                    .to_owned();
                map.insert("external_instruction_reference".into(), json!(reference));
                let at = timestamp(map.get("instruction_at").ok_or_else(invalid)?)?
                    .ok_or_else(invalid)?;
                if at > now {
                    return Err(invalid());
                }
                map.insert(
                    "instruction_at".into(),
                    json!(crate::routes::ports::pydantic_datetime(at)),
                );
                for name in [
                    "acknowledge_exact_terms",
                    "acknowledge_executable_standing_order",
                ] {
                    if map.get(name) != Some(&json!(true)) {
                        return Err(invalid());
                    }
                }
                let evidence = match map.get("evidence_excerpt") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let s = v.as_str().ok_or_else(invalid)?;
                        if s.chars().count() > 4000 {
                            return Err(invalid());
                        }
                        let s = s.trim_matches(crate::market::whitespace::is_python_whitespace);
                        (!s.is_empty()).then(|| s.to_owned())
                    }
                };
                map.insert("evidence_excerpt".into(), json!(evidence));
                Ok(v)
            })
            .transpose()?;
        Ok(Self {
            fields,
            supplied,
            confirmation,
            quantity,
            price,
            expiry,
            product,
            point,
        })
    }
    pub fn economic_payload(&self) -> Result<Value, MarketWriteError> {
        let mut fields = self.fields.clone();
        // Only schema fields are economic authority; unknown request fields are ignored.
        fields.retain(|k, _| {
            [
                "side",
                "product_id",
                "delivery_point_id",
                "port_id",
                "vessel_id",
                "quantity_mt",
                "price_per_mt_usd",
                "availability_window",
                "certifications",
                "certification_scheme",
                "expires_at",
                "is_anonymous",
                "certification_declared",
                "specification_standard",
                "msds_available",
                "carbon_intensity_gco2_mj",
                "carbon_intensity_method",
                "feedstock",
                "origin",
                "off_spec",
                "off_spec_notes",
                "fame_terms",
                "lng_terms",
            ]
            .contains(&k.as_str())
        });
        for name in ["fame_terms", "lng_terms"] {
            if fields[name].is_null() {
                fields.remove(name);
            }
        }
        Ok(Value::Object(fields))
    }
    pub fn confirmation_payload(&self) -> Option<Value> {
        self.confirmation.clone()
    }
    pub fn request_hash(&self, assisted: bool) -> Result<String, MarketWriteError> {
        let order = self.economic_payload()?;
        let payload = if assisted {
            json!({"order":order,"support_confirmation":self.confirmation})
        } else {
            order
        };
        let canonical = Canonical::from_json(&payload).map_err(|_| invalid())?;
        idempotency_request_hash(&canonical).map_err(|_| invalid())
    }
}

#[derive(Clone, Debug)]
pub struct CreateSettings {
    pub auto_matching_enabled: bool,
    pub kyc_execution_gate_enabled: bool,
    pub support_enabled: bool,
    pub consent_version: String,
    pub consent_reference: String,
    pub fees: crate::subscriptions::FeeConfig,
}
impl Default for CreateSettings {
    fn default() -> Self {
        Self {
            auto_matching_enabled: true,
            kyc_execution_gate_enabled: true,
            support_enabled: true,
            consent_version: "2026-09-01".into(),
            consent_reference: "Market support consent".into(),
            fees: Default::default(),
        }
    }
}
#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    settings: CreateSettings,
}
pub fn router(
    app: AppState,
    verification: VerificationSettings,
    settings: CreateSettings,
) -> Router {
    Router::new()
        .route("/api/orderbook", post(create))
        .with_state(RouteState {
            app,
            verification: Arc::new(verification),
            settings,
        })
}
async fn create(State(state): State<RouteState>, headers: HeaderMap, body: Bytes) -> Response {
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
        let input = CreateInput::parse(serde_json::from_slice(&body).map_err(|_| invalid())?, now)?;
        let header = |name| {
            headers
                .get(name)
                .map(|v| v.to_str().map_err(|_| bad_request()))
                .transpose()
        };
        let context = header("x-verdaxis-market-support-context")?
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<Uuid>().map_err(|_| {
                    let mut e = MarketWriteError::new(MarketErrorCode::MarketSupportContextInvalid);
                    e.status = StatusCode::BAD_REQUEST;
                    e
                })
            })
            .transpose()?;
        let ip = crate::runtime::http::client_ip(None, header("x-forwarded-for")?);
        let audit = AuditContext::new(
            Some(ip),
            Some(crate::runtime::logging::request_id(header("x-request-id")?)),
        )?;
        crate::order_write::order_create::dispatch(
            state.app,
            access,
            input,
            context,
            header("idempotency-key")?.map(str::to_owned),
            audit,
            state.settings,
            now,
        )
        .await
    }
    .await;
    match result {
        Ok(reply) => (StatusCode::CREATED, axum::Json(reply.response_body)).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(pool: sqlx::PgPool, key: &str, settings: CreateSettings) -> Router {
    let config=serde_json::from_value(json!({"environment":"test","release_sha":"test","migration_revision":"p89_20261022_trade_freeze","database_url":"","db_session_time_zone":"UTC","bind":"127.0.0.1:0","read_admission_slots":8,"pool_max":9,"python_budget":0,"configured_max_connections":100,"kyc_execution_gate_enabled":settings.kyc_execution_gate_enabled})).unwrap();
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
    let verification = VerificationSettings::new(
        key.into(),
        None,
        "create-tests".into(),
        "create-tests".into(),
    );
    let routes = router(app.clone(), verification, settings);
    crate::runtime::http::router_with(routes.with_state(()), app)
}
