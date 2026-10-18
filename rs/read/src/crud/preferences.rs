//! Personal preferences. The authenticated owner holds the user lock through commit.
use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::auth::limits::SharedLimits;
use crate::auth::request::{RequestError, parse_json_body};
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, VerificationSettings, run_authenticated,
    verify_authorization,
};
use crate::{ApiError, ApiErrorKind, AppState};

const MAX_BYTES: usize = 8192;
const NOTIFICATION_FIELDS: &[&str] = &[
    "email_trade_updates",
    "email_market_alerts",
    "email_compliance_digest",
    "email_system_announcements",
    "inapp_trade_updates",
    "inapp_market_alerts",
    "inapp_order_matches",
];

fn refusal(status: StatusCode, detail: Value) -> ApiError {
    ApiError {
        status,
        detail,
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

fn diagnostic(kind: &str, loc: Value, message: &str, input: &Value) -> Value {
    json!({"type":kind,"loc":loc,"msg":message,"input":input,
        "url":format!("https://errors.pydantic.dev/2.13/v/{kind}")})
}

/// Validate without replacing the caller's JSON with a schema projection.
pub fn validate_value(namespace: &str, value: Value) -> Result<Value, ApiError> {
    let (class, fields) = match namespace {
        "market_watch" => ("MarketWatchPreferences", &["products", "portIds"][..]),
        "notifications" => ("NotificationPreferences", NOTIFICATION_FIELDS),
        "tutorial" => ("TutorialPreferences", &["completed"][..]),
        _ => {
            return Err(refusal(
                StatusCode::NOT_FOUND,
                json!("Unknown preference namespace"),
            ));
        }
    };
    // All schema-valid leaves are strings or booleans. serde_json uses the same
    // compact UTF-8 escaping as Python for these leaves (including control bytes).
    if serde_json::to_vec(&value)
        .map_err(|_| ApiError::internal())?
        .len()
        > MAX_BYTES
    {
        return Err(refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!("Preference payload too large"),
        ));
    }
    let Some(object) = value.as_object() else {
        let mut error = diagnostic(
            "model_type",
            json!([]),
            &format!("Input should be a valid dictionary or instance of {class}"),
            &value,
        );
        error["ctx"] = json!({"class_name":class});
        return Err(refusal(StatusCode::UNPROCESSABLE_ENTITY, json!([error])));
    };
    // The exported DTO retains Python enum order as well as accepted values.
    let schemas: Value = serde_json::from_str(include_str!(
        "../../../../tests/rust_contract/phase2/routes.json"
    ))
    .expect("pinned schema export");
    let products = schemas["schemas"]["preference:market_watch"]["$defs"]["MarketProduct"]["enum"]
        .as_array()
        .expect("exported product enum");
    let names: Vec<String> = products
        .iter()
        .map(|v| format!("'{}'", v.as_str().expect("product code")))
        .collect();
    let expected = format!(
        "{} or {}",
        names[..names.len() - 1].join(", "),
        names.last().unwrap()
    );
    let mut errors = vec![];
    for field in fields {
        let Some(input) = object.get(*field) else {
            errors.push(diagnostic(
                "missing",
                json!([field]),
                "Field required",
                &value,
            ));
            continue;
        };
        if namespace != "market_watch" {
            if !input.is_boolean() {
                errors.push(diagnostic(
                    "bool_type",
                    json!([field]),
                    "Input should be a valid boolean",
                    input,
                ));
            }
        } else if let Some(items) = input.as_array() {
            for (index, item) in items.iter().enumerate() {
                if *field == "portIds" && !item.is_string() {
                    errors.push(diagnostic(
                        "string_type",
                        json!([field, index]),
                        "Input should be a valid string",
                        item,
                    ));
                } else if *field == "products" && !products.contains(item) {
                    let mut error = diagnostic(
                        "enum",
                        json!([field, index]),
                        &format!("Input should be {expected}"),
                        item,
                    );
                    error["ctx"] = json!({"expected":expected});
                    errors.push(error);
                }
            }
        } else {
            errors.push(diagnostic(
                "list_type",
                json!([field]),
                "Input should be a valid list",
                input,
            ));
        }
    }
    for (key, input) in object {
        if !fields.contains(&key.as_str()) {
            errors.push(diagnostic(
                "extra_forbidden",
                json!([key]),
                "Extra inputs are not permitted",
                input,
            ));
        }
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(refusal(StatusCode::UNPROCESSABLE_ENTITY, json!(errors)))
    }
}

type PreferenceControls = fn(&HeaderMap) -> (DateTime<Utc>, Uuid);

#[derive(Clone)]
struct PreferencesState {
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
    // This closure is supplied by the disposable example only. Production
    // composition uses router(), which never reads test headers.
    controls: Option<PreferenceControls>,
}

pub fn router(
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
) -> Router {
    compose(PreferencesState {
        app,
        verification,
        limits,
        controls: None,
    })
}

#[cfg(feature = "contract-tests")]
pub fn contract_router(
    app: AppState,
    verification: Arc<VerificationSettings>,
    limits: SharedLimits,
    controls: fn(&HeaderMap) -> (DateTime<Utc>, Uuid),
) -> Router {
    assert_eq!(
        app.config.environment, "test",
        "disposable controls require test environment"
    );
    compose(PreferencesState {
        app,
        verification,
        limits,
        controls: Some(controls),
    })
}

fn compose(state: PreferencesState) -> Router {
    Router::new()
        .route("/api/users/me/preferences", get(read))
        .route("/api/users/me/preferences/{namespace}", put(write))
        .method_not_allowed_fallback(|| async {
            (
                StatusCode::METHOD_NOT_ALLOWED,
                [("allow", "GET")],
                Json(json!({"detail":"Method Not Allowed"})),
            )
        })
        .fallback(|| async { (StatusCode::NOT_FOUND, Json(json!({"detail":"Not Found"}))) })
        .route(
            "/api/users/me/preferences/",
            axum::routing::any(|headers: HeaderMap| async move {
                let host = headers
                    .get("host")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("localhost");
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [(
                        "location",
                        format!("http://{host}/api/users/me/preferences"),
                    )],
                )
            }),
        )
        .with_state(state)
}

