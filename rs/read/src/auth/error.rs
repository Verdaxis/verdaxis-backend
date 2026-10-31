//! Errors returned by the credential and session routes, and the auth request
//! body limit.
//!
//! Phase 2's [`super::AuthError`] stays the access-token verifier's error.
//! Credential routes return [`AuthFailure`], which always renders the client
//! envelope `{"detail": {"code": "...", "message": "..."}}`. A code fixes its
//! HTTP status, so a failure cannot pair a code with the wrong status.

use axum::{
    body::{Body, Bytes},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_LENGTH, RETRY_AFTER, WWW_AUTHENTICATE},
    },
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::{ApiError, ApiErrorKind};

/// The largest request body an auth route reads: 128 KiB, the limit of the
/// Python auth routes. The largest valid auth body is a few kilobytes.
pub const AUTH_BODY_LIMIT_BYTES: usize = 128 * 1024;

/// Credential and session route codes, including the shared HTTP boundary
/// (origin, rate limit, body size and validation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthCode {
    CredentialsInvalid,
    EmailUnverified,
    AccountNotActive,
    RefreshTokenMissing,
    RefreshTokenExpired,
    RefreshTokenInvalid,
    RefreshSessionRevoked,
    RefreshTokenReplayed,
    RefreshAccountInactive,
    RefreshPasswordChanged,
    RefreshDeviceRequired,
    RefreshRotationInProgress,
    OrganizationRequired,
    CurrentPasswordInvalid,
    InvitationInvalid,
    ResetTokenInvalid,
    EmailTokenInvalid,
    RegistrationTokenInvalid,
    RegistrationConflict,
    BrowserOriginRejected,
    RateLimited,
    RequestTooLarge,
    RequestInvalid,
    ValidationFailed,
}

impl AuthCode {
    /// Every code, for tests and inventories. Add a new variant here too.
    pub const ALL: [Self; 24] = [
        Self::CredentialsInvalid,
        Self::EmailUnverified,
        Self::AccountNotActive,
        Self::RefreshTokenMissing,
        Self::RefreshTokenExpired,
        Self::RefreshTokenInvalid,
        Self::RefreshSessionRevoked,
        Self::RefreshTokenReplayed,
        Self::RefreshAccountInactive,
        Self::RefreshPasswordChanged,
        Self::RefreshDeviceRequired,
        Self::RefreshRotationInProgress,
        Self::OrganizationRequired,
        Self::CurrentPasswordInvalid,
        Self::InvitationInvalid,
        Self::ResetTokenInvalid,
        Self::EmailTokenInvalid,
        Self::RegistrationTokenInvalid,
        Self::RegistrationConflict,
        Self::BrowserOriginRejected,
        Self::RateLimited,
        Self::RequestTooLarge,
        Self::RequestInvalid,
        Self::ValidationFailed,
    ];

    /// The wire code the frontend reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CredentialsInvalid => "AUTH_CREDENTIALS_INVALID",
            Self::EmailUnverified => "AUTH_EMAIL_UNVERIFIED",
            Self::AccountNotActive => "AUTH_ACCOUNT_NOT_ACTIVE",
            Self::RefreshTokenMissing => "REFRESH_TOKEN_MISSING",
            Self::RefreshTokenExpired => "REFRESH_TOKEN_EXPIRED",
            Self::RefreshTokenInvalid => "REFRESH_TOKEN_INVALID",
            Self::RefreshSessionRevoked => "REFRESH_SESSION_REVOKED",
            Self::RefreshTokenReplayed => "REFRESH_TOKEN_REPLAYED",
            Self::RefreshAccountInactive => "REFRESH_ACCOUNT_INACTIVE",
            Self::RefreshPasswordChanged => "REFRESH_PASSWORD_CHANGED",
            Self::RefreshDeviceRequired => "REFRESH_DEVICE_REQUIRED",
            Self::RefreshRotationInProgress => "REFRESH_ROTATION_IN_PROGRESS",
            Self::OrganizationRequired => "AUTH_ORGANIZATION_REQUIRED",
            Self::CurrentPasswordInvalid => "AUTH_CURRENT_PASSWORD_INVALID",
            Self::InvitationInvalid => "INVITATION_INVALID",
            Self::ResetTokenInvalid => "RESET_TOKEN_INVALID",
            Self::EmailTokenInvalid => "EMAIL_TOKEN_INVALID",
            Self::RegistrationTokenInvalid => "REGISTRATION_TOKEN_INVALID",
            Self::RegistrationConflict => "REGISTRATION_CONFLICT",
            Self::BrowserOriginRejected => "BROWSER_ORIGIN_REJECTED",
            Self::RateLimited => "RATE_LIMITED",
            Self::RequestTooLarge => "REQUEST_TOO_LARGE",
            Self::RequestInvalid => "REQUEST_INVALID",
            Self::ValidationFailed => "VALIDATION_FAILED",
        }
    }

    /// The one status each code is returned with.
    pub fn status(self) -> StatusCode {
        match self {
            Self::CredentialsInvalid
            | Self::RefreshTokenMissing
            | Self::RefreshTokenExpired
            | Self::RefreshTokenInvalid
            | Self::RefreshSessionRevoked
            | Self::RefreshTokenReplayed
            | Self::RefreshAccountInactive
            | Self::RefreshPasswordChanged
            | Self::RefreshDeviceRequired => StatusCode::UNAUTHORIZED,
            Self::EmailUnverified
            | Self::AccountNotActive
            | Self::OrganizationRequired
            | Self::BrowserOriginRejected => StatusCode::FORBIDDEN,
            Self::RefreshRotationInProgress | Self::RegistrationConflict => StatusCode::CONFLICT,
            Self::CurrentPasswordInvalid
            | Self::InvitationInvalid
            | Self::ResetTokenInvalid
            | Self::EmailTokenInvalid
            | Self::RegistrationTokenInvalid
            | Self::RequestInvalid => StatusCode::BAD_REQUEST,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::RequestTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::ValidationFailed => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    /// The default message. Wording is free; the code is the contract.
    pub fn message(self) -> &'static str {
        match self {
            Self::CredentialsInvalid => "Incorrect email or password",
            Self::EmailUnverified => "Verify your email address before signing in",
            Self::AccountNotActive => "This account is not active",
            Self::RefreshTokenMissing => "No session to refresh; sign in again",
            Self::RefreshTokenExpired => "Session expired; sign in again",
            Self::RefreshTokenInvalid => "Session is not valid; sign in again",
            Self::RefreshSessionRevoked => "Session was revoked; sign in again",
            Self::RefreshTokenReplayed => "Session was already used; sign in again",
            Self::RefreshAccountInactive => "This account is not active; sign in again",
            Self::RefreshPasswordChanged => "Password was changed; sign in again",
            Self::RefreshDeviceRequired => "This browser session is not recognized; sign in again",
            Self::RefreshRotationInProgress => "Session refresh is in progress; retry shortly",
            Self::OrganizationRequired => "An organization is required for live updates",
            Self::CurrentPasswordInvalid => "Current password is incorrect",
            Self::InvitationInvalid => "Invitation is not valid or has expired",
            Self::ResetTokenInvalid => "Reset link is not valid or has expired",
            Self::EmailTokenInvalid => "Verification link is not valid or has expired",
            Self::RegistrationTokenInvalid => "Registration link is not valid or has expired",
            Self::RegistrationConflict => "Registration could not be completed",
            Self::BrowserOriginRejected => {
                "Browser origin is not allowed for this authentication operation"
            }
            Self::RateLimited => "Too many requests; retry later",
            Self::RequestTooLarge => "Authentication request is too large",
            Self::RequestInvalid => "Request could not be read",
            Self::ValidationFailed => "Request is not valid",
        }
    }
}

