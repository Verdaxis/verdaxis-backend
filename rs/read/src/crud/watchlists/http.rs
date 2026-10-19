//! Authenticated watchlist routes, composed explicitly with verification policy.
//!
//! `router` is the one composition point for the watchlist group. Handlers for
//! the container and target writes live in their own modules and share `run`.
use super::validation::{path_uuid, unprocessable};
use super::{capture_visible, containers, load_legacy_list, targets};
use crate::auth::{
    AuthenticatedError, AuthenticatedRequest, AuthenticatedTransaction, VerificationSettings,
    run_authenticated, verify_authorization,
};
use crate::{ApiError, ApiErrorKind, AppState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Arc};
use uuid::Uuid;

/// Generated values for one write request: the row clock and row identifiers.
///
/// Production requests use the real clock and random identifiers. A disposable
/// run supplies the same clock and identifier sequence to both stacks so every
/// generated value stays in the response and state comparison.
pub struct WriteControls {
    now: DateTime<Utc>,
    ids: VecDeque<Uuid>,
    controlled: bool,
    pub(super) fail_summary: bool,
}

impl WriteControls {
    /// The production source of generated values.
    pub fn real() -> Self {
        Self {
            now: Utc::now(),
            ids: VecDeque::new(),
            controlled: false,
            fail_summary: false,
        }
    }

    /// `fail_summary` makes the summary read after a committed write fail.
    pub fn controlled(now: DateTime<Utc>, ids: Vec<Uuid>, fail_summary: bool) -> Self {
        Self {
            now,
            ids: ids.into(),
            controlled: true,
            fail_summary,
        }
    }

    pub(super) fn now(&self) -> DateTime<Utc> {
        self.now
    }

    pub(super) fn next_id(&mut self) -> Uuid {
        match self.ids.pop_front() {
            Some(id) => id,
            None if self.controlled => panic!("controlled identifier sequence exhausted"),
            None => Uuid::new_v4(),
        }
    }
}

pub type ControlSource = fn(&HeaderMap) -> WriteControls;

#[derive(Clone)]
pub struct WatchlistSettings {
    verification: Arc<VerificationSettings>,
    best_price_cutoff: DateTime<Utc>,
    controls: Option<ControlSource>,
}

impl WatchlistSettings {
    pub fn new(verification: VerificationSettings, best_price_cutoff: DateTime<Utc>) -> Self {
        Self {
            verification: Arc::new(verification),
            best_price_cutoff,
            controls: None,
        }
    }

    /// Disposable runs only. Release composition never calls this.
    #[cfg(feature = "contract-tests")]
    pub fn with_controls(mut self, controls: ControlSource) -> Self {
        self.controls = Some(controls);
        self
    }

    pub(super) fn best_price_cutoff(&self) -> DateTime<Utc> {
        self.best_price_cutoff
    }

    pub(super) fn controls_for(&self, headers: &HeaderMap) -> WriteControls {
        match self.controls {
            Some(source) => source(headers),
            None => WriteControls::real(),
        }
    }
}

#[derive(Clone)]
pub(super) struct WatchlistState {
    pub(super) app: AppState,
    pub(super) settings: WatchlistSettings,
}

pub fn router(app: AppState, settings: WatchlistSettings) -> Router {
    Router::new()
        .route("/api/watchlists", get(list).post(containers::create))
        .route("/api/watchlists/me", post(containers::radar))
        .route(
            "/api/watchlists/{watchlist_id}",
            get(detail).delete(containers::delete),
        )
        .route(
            "/api/watchlists/{watchlist_id}/targets",
            post(targets::create),
        )
        .route(
            "/api/watchlists/{watchlist_id}/targets/{target_id}",
            axum::routing::delete(targets::delete),
        )
        .with_state(WatchlistState { app, settings })
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

pub(super) fn not_found(detail: &'static str) -> ApiError {
    refusal(StatusCode::NOT_FOUND, detail)
}

pub(super) fn refusal(status: StatusCode, detail: impl Into<String>) -> ApiError {
    ApiError {
        status,
        detail: serde_json::Value::String(detail.into()),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

/// Verify the bearer, then run `work` in the one authenticated transaction.
/// Authentication failures and database errors are already responses.
pub(super) async fn run<T: Send + 'static>(
    state: &WatchlistState,
    headers: &HeaderMap,
    method: &str,
    template: &str,
    work: impl for<'a> FnOnce(
        &'a mut AuthenticatedTransaction<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>
    + Send
    + 'static,
) -> Result<T, Response> {
    let access = verify_authorization(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        &state.settings.verification,
        Utc::now(),
    )
    .map_err(|error| crate::auth::request::RequestError::Authentication(error).into_response())?;
    let request = AuthenticatedRequest::new(
        method,
        template,
        crate::auth::scope::support_present(headers),
    );
    run_authenticated(&state.app, access, request, work)
        .await
        .map_err(error)
}

async fn list(State(state): State<WatchlistState>, headers: HeaderMap) -> Response {
    let result = run(&state, &headers, "GET", "/api/watchlists", |transaction| {
        Box::pin(async move {
            let actor = transaction.actor().id;
            load_legacy_list(transaction.connection(), actor)
                .await
                .map_err(|_| ApiError::database())
        })
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn detail(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let now = Utc::now();
    let policy = state.app.policy.clone();
    let config = state.app.config.clone();
    let cutoff = state.settings.best_price_cutoff();
    let result = run(
        &state,
        &headers,
        "GET",
        "/api/watchlists/{watchlist_id}",
        move |transaction| {
            Box::pin(async move {
                // User state precedes path validation, as in the FastAPI dependency chain.
                let id =
                    path_uuid("watchlist_id", &raw).map_err(|item| unprocessable(vec![item]))?;
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
                    .detail(transaction.connection(), actor, id, cutoff)
                    .await
                    .map_err(|_| ApiError::database())?
                    .ok_or_else(|| not_found("Watchlist not found"))
            })
        },
    )
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant() -> DateTime<Utc> {
        "2026-10-19T00:00:00Z".parse().unwrap()
    }

    #[test]
    fn controlled_identifiers_are_consumed_in_order() {
        let mut controls = WriteControls::controlled(
            instant(),
            vec![Uuid::from_u128(1), Uuid::from_u128(2)],
            false,
        );
        assert_eq!(controls.now(), instant());
        assert_eq!(controls.next_id(), Uuid::from_u128(1));
        assert_eq!(controls.next_id(), Uuid::from_u128(2));
    }

    #[test]
    #[should_panic(expected = "controlled identifier sequence exhausted")]
    fn an_exhausted_controlled_sequence_never_falls_back_to_random() {
        let mut controls = WriteControls::controlled(instant(), vec![], false);
        controls.next_id();
    }

    #[test]
    fn real_controls_generate_distinct_identifiers() {
        let mut controls = WriteControls::real();
        assert_ne!(controls.next_id(), controls.next_id());
        assert!(!controls.fail_summary);
    }
}
