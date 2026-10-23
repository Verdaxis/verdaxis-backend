//! Bounded, scope-specific fan-out. Registration never waits for a consumer.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use axum::response::sse::Event;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use super::policy::{MAX_EVENT_BYTES, MAX_PARTICIPANTS, StreamPolicy};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublicChannel {
    Prices,
    Orderbook,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    Overflow,
    Retention,
    CursorAhead,
    AuthExpired,
    AuthRevoked,
    AuthorizationUnavailable,
    Unavailable,
    Capacity,
    InvalidEvent,
    Shutdown,
}

impl Terminal {
    pub fn event_name(self) -> &'static str {
        match self {
            Self::AuthExpired => "auth_expired",
            Self::AuthRevoked | Self::AuthorizationUnavailable => "auth_revoked",
            Self::Unavailable | Self::Capacity => "error",
            _ => "reset",
        }
    }

    pub fn payload(self) -> Value {
        let (reason, resync, error) = match self {
            Self::Overflow => ("subscriber_overflow", true, None),
            Self::Retention => ("replay_retention", true, None),
            Self::CursorAhead => ("cursor_ahead", true, None),
            Self::AuthExpired => ("token_expired", false, Some("Stream token expired")),
            Self::AuthRevoked => (
                "authorization_changed",
                true,
                Some("Stream authorization changed"),
            ),
            Self::AuthorizationUnavailable => (
                "authorization_unavailable",
                true,
                Some("Stream authorization unavailable"),
            ),
            Self::Unavailable => ("stream_unavailable", true, Some("Stream unavailable")),
            Self::Capacity => (
                "connection_capacity",
                false,
                Some("Stream connection capacity reached"),
            ),
            Self::InvalidEvent => ("event_unavailable", true, None),
            Self::Shutdown => ("service_restarting", true, None),
        };
        let mut payload = json!({"schema_version": 1, "reason": reason, "resync_required": resync});
        if let Some(message) = error {
            payload["error"] = json!(message);
        }
        payload
    }

    pub fn into_event(self) -> Event {
        Event::default()
            .event(self.event_name())
            .data(self.payload().to_string())
    }
}

