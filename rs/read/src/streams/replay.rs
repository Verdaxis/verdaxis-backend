//! Pull-driven private replay. Each read finishes before a frame is returned.
//! The controller owns one bounded page and the subscription's bounded queue;
//! it does not spawn an output task or hold a database slot during client waits.

use std::{collections::VecDeque, future::Future, time::Duration};

use axum::response::sse::Event;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    auth::{StreamError, StreamPrincipal, revalidate_from_pool},
    bus::{Delivery, StreamHub, Subscription, Terminal},
    store::{self, PageLimits, ReplayBoundary, ReplayPage, StoreError, StoredEvent},
};

const RECHECK: Duration = Duration::from_secs(15);
const HEARTBEAT: Duration = Duration::from_secs(30);

/// Snapshot and admission authority. Implementations return owned pages, never
/// a transaction or connection that could cross a client yield.
pub trait ReplaySource: Send + Sync {
    fn boundary(&self) -> impl Future<Output = Result<ReplayBoundary, StoreError>> + Send;
    fn page(
        &self,
        organization: Uuid,
        after: i64,
        through: i64,
        limits: PageLimits,
    ) -> impl Future<Output = Result<ReplayPage, StoreError>> + Send;
    fn revalidate(
        &self,
        principal: &StreamPrincipal,
    ) -> impl Future<Output = Result<(), StreamError>> + Send;
}

