//! The three market stream routes.
//!
//! Public prices and orderbook streams need no authentication. The private
//! trades stream admits one short-lived stream token, replays retained rows
//! and then follows live dispatch. Every pre-header failure is a structured
//! HTTP error; terminal stream controls stay in-band. The routes own a
//! stream-local method fallback so unrelated REST behaviour is untouched.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use futures_util::stream::Stream;
use serde::Deserialize;

use super::{
    auth::{StreamError, verify_stream},
    bus::{Delivery, PublicChannel, StreamHub, Subscription},
    replay::{PgReplaySource, Replay, ReplayFrame, WallClock},
};
use crate::AppState;

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

#[derive(Deserialize)]
struct TradesQuery {
    stream_token: Option<String>,
    last_event_id: Option<String>,
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

/// The stream-local method fallback: GET only, no implicit HEAD, and the same
/// structured envelope the client library already parses.
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

pub fn routes() -> Router<StreamsState> {
    Router::new()
        .route(
            "/api/stream/prices",
            get(prices)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
        .route(
            "/api/stream/prices/",
            get(prices)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
        .route(
            "/api/stream/orderbook",
            get(orderbook)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
        .route(
            "/api/stream/orderbook/",
            get(orderbook)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
        .route(
            "/api/stream/trades",
            get(trades)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
        .route(
            "/api/stream/trades/",
            get(trades)
                .post(method_not_allowed)
                .head(method_not_allowed),
        )
}

fn sse(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        "text/event-stream".parse().expect("static mime"),
    );
    headers.insert(header::CACHE_CONTROL, "no-cache".parse().expect("static"));
    headers.insert("x-accel-buffering", "no".parse().expect("static"));
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
    let hub = streams.hub.clone();
    let Ok(subscription) = hub.subscribe_public(channel) else {
        return unavailable();
    };
    let body = Sse::new(SubscriptionStream { subscription })
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(30)));
    let mut response = body.into_response();
    sse(&mut response);
    response
}

/// One owned public body: the subscription itself is the stream, so dropping
/// the body (client cancel or disconnect) drops the registration.
struct SubscriptionStream {
    subscription: Subscription,
}

impl Stream for SubscriptionStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures_util::StreamExt;
        self.subscription.poll_next_unpin(cx).map(|option| {
            option.map(|delivery| {
                Ok(delivery.into_event())
            })
        })
    }
}

/// The trades admission order: cursor syntax, Authorization conflict, token
/// presence, then token validity and current account state.
async fn trades(
    State(state): State<StreamsState>,
    Query(query): Query<TradesQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(streams) = state.state.streams.as_ref() else {
        return unavailable();
    };
    let header_cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty());
    let query_cursor = query.last_event_id.as_deref().filter(|v| !v.is_empty());
    let cursor_text = header_cursor.or(query_cursor);
    let Some(cursor) = parse_cursor(cursor_text) else {
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
        return stream_error(
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_REQUIRED",
            "A stream token is required",
        );
    };
    let principal = match verify_stream(
        token,
        &streams.verification,
        &state.state.config.environment,
        chrono::Utc::now(),
    ) {
        Ok(principal) => principal,
        Err(StreamError::TokenRequired) => {
            return stream_error(
                StatusCode::UNAUTHORIZED,
                "STREAM_TOKEN_REQUIRED",
                "A stream token is required",
            )
        }
        Err(StreamError::TokenInvalid) => {
            return stream_error(
                StatusCode::UNAUTHORIZED,
                "STREAM_TOKEN_INVALID",
                "The stream token is invalid",
            )
        }
        Err(StreamError::AuthorizationUnavailable) => {
            return stream_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "STREAM_AUTH_UNAVAILABLE",
                "Stream authorization is temporarily unavailable",
            )
        }
    };
    let source = PgReplaySource::new(state.state.pools.read.clone());
    match Replay::open(
        source,
        &streams.hub,
        principal,
        cursor,
        WallClock,
    )
    .await
    {
        Ok(replay) => {
            let body = Sse::new(ReplayStream { replay })
                .keep_alive(KeepAlive::new().interval(Duration::from_secs(30)));
            let mut response = body.into_response();
            sse(&mut response);
            response
        }
        Err(super::replay::AdmissionError::Authorization(
            StreamError::AuthorizationUnavailable,
        )) => stream_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "STREAM_AUTH_UNAVAILABLE",
            "Stream authorization is temporarily unavailable",
        ),
        Err(super::replay::AdmissionError::Authorization(_)) => stream_error(
            StatusCode::UNAUTHORIZED,
            "STREAM_TOKEN_INVALID",
            "The stream token is invalid",
        ),
        Err(_) => unavailable(),
    }
}

/// A cursor is absent, or 1 to 19 ASCII digits within `i64`. Anything else is
/// refused before the token is even read.
fn parse_cursor(raw: Option<&str>) -> Option<Option<i64>> {
    match raw {
        None => Some(None),
        Some(text)
            if !text.is_empty()
                && text.len() <= 19
                && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse::<i64>().ok().map(Some)
        }
        Some(_) => None,
    }
}

/// One owned private body; dropping it cancels replay and releases the
/// registration and any in-flight read.
struct ReplayStream<S> {
    replay: Replay<S>,
}

impl<S: super::replay::ReplaySource> Stream for ReplayStream<S> {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures_util::StreamExt;
        self.replay.poll_next_unpin(cx).map(|option| {
            option.map(|frame| Ok(match frame {
                ReplayFrame::Business(_) | ReplayFrame::Terminal(_) => frame.into_event(),
                ReplayFrame::Heartbeat => Event::default().comment("keepalive"),
            }))
        })
    }
}
