//! Private Phase 2 runtime settings (Tasks 22 and 23).
//!
//! The deploy transaction projects these values into the Rust runtime file
//! only. They are never printed, logged or committed: the debug rendering
//! below redacts every secret. A configuration that carries any Phase 2 key
//! must carry all of them, so a half-filled environment cannot start a
//! service that would answer with the wrong token authority or sender.
use std::fmt;

use chrono::{DateTime, Utc};

use crate::Phase2Options;
use crate::auth::VerificationSettings;
use crate::crud::referrals::invite::MailClient;

/// The verification and invite authority shared by the Phase 2 route groups.
#[derive(Clone)]
pub struct Phase2Settings {
    pub verification: VerificationSettings,
    pub frontend_url: String,
    pub email_from: String,
    pub mail: MailClient,
    /// When the trusted watchlist event producer went live in this
    /// environment, as the release metadata recorded it (PD5). `None` while
    /// no release has recorded one.
    pub watchlist_activation: Option<DateTime<Utc>>,
}

impl Phase2Settings {
    /// Build the settings from the deployed environment projection.
    ///
    /// All keys absent means the groups are not mounted in this composition
    /// (the contract harness). Any key present requires every key, because a
    /// partial projection cannot verify tokens or send an invite.
    pub fn from_options(options: &Phase2Options) -> Result<Option<Self>, String> {
        // A recorded value is checked even where the groups stay unmounted,
        // so an unreadable record always stops the start.
        let watchlist_activation = watchlist_activation(options)?;
        let present = [
            options.jwt_secret.as_ref(),
            options.jwt_secret_previous.as_ref(),
            options.jwt_issuer.as_ref(),
            options.jwt_audience.as_ref(),
            options.frontend_url.as_ref(),
            options.email_from.as_ref(),
            options.resend_api_key.as_ref(),
        ];
        let required = [
            "JWT_SECRET",
            "JWT_SECRET_PREVIOUS",
            "JWT_ISSUER",
            "JWT_AUDIENCE",
            "FRONTEND_URL",
            "EMAIL_FROM",
            "RESEND_API_KEY",
        ];
        if present.iter().all(Option::is_none) {
            return Ok(None);
        }
        let value = |index: usize| -> Result<&str, String> {
            present[index]
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{} must be set for Phase 2 routes", required[index]))
        };
        let jwt_secret = value(0)?;
        if jwt_secret.len() < 32 {
            return Err("JWT_SECRET must be at least 32 characters".into());
        }
        // An empty previous key is the ordinary "no rotation in progress"
        // state; it must be absent rather than blank so it is never a key.
        let previous = match present[1].map(String::as_str) {
            None => None,
            Some(previous) if previous.trim().is_empty() => None,
            Some(previous) if previous.len() < 32 => {
                return Err("JWT_SECRET_PREVIOUS must be at least 32 characters".into());
            }
            Some(previous) => Some(previous.to_owned()),
        };
        let issuer = value(2)?.to_owned();
        let audience = value(3)?.to_owned();
        let frontend_url = value(4)?.to_owned();
        // Validate, but keep the deployed spelling: R01 appends `/invite/<code>`
        // to the raw value exactly as Python does, while the invite email trims
        // a trailing slash itself.
        url::Url::parse(&frontend_url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .filter(|url| url.host_str().is_some())
            .filter(|_| !frontend_url.chars().any(char::is_whitespace))
            .ok_or("FRONTEND_URL must be an absolute http or https URL")?;
        let email_from = value(5)?.to_owned();
        let resend_api_key = value(6)?.to_owned();
        let mail = MailClient::production(Some(resend_api_key))
            .map_err(|_| "the referral mail client cannot be built".to_owned())?;
        Ok(Some(Self {
            verification: VerificationSettings::new(
                jwt_secret.to_owned(),
                previous,
                issuer,
                audience,
            ),
            frontend_url,
            email_from,
            mail,
            watchlist_activation,
        }))
    }
}

const ACTIVATION_REFUSAL: &str =
    "WATCHLIST_PRODUCER_ACTIVATED_AT must be an RFC 3339 instant with an offset";

/// PD5: the release metadata records the activation as an RFC 3339 instant
/// with its offset. No clock reading or fixed instant stands in for a missing
/// record, and any other spelling is refused rather than reinterpreted.
fn watchlist_activation(options: &Phase2Options) -> Result<Option<DateTime<Utc>>, String> {
    let Some(recorded) = options.watchlist_producer_activated_at.as_deref() else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(recorded)
        .map(|recorded| Some(recorded.with_timezone(&Utc)))
        .map_err(|_| ACTIVATION_REFUSAL.to_owned())
}

impl fmt::Debug for Phase2Settings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Phase2Settings")
            .field("verification", &"[REDACTED]")
            .field("frontend_url", &self.frontend_url)
            .field("email_from", &self.email_from)
            .field("mail", &"[REDACTED]")
            .field("watchlist_activation", &self.watchlist_activation)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn complete() -> Phase2Options {
        Phase2Options {
            jwt_secret: Some("c".repeat(40)),
            jwt_secret_previous: Some("p".repeat(40)),
            jwt_issuer: Some("verdaxis-staging-api".into()),
            jwt_audience: Some("verdaxis-staging-web".into()),
            frontend_url: Some("https://staging.verdaxis.exchange".into()),
            email_from: Some("Verdaxis <noreply@verdaxis.exchange>".into()),
            resend_api_key: Some("re_disposable_key".into()),
            auth_domain_datasets: None,
            monitor_token: None,
            watchlist_producer_activated_at: None,
            system_status: None,
            system_status_message: None,
        }
    }

