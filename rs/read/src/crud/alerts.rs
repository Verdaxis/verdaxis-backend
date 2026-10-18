//! Organization-owned price-alert CRUD. Triggering remains Python-owned.
#[cfg(feature = "contract-tests")]
#[path = "alerts_contract.rs"]
pub mod contract;
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::auth::limits::SharedLimits;
use crate::auth::request::{RequestAdmission, RequestError, parse_json_body, validate_and_admit};
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, VerificationSettings, run_authenticated,
    verify_authorization,
};
use crate::market::decimal::ExactDecimal;
use crate::{ApiError, ApiErrorKind, AppState};

const FREE_TIER_LIMIT: i64 = 5;

#[derive(Debug)]
pub struct AlertCreate {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub direction: String,
    /// Bind decimal text and let PostgreSQL apply NUMERIC(10,2), not a float.
    pub threshold_usd: String,
}

fn refusal(status: StatusCode, detail: &str) -> ApiError {
    ApiError {
        status,
        detail: json!(detail),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn validation(errors: Vec<Value>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!(errors),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn field_error(field: &str, kind: &str, message: &str, input: &Value) -> Value {
    json!({"type":kind,"loc":["body",field],"msg":message,"input":input})
}

fn uuid_value(field: &str, value: &Value) -> Result<Uuid, Value> {
    let Some(text) = value.as_str() else {
        return Err(field_error(
            field,
            "uuid_type",
            "UUID input should be a string, bytes or UUID object",
            value,
        ));
    };
    Uuid::parse_str(text).map_err(|cause| {
        let cause = cause.to_string();
        // Pydantic's UUID parser reports character positions from one.
        let cause = if cause.starts_with("invalid character:") {
            cause
                .rsplit_once(" at ")
                .and_then(|(prefix, position)| {
                    position
                        .parse::<usize>()
                        .ok()
                        .map(|position| format!("{prefix} at {}", position + 1))
                })
                .unwrap_or(cause)
        } else if let Some(length) = cause.strip_prefix("invalid length: found ") {
            format!("invalid length: expected length 32 for simple format, found {length}")
        } else {
            cause
        };
        let mut error = field_error(
            field,
            "uuid_parsing",
            &format!("Input should be a valid UUID, {cause}"),
            value,
        );
        error["ctx"] = json!({"error":cause});
        error
    })
}

impl AlertCreate {
    pub fn validate(body: Value) -> Result<Self, ApiError> {
        let Some(object) = body.as_object() else {
            let (kind, message) = if body.is_null() {
                ("missing", "Field required")
            } else {
                (
                    "model_attributes_type",
                    "Input should be a valid dictionary or object to extract fields from",
                )
            };
            return Err(validation(vec![
                json!({"type":kind,"loc":["body"],"msg":message,"input":body}),
            ]));
        };
        let mut errors = Vec::new();
        let mut required = |name: &str| {
            if !object.contains_key(name) {
                errors.push(field_error(name, "missing", "Field required", &body));
            }
        };
        // Collect in schema order, including independent field errors.
        required("product_id");
        let product_id =
            object
                .get("product_id")
                .and_then(|value| match uuid_value("product_id", value) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        errors.push(error);
                        None
                    }
                });
        let delivery_point_id = match object.get("delivery_point_id") {
            None | Some(Value::Null) => None,
            Some(value) => match uuid_value("delivery_point_id", value) {
                Ok(id) => Some(id),
                Err(error) => {
                    errors.push(error);
                    None
                }
            },
        };
        let direction = match object.get("direction") {
            None => {
                errors.push(field_error("direction", "missing", "Field required", &body));
                None
            }
            Some(value) => match value.as_str() {
                Some("above" | "below") => value.as_str().map(str::to_owned),
                Some(_) => {
                    let mut error = field_error(
                        "direction",
                        "value_error",
                        "Value error, direction must be 'above' or 'below'",
                        value,
                    );
                    error["ctx"] = json!({"error":{}});
                    errors.push(error);
                    None
                }
                None => {
                    errors.push(field_error(
                        "direction",
                        "string_type",
                        "Input should be a valid string",
                        value,
                    ));
                    None
                }
            },
        };
        let threshold_usd = match object.get("threshold_usd") {
            None => {
                errors.push(field_error(
                    "threshold_usd",
                    "missing",
                    "Field required",
                    &body,
                ));
                None
            }
            Some(value) => match threshold(value) {
                Ok(text) => Some(text),
                Err(error) => {
                    errors.push(error);
                    None
                }
            },
        };
        if !errors.is_empty() {
            return Err(validation(errors));
        }
        Ok(Self {
            product_id: product_id.expect("validated UUID"),
            delivery_point_id,
            direction: direction.expect("validated direction"),
            threshold_usd: threshold_usd.expect("validated decimal"),
        })
    }
}

fn threshold(value: &Value) -> Result<String, Value> {
    let text = match value {
        Value::String(text) => text.trim().replace('_', ""),
        Value::Number(number) => number.to_string(),
        _ => {
            return Err(field_error(
                "threshold_usd",
                "decimal_type",
                "Decimal input should be an integer, float, string or Decimal object",
                value,
            ));
        }
    };
    let unsigned = text.trim_start_matches(['+', '-']).to_ascii_lowercase();
    if unsigned == "inf"
        || unsigned == "infinity"
        || unsigned.starts_with("nan")
        || unsigned.starts_with("snan")
    {
        return Err(field_error(
            "threshold_usd",
            "finite_number",
            "Input should be a finite number",
            value,
        ));
    }
    let decimal = ExactDecimal::parse(&text).map_err(|_| {
        field_error(
            "threshold_usd",
            "decimal_parsing",
            "Input should be a valid decimal",
            value,
        )
    })?;
    if decimal <= 0 {
        let mut error = field_error(
            "threshold_usd",
            "value_error",
            "Value error, threshold_usd must be positive",
            value,
        );
        error["ctx"] = json!({"error":{}});
        return Err(error);
    }
    Ok(text)
}

pub fn organization_id(id: Option<Uuid>) -> Result<Uuid, ApiError> {
    id.ok_or_else(|| {
        refusal(
            StatusCode::BAD_REQUEST,
            "User must belong to an organization",
        )
    })
}

pub fn authorize_delete(organization: Uuid, owner: Option<Uuid>) -> Result<(), ApiError> {
    match owner {
        None => Err(refusal(StatusCode::NOT_FOUND, "Alert not found")),
        Some(owner) if owner != organization => Err(refusal(
            StatusCode::FORBIDDEN,
            "You can only delete your own alerts",
        )),
        Some(_) => Ok(()),
    }
}

fn project(row: sqlx::postgres::PgRow, policy: &crate::MarketPolicy) -> Value {
    let name: Option<String> = row.get("product_name");
    let fuel_type: Option<String> = row.get("fuel_type");
    let fuel_grade: Option<String> = row.get("fuel_grade");
    // Python derives this property from the exact label tuple, not the row ID
    // or activity flag. Legacy and inactive products still remain listable.
    let market_product = policy
        .products
        .iter()
        .find(|product| {
            name.as_deref() == Some(product.name.as_str())
                && fuel_type.as_deref() == Some(product.fuel_type.as_str())
                && fuel_grade.as_deref() == Some(product.fuel_grade.as_str())
        })
        .map(|product| product.code.as_str());
    let created: chrono::DateTime<chrono::Utc> = row.get("created_at");
    let triggered: Option<chrono::DateTime<chrono::Utc>> = row.get("triggered_at");
    json!({
        "id":row.get::<Uuid,_>("id"), "org_id":row.get::<Uuid,_>("org_id"),
        "product_id":row.get::<Uuid,_>("product_id"), "product_name":row.get::<Option<String>,_>("product_name"),
        "market_product":market_product,
        "delivery_point_id":row.get::<Option<Uuid>,_>("delivery_point_id"),
        "direction":row.get::<String,_>("direction"), "threshold_usd":row.get::<String,_>("threshold_usd"),
        "is_active":row.get::<bool,_>("is_active"), "triggered_at":triggered, "created_at":created
    })
}

/// Caller owns the authenticated transaction. No implicit retry or org lock:
/// the source organization-cap race must be compared by the runner lane.
pub async fn create(
    connection: &mut PgConnection,
    organization: Uuid,
    input: AlertCreate,
    policy: &crate::MarketPolicy,
) -> Result<Value, ApiError> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM price_alerts WHERE org_id=$1 AND is_active=true")
            .bind(organization)
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| ApiError::database())?;
    if count >= FREE_TIER_LIMIT {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "Free tier limit of 5 active alerts reached. Delete an existing alert to create a new one.",
        ));
    }
    #[cfg(feature = "contract-tests")]
    contract::after_count().await;
    #[cfg(feature = "contract-tests")]
    let id = contract::id().unwrap_or_else(Uuid::new_v4);
    #[cfg(not(feature = "contract-tests"))]
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO price_alerts (id,org_id,product_id,delivery_point_id,direction,threshold_usd,is_active) VALUES ($1,$2,$3,$4,$5,$6::text::numeric,true)")
        .bind(id).bind(organization).bind(input.product_id).bind(input.delivery_point_id).bind(input.direction).bind(input.threshold_usd)
        .execute(&mut *connection).await.map_err(|_| ApiError::database())?;
    #[cfg(feature = "contract-tests")]
    contract::after_insert(connection, id).await?;
    let row = sqlx::query("SELECT a.id,a.org_id,a.product_id,p.name AS product_name,p.fuel_type,p.fuel_grade,a.delivery_point_id,a.direction,a.threshold_usd::text AS threshold_usd,a.is_active,a.triggered_at,a.created_at FROM price_alerts a LEFT JOIN products p ON p.id=a.product_id WHERE a.id=$1").bind(id).fetch_one(&mut *connection).await.map_err(|_| ApiError::database())?;
    Ok(project(row, policy))
}

