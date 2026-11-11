//! Release-switched assistant routes and the process-lifetime model transport.
use std::sync::Arc;

use axum::{Router, middleware};

use crate::{
    AppState,
    agent::{llm::GoClient, routes::RouteState, turn::ModelClient},
    auth::limits::SharedLimits,
};

use super::agent_config::AgentOptions;

pub const SWITCH: &str = "AGENT_ENABLED";

pub(crate) fn startup_model(options: &AgentOptions) -> Option<Arc<dyn ModelClient>> {
    if !options.enabled {
        return None;
    }
    match GoClient::from_environment() {
        Ok(client) => Some(Arc::new(client)),
        Err(code) => {
            tracing::warn!(error_code = ?code, "assistant routes not mounted");
            None
        }
    }
}

/// All assistant routes remain absent unless both startup gates succeed.
pub fn mounts(app: &AppState) -> bool {
    app.config.phase2.agent.enabled && app.agent_model.is_some() && app.phase2.is_some()
}

pub(crate) fn routes(app: &AppState, limits: SharedLimits) -> Option<Router> {
    if !mounts(app) {
        return None;
    }
    let state = RouteState {
        app: app.clone(),
        model_client: app.agent_model.as_ref()?.clone(),
    };
    let admin = crate::agent::routes::admin_routes(state.clone()).route_layer(
        middleware::from_fn_with_state(limits, super::offboarding::admin_quota),
    );
    Some(crate::agent::routes::user_routes(state).merge(admin))
}