/// One auth route failure: a code, an optional message override and an
/// optional retry delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthFailure {
    code: AuthCode,
    message: Option<&'static str>,
    retry_after_seconds: Option<u32>,
}

impl AuthFailure {
    pub fn new(code: AuthCode) -> Self {
        Self {
            code,
            message: None,
            retry_after_seconds: None,
        }
    }

    /// Replace the default message, for example with a validation reason.
    pub fn with_message(self, message: &'static str) -> Self {
        Self {
            message: Some(message),
            ..self
        }
    }

    /// Ask the client to wait. Renders a `Retry-After` header and the
    /// `retry_after_seconds` detail field the frontend reads on a 409.
    pub fn with_retry_after(self, seconds: u32) -> Self {
        Self {
            retry_after_seconds: Some(seconds),
            ..self
        }
    }

    pub fn code(&self) -> AuthCode {
        self.code
    }

    pub fn status(&self) -> StatusCode {
        self.code.status()
    }

    pub fn message(&self) -> &'static str {
        self.message.unwrap_or_else(|| self.code.message())
    }

    pub fn retry_after_seconds(&self) -> Option<u32> {
        self.retry_after_seconds
    }

    /// Only a 401 challenges for a bearer credential.
    pub fn www_authenticate(&self) -> bool {
        self.status() == StatusCode::UNAUTHORIZED
    }

    /// The `detail` object of the response body.
    pub fn detail(&self) -> Value {
        let mut detail = json!({"code": self.code.as_str(), "message": self.message()});
        if let Some(seconds) = self.retry_after_seconds {
            detail["retry_after_seconds"] = json!(seconds);
        }
        detail
    }
}

impl IntoResponse for AuthFailure {
    fn into_response(self) -> Response {
        let mut response =
            (self.status(), axum::Json(json!({"detail": self.detail()}))).into_response();
        let headers = response.headers_mut();
        if self.www_authenticate() {
            headers.insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if let Some(seconds) = self.retry_after_seconds {
            headers.insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

/// Preserve auth details in the shared error path. [`ApiError`] derives the
/// bearer challenge from the 401 status, as this response does.
impl From<AuthFailure> for ApiError {
    fn from(failure: AuthFailure) -> Self {
        ApiError {
            kind: ApiErrorKind::Expected,
            status: failure.status(),
            detail: failure.detail(),
            retry_after: failure.retry_after_seconds,
        }
    }
}

/// Read an auth request body of at most [`AUTH_BODY_LIMIT_BYTES`].
///
/// A declared `Content-Length` above the limit is refused before any byte is
/// read, and a body that grows past the limit is refused as soon as it does,
/// so an oversized body is never buffered or parsed. Both are 413
/// `REQUEST_TOO_LARGE`. A malformed `Content-Length` or a body that fails while
/// it is read is 400 `REQUEST_INVALID`. Neither is a 500.
pub async fn read_auth_body(headers: &HeaderMap, body: Body) -> Result<Bytes, AuthFailure> {
    let too_large = || AuthFailure::new(AuthCode::RequestTooLarge);
    let unreadable = || AuthFailure::new(AuthCode::RequestInvalid);

    if let Some(declared) = headers.get(CONTENT_LENGTH) {
        let declared: u64 = declared
            .to_str()
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .ok_or_else(unreadable)?;
        if declared > AUTH_BODY_LIMIT_BYTES as u64 {
            return Err(too_large());
        }
    }

    let mut chunks = body.into_data_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unreadable())?;
        if buffer.len() + chunk.len() > AUTH_BODY_LIMIT_BYTES {
            return Err(too_large());
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buffer))
}
