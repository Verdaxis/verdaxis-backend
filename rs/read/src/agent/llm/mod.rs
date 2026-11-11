//! Bounded async OpenCode Go transport. No provider body or credential enters an error.
mod credentials;
pub mod models;

use credentials::{Credential, contains_key, value_contains_key};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, SystemTime};
use tokio::time::Instant;
use uuid::Uuid;

const CHAT_ENDPOINT: &str = "https://opencode.ai/inference/go/openai/v1/chat/completions";
const USAGE_ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";
const RESPONSE_LIMIT: usize = 256 * 1024;
const USAGE_TTL: Duration = Duration::from_secs(60);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ModelNotAllowed,
    InvalidRequest,
    CredentialRejected,
    NoCredentials,
    NoHeadroom,
    ClientConfiguration,
    Transport,
    Timeout,
    ResponseLimit,
    InvalidResponse,
    CredentialEcho,
    HttpStatus,
    RetryDeferred,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    /// Visible output allowance; thinking calls add the table's reasoning headroom.
    pub max_tokens: u32,
    pub thinking: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Receipt {
    pub slot: u8,
    pub model: String,
    pub request_id: Uuid,
    pub attempt: u32,
    pub latency_ms: u128,
    pub status: Option<u16>,
    pub usage: Usage,
    /// Exact provider JSON number, never a guessed zero.
    pub cost: Option<serde_json::Number>,
    pub error: Option<ErrorCode>,
}

#[derive(Debug)]
pub struct Failure {
    pub code: ErrorCode,
    pub receipts: Vec<Receipt>,
}

pub struct Completion {
    pub content: String,
    pub receipts: Vec<Receipt>,
}

/// A mutable client owns the account cache and disabled-slot state until restart.
/// Keep one instance for the service lifetime, rather than one per turn.
pub struct GoClient {
    http: Client,
    credentials: Vec<Credential>,
    chat_endpoint: String,
    usage_endpoint: String,
}

impl GoClient {
    pub fn from_environment() -> Result<Self, ErrorCode> {
        Self::build(
            credentials::load(|name| std::env::var(name).ok())?,
            CHAT_ENDPOINT,
            USAGE_ENDPOINT,
            TOTAL_TIMEOUT,
        )
    }

