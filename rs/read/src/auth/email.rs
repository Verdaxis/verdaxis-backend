//! Shared credential mail rendering and bounded delivery.
use crate::auth::token_bytes::token_digest;
use crate::crud::referrals::invite::MailClient;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

pub enum AuthMail {
    SignupAlert {
        user_id: uuid::Uuid,
        environment: String,
        email: String,
        name: String,
        role: String,
        organization: String,
    },
    Verification {
        to: String,
        name: String,
        token: String,
        pending: Option<String>,
    },
    Reset {
        to: String,
        name: String,
        token: String,
    },
    AccountExists {
        to: String,
    },
}
impl std::fmt::Debug for AuthMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthMail([redacted])")
    }
}
impl AuthMail {
    fn recipient(&self) -> &str {
        match self {
            Self::SignupAlert { .. } => "admin@verdaxis.exchange",
            Self::Verification { to, .. } | Self::Reset { to, .. } | Self::AccountExists { to } => {
                to
            }
        }
    }
}

#[derive(Clone)]
pub struct MailDispatcher {
    client: MailClient,
    slots: Arc<Semaphore>,
    done: Arc<Notify>,
    sender: Arc<str>,
    frontend: Arc<str>,
}
impl MailDispatcher {
    pub fn new(client: MailClient) -> Self {
        Self {
            client,
            slots: Arc::new(Semaphore::new(32)),
            done: Arc::new(Notify::new()),
            sender: "Verdaxis <noreply@verdaxis.exchange>".into(),
            frontend: "https://app.verdaxis.exchange".into(),
        }
    }
    pub fn with_rendering(mut self, sender: String, frontend: String) -> Self {
        self.sender = sender.into();
        self.frontend = frontend.into();
        self
    }
    pub fn dispatch(&self, mail: AuthMail) {
        let digest = token_digest(mail.recipient());
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            tracing::warn!(recipient_digest = &digest[..12], "auth_mail_capacity");
            return;
        };
        let dispatcher = self.clone();
        use tracing::instrument::WithSubscriber;
        tokio::spawn(
            async move {
                let payload = render(&mail, &dispatcher.sender, &dispatcher.frontend);
                let idempotency = match &mail {
                    AuthMail::SignupAlert {
                        user_id,
                        environment,
                        ..
                    } => Some(format!("signup-alert/{environment}/{user_id}")),
                    _ => None,
                };
                let mut sent = false;
                for delay in [0, 1, 3] {
                    if delay != 0 {
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                    if tokio::time::timeout(
                        Duration::from_secs(10),
                        dispatcher
                            .client
                            .send_with_idempotency(&payload, idempotency.as_deref()),
                    )
                    .await
                    .unwrap_or(false)
                    {
                        sent = true;
                        break;
                    }
                }
                if !sent {
                    tracing::warn!(
                        recipient_digest = &digest[..12],
                        "auth_mail_delivery_failed"
                    );
                }
                drop(permit);
                dispatcher.done.notify_waiters();
            }
            .with_current_subscriber(),
        );
    }
    /// Await the currently admitted deliveries during graceful shutdown.
    pub async fn drain(&self) {
        loop {
            let notified = self.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.slots.available_permits() == 32 {
                return;
            }
            notified.await;
        }
    }
}

pub fn suppress_verification(monitor_admitted: bool, email: &str) -> bool {
    let (local, domain) = email.split_once('@').unwrap_or(("", ""));
    monitor_admitted
        && local.starts_with("canary+")
        && domain.ends_with(".canary.verdaxis.exchange")
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
pub fn render(mail: &AuthMail, sender: &str, frontend: &str) -> Value {
    if let AuthMail::SignupAlert {
        environment,
        email,
        name,
        role,
        organization,
        ..
    } = mail
    {
        return json!({"from":sender,"to":[mail.recipient()],"subject":format!("[{environment}] New Verdaxis account application"),"html":format!("<p>ADMIN REVIEW [{}]</p><p>{}</p><p>{}</p><p>{}</p><p>{}</p><a href=\"{}/app/admin/users\">Review application</a>",escape(environment),escape(name),escape(email),escape(role),escape(organization),escape(frontend.trim_end_matches('/')))});
    }
    let (title, template, name, link) = match mail {
        AuthMail::SignupAlert { .. } => unreachable!("alert rendered above"),
        AuthMail::Verification {
            name,
            token,
            pending,
            ..
        } => (
            "Verify your email",
            include_str!("templates/verify.html"),
            name.as_str(),
            format!(
                "{}/verify-email?token={}",
                frontend.trim_end_matches('/'),
                encode(pending.as_deref().unwrap_or(token))
            ),
        ),
        AuthMail::Reset { name, token, .. } => (
            "Reset your password",
            include_str!("templates/reset.html"),
            name.as_str(),
            format!(
                "{}/reset-password?token={}",
                frontend.trim_end_matches('/'),
                encode(token)
            ),
        ),
        AuthMail::AccountExists { .. } => (
            "Your Verdaxis account",
            "<p>You have a Verdaxis account.</p><p><a href=\"{{URL}}\">Sign in</a></p>",
            "",
            format!("{}/login", frontend.trim_end_matches('/')),
        ),
    };
    let mut html = String::new();
    let mut rest = template;
    while let Some((prefix, tail)) = rest.split_once("{{") {
        html.push_str(prefix);
        let (key, suffix) = tail.split_once("}}").expect("static mail template");
        match key {
            "NAME" => html.push_str(&escape(name)),
            "URL" => html.push_str(&escape(&link)),
            _ => unreachable!("static mail key"),
        }
        rest = suffix;
    }
    html.push_str(rest);
    json!({"from":sender,"to":[mail.recipient()],"subject":title,"html":html})
}

/// Shared authenticated monitor admission; an address alone never grants authority.
pub(crate) fn monitor_admitted(expected: Option<&str>, headers: &axum::http::HeaderMap) -> bool {
    use sha2::{Digest, Sha256, digest::CtOutput};
    let Some(expected) = expected else {
        return false;
    };
    let Some(presented) = headers.get("x-monitor-token") else {
        return false;
    };
    CtOutput::<Sha256>::new(Sha256::digest(expected.as_bytes()))
        == CtOutput::<Sha256>::new(Sha256::digest(presented.as_bytes()))
}
