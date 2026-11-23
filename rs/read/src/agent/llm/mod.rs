//! Bounded async Mistral transport. No provider body or credential enters an error.
mod credentials;
pub mod models;

use credentials::{Credential, contains_key, value_contains_key};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::time::Instant;
use uuid::Uuid;

const CHAT_ENDPOINT: &str = "https://api.mistral.ai/v1/chat/completions";
const RESPONSE_LIMIT: usize = 256 * 1024;
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
    /// Visible output allowance; the transport adds the table's reasoning headroom.
    pub max_tokens: u32,
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

/// One shared client remembers credential rejection until process restart.
pub struct MistralClient {
    http: Client,
    credential: Credential,
    disabled: AtomicBool,
    chat_endpoint: String,
}

impl MistralClient {
    pub fn from_environment() -> Result<Self, ErrorCode> {
        Self::build(
            credentials::load(|name| std::env::var(name).ok())?,
            CHAT_ENDPOINT,
            TOTAL_TIMEOUT,
        )
    }

    fn build(credential: Credential, chat: &str, timeout: Duration) -> Result<Self, ErrorCode> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Verdaxis/1.0 Rust agent")
            .build()
            .map_err(|_| ErrorCode::ClientConfiguration)?;
        Ok(Self {
            http,
            credential,
            disabled: AtomicBool::new(false),
            chat_endpoint: chat.to_owned(),
        })
    }

    pub async fn complete(&self, request: &Request) -> Result<Completion, Failure> {
        let payload = models::payload(&request.model, request.max_tokens, &request.messages)
            .map_err(|code| Failure {
                code,
                receipts: vec![],
            })?;
        let body = serde_json::to_vec(&payload).map_err(|_| Failure {
            code: ErrorCode::InvalidRequest,
            receipts: vec![],
        })?;
        if contains_key(&body, &self.credential) || value_contains_key(&payload, &self.credential) {
            return Err(Failure {
                code: ErrorCode::CredentialRejected,
                receipts: vec![],
            });
        }
        let request_id = Uuid::new_v4();
        let mut receipts = Vec::new();
        for attempt in 1..=MAX_ATTEMPTS {
            if self.disabled.load(Ordering::Acquire) {
                return Err(Failure {
                    code: ErrorCode::NoCredentials,
                    receipts,
                });
            }
            let started = Instant::now();
            let mut receipt = Receipt {
                slot: 1,
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
                .bearer_auth(&self.credential.key)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
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
                        self.disabled.store(true, Ordering::Release);
                        Err(ErrorCode::NoCredentials)
                    } else {
                        if matches!(status, 429 | 502 | 503 | 504) && attempt < MAX_ATTEMPTS {
                            wait = Some(retry_wait(response.headers().get("retry-after"), attempt));
                        }
                        match bounded_body(response).await {
                            Err(code) => Err(code),
                            Ok(bytes) => {
                                if contains_key(&bytes, &self.credential) {
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
        if value_contains_key(&value, &self.credential) {
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
                    .is_some_and(|cost| cost.is_finite() && cost > 0.0)
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
        if serde_json::from_str::<Value>(content)
            .ok()
            .is_some_and(|decision| value_contains_key(&decision, &self.credential))
        {
            return Err(ErrorCode::CredentialEcho);
        }
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