    fn build(
        credentials: Vec<Credential>,
        chat: &str,
        usage: &str,
        timeout: Duration,
    ) -> Result<Self, ErrorCode> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Verdaxis/1.0 Rust agent")
            .build()
            .map_err(|_| ErrorCode::ClientConfiguration)?;
        Ok(Self {
            http,
            credentials,
            chat_endpoint: chat.to_owned(),
            usage_endpoint: usage.to_owned(),
        })
    }

    fn disable(&mut self, index: usize) {
        self.credentials[index].disabled = true;
        tracing::warn!(
            slot = self.credentials[index].slot,
            "agent credential slot disabled"
        );
    }

    async fn select(&mut self, excluded: &[usize]) -> Result<usize, ErrorCode> {
        for index in 0..self.credentials.len() {
            if self.credentials[index].disabled || excluded.contains(&index) {
                continue;
            }
            if self.credentials[index]
                .usage
                .is_some_and(|(at, _)| at.elapsed() < USAGE_TTL)
            {
                continue;
            }
            let response = self
                .http
                .get(&self.usage_endpoint)
                .bearer_auth(&self.credentials[index].key)
                .header("Accept", "application/json")
                .send()
                .await;
            let percent = match response {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if matches!(status, 401 | 403) {
                        self.disable(index);
                    }
                    if status == 200 {
                        match bounded_body(response).await {
                            Ok(bytes) if !contains_key(&bytes, &self.credentials) => {
                                serde_json::from_slice::<Value>(&bytes)
                                    .ok()
                                    .filter(|value| !value_contains_key(value, &self.credentials))
                                    .and_then(|value| credentials::highest_usage(&value))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };
            self.credentials[index].usage = Some((Instant::now(), percent));
        }
        self.credentials
            .iter()
            .enumerate()
            .filter(|(index, key)| !key.disabled && !excluded.contains(index))
            .filter_map(|(index, key)| {
                key.usage
                    .and_then(|(_, percent)| percent.map(|percent| (index, percent)))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .map(|(index, _)| index)
            .ok_or(ErrorCode::NoHeadroom)
    }

    pub async fn complete(&mut self, request: &Request) -> Result<Completion, Failure> {
        let payload = models::payload(
            &request.model,
            request.max_tokens,
            request.thinking,
            &request.messages,
        )
        .map_err(|code| Failure {
            code,
            receipts: vec![],
        })?;
        let body = serde_json::to_vec(&payload).map_err(|_| Failure {
            code: ErrorCode::InvalidRequest,
            receipts: vec![],
        })?;
        if contains_key(&body, &self.credentials) || value_contains_key(&payload, &self.credentials)
        {
            return Err(Failure {
                code: ErrorCode::CredentialRejected,
                receipts: vec![],
            });
        }
        let request_id = Uuid::new_v4();
        let mut receipts = Vec::new();
        let mut excluded = Vec::new();
        for attempt in 1..=MAX_ATTEMPTS {
            let index = self.select(&excluded).await.map_err(|code| Failure {
                code,
                receipts: receipts.clone(),
            })?;
            let started = Instant::now();
            let mut receipt = Receipt {
                slot: self.credentials[index].slot,
                model: request.model.clone(),
                request_id,
                attempt,
                latency_ms: 0,
                status: None,
                usage: Usage::default(),
                cost: None,
                error: None,
            };
            let response = self
                .http
                .post(&self.chat_endpoint)
                .bearer_auth(&self.credentials[index].key)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .header("x-opencode-session", format!("vd-{request_id}"))
                .body(body.clone())
                .send()
                .await;
            let mut wait = None;
            let outcome = match response {
                Err(error) => Err(if error.is_timeout() {
                    ErrorCode::Timeout
                } else {
                    ErrorCode::Transport
                }),
                Ok(response) => {
                    let status = response.status().as_u16();
                    receipt.status = Some(status);
                    if matches!(status, 401 | 403) {
                        self.disable(index);
                    }
                    if status == 429 {
                        excluded.push(index);
                        self.credentials[index].usage = Some((Instant::now(), None));
                    }
                    if matches!(status, 429 | 502 | 503 | 504) && attempt < MAX_ATTEMPTS {
                        wait = Some(retry_wait(response.headers().get("retry-after"), attempt));
                    }
                    match bounded_body(response).await {
                        Err(code) => {
                            wait = None;
                            Err(code)
                        }
                        Ok(bytes) => {
                            if contains_key(&bytes, &self.credentials) {
                                wait = None;
                                Err(ErrorCode::CredentialEcho)
                            } else if status != 200 {
                                Err(ErrorCode::HttpStatus)
                            } else {
                                self.interpret(&bytes, &mut receipt)
                            }
                        }
                    }
                }
            };
            receipt.latency_ms = started.elapsed().as_millis();
            receipt.error = outcome.as_ref().err().copied();
            receipts.push(receipt);
            match outcome {
                Ok(content) => return Ok(Completion { content, receipts }),
                Err(code) => match wait {
                    Some(Ok(duration)) => tokio::time::sleep(duration).await,
                    Some(Err(code)) => return Err(Failure { code, receipts }),
                    None => return Err(Failure { code, receipts }),
                },
            }
        }
        unreachable!("last attempt always returns")
    }

    fn interpret(&self, bytes: &[u8], receipt: &mut Receipt) -> Result<String, ErrorCode> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| ErrorCode::InvalidResponse)?;
        if value_contains_key(&value, &self.credentials) {
            return Err(ErrorCode::CredentialEcho);
        }
        receipt.usage = Usage {
            prompt_tokens: value["usage"]["prompt_tokens"].as_u64(),
            completion_tokens: value["usage"]["completion_tokens"].as_u64(),
            total_tokens: value["usage"]["total_tokens"].as_u64(),
        };
        receipt.cost = value["usage"]["cost"]
            .as_number()
            .filter(|cost| {
                cost.as_f64()
                    .is_some_and(|cost| cost.is_finite() && cost >= 0.0)
            })
            .cloned();
        let choices = value["choices"]
            .as_array()
            .ok_or(ErrorCode::InvalidResponse)?;
        if value["model"] != receipt.model
            || choices.len() != 1
            || choices[0]["finish_reason"] != "stop"
            || choices[0]["message"]
                .get("tool_calls")
                .is_some_and(|calls| !calls.is_null())
        {
            return Err(ErrorCode::InvalidResponse);
        }
        let content = choices[0]["message"]["content"]
            .as_str()
            .ok_or(ErrorCode::InvalidResponse)?;
        if content.contains("DSML") {
            return Err(ErrorCode::InvalidResponse);
        }
        Ok(content.to_owned())
    }
}

async fn bounded_body(mut response: Response) -> Result<Vec<u8>, ErrorCode> {
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(ErrorCode::ResponseLimit);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            ErrorCode::Timeout
        } else {
            ErrorCode::Transport
        }
    })? {
        if chunk.len() > RESPONSE_LIMIT - bytes.len() {
            return Err(ErrorCode::ResponseLimit);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn retry_wait(
    header: Option<&reqwest::header::HeaderValue>,
    attempt: u32,
) -> Result<Duration, ErrorCode> {
    let duration = if let Some(header) = header {
        let text = header.to_str().map_err(|_| ErrorCode::RetryDeferred)?;
        if let Ok(seconds) = text.parse::<u64>() {
            Duration::from_secs(seconds)
        } else {
            let date =
                chrono::DateTime::parse_from_rfc2822(text).map_err(|_| ErrorCode::RetryDeferred)?;
            let at: SystemTime = date.with_timezone(&chrono::Utc).into();
            at.duration_since(SystemTime::now()).unwrap_or_default()
        }
    } else {
        Duration::from_secs(2u64.pow(attempt))
    };
    if duration > Duration::from_secs(30) {
        return Err(ErrorCode::RetryDeferred);
    }
    Ok(duration)
}

#[cfg(test)]
mod tests;