    #[test]
    fn absent_options_mount_no_phase2_group() {
        assert!(
            Phase2Settings::from_options(&Phase2Options::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn partial_projection_is_refused() {
        let mut options = complete();
        options.resend_api_key = None;
        let error = Phase2Settings::from_options(&options).unwrap_err();
        assert!(error.contains("RESEND_API_KEY"), "{error}");
    }

    #[test]
    fn short_secret_and_blank_previous_are_refused_or_absent() {
        let mut options = complete();
        options.jwt_secret = Some("short".into());
        assert!(Phase2Settings::from_options(&options).is_err());
        let mut options = complete();
        options.jwt_secret_previous = Some("  ".into());
        let settings = Phase2Settings::from_options(&options).unwrap().unwrap();
        assert!(!format!("{settings:?}").contains(&"c".repeat(40)));
        assert!(!format!("{settings:?}").contains(&"p".repeat(40)));
    }

    #[test]
    fn verification_keeps_the_deployed_key_and_claim_bytes() {
        use chrono::Utc;
        use jsonwebtoken::{EncodingKey, Header, encode};
        use serde_json::json;

        let mut options = complete();
        options.jwt_secret = Some(format!(" {} ", "c".repeat(40)));
        options.jwt_secret_previous = Some(format!(" {} ", "p".repeat(40)));
        options.jwt_issuer = Some(" issuer ".into());
        options.jwt_audience = Some(" audience ".into());
        options.email_from = Some(" sender@example.test ".into());
        let settings = Phase2Settings::from_options(&options).unwrap().unwrap();
        let now = Utc::now();
        let claims = json!({
            "sub": "20000000-0000-4000-8000-000000000001",
            "iss": " issuer ",
            "aud": " audience ",
            "exp": now.timestamp() + 300,
            "type": "access",
        });
        for secret in [&options.jwt_secret, &options.jwt_secret_previous] {
            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(secret.as_ref().unwrap().as_bytes()),
            )
            .unwrap();
            assert!(crate::auth::verify_access(&token, &settings.verification, now).is_ok());
        }
        assert_eq!(settings.email_from, " sender@example.test ");
    }

    #[test]
    fn the_watchlist_activation_is_the_recorded_instant_or_none() {
        let settings = Phase2Settings::from_options(&complete()).unwrap().unwrap();
        assert_eq!(settings.watchlist_activation, None);
        let mut options = complete();
        options.watchlist_producer_activated_at = Some("2026-11-01T08:00:00+08:00".into());
        let settings = Phase2Settings::from_options(&options).unwrap().unwrap();
        let recorded = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(settings.watchlist_activation, Some(recorded));
        // An unreadable record stops the start even with the groups unmounted.
        let options = Phase2Options {
            watchlist_producer_activated_at: Some("2026-11-01".into()),
            ..Phase2Options::default()
        };
        assert!(Phase2Settings::from_options(&options).is_err());
    }

    #[test]
    fn frontend_url_must_be_absolute_http() {
        for url in [
            "",
            "staging.verdaxis.exchange",
            "ftp://example.test",
            "https://a b",
        ] {
            let mut options = complete();
            options.frontend_url = Some(url.into());
            assert!(Phase2Settings::from_options(&options).is_err(), "{url}");
        }
    }

    #[test]
    fn frontend_url_keeps_the_deployed_spelling() {
        // R01 appends `/invite/<code>` to the raw value, exactly like Python;
        // normalising a trailing slash here would produce a double slash.
        let settings = Phase2Settings::from_options(&complete()).unwrap().unwrap();
        assert_eq!(settings.frontend_url, "https://staging.verdaxis.exchange");
        let mut options = complete();
        options.frontend_url = Some("https://staging.verdaxis.exchange/".into());
        let settings = Phase2Settings::from_options(&options).unwrap().unwrap();
        assert_eq!(settings.frontend_url, "https://staging.verdaxis.exchange/");
    }
}
