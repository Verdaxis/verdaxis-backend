//! The status heartbeat every stream sends.
//!
//! A heartbeat is the named event `heartbeat` with one JSON data line:
//! `{"status": ..., "message": ..., "server_time": ...}`. It has no `id` line,
//! so it never moves a client's `Last-Event-ID`. Every stream sends one when it
//! opens and then one every [`HEARTBEAT_INTERVAL`], whatever else it sends: a
//! client treats a stream as stale after 75 seconds without one.
//!
//! The status and message are runtime configuration (`SYSTEM_STATUS` and
//! `SYSTEM_STATUS_MESSAGE`), validated once at startup. The JSON encoder
//! escapes every control character, so no message can end a line or a frame.

use std::time::Duration;

use axum::response::sse::Event;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use tokio::time::Instant;

/// The longest time between two heartbeats on one stream.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// The longest status message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 280;

/// The SSE event name of a heartbeat.
pub const EVENT_NAME: &str = "heartbeat";

/// The operating state the service reports to clients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemStatus {
    #[default]
    Operational,
    Degraded,
    Maintenance,
}

impl SystemStatus {
    /// Parse a configured value: surrounding whitespace and case are ignored,
    /// and any other spelling is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "operational" => Ok(Self::Operational),
            "degraded" => Ok(Self::Degraded),
            "maintenance" => Ok(Self::Maintenance),
            _ => Err("SYSTEM_STATUS must be operational, degraded or maintenance".into()),
        }
    }
}

/// The validated status and optional message every heartbeat reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeartbeatStatus {
    status: SystemStatus,
    message: Option<String>,
}

/// The wire form of one heartbeat. Field order is the documented order.
#[derive(Serialize)]
struct Payload<'a> {
    status: SystemStatus,
    message: Option<&'a str>,
    server_time: String,
}

impl HeartbeatStatus {
    /// Validate the two configured values. An absent status is operational.
    /// A present status that is blank or unknown is refused. The message is
    /// trimmed; a blank message is no message; a longer one is refused.
    pub fn parse(status: Option<&str>, message: Option<&str>) -> Result<Self, String> {
        let status = status
            .map(SystemStatus::parse)
            .transpose()?
            .unwrap_or_default();
        let message = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned);
        if message
            .as_deref()
            .is_some_and(|message| message.chars().count() > MAX_MESSAGE_CHARS)
        {
            return Err(format!(
                "SYSTEM_STATUS_MESSAGE must be at most {MAX_MESSAGE_CHARS} characters"
            ));
        }
        Ok(Self { status, message })
    }

    pub fn status(&self) -> SystemStatus {
        self.status
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The JSON data line of a heartbeat sent at `now`.
    pub fn data(&self, now: DateTime<Utc>) -> String {
        serde_json::to_string(&Payload {
            status: self.status,
            message: self.message(),
            server_time: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        })
        .expect("a heartbeat payload is always valid JSON")
    }

    /// The SSE frame of a heartbeat sent at `now`: a name and one data line,
    /// never an `id`.
    pub fn event(&self, now: DateTime<Utc>) -> Event {
        Event::default().event(EVENT_NAME).data(self.data(now))
    }
}

/// When a stream's next heartbeat is due. A new schedule is due at once, so the
/// first heartbeat follows the open. Data frames never move the deadline; only
/// sending a heartbeat does.
#[derive(Clone, Copy, Debug)]
pub struct HeartbeatSchedule {
    due: Instant,
}

impl HeartbeatSchedule {
    pub fn new() -> Self {
        Self {
            due: Instant::now(),
        }
    }

    pub fn is_due(&self) -> bool {
        Instant::now() >= self.due
    }

    /// The instant a waiting stream wakes to send a heartbeat.
    pub fn deadline(&self) -> Instant {
        self.due
    }

    /// Record that a heartbeat was just sent.
    pub fn sent(&mut self) {
        self.due = Instant::now() + HEARTBEAT_INTERVAL;
    }
}

impl Default for HeartbeatSchedule {
    fn default() -> Self {
        Self::new()
    }
}
