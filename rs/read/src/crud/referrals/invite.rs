//! R04 preparation and bounded provider transport. No outbox or referral insert.
use crate::{ApiError, ApiErrorKind};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;

const SUPPORT: &str = "admin@verdaxis.exchange";
const PROVIDER: &str = "https://api.resend.com/emails";

fn refusal(status: StatusCode, detail: Value) -> ApiError {
    ApiError {
        status,
        detail,
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

/// Typed validation runs after current-account admission and before quota.
/// Domain conversion uses the same UTS46 family as email-validator. Less common
/// EmailStr grammar and diagnostic vectors remain a differential runner gate.
pub fn normalize_email(body: &Value) -> Result<String, ApiError> {
    let Some(value) = body.get("email") else {
        return Err(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!([{"type":"missing","loc":["body","email"],"msg":"Field required","input":body}]),
        ));
    };
    let Some(text) = value.as_str() else {
        return Err(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!([{"type":"string_type","loc":["body","email"],"msg":"Input should be a valid string","input":value}]),
        ));
    };
    let text = text.trim();
    let address = if text.ends_with('>') {
        text.rsplit_once('<')
            .map(|(_, rest)| rest[..rest.len() - 1].trim())
            .unwrap_or(text)
    } else {
        text
    };
    let invalid = |reason: &str| {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!([{"type":"value_error","loc":["body","email"],"msg":format!("value is not a valid email address: {reason}"),"input":value,"ctx":{"reason":reason}}]),
        )
    };
    if text.contains('<') && !text.ends_with('>') {
        return Err(invalid(
            "An open angle bracket at the start of the email address has to be followed by a close angle bracket at the end.",
        ));
    }
    let (local, domain) = address
        .rsplit_once('@')
        .ok_or_else(|| invalid("An email address must have an @-sign."))?;
    if local.is_empty() {
        return Err(invalid("There must be something before the @-sign."));
    }
    if local.contains(' ') {
        return Err(invalid(
            "The email address contains invalid characters before the @-sign: SPACE.",
        ));
    }
    if local.contains('\u{200b}') {
        return Err(invalid(
            "The email address contains unsafe characters: ZERO WIDTH SPACE.",
        ));
    }
    if local.starts_with('.') {
        return Err(invalid("An email address cannot start with a period."));
    }
    if local.ends_with('.') {
        return Err(invalid(
            "An email address cannot have a period immediately before the @-sign.",
        ));
    }
    if local.contains("..") {
        return Err(invalid(
            "An email address cannot have two periods in a row.",
        ));
    }
    if local
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "@<>(),;:\\\"[]".contains(c))
    {
        return Err(invalid(
            "The email address contains invalid characters before the @-sign.",
        ));
    }
    if domain.is_empty() {
        return Err(invalid("There must be something after the @-sign."));
    }
    if domain.contains('_') {
        return Err(invalid(
            "The part after the @-sign contains invalid characters: '_'.",
        ));
    }
    if domain.starts_with('-') {
        return Err(invalid(
            "An email address cannot have a hyphen immediately after the @-sign.",
        ));
    }
    if domain.ends_with('.') {
        return Err(invalid("An email address cannot end with a period."));
    }
    let ascii = idna::domain_to_ascii(domain)
        .map_err(|_| invalid("The part after the @-sign is not valid IDNA."))?;
    if !ascii.contains('.') {
        return Err(invalid(
            "The part after the @-sign is not valid. It should have a period.",
        ));
    }
    if ascii
        .rsplit('.')
        .next()
        .is_some_and(|part| part.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(invalid(
            "The part after the @-sign is not valid. It is not within a valid top-level domain.",
        ));
    }
    if ["test", "invalid", "localhost", "local", "onion"]
        .iter()
        .any(|suffix| ascii == *suffix || ascii.ends_with(&format!(".{suffix}")))
    {
        return Err(invalid(
            "The part after the @-sign is a special-use or reserved name that cannot be used with email.",
        ));
    }
    if ascii.len() > 253
        || ascii.split('.').any(|part| {
            part.is_empty() || part.len() > 63 || part.starts_with('-') || part.ends_with('-')
        })
    {
        return Err(invalid("The part after the @-sign is not valid."));
    }
    use unicode_normalization::UnicodeNormalization;
    let local: String = local.nfc().collect();
    let (domain, _) = idna::domain_to_unicode(&ascii);
    let result = format!("{local}@{}", domain.to_lowercase());
    if result.len() > 254 {
        return Err(invalid("The email address is too long."));
    }
    Ok(result)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub fn render_invite(
    sender: &str,
    recipient: &str,
    name: &str,
    code: &str,
    frontend: &str,
) -> Value {
    let name = if name.is_empty() { "Someone" } else { name };
    let title = "You are invited to Verdaxis.";
    let preview = format!("{name} has invited you to join the low-carbon fuels exchange.");
    let body = format!(
        "<p style=\"margin:0 0 16px;\">{} has invited you to join Verdaxis.</p>\n          <p style=\"margin:0;\">Connect with buyers and suppliers, explore the market, and take your next step in low-carbon fuels.</p>",
        escape(name)
    );
    let encoded: String = code
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let url = escape(&format!(
        "{}/invite/{encoded}",
        frontend.trim_end_matches('/')
    ));
    // One replacement pass: user text cannot introduce another template token.
    let values = [
        ("TITLE", escape(title)),
        ("PREVIEW", escape(&preview)),
        ("LABEL", "YOUR INVITATION".into()),
        ("BODY", body),
        ("URL", url),
        ("ACTION", "Explore your invitation".into()),
        ("SUPPORT", SUPPORT.into()),
    ];
    let mut html = String::new();
    let mut template = include_str!("invite.html");
    while let Some((prefix, tail)) = template.split_once("{{") {
        html.push_str(prefix);
        let (key, rest) = tail.split_once("}}").expect("static email template");
        html.push_str(
            &values
                .iter()
                .find(|(name, _)| *name == key)
                .expect("static email token")
                .1,
        );
        template = rest;
    }
    html.push_str(template);
    json!({"from":sender,"reply_to":SUPPORT,"to":[recipient],"subject":format!("{name} invited you to Verdaxis"),"html":html})
}

