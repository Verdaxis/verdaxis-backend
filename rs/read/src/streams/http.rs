//! The three market stream routes.
//!
//! Public prices and orderbook streams need no authentication. The private
//! trades stream admits one short-lived stream token, replays retained rows
//! and then follows live dispatch. Every pre-header failure is a structured
//! HTTP error. Capacity, like every terminal condition after admission, is one
//! in-band control frame after a 200. The routes own a stream-local method
//! fallback, so unrelated REST behaviour is untouched.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{RawQuery, State},
    handler::Handler,
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{MethodRouter, get},
};
use futures_util::stream::{self, unfold};

use super::{
    auth::{StreamError, verify_stream},
    bus::{PublicChannel, StreamHub, Terminal},
    replay::{AdmissionError, PgReplaySource, Replay, WallClock},
};
use crate::AppState;

/// The longest silence on an open stream: a keep-alive comment follows 30
/// seconds without a frame.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// The replay cursor header a reconnecting `EventSource` sends.
const LAST_EVENT_ID: &str = "last-event-id";

/// The longest cursor: the number of digits of `i64::MAX`.
const MAX_CURSOR_DIGITS: usize = 19;

/// The live stream subsystem the runtime owns: one shared hub plus the
/// verification settings stream admission uses.
pub struct Streams {
    pub hub: StreamHub,
    pub verification: crate::auth::VerificationSettings,
}

/// Stream routes take the application state directly; `AppState::streams`
/// carries the subsystem. Its absence means the subsystem is down, and every
/// route then answers the same bounded 503 while REST stays available.
#[derive(Clone)]
pub struct StreamsState {
    pub state: Arc<AppState>,
}

/// The trades query parameters. The last value of a repeated parameter wins;
/// other parameters are ignored.
#[derive(Default)]
struct TradesQuery {
    stream_token: Option<String>,
    last_event_id: Option<String>,
}

impl TradesQuery {
    fn parse(query: Option<&str>) -> Self {
        let mut parsed = Self::default();
        for (name, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            match name.as_ref() {
                "stream_token" => parsed.stream_token = Some(value.into_owned()),
                "last_event_id" => parsed.last_event_id = Some(value.into_owned()),
                _ => {}
            }
        }
        parsed
    }
}

/// Neutral failures: status, code and a bounded message, before any header.
fn stream_error(status: StatusCode, code: &str, message: &str) -> Response {
    crate::ApiError {
        status,
        detail: serde_json::json!({"code": code, "message": message}),
        retry_after: None,
        kind: crate::ApiErrorKind::Expected,
    }
    .into_response()
}

fn unavailable() -> Response {
    stream_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "STREAM_UNAVAILABLE",
        "Market streams are temporarily unavailable",
    )
}

/// A token or account refusal before any header. Each refusal class has one
/// status and code.
fn authorization_error(error: StreamError) -> Response {
    let message = match error {
        StreamError::TokenRequired => "A stream token is required",
        StreamError::TokenInvalid => "The stream token is invalid",
        StreamError::AuthorizationUnavailable => "Stream authorization is temporarily unavailable",
    };
    let status = StatusCode::from_u16(error.status()).expect("bounded stream status");
    stream_error(status, error.code(), message)
}

/// The stream-local method refusal: the same structured envelope the client
/// library already parses, with `Allow: GET`.
async fn method_not_allowed() -> Response {
    let mut response = stream_error(
        StatusCode::METHOD_NOT_ALLOWED,
        "STREAM_METHOD_NOT_ALLOWED",
        "Streams accept GET only",
    );
    response
        .headers_mut()
        .insert(header::ALLOW, "GET".parse().expect("static header"));
    response
}

/// GET only. HEAD and every other method get the stream-local refusal. The
/// explicit HEAD handler keeps Axum from answering HEAD through GET, and the
/// method fallback keeps the serving router's shared fallback off these paths.
fn get_only<H, T>(handler: H) -> MethodRouter<StreamsState>
where
    H: Handler<T, StreamsState>,
    T: 'static,
{
    get(handler)
        .head(method_not_allowed)
        .fallback(method_not_allowed)
}

pub fn routes() -> Router<StreamsState> {
    Router::new()
        .route("/api/stream/prices", get_only(prices))
        .route("/api/stream/prices/", get_only(prices))
        .route("/api/stream/orderbook", get_only(orderbook))
        .route("/api/stream/orderbook/", get_only(orderbook))
        .route("/api/stream/trades", get_only(trades))
        .route("/api/stream/trades/", get_only(trades))
}