#[derive(Clone)]
pub struct PgReplaySource {
    pool: PgPool,
}
impl PgReplaySource {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
impl ReplaySource for PgReplaySource {
    async fn boundary(&self) -> Result<ReplayBoundary, StoreError> {
        store::replay_boundary(&self.pool).await
    }
    async fn page(
        &self,
        organization: Uuid,
        after: i64,
        through: i64,
        limits: PageLimits,
    ) -> Result<ReplayPage, StoreError> {
        store::replay_page(&self.pool, organization, after, through, limits).await
    }
    async fn revalidate(&self, principal: &StreamPrincipal) -> Result<(), StreamError> {
        revalidate_from_pool(&self.pool, principal).await
    }
}

pub trait ReplayClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
#[derive(Clone, Copy, Default)]
pub struct WallClock;
impl ReplayClock for WallClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug)]
pub enum ReplayFrame {
    Business(StoredEvent),
    Terminal(Terminal),
    Heartbeat,
}
impl ReplayFrame {
    pub fn sequence(&self) -> Option<i64> {
        match self {
            Self::Business(event) => Some(event.seq),
            _ => None,
        }
    }
    pub fn terminal(&self) -> Option<Terminal> {
        match self {
            Self::Terminal(terminal) => Some(*terminal),
            _ => None,
        }
    }
    pub fn into_event(self) -> Event {
        match self {
            Self::Business(event) => Event::default()
                .event(event.event_type)
                .id(event.seq.to_string())
                .data(event.payload.to_string()),
            Self::Terminal(terminal) => terminal.into_event(),
            Self::Heartbeat => Event::default().comment("keepalive"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    Authorization(StreamError),
    Store(StoreError),
    Capacity,
}

/// One body owns this value. Drop (including cancellation) releases its
/// registration and buffered page without an orphaned worker.
pub struct Replay<S, C = WallClock> {
    source: S,
    clock: C,
    principal: StreamPrincipal,
    subscription: Option<Subscription>,
    cursor: i64,
    through: i64,
    replaying: bool,
    page: VecDeque<StoredEvent>,
    more: bool,
    pending_terminal: Option<Terminal>,
    finished: bool,
    checked_at: DateTime<Utc>,
    recheck_at: Instant,
    heartbeat_at: Instant,
}

impl<S: ReplaySource, C: ReplayClock> Replay<S, C> {
    /// Admission failures remain HTTP errors. Cursor/floor outcomes are owned
    /// terminal controls. Registration precedes capture to cover live overlap.
    pub async fn open(
        source: S,
        hub: &StreamHub,
        principal: StreamPrincipal,
        cursor: Option<i64>,
        clock: C,
    ) -> Result<Self, AdmissionError> {
        if cursor.is_some_and(|cursor| cursor < 0) {
            return Err(AdmissionError::Store(StoreError::InvalidPageRequest));
        }
        source
            .revalidate(&principal)
            .await
            .map_err(AdmissionError::Authorization)?;
        let subscription = hub
            .subscribe_org(principal.organization_id())
            .map_err(|_| AdmissionError::Capacity)?;
        let boundary = source.boundary().await.map_err(AdmissionError::Store)?;
        let now = clock.now();
        let pending_terminal = if now >= principal.expires_at() {
            Some(Terminal::AuthExpired)
        } else if cursor.is_some_and(|cursor| cursor < boundary.discarded_through) {
            Some(Terminal::Retention)
        } else if cursor.is_some_and(|cursor| cursor > boundary.high_water) {
            Some(Terminal::CursorAhead)
        } else {
            None
        };
        Ok(Self {
            source,
            clock,
            principal,
            subscription: Some(subscription),
            cursor: cursor.unwrap_or(boundary.high_water),
            through: boundary.high_water,
            replaying: cursor.is_some(),
            page: VecDeque::new(),
            more: false,
            pending_terminal,
            finished: false,
            checked_at: now,
            recheck_at: Instant::now() + RECHECK,
            heartbeat_at: Instant::now() + HEARTBEAT,
        })
    }

    fn observed_terminal(&self) -> Option<Terminal> {
        self.subscription
            .as_ref()
            .and_then(Subscription::terminal)
            .or(self.pending_terminal)
            .or_else(|| {
                (self.clock.now() >= self.principal.expires_at()).then_some(Terminal::AuthExpired)
            })
    }
    fn finish(&mut self, terminal: Terminal) -> Option<ReplayFrame> {
        if let Some(subscription) = self.subscription.take() {
            subscription.terminate(terminal);
        }
        self.page.clear();
        self.finished = true;
        Some(ReplayFrame::Terminal(terminal))
    }
    fn check_due(&self) -> bool {
        Instant::now() >= self.recheck_at
            || self
                .clock
                .now()
                .signed_duration_since(self.checked_at)
                .num_seconds()
                >= 15
    }
    fn expiry_deadline(&self) -> Instant {
        Instant::now()
            + self
                .principal
                .expires_at()
                .signed_duration_since(self.clock.now())
                .to_std()
                .unwrap_or_default()
    }

    /// Cancel-safe between calls: captured data stays in this controller until
    /// its final auth/terminal check. No output queue can hide terminal state.
    pub async fn next(&mut self) -> Option<ReplayFrame> {
        if self.finished {
            return None;
        }
        loop {
            if let Some(terminal) = self.observed_terminal() {
                return self.finish(terminal);
            }
            if self.check_due() {
                let expiry = self.expiry_deadline();
                let result = tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(expiry) => return self.finish(Terminal::AuthExpired),
                    result = self.source.revalidate(&self.principal) => result,
                };
                if let Some(terminal) = self.observed_terminal() {
                    return self.finish(terminal);
                }
                if let Err(error) = result {
                    return self.finish(auth_terminal(error));
                }
                self.checked_at = self.clock.now();
                self.recheck_at = Instant::now() + RECHECK;
            }
            if let Some(event) = self.page.pop_front() {
                if event.seq <= self.cursor {
                    continue;
                }
                self.cursor = event.seq;
                self.heartbeat_at = Instant::now() + HEARTBEAT;
                return Some(ReplayFrame::Business(event));
            }
            if self.replaying {
                let expiry = self.expiry_deadline();
                let result = tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(expiry) => return self.finish(Terminal::AuthExpired),
                    result = self.source.page(self.principal.organization_id(), self.cursor, self.through, PageLimits::default()) => result,
                };
                if let Some(terminal) = self.observed_terminal() {
                    return self.finish(terminal);
                }
                let page = match result {
                    Ok(page) => page,
                    Err(error) => return self.finish(store_terminal(error)),
                };
                if page.boundary.discarded_through > self.cursor {
                    return self.finish(Terminal::Retention);
                }
                if !valid_page(
                    &page,
                    self.cursor,
                    self.through,
                    self.principal.organization_id(),
                ) {
                    return self.finish(Terminal::InvalidEvent);
                }
                self.more = page.more;
                self.page = page.events.into();
                // Every captured complete page is valid even if a prune commits
                // while the client consumes it. The next snapshot checks again.
                self.replaying = self.more;
                continue;
            }
            let expiry = self.expiry_deadline();
            let subscription = self.subscription.as_mut()?;
            let delivery = tokio::select! {
                biased;
                _ = tokio::time::sleep_until(expiry) => return self.finish(Terminal::AuthExpired),
                _ = tokio::time::sleep_until(self.recheck_at) => continue,
                delivery = subscription.recv() => delivery,
                _ = tokio::time::sleep_until(self.heartbeat_at) => {
                    self.heartbeat_at = Instant::now() + HEARTBEAT;
                    return Some(ReplayFrame::Heartbeat);
                }
            };
            match delivery {
                Some(Delivery::Terminal(terminal)) => return self.finish(terminal),
                Some(Delivery::Private(event)) if event.seq() > self.cursor => {
                    if !event
                        .participants()
                        .contains(&self.principal.organization_id())
                    {
                        return self.finish(Terminal::InvalidEvent);
                    }
                    self.page.push_back(StoredEvent {
                        seq: event.seq(),
                        participants: event.participants().to_vec(),
                        event_type: event.event_type().to_owned(),
                        payload: event.payload().clone(),
                        public_invalidated: event.public_invalidated(),
                    });
                }
                Some(Delivery::Private(_)) => {}
                Some(Delivery::PublicInvalidation) => return self.finish(Terminal::InvalidEvent),
                None => return self.finish(Terminal::Unavailable),
            }
        }
    }
}

fn auth_terminal(error: StreamError) -> Terminal {
    match error {
        StreamError::AuthorizationUnavailable => Terminal::AuthorizationUnavailable,
        _ => Terminal::AuthRevoked,
    }
}
fn store_terminal(error: StoreError) -> Terminal {
    match error {
        StoreError::InvalidEvent(_) => Terminal::InvalidEvent,
        _ => Terminal::Unavailable,
    }
}
fn valid_page(page: &ReplayPage, after: i64, through: i64, organization: Uuid) -> bool {
    if page.events.len() > PageLimits::default().rows || (page.more && page.events.is_empty()) {
        return false;
    }
    let mut previous = after;
    page.events.iter().all(|event| {
        let valid = event.seq > previous
            && event.seq <= through
            && event.participants.contains(&organization);
        previous = event.seq;
        valid
    })
}
