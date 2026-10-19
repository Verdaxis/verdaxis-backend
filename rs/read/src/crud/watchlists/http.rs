//! Authenticated watchlist reads, composed explicitly with verification policy.
use super::validation::{path_uuid, unprocessable};
use super::{capture_visible, load_legacy_list};
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, VerificationSettings, run_authenticated,
    verify_authorization,
};
use crate::{ApiError, ApiErrorKind, AppState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Clone)]
pub struct ReadSettings {
    verification: Arc<VerificationSettings>,
    best_price_cutoff: DateTime<Utc>,
}
impl ReadSettings {
    pub fn new(verification: VerificationSettings, best_price_cutoff: DateTime<Utc>) -> Self {
        Self {
            verification: Arc::new(verification),
            best_price_cutoff,
        }
    }
}

pub fn read_routes(settings: ReadSettings) -> Router<AppState> {
    let list_settings = settings.clone();
    Router::new()
        .route(
            "/api/watchlists",
            get(move |State(state): State<AppState>, headers: HeaderMap| {
                let settings = list_settings.clone();
                async move { list(state, headers, settings).await }
            }),
        )
        .route(
            "/api/watchlists/{watchlist_id}",
            get(
                move |State(state): State<AppState>,
                      headers: HeaderMap,
                      Path(raw): Path<String>| {
                    let settings = settings.clone();
                    async move { detail(state, headers, raw, settings).await }
                },
            ),
        )
}

fn error(error: AuthenticatedError) -> Response {
    match error {
        AuthenticatedError::Authentication(error) => {
            crate::auth::request::RequestError::Authentication(error).into_response()
        }
        AuthenticatedError::Database(error) => error.into_response(),
        AuthenticatedError::AmbiguousCommit => ApiError::database().into_response(),
    }
}

async fn list(state: AppState, headers: HeaderMap, settings: ReadSettings) -> Response {
    let access = match verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &settings.verification,
        Utc::now(),
    ) {
        Ok(access) => access,
        Err(error) => {
            return crate::auth::request::RequestError::Authentication(error).into_response();
        }
    };
    let request = AuthenticatedRequest::new(
        "GET",
        "/api/watchlists",
        crate::auth::scope::support_present(&headers),
    );
    match run_authenticated(&state, access, request, |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().id;
            load_legacy_list(transaction.connection(), actor)
                .await
                .map_err(|_| ApiError::database())
        })
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(failure) => error(failure),
    }
}

async fn detail(
    state: AppState,
    headers: HeaderMap,
    raw: String,
    settings: ReadSettings,
) -> Response {
    let now = Utc::now();
    let access = match verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &settings.verification,
        now,
    ) {
        Ok(access) => access,
        Err(error) => {
            return crate::auth::request::RequestError::Authentication(error).into_response();
        }
    };
    let request = AuthenticatedRequest::new(
        "GET",
        "/api/watchlists/{watchlist_id}",
        crate::auth::scope::support_present(&headers),
    );
    let policy = state.policy.clone();
    let config = state.config.clone();
    match run_authenticated(&state, access, request, move |transaction| {
        Box::pin(async move {
            // User state precedes path validation, as in the FastAPI dependency chain.
            let id = path_uuid("watchlist_id", &raw).map_err(|item| unprocessable(vec![item]))?;
            let actor = transaction.actor().id;
            let visible = capture_visible(
                transaction.connection(),
                &policy,
                &crate::market::visibility::VisibilityClock::at(now),
                &config,
            )
            .await
            .map_err(|_| ApiError::database())?;
            visible
                .detail(
                    transaction.connection(),
                    actor,
                    id,
                    settings.best_price_cutoff,
                )
                .await
                .map_err(|_| ApiError::database())?
                .ok_or(ApiError {
                    status: StatusCode::NOT_FOUND,
                    detail: serde_json::json!("Watchlist not found"),
                    retry_after: None,
                    kind: ApiErrorKind::Expected,
                })
        })
    })
    .await
    {
        Ok(value) => Json(value).into_response(),
        Err(failure) => error(failure),
    }
}
