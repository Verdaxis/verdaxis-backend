//! Stream-token issuance for an actor admitted by the shared bearer boundary.
//!
//! Call inside `run_authenticated` so organization membership and account
//! cutoffs remain protected by its user lock. This component neither verifies
//! alternate credentials nor mounts a serving route.
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    ApiError,
    auth::{
        CurrentActor, TokenIssuer,
        error::{AuthCode, AuthFailure},
        limits::SharedLimits,
    },
};

#[derive(Serialize)]
pub struct StreamTokenResponse {
    pub stream_token: String,
}

pub struct StreamTokens {
    issuer: TokenIssuer,
    environment: String,
    limits: SharedLimits,
}

impl StreamTokens {
    pub fn new(issuer: TokenIssuer, environment: &str, limits: SharedLimits) -> Self {
        Self {
            issuer,
            environment: environment.trim().to_lowercase(),
            limits,
        }
    }

    /// The actor is the current database actor, not claims from an access token
    /// or caller-supplied organization. Authentication must precede this call.
    pub fn issue(
        &self,
        actor: &CurrentActor,
        client: &str,
        now: DateTime<Utc>,
    ) -> Result<StreamTokenResponse, ApiError> {
        let organization = actor
            .organization_id
            .ok_or_else(|| ApiError::from(AuthFailure::new(AuthCode::OrganizationRequired)))?;
        self.limits.check_endpoint(
            client,
            "/api/auth/stream-token",
            now.timestamp().max(0) as u64,
        )?;
        Ok(StreamTokenResponse {
            stream_token: self
                .issuer
                .stream(actor.id, organization, &self.environment, now),
        })
    }
}
