//! Best-effort conversion delivery. Call only after a known successful commit.
use crate::{analytics::events::ServerEvent, providers::umami::UmamiConfig};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

const MAX_RESPONSE_BYTES: usize = 1_000_000;
const FALLBACK_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
#[derive(Clone, Default)]
pub struct RequestMetadata {
    agent: Option<String>,
    hostname: Option<String>,
}
impl RequestMetadata {
    pub fn new(agent: Option<&str>, hostname: Option<&str>) -> Self {
        Self {
            agent: agent
                .map(|v| {
                    v.chars()
                        .filter(|c| c.is_ascii() && !c.is_ascii_control())
                        .take(256)
                        .collect::<String>()
                        .trim()
                        .to_owned()
                })
                .filter(|v| !v.is_empty()),
            hostname: hostname.and_then(bounded_hostname),
        }
    }
}
fn bounded_hostname(value: &str) -> Option<String> {
    let value = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .take(253)
        .collect::<String>();
    (!value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'))
    .then_some(value)
}
#[derive(Clone)]
pub struct AnalyticsSender {
    inner: Arc<Inner>,
}
impl std::fmt::Debug for AnalyticsSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The collector URL and monitor digest are configuration, not state
        // a debug dump should echo.
        formatter
            .debug_struct("AnalyticsSender")
            .finish_non_exhaustive()
    }
}
struct Inner {
    http: reqwest::Client,
    url: Option<String>,
    website: Option<String>,
    monitor: Option<String>,
    hostname: String,
    slots: Arc<Semaphore>,
    timeout: Duration,
}
impl AnalyticsSender {
    pub fn new(
        config: &UmamiConfig,
        monitor: Option<String>,
        frontend_url: &str,
        concurrency: usize,
    ) -> Result<Self, &'static str> {
        if concurrency == 0
            || concurrency > 64
            || config.policy.timeout.is_zero()
            || config.policy.timeout > Duration::from_secs(10)
        {
            return Err("Analytics delivery bounds are invalid");
        }
        let base = config
            .base_url
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .map(|value| {
                let mut url = url::Url::parse(value).map_err(|_| "Analytics URL is invalid")?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err("Analytics URL is invalid");
                }
                url.set_path(&format!("{}/api/send", url.path().trim_end_matches('/')));
                url.set_query(None);
                url.set_fragment(None);
                Ok(url.to_string())
            })
            .transpose()?;
        let website = config.website_id.clone().filter(|v| !v.trim().is_empty());
        let url = if config.enabled && website.is_some() {
            base
        } else {
            None
        };
        let hostname = url::Url::parse(frontend_url)
            .ok()
            .and_then(|u| u.host_str().and_then(bounded_hostname))
            .unwrap_or_else(|| "localhost".into());
        let http = reqwest::Client::builder()
            .timeout(config.policy.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Analytics client is unavailable")?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                url,
                website,
                monitor: monitor.filter(|v| !v.is_empty()),
                hostname,
                slots: Arc::new(Semaphore::new(concurrency)),
                timeout: config.policy.timeout,
            }),
        })
    }
    /// Admission never waits and holds no queue. Saturated collectors drop work.
    pub fn schedule(&self, event: ServerEvent, metadata: RequestMetadata, monitor: Option<&str>) {
        if let (Some(expected), Some(actual)) = (self.inner.monitor.as_deref(), monitor) {
            use sha2::digest::CtOutput;
            use sha2::{Digest, Sha256};
            if CtOutput::<Sha256>::new(Sha256::digest(expected.as_bytes()))
                == CtOutput::<Sha256>::new(Sha256::digest(actual.as_bytes()))
            {
                return;
            }
        }
        let Some(url) = self.inner.url.clone() else {
            return;
        };
        let Ok(permit) = self.inner.slots.clone().try_acquire_owned() else {
            tracing::warn!(
                event = event.name(),
                "analytics event dropped at delivery bound"
            );
            return;
        };
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let name = event.name();
            let data: serde_json::Map<String, serde_json::Value> = event
                .data()
                .into_iter()
                .map(|(k, v)| (k.into(), json!(v)))
                .collect();
            let payload = json!({"type":"event","payload":{"website":inner.website,"hostname":metadata.hostname.unwrap_or_else(||inner.hostname.clone()),"url":"/server-events","title":"Verdaxis server event","name":name,"data":data}});
            let delivery = async {
                let mut response = inner
                    .http
                    .post(url)
                    .header(
                        reqwest::header::USER_AGENT,
                        metadata.agent.as_deref().unwrap_or(FALLBACK_AGENT),
                    )
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
                    if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                        return Err(());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let response: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
                if !response.is_object() || response.get("beep") == Some(&json!("boop")) {
                    return Err(());
                }
                Ok(())
            };
            if !matches!(
                tokio::time::timeout(inner.timeout, delivery).await,
                Ok(Ok(()))
            ) {
                tracing::warn!(event = name, "analytics event delivery failed");
            }
        });
    }
}
