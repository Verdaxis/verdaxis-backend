//! The individual offboarding serving composition and its release switch.
//!
//! One switch, `OFFBOARDING_ENABLED` in the deployed environment, mounts four
//! administrator routes together:
//!
//! - `POST /api/auth/admin/users/{user_id}/offboard`: the offboarding command;
//! - `GET /api/auth/admin/users/{user_id}/offboarding-preview`: its preview;
//! - `GET /api/auth/admin/users/{user_id}/offboarding-trades`: the obligation
//!   list;
//! - `POST /api/auth/admin/trades/{trade_id}/reassign-responsible`: the
//!   responsible-person reassignment.
//!
//! The switch is off unless the environment turns it on. While it is off,
//! [`routes`] returns `None` and no served router has any of the four routes:
//! there is no partial command and no fallback. The command is composed only
//! with the real user-scoped market invalidation
//! ([`UserMarketInvalidation`]); no configuration selects another market
//! implementation. Python has no route on these paths. Caddy sends them to
//! Rust only through `deploy/caddy/rust-offboarding.caddy`.
//!
//! Every request to the group draws from the one `/api/admin/` client quota
//! of the shared quota authority before its handler runs, the same counter
//! the pre-authentication gate charges for every administrator request.
use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    middleware::{self, Next},
    response::Response,
};

use crate::{
    AppState,
    auth::limits::SharedLimits,
    business::{
        admission::AdmissionEvents,
        offboarding::{OffboardingService, UserMarketInvalidation, router},
    },
};

/// The deployed environment key of the release switch.
pub const SWITCH: &str = "OFFBOARDING_ENABLED";

/// True when the served router mounts the offboarding group: the switch is
/// on, and the Phase 2 and market write settings it needs exist.
pub fn mounts(app: &AppState) -> bool {
    app.config.phase2.offboarding_enabled && app.phase2.is_some() && app.market_write.is_some()
}

/// The serving parts of the command: the user-scoped market invalidation with
/// this process's market policy and configuration, and the admission event
/// producer.
pub fn service(
    app: &AppState,
) -> OffboardingService<UserMarketInvalidation, impl AdmissionEvents + use<>> {
    OffboardingService {
        market: Arc::new(UserMarketInvalidation::for_app(app)),
        events: Arc::new(super::phase4_adapters::Events(app.clone())),
    }
}

/// The four routes behind the release switch, or `None` while it is off.
/// Both route groups mount together or not at all.
pub(crate) fn routes(app: &AppState, limits: SharedLimits) -> Option<Router> {
    if !mounts(app) {
        return None;
    }
    let verification = app.phase2.as_ref()?.verification.clone();
    let command = router(app.clone(), verification, service(app));
    let reassignment = super::market_write::reassignment_routes(app)?;
    Some(
        command
            .merge(reassignment)
            .route_layer(middleware::from_fn_with_state(limits, admin_quota)),
    )
}

/// Charge the request to the client's `/api/admin/` quota before its handler
/// runs, so an unauthenticated flood is refused before any database work.
async fn admin_quota(State(limits): State<SharedLimits>, request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client = super::http::client_ip(
        peer,
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
    );
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if !limits.admit_admin(&client, now) {
        return super::http::rate_limit_response();
    }
    next.run(request).await
}