#[derive(Clone)]
pub struct MailClient {
    client: reqwest::Client,
    endpoint: String,
    key: Option<String>,
}
impl MailClient {
    pub fn production(key: Option<String>) -> Result<Self, reqwest::Error> {
        Self::build(PROVIDER.into(), key)
    }
    pub fn disabled() -> Self {
        Self::production(None).expect("static mail client configuration")
    }
    fn build(endpoint: String, key: Option<String>) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()?,
            endpoint,
            key,
        })
    }
    /// Explicit loopback-only transport for disposable tests. Never reads env.
    pub fn loopback(endpoint: String, key: String) -> Result<Self, &'static str> {
        let url = url::Url::parse(&endpoint).map_err(|_| "invalid stub URL")?;
        let local = url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if url.scheme() != "http"
            || !local
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("stub must use numeric HTTP loopback");
        }
        Self::build(endpoint, Some(key)).map_err(|_| "invalid stub client")
    }
    pub async fn send(&self, payload: &Value) -> bool {
        let Some(key) = self.key.as_ref().filter(|key| !key.is_empty()) else {
            return false;
        };
        // Do not parse, retain or log provider bodies or recipient addresses.
        let Ok(mut response) = self
            .client
            .post(&self.endpoint)
            .bearer_auth(key)
            .json(payload)
            .send()
            .await
        else {
            return false;
        };
        let accepted = matches!(response.status().as_u16(), 200 | 201);
        // HTTPX completes the response body before returning from post(). Drain
        // without retaining provider content, under the same request deadline.
        loop {
            match response.chunk().await {
                Ok(Some(_)) => (),
                Ok(None) => return accepted,
                Err(_) => return false,
            }
        }
    }
}