fn authenticated_result(result: Result<Value, AuthenticatedError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(AuthenticatedError::Authentication(error)) => {
            RequestError::Authentication(error).into_response()
        }
        Err(AuthenticatedError::Database(error)) => error.into_response(),
        Err(AuthenticatedError::AmbiguousCommit) => ApiError::database().into_response(),
    }
}

async fn read(State(state): State<PreferencesState>, headers: HeaderMap) -> Response {
    let access = match verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &state.verification,
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(error) => return RequestError::Authentication(error).into_response(),
    };
    authenticated_result(
        run_authenticated(
            &state.app,
            access,
            AuthenticatedRequest::new(
                "GET",
                "/api/users/me/preferences",
                crate::auth::scope::support_present(&headers),
            ),
            |tx| {
                Box::pin(async move {
                    let actor = tx.actor().id;
                    let rows = sqlx::query(
                        "SELECT namespace,value FROM user_preferences WHERE user_id=$1",
                    )
                    .bind(actor)
                    .fetch_all(tx.connection())
                    .await
                    .map_err(|_| ApiError::database())?;
                    let mut values = serde_json::Map::new();
                    for row in rows {
                        values.insert(row.get("namespace"), row.get("value"));
                    }
                    Ok(Value::Object(values))
                })
            },
        )
        .await,
    )
}

async fn write(
    State(state): State<PreferencesState>,
    Path(namespace): Path<String>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    raw: Bytes,
) -> Response {
    let body = if raw.is_empty() {
        Value::Null
    } else {
        match parse_json_body(&raw) {
            Ok(body) => body,
            Err(mut error) => {
                // Python JSONDecoder reports this common malformed-object case
                // at the insertion position rather than serde's last byte.
                let reason = error.detail[0]["ctx"]["error"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                if reason.starts_with("EOF while parsing an object")
                    || reason.starts_with("trailing comma")
                    || reason.starts_with("key must be a string")
                {
                    error.detail[0]["ctx"]["error"] =
                        json!("Expecting property name enclosed in double quotes");
                    if reason.starts_with("EOF") {
                        error.detail[0]["loc"][1] = json!(raw.len());
                    }
                }
                return error.into_response();
            }
        }
    };
    let access = match verify_authorization(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &state.verification,
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(error) => return RequestError::Authentication(error).into_response(),
    };
    let request = AuthenticatedRequest::new(
        "PUT",
        "/api/users/me/preferences/{namespace}",
        crate::auth::scope::support_present(&headers),
    );
    authenticated_result(run_authenticated(&state.app, access, request, move |tx| Box::pin(async move {
        if body.is_null() {
            return Err(refusal(StatusCode::UNPROCESSABLE_ENTITY, json!([{"type":"missing","loc":["body"],"msg":"Field required","input":null}])));
        }
        // Python's Any body has no schema validator before the endpoint wrapper:
        // the quota is consumed before namespace/size/schema validation.
        let client = crate::runtime::http::client_ip(peer.map(|p| p.0.0.ip()), headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()));
        state.limits.check_endpoint(&client, "/api/users/me/preferences/", Utc::now().timestamp() as u64)?;
        let value = validate_value(&namespace, body)?;
        let (now, id) = state.controls.map(|controls| controls(&headers)).unwrap_or_else(|| (Utc::now(), Uuid::new_v4()));
        let actor = tx.actor().id;
        sqlx::query("INSERT INTO user_preferences(id,user_id,namespace,value,updated_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id,namespace) DO UPDATE SET value=EXCLUDED.value,updated_at=EXCLUDED.updated_at")
            .bind(id).bind(actor).bind(namespace).bind(&value).bind(now).execute(tx.connection()).await.map_err(|_| ApiError::database())?;
        Ok(json!({"value":value,"updated_at":now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi,false)}))
    })).await)
}
