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
    let address = if let Some((_, rest)) = text.rsplit_once('<') {
        rest.strip_suffix('>').unwrap_or(rest).trim()
    } else {
        text
    };
    let invalid = || {
        refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!([{"type":"value_error","loc":["body","email"],"msg":"value is not a valid email address","input":value}]),
        )
    };
    let (local, domain) = address.rsplit_once('@').ok_or_else(invalid)?;
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || local
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "@<>(),;:\\\"[]".contains(c))
    {
        return Err(invalid());
    }
    let ascii = idna::domain_to_ascii(domain).map_err(|_| invalid())?;
    if ascii.len() > 253
        || !ascii.contains('.')
        || ascii.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || part.starts_with('-')
                || part.ends_with('-')
                || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    let (domain, _) = idna::domain_to_unicode(&ascii);
    let result = format!("{local}@{}", domain.to_lowercase());
    if result.len() > 254 {
        return Err(invalid());
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
        matches!(self.client.post(&self.endpoint).bearer_auth(key).json(payload).send().await, Ok(response) if matches!(response.status().as_u16(), 200 | 201))
    }
}