pub async fn list(
    connection: &mut PgConnection,
    organization: Uuid,
    policy: &crate::MarketPolicy,
) -> Result<Value, ApiError> {
    let rows = sqlx::query("SELECT a.id,a.org_id,a.product_id,p.name AS product_name,p.fuel_type,p.fuel_grade,a.delivery_point_id,a.direction,a.threshold_usd::text AS threshold_usd,a.is_active,a.triggered_at,a.created_at FROM price_alerts a LEFT JOIN products p ON p.id=a.product_id WHERE a.org_id=$1 ORDER BY a.created_at DESC").bind(organization).fetch_all(connection).await.map_err(|_| ApiError::database())?;
    Ok(Value::Array(
        rows.into_iter().map(|row| project(row, policy)).collect(),
    ))
}

pub async fn delete(
    connection: &mut PgConnection,
    organization: Uuid,
    id: Uuid,
) -> Result<(), ApiError> {
    let owner = sqlx::query_scalar::<_, Uuid>("SELECT org_id FROM price_alerts WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|_| ApiError::database())?;
    authorize_delete(organization, owner)?;
    sqlx::query("DELETE FROM price_alerts WHERE id=$1")
        .bind(id)
        .execute(connection)
        .await
        .map_err(|_| ApiError::database())?;
    Ok(())
}

#[derive(Clone)]
struct RouteState {
    app: AppState,
    verification: std::sync::Arc<VerificationSettings>,
    limits: SharedLimits,
}

/// Runtime composition supplies verification settings; this lane does not read
/// secrets or add a second runtime configuration path.
pub fn router(app: AppState, verification: VerificationSettings, limits: SharedLimits) -> Router {
    Router::new()
        .route("/api/alerts", get(get_alerts).post(post_alert))
        .route(
            "/api/alerts/{alert_id}",
            axum::routing::delete(delete_alert),
        )
        .with_state(RouteState {
            app,
            verification: std::sync::Arc::new(verification),
            limits,
        })
}

fn completed(result: Result<Response, AuthenticatedError>) -> Response {
    match result {
        Ok(response) => response,
        Err(AuthenticatedError::Authentication(error)) => {
            RequestError::Authentication(error).into_response()
        }
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

async fn dispatch(
    state: RouteState,
    headers: HeaderMap,
    method: &'static str,
    body: Option<Value>,
    id: Option<String>,
) -> Response {
    let now = chrono::Utc::now();
    let access = match verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &state.verification,
        now,
    ) {
        Ok(access) => access,
        Err(error) => return RequestError::Authentication(error).into_response(),
    };
    let request = AuthenticatedRequest::new(method, "/api/alerts", false);
    let admission = RequestAdmission {
        client: "unused-unlimited-alert-route".into(),
        path: "/api/alerts".into(),
        now_seconds: now.timestamp().max(0) as u64,
        limits: state.limits,
    };
    let policy = state.app.policy.clone();
    #[cfg(feature = "contract-tests")]
    let controls = contract::capture();
    completed(
        run_authenticated(&state.app, access, request, move |tx| {
            Box::pin(async move {
                #[cfg(feature = "contract-tests")]
                return contract::CONTROLS
                    .scope(controls, async move {
                        execute_request(tx, method, body, id, admission, policy).await
                    })
                    .await;
                #[cfg(not(feature = "contract-tests"))]
                execute_request(tx, method, body, id, admission, policy).await
            })
        })
        .await,
    )
}

async fn execute_request(
    tx: &mut crate::auth::AuthenticatedTransaction<'_>,
    method: &str,
    body: Option<Value>,
    id: Option<String>,
    admission: RequestAdmission,
    policy: std::sync::Arc<crate::MarketPolicy>,
) -> Result<Response, ApiError> {
    let input = if method == "POST" {
        Some(validate_and_admit(
            body.unwrap_or(Value::Null),
            &admission,
            AlertCreate::validate,
        )?)
    } else {
        None
    };
    let id = if let Some(text) = id {
        Some(uuid_value("alert_id", &json!(text)).map_err(|mut error| {
            error["loc"] = json!(["path", "alert_id"]);
            validation(vec![error])
        })?)
    } else {
        None
    };
    let organization = organization_id(tx.actor().organization_id)?;
    match method {
        "POST" => Ok((
            StatusCode::CREATED,
            axum::Json(
                create(
                    tx.connection(),
                    organization,
                    input.expect("POST input"),
                    &policy,
                )
                .await?,
            ),
        )
            .into_response()),
        "DELETE" => {
            delete(tx.connection(), organization, id.expect("DELETE id")).await?;
            Ok((
                StatusCode::NO_CONTENT,
                [("content-type", "application/json")],
            )
                .into_response())
        }
        _ => Ok(axum::Json(list(tx.connection(), organization, &policy).await?).into_response()),
    }
}

async fn post_alert(State(state): State<RouteState>, headers: HeaderMap, raw: Bytes) -> Response {
    match parse_json_body(&raw) {
        Ok(body) => {
            #[cfg(feature = "contract-tests")]
            if contract::enabled(&headers) {
                return contract::scope(
                    headers.clone(),
                    dispatch(state, headers, "POST", Some(body), None),
                )
                .await;
            }
            dispatch(state, headers, "POST", Some(body), None).await
        }
        Err(error) => error.into_response(),
    }
}
async fn get_alerts(State(state): State<RouteState>, headers: HeaderMap) -> Response {
    dispatch(state, headers, "GET", None, None).await
}
async fn delete_alert(
    State(state): State<RouteState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    dispatch(state, headers, "DELETE", None, Some(id)).await
}
