use super::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Reply {
    status: u16,
    headers: String,
    body: String,
    delay: Duration,
}
impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: String::new(),
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }
    fn retry(status: u16, wait: &str) -> Self {
        Self {
            headers: format!("Retry-After: {wait}\r\n"),
            ..Self::json(status, json!({}))
        }
    }
}
struct Mock {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Mock {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ")
                                    .and_then(|length| length.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap());
                tokio::time::sleep(reply.delay).await;
                let header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{}\r\n",
                    reply.status,
                    reply.body.len(),
                    reply.headers
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(reply.body.as_bytes()).await;
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
    fn client(&self, count: usize) -> GoClient {
        let keys = credentials::load(|name| {
            credentials::VARIABLES
                .iter()
                .position(|candidate| candidate == &name)
                .filter(|index| *index < count)
                .map(|index| format!("fixture-account-{index}"))
        })
        .unwrap();
        GoClient::build(
            keys,
            &format!("{}/chat", self.url),
            &format!("{}/usage", self.url),
            Duration::from_secs(2),
        )
        .unwrap()
    }
    fn captured(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn meter(percent: u32) -> Reply {
    let meter = json!({"status": "ok", "percent": percent});
    Reply::json(
        200,
        json!({"usage": {"rolling": meter, "weekly": meter, "monthly": meter}}),
    )
}
fn success() -> Reply {
    Reply::json(
        200,
        json!({"model": models::MODELS[0].id, "choices": [{"finish_reason":"stop", "message":{"content":"{\"reply\":\"ready\"}"}}], "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}}),
    )
}
fn request() -> Request {
    Request {
        model: models::MODELS[0].id.to_owned(),
        messages: vec![Message {
            role: Role::System,
            content: "Return one json object.".into(),
        }],
        max_tokens: 1024,
        thinking: true,
    }
}
fn failure(result: Result<Completion, Failure>) -> Failure {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected refusal"),
    }
}

#[tokio::test]
async fn selects_most_headroom_and_skips_exhausted_slots() {
    let mock = Mock::new(vec![meter(100), meter(40), meter(5), success()]).await;
    let completion = mock.client(3).complete(&request()).await.unwrap();
    assert_eq!(completion.receipts[0].slot, 3);
    assert_eq!(mock.captured().len(), 4);
}

#[tokio::test]
async fn usage_cache_expires_at_sixty_seconds_per_key() {
    let mock = Mock::new(vec![
        meter(20),
        meter(10),
        success(),
        success(),
        meter(1),
        success(),
    ])
    .await;
    let mut client = mock.client(2);
    assert_eq!(
        client.complete(&request()).await.unwrap().receipts[0].slot,
        2
    );
    assert_eq!(
        client.complete(&request()).await.unwrap().receipts[0].slot,
        2
    );
    assert_eq!(mock.captured().len(), 4);
    client.credentials[0].usage.as_mut().unwrap().0 = Instant::now() - USAGE_TTL;
    assert_eq!(
        client.complete(&request()).await.unwrap().receipts[0].slot,
        1
    );
    assert_eq!(
        mock.captured()
            .iter()
            .filter(|request| request.starts_with("GET"))
            .count(),
        3
    );
}

#[tokio::test]
async fn rotates_on_429_to_next_cached_slot() {
    let mock = Mock::new(vec![meter(1), meter(2), Reply::retry(429, "0"), success()]).await;
    let completion = mock.client(2).complete(&request()).await.unwrap();
    assert_eq!(
        completion
            .receipts
            .iter()
            .map(|receipt| receipt.slot)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(completion.receipts[0].status, Some(429));
}

#[tokio::test]
async fn unauthorized_chat_slots_stay_disabled_without_retry() {
    for status in [401, 403] {
        let mock = Mock::new(vec![
            meter(1),
            meter(2),
            Reply::json(status, json!({})),
            success(),
        ])
        .await;
        let mut client = mock.client(2);
        assert_eq!(failure(client.complete(&request()).await).receipts.len(), 1);
        assert!(client.credentials[0].disabled);
        assert_eq!(
            client.complete(&request()).await.unwrap().receipts[0].slot,
            2
        );
    }
}

#[tokio::test]
async fn unauthorized_usage_slots_are_disabled_and_never_selected() {
    for status in [401, 403] {
        let mock = Mock::new(vec![
            Reply::json(status, json!({})),
            meter(2),
            success(),
            success(),
        ])
        .await;
        let mut client = mock.client(2);
        assert_eq!(
            client.complete(&request()).await.unwrap().receipts[0].slot,
            2
        );
        client.complete(&request()).await.unwrap();
        assert!(client.credentials[0].disabled);
        assert_eq!(mock.captured().len(), 4);
    }
}

#[tokio::test]
async fn retry_after_is_honoured_and_attempts_are_bounded() {
    let mock = Mock::new(vec![
        meter(1),
        Reply::retry(503, "1"),
        Reply::retry(502, "0"),
        Reply::retry(504, "0"),
    ])
    .await;
    let started = Instant::now();
    let error = failure(mock.client(1).complete(&request()).await);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(error.receipts.len(), 3);
    assert_eq!(mock.captured().len(), 4);
}

#[tokio::test]
async fn long_or_invalid_retry_after_gives_up_immediately() {
    for wait in ["31", "999999999999999999999999", "invalid"] {
        let mock = Mock::new(vec![meter(1), Reply::retry(503, wait)]).await;
        let error = failure(mock.client(1).complete(&request()).await);
        assert_eq!(error.code, ErrorCode::RetryDeferred);
        assert_eq!(error.receipts.len(), 1);
    }
}

#[test]
fn retry_after_dates_and_exponential_fallback() {
    assert_eq!(retry_wait(None, 1).unwrap(), Duration::from_secs(2));
    assert_eq!(retry_wait(None, 2).unwrap(), Duration::from_secs(4));
    let past = reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT");
    assert_eq!(retry_wait(Some(&past), 1).unwrap(), Duration::ZERO);
    let future = reqwest::header::HeaderValue::from_static("Wed, 21 Oct 2099 07:28:00 GMT");
    assert_eq!(retry_wait(Some(&future), 1), Err(ErrorCode::RetryDeferred));
}

#[tokio::test]
async fn response_size_cap_rejects_large_body() {
    let mut reply = success();
    reply.body = " ".repeat(RESPONSE_LIMIT + 1);
    let mock = Mock::new(vec![meter(1), reply]).await;
    assert_eq!(
        failure(mock.client(1).complete(&request()).await).code,
        ErrorCode::ResponseLimit
    );
}

#[tokio::test]
async fn total_timeout_is_not_retried() {
    let mut reply = success();
    reply.delay = Duration::from_secs(1);
    let mock = Mock::new(vec![meter(1), reply]).await;
    let mut client = mock.client(1);
    client.http = Client::builder()
        .timeout(Duration::from_millis(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let error = failure(client.complete(&request()).await);
    assert_eq!(error.code, ErrorCode::Timeout);
    assert_eq!(error.receipts.len(), 1);
}

#[tokio::test]
async fn redirects_and_other_http_failures_are_not_retried() {
    for status in [302, 400, 408, 500] {
        let mut reply = Reply::json(status, json!({}));
        reply.headers = "Location: /elsewhere\r\n".into();
        let mock = Mock::new(vec![meter(1), reply]).await;
        let error = failure(mock.client(1).complete(&request()).await);
        assert_eq!(error.receipts.len(), 1);
        assert_eq!(error.receipts[0].status, Some(status));
        assert_eq!(mock.captured().len(), 2);
    }
}

#[tokio::test]
async fn request_headers_body_and_receipts_keep_credentials_separate() {
    let mock = Mock::new(vec![meter(1), success()]).await;
    let completion = mock.client(1).complete(&request()).await.unwrap();
    let requests = mock.captured();
    let (headers, body) = requests[1].split_once("\r\n\r\n").unwrap();
    assert!(headers.to_lowercase().contains("user-agent: verdaxis/"));
    assert!(headers.to_lowercase().contains("x-opencode-session: vd-"));
    assert!(headers.contains("Bearer fixture-account-0"));
    assert!(!body.contains("fixture-account"));
    let receipt = serde_json::to_string(&completion.receipts).unwrap();
    assert!(!receipt.contains("fixture-account"));
    assert_eq!(completion.receipts[0].usage.total_tokens, Some(12));
    assert!(completion.receipts[0].cost.is_none());
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["max_tokens"], 5120);
}

#[tokio::test]
async fn credentials_in_prompt_are_refused_before_network() {
    let mock = Mock::new(vec![]).await;
    let mut request = request();
    request.messages[0].content.push_str(" fixture-account-1");
    let error = failure(mock.client(2).complete(&request).await);
    assert_eq!(error.code, ErrorCode::CredentialRejected);
    assert!(error.receipts.is_empty());
    assert!(mock.captured().is_empty());
}

#[tokio::test]
async fn echoed_credentials_never_enter_receipts_or_output() {
    for body in [
        json!({"error":"fixture-account-1"}),
        json!({"error":"fixture-account-1"})
            .to_string()
            .replace("fixture", "\\u0066ixture")
            .parse::<Value>()
            .unwrap(),
    ] {
        let mock = Mock::new(vec![meter(1), meter(2), Reply::json(200, body)]).await;
        let error = failure(mock.client(2).complete(&request()).await);
        assert_eq!(error.code, ErrorCode::CredentialEcho);
        assert!(!format!("{error:?}").contains("fixture-account"));
        assert!(
            !serde_json::to_string(&error.receipts)
                .unwrap()
                .contains("fixture-account")
        );
    }
}

#[test]
fn credentials_and_model_contract_are_validated_without_values() {
    for key in ["a\nb", "a\tb", "a\u{7f}b", "a\u{0}b"] {
        assert!(credentials::load(|_| Some(key.into())).is_err());
    }
    assert!(credentials::load(|_| Some("duplicate".into())).is_err());
    assert!(credentials::load(|_| None).is_err());
    assert!(models::settings("unlisted").is_err());
    assert_eq!(MODELS_COUNT, 1);
    let mut request = request();
    request.messages[0].content = "No format instruction".into();
    assert!(models::payload(&request.model, 1024, false, &request.messages).is_err());
    let body = models::payload(
        &request.model,
        1024,
        false,
        &[Message {
            role: Role::System,
            content: "json".into(),
        }],
    )
    .unwrap();
    assert_eq!(body["thinking"]["type"], "disabled");
}
const MODELS_COUNT: usize = models::MODELS.len();

#[tokio::test]
async fn encoded_decision_credentials_are_refused() {
    let content = format!(r#"{{"reply":"{}u0066ixture-account-0"}}"#, char::from(92));
    let reply = Reply::json(
        200,
        json!({"model": models::MODELS[0].id,
        "choices": [{"finish_reason":"stop","message":{"content":content}}]}),
    );
    let mock = Mock::new(vec![meter(1), reply]).await;
    let error = failure(mock.client(1).complete(&request()).await);
    assert_eq!(error.code, ErrorCode::CredentialEcho);
    assert!(!format!("{error:?}").contains("fixture-account"));
}

#[tokio::test]
async fn oversized_retryable_error_body_does_not_cancel_retry() {
    let mut reply = Reply::retry(503, "0");
    reply.body = " ".repeat(RESPONSE_LIMIT + 1);
    let mock = Mock::new(vec![meter(1), reply, success()]).await;
    let completion = mock.client(1).complete(&request()).await.unwrap();
    assert_eq!(completion.receipts.len(), 2);
    assert_eq!(completion.receipts[0].error, Some(ErrorCode::ResponseLimit));
}

#[tokio::test]
async fn zero_cost_stays_unknown_but_positive_cost_is_retained() {
    for (cost, known) in [(0, false), (1, true)] {
        let reply = Reply::json(
            200,
            json!({"model":models::MODELS[0].id,
            "choices":[{"finish_reason":"stop","message":{"content":"{}"}}],"usage":{"cost":cost}}),
        );
        let mock = Mock::new(vec![meter(1), reply]).await;
        let completion = mock.client(1).complete(&request()).await.unwrap();
        assert_eq!(completion.receipts[0].cost.is_some(), known);
    }
}