/// Validated shared envelope. Store-side size checks must precede JSON decoding.
#[derive(Debug)]
pub struct StoredEvent {
    seq: i64,
    participants: Vec<Uuid>,
    event_type: String,
    payload: Value,
    public_invalidated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEvent;

impl StoredEvent {
    pub fn new(
        seq: i64,
        participants: Vec<Uuid>,
        event_type: &str,
        mut payload: Value,
    ) -> Result<Self, InvalidEvent> {
        if seq <= 0
            || participants.len() > MAX_PARTICIPANTS
            || event_type.is_empty()
            || event_type.len() > 80
            || !event_type
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !payload.is_object()
            || serde_json::to_vec(&payload)
                .map_err(|_| InvalidEvent)?
                .len()
                > MAX_EVENT_BYTES
        {
            return Err(InvalidEvent);
        }
        let object = payload.as_object_mut().ok_or(InvalidEvent)?;
        let public_invalidated = object
            .remove("_public_market_invalidation")
            .is_some_and(|value| value == Value::Bool(true));
        object.entry("schema_version").or_insert(json!(1));
        Ok(Self {
            seq,
            participants,
            event_type: event_type.to_owned(),
            payload,
            public_invalidated,
        })
    }
    pub fn seq(&self) -> i64 {
        self.seq
    }
    pub fn participants(&self) -> &[Uuid] {
        &self.participants
    }
    pub fn event_type(&self) -> &str {
        &self.event_type
    }
    pub fn payload(&self) -> &Value {
        &self.payload
    }
    pub fn public_invalidated(&self) -> bool {
        self.public_invalidated
    }
    pub fn into_event(&self) -> Event {
        Event::default()
            .event(&self.event_type)
            .id(self.seq.to_string())
            .data(self.payload.to_string())
    }
}

#[derive(Debug)]
pub enum Delivery {
    Private(Arc<StoredEvent>),
    PublicInvalidation,
    Terminal(Terminal),
}

impl Delivery {
    pub fn into_event(self) -> Event {
        match self {
            Self::Private(event) => event.into_event(),
            Self::PublicInvalidation => Event::default()
                .event("market_invalidated")
                .data(json!({"schema_version": 1, "resync_required": true}).to_string()),
            Self::Terminal(terminal) => terminal.into_event(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Channel {
    Public(PublicChannel),
    Org(Uuid),
}

struct Registration {
    data: mpsc::Sender<Delivery>,
    terminal: watch::Sender<Option<Terminal>>,
}

#[derive(Default)]
struct Registry {
    channels: HashMap<Channel, HashMap<u64, Registration>>,
    admitted_by_channel: HashMap<Channel, usize>,
    admitted: usize,
    count: usize,
    next_id: u64,
}

struct Inner {
    policy: StreamPolicy,
    registry: Mutex<Registry>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn detach(&self, channel: Channel, id: u64, terminal: Option<Terminal>) {
        let mut registry = self.lock();
        if let Some(entries) = registry.channels.get_mut(&channel) {
            if let Some(entry) = entries.remove(&id) {
                if let Some(terminal) = terminal {
                    entry.terminal.send_replace(Some(terminal));
                }
                registry.count -= 1;
            }
            if registry
                .channels
                .get(&channel)
                .is_some_and(HashMap::is_empty)
            {
                registry.channels.remove(&channel);
            }
        }
    }
}

#[derive(Clone)]
pub struct StreamHub {
    inner: Arc<Inner>,
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new(StreamPolicy::default())
    }
}

impl StreamHub {
    pub fn new(policy: StreamPolicy) -> Self {
        Self {
            inner: Arc::new(Inner {
                policy,
                registry: Mutex::new(Registry::default()),
            }),
        }
    }
    pub fn subscribe_public(&self, channel: PublicChannel) -> Result<Subscription, Capacity> {
        self.subscribe(Channel::Public(channel))
    }
    pub fn subscribe_org(&self, org: Uuid) -> Result<Subscription, Capacity> {
        self.subscribe(Channel::Org(org))
    }
    fn subscribe(&self, channel: Channel) -> Result<Subscription, Capacity> {
        let mut registry = self.inner.lock();
        if registry.admitted >= self.inner.policy.process
            || registry
                .admitted_by_channel
                .get(&channel)
                .copied()
                .unwrap_or(0)
                >= self.inner.policy.per_channel
        {
            return Err(Capacity);
        }
        let id = registry.next_id.checked_add(1).ok_or(Capacity)?;
        let (sender, data) = mpsc::channel(self.inner.policy.queue);
        let (terminal_sender, terminal) = watch::channel(None);
        registry.channels.entry(channel).or_default().insert(
            id,
            Registration {
                data: sender,
                terminal: terminal_sender,
            },
        );
        registry.next_id = id;
        registry.count += 1;
        registry.admitted += 1;
        *registry.admitted_by_channel.entry(channel).or_default() += 1;
        Ok(Subscription {
            data,
            terminal,
            finished: false,
            admission: Admission {
                inner: self.inner.clone(),
                channel,
                id,
            },
        })
    }
    pub fn publish_private(&self, event: Arc<StoredEvent>) -> Result<(), HubError> {
        let mut participants = event.participants.clone();
        participants.sort_unstable();
        participants.dedup();
        for org in participants {
            self.publish(Channel::Org(org), || Delivery::Private(event.clone()));
        }
        Ok(())
    }
    pub fn invalidate_public(&self, channel: PublicChannel) -> Result<(), HubError> {
        self.publish(Channel::Public(channel), || Delivery::PublicInvalidation);
        Ok(())
    }
    fn publish(&self, channel: Channel, delivery: impl Fn() -> Delivery) {
        let mut registry = self.inner.lock();
        let mut removed = 0;
        if let Some(entries) = registry.channels.get_mut(&channel) {
            entries.retain(|_, entry| match entry.data.try_send(delivery()) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    entry.terminal.send_replace(Some(Terminal::Overflow));
                    removed += 1;
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    removed += 1;
                    false
                }
            });
            if entries.is_empty() {
                registry.channels.remove(&channel);
            }
        }
        registry.count -= removed;
    }
    pub fn terminate_all(&self, terminal: Terminal) {
        let mut registry = self.inner.lock();
        for entries in registry.channels.values() {
            for entry in entries.values() {
                entry.terminal.send_replace(Some(terminal));
            }
        }
        registry.channels.clear();
        registry.count = 0;
    }
    pub fn subscriber_count(&self) -> usize {
        self.inner.lock().count
    }
}

/// The body owns this guard. Dropping a cancelled receive alone is harmless.
pub struct Subscription {
    data: mpsc::Receiver<Delivery>,
    terminal: watch::Receiver<Option<Terminal>>,
    finished: bool,
    // Fields drop in declaration order: release the queue before its admission.
    admission: Admission,
}

struct Admission {
    inner: Arc<Inner>,
    channel: Channel,
    id: u64,
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.inner.detach(self.channel, self.id, None);
        let mut registry = self.inner.lock();
        registry.admitted -= 1;
        if let Some(count) = registry.admitted_by_channel.get_mut(&self.channel) {
            *count -= 1;
            if *count == 0 {
                registry.admitted_by_channel.remove(&self.channel);
            }
        }
    }
}

impl Subscription {
    pub fn terminate(&self, terminal: Terminal) {
        let admission = &self.admission;
        admission
            .inner
            .detach(admission.channel, admission.id, Some(terminal));
    }

    pub fn terminal(&self) -> Option<Terminal> {
        *self.terminal.borrow()
    }

    /// Wait only for terminal control. This does not receive, copy or drain
    /// queued data, and cancelling the wait does not consume the terminal.
    pub async fn wait_terminal(&mut self) -> Terminal {
        loop {
            if let Some(terminal) = self.terminal() {
                return terminal;
            }
            if self.terminal.changed().await.is_err() {
                return Terminal::Unavailable;
            }
        }
    }

    fn finish(&mut self, terminal: Terminal) -> Option<Delivery> {
        self.finished = true;
        self.data.close();
        while self.data.try_recv().is_ok() {}
        Some(Delivery::Terminal(terminal))
    }

    pub async fn recv(&mut self) -> Option<Delivery> {
        if self.finished {
            return None;
        }
        if let Some(terminal) = self.terminal() {
            return self.finish(terminal);
        }
        let delivery = tokio::select! {
            biased;
            changed = self.terminal.changed() => {
                if changed.is_ok() && let Some(terminal) = self.terminal() {
                    return self.finish(terminal);
                }
                None
            }
            delivery = self.data.recv() => delivery,
        };
        if let Some(terminal) = self.terminal() {
            return self.finish(terminal);
        }
        if delivery.is_none() {
            self.finished = true;
        }
        delivery
    }
}