fn with_stream_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "text/event-stream".parse().expect("static mime"),
    );
    headers.insert(header::CACHE_CONTROL, "no-cache".parse().expect("static"));
    headers.insert("x-accel-buffering", "no".parse().expect("static"));
    response
}

/// A full channel or the process subscriber cap: 200 with the stream headers,
/// one `error` control with reason `connection_capacity`, then the end of the
/// body. The client closes and retries with its backoff.
fn capacity() -> Response {
    let frame: Result<Event, Infallible> = Ok(Terminal::Capacity.into_event());
    with_stream_headers(Sse::new(stream::iter([frame])).into_response())
}

async fn prices(State(state): State<StreamsState>) -> Response {
    public(&state, PublicChannel::Prices)
}

async fn orderbook(State(state): State<StreamsState>) -> Response {
    public(&state, PublicChannel::Orderbook)
}

fn public(state: &StreamsState, channel: PublicChannel) -> Response {
    let Some(streams) = state.state.streams.as_ref() else {
        return unavailable();
    };
    let Ok(subscription) = streams.hub.subscribe_public(channel) else {
        return capacity();
    };
    let body = Sse::new(unfold(subscription, |mut subscription| async move {
        subscription.recv().await.map(|delivery| {
            let event: Result<Event, Infallible> = Ok(delivery.into_event());
            (event, subscription)
        })
    }))
    .keep_alive(KeepAlive::new().interval(KEEP_ALIVE_INTERVAL));
    with_stream_headers(body.into_response())
}

/// The trades admission order: cursor syntax, Authorization conflict, token
/// presence, then token validity and current account state.
async fn trades(
    State(state): State<StreamsState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(streams) = state.state.streams.as_ref() else {
        return unavailable();
    };
    let query = TradesQuery::parse(query.as_deref());
    let Ok(cursor) = requested_cursor(&headers, query.last_event_id.as_deref()) else {
        return stream_error(
            StatusCode::BAD_REQUEST,
            "STREAM_CURSOR_INVALID",
            "The stream cursor is invalid",
        );
    };
    if headers.contains_key(header::AUTHORIZATION) {
        return stream_error(
            StatusCode::BAD_REQUEST,
            "STREAM_AUTH_CONFLICT",
            "Use the stream token query parameter, not a bearer header",
        );
    }
    let Some(token) = query
        .stream_token
        .as_deref()
        .filter(|token| !token.is_empty())
    else {
        return authorization_error(StreamError::TokenRequired);
    };
    let principal = match verify_stream(
        token,
        &streams.verification,
        &state.state.config.environment,
        chrono::Utc::now(),
    ) {
        Ok(principal) => principal,
        Err(error) => return authorization_error(error),
    };
    let source = PgReplaySource::new(state.state.pools.read.clone());
    match Replay::open(source, &streams.hub, principal, cursor, WallClock).await {
        Ok(replay) => {
            let body = Sse::new(unfold(replay, |mut replay| async move {
                replay.next().await.map(|frame| {
                    let event: Result<Event, Infallible> = Ok(frame.into_event());
                    (event, replay)
                })
            }))
            .keep_alive(KeepAlive::new().interval(KEEP_ALIVE_INTERVAL));
            with_stream_headers(body.into_response())
        }
        Err(AdmissionError::Authorization(error)) => authorization_error(error),
        Err(AdmissionError::Capacity) => capacity(),
        Err(AdmissionError::Store(_)) => unavailable(),
    }
}

/// An unreadable or out-of-range replay cursor.
struct InvalidCursor;

/// The replay cursor. A non-empty `Last-Event-ID` header wins over the
/// `last_event_id` query value, and an empty header falls back to the query.
/// A header that is not visible ASCII is an invalid cursor, not an absent one.
fn requested_cursor(
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<Option<i64>, InvalidCursor> {
    let text = match headers.get(LAST_EVENT_ID).filter(|value| !value.is_empty()) {
        Some(value) => Some(value.to_str().map_err(|_| InvalidCursor)?),
        None => query.filter(|value| !value.is_empty()),
    };
    text.map(parse_cursor).transpose()
}

/// A cursor is at most 19 ASCII digits and fits in `i64`.
fn parse_cursor(text: &str) -> Result<i64, InvalidCursor> {
    if text.len() > MAX_CURSOR_DIGITS || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InvalidCursor);
    }
    text.parse().map_err(|_| InvalidCursor)
}
