//! Composition-time credentials for notifications; runtime secret loading remains integration-owned.
use std::{net::SocketAddr, time::Instant};

use axum::{
    extract::ConnectInfo,
    http::{HeaderMap, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};
use chrono::Utc;

use crate::{
    ApiError,
    auth::{
        AuthError, AuthenticatedError, VerificationSettings, VerifiedAccess,
        limits::SharedLimits,
        request::{RequestAdmission, RequestError},
        verify_authorization,
    },
    runtime::http::client_ip,
};

/// Supply the same quota authority used by the outer HTTP middleware.
pub struct NotificationAuthority {
    settings: VerificationSettings,
    limits: SharedLimits,
    started: Instant,
}

impl NotificationAuthority {
    pub fn new(settings: VerificationSettings, limits: SharedLimits) -> Self {
        Self {
            settings,
            limits,
            started: Instant::now(),
        }
    }

    pub fn authenticate(&self, headers: &HeaderMap) -> Result<VerifiedAccess, AuthError> {
        verify_authorization(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            &self.settings,
            Utc::now(),
        )
    }

    pub fn request_admission(&self, path: &str, client: String) -> RequestAdmission {
        RequestAdmission {
            client,
            path: path.to_owned(),
            now_seconds: self.started.elapsed().as_secs(),
            limits: self.limits.clone(),
        }
    }
}

pub(super) fn client_of<B>(request: &axum::http::Request<B>) -> String {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let forwarded = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok());
    client_ip(peer, forwarded)
}

pub enum NotificationError {
    Authentication(AuthError),
    Transaction(AuthenticatedError),
}

impl From<AuthenticatedError> for NotificationError {
    fn from(error: AuthenticatedError) -> Self {
        Self::Transaction(error)
    }
}

impl IntoResponse for NotificationError {
    fn into_response(self) -> Response {
        match self {
            Self::Authentication(error)
            | Self::Transaction(AuthenticatedError::Authentication(error)) => {
                RequestError::Authentication(error).into_response()
            }
            Self::Transaction(AuthenticatedError::Database(error)) => error.into_response(),
            // Never retry a write whose commit acknowledgement was lost.
            Self::Transaction(AuthenticatedError::AmbiguousCommit) => {
                ApiError::database().into_response()
            }
        }
    }
}
