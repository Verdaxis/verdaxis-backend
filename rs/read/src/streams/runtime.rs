//! Owned stream startup, supervision and shutdown in the serving process.

use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;

use super::{
    bus::{StreamHub, Terminal},
    dispatch::{Dispatch, PgDispatchStore},
    election::SessionSettings,
    http::Streams,
    policy::{self, StreamPolicy},
};
use crate::AppState;

pub const STREAM_REVISION: &str = "sse_20261021_replay_floor";
const RESTART_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub struct CapacityConfig {
    pub per_channel: usize,
    pub process: usize,
    pub queue: usize,
    pub page_rows: usize,
    pub page_bytes: usize,
    pub event_bytes: usize,
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            per_channel: policy::MAX_CHANNEL_SUBSCRIBERS,
            process: policy::MAX_PROCESS_SUBSCRIBERS,
            queue: policy::MAX_QUEUED_EVENTS,
            page_rows: policy::MAX_PAGE_ROWS,
            page_bytes: policy::MAX_PAGE_BYTES,
            event_bytes: policy::MAX_EVENT_BYTES,
        }
    }
}

impl CapacityConfig {
    pub fn policy(self) -> Result<StreamPolicy, &'static str> {
        if self.page_rows != policy::MAX_PAGE_ROWS
            || self.page_bytes != policy::MAX_PAGE_BYTES
            || self.event_bytes != policy::MAX_EVENT_BYTES
        {
            return Err("stream store bounds must match the compiled policy");
        }
        StreamPolicy::new(self.per_channel, self.process, self.queue)
            .map_err(|_| "stream subscriber limits are outside the compiled policy")
    }
}

#[derive(Default)]
pub struct RestartBudget {
    restarts: u8,
}

impl RestartBudget {
    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.restarts >= 3 {
            return None;
        }
        self.restarts += 1;
        Some(RESTART_COOLDOWN)
    }
}

pub fn enabled(config: &crate::Config) -> bool {
    config
        .prepared_budget
        .as_deref()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .is_some_and(|budget| budget.get("stream_listener").and_then(|v| v.as_u64()) == Some(1))
}

pub fn ready(state: &AppState) -> bool {
    !enabled(&state.config)
        || state
            .streams
            .as_ref()
            .is_some_and(|streams| streams.hub.is_available())
}

/// Drop requests cancellation; explicit shutdown also waits for session release.
pub struct StreamRuntime {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    hub: StreamHub,
}

impl StreamRuntime {
    pub async fn shutdown(mut self) {
        self.hub.set_available(false, Terminal::Shutdown);
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for StreamRuntime {
    fn drop(&mut self) {
        self.hub.set_available(false, Terminal::Shutdown);
        self.stop.send_replace(true);
    }
}

/// A failed component start never fails REST startup. No handle is exposed until
/// the floor and scan baseline have been read and the dedicated slot attested.
pub async fn start_streams(
    state: &mut AppState,
    capacity: CapacityConfig,
) -> Option<StreamRuntime> {
    if !enabled(&state.config) {
        return None;
    }
    match start(state, capacity).await {
        Ok(runtime) => Some(runtime),
        Err(()) => {
            tracing::warn!(reason = "stream_unavailable", "stream startup unavailable");
            None
        }
    }
}

async fn start(state: &mut AppState, capacity: CapacityConfig) -> Result<StreamRuntime, ()> {
    let config = &state.config;
    if config.migration_revision != STREAM_REVISION {
        return Err(());
    }
    crate::runtime::config::validate_prepared_budget(
        config,
        config.prepared_budget.as_deref().ok_or(())?,
    )
    .map_err(|_| ())?;
    let policy = capacity.policy().map_err(|_| ())?;
    let options = &config.phase2;
    let verification = crate::auth::VerificationSettings::new(
        options
            .jwt_secret
            .clone()
            .filter(|secret| secret.len() >= 32)
            .ok_or(())?,
        options.jwt_secret_previous.clone(),
        options
            .jwt_issuer
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or(())?,
        options
            .jwt_audience
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or(())?,
    );
    let settings = SessionSettings::from_config(config).map_err(|_| ())?;
    let hub = StreamHub::new(policy);
    hub.set_available(false, Terminal::Unavailable);
    let mut store = PgDispatchStore::new(state.pools.read.clone());
    let dispatch = Dispatch::initialize(&mut store, hub.clone())
        .await
        .map_err(|_| ())?;
    // Verify the dedicated connection before making the hub available. The actor
    // owns this same session from its first iteration, without an extra slot.
    let session = super::election::ElectionSession::open(&settings)
        .await
        .map_err(|_| ())?;
    let (stop, shutdown) = watch::channel(false);
    hub.set_available(true, Terminal::Unavailable);
    state.streams = Some(Arc::new(Streams {
        hub: hub.clone(),
        verification,
    }));
    let task_hub = hub.clone();
    let read = state.pools.read.clone();
    let task = tokio::spawn(async move {
        supervise(settings, read, dispatch, session, task_hub, shutdown).await;
    });
    Ok(StreamRuntime {
        stop,
        task: Some(task),
        hub,
    })
}

async fn stopping(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

struct AvailabilityGuard(StreamHub);

impl Drop for AvailabilityGuard {
    fn drop(&mut self) {
        self.0.set_available(false, Terminal::Unavailable);
    }
}

async fn supervise(
    settings: SessionSettings,
    read: sqlx::PgPool,
    mut dispatch: Dispatch<StreamHub>,
    session: super::election::ElectionSession,
    hub: StreamHub,
    mut shutdown: watch::Receiver<bool>,
) {
    let _availability = AvailabilityGuard(hub.clone());
    let mut initial = Some(session);
    let mut budget = RestartBudget::default();
    loop {
        let (cycle_stop, cycle_shutdown) = watch::channel(false);
        let wake = Arc::new(Notify::new());
        let (sender, mut receiver) = mpsc::channel(1);
        let mut election = tokio::spawn(super::election::run_election_with_session(
            settings.clone(),
            initial.take(),
            wake.clone(),
            cycle_shutdown.clone(),
        ));
        let mut store = PgDispatchStore::new(read.clone());
        let drain_hub = hub.clone();
        let mut drain = tokio::spawn(async move {
            let _availability = AvailabilityGuard(drain_hub);
            dispatch
                .run(&mut store, &mut receiver, &mut cycle_shutdown.clone())
                .await
        });
        let bridge_stop = cycle_stop.subscribe();
        let bridge = tokio::spawn(async move {
            let mut stop = bridge_stop;
            loop {
                tokio::select! {
                    biased;
                    () = stopping(&mut stop) => break,
                    () = wake.notified() => { let _ = sender.try_send(()); }
                }
            }
        });
        let stopped = tokio::select! {
            biased;
            () = stopping(&mut shutdown) => true,
            _ = &mut election => false,
            _ = &mut drain => false,
        };
        hub.set_available(
            false,
            if stopped {
                Terminal::Shutdown
            } else {
                Terminal::Unavailable
            },
        );
        cycle_stop.send_replace(true);
        // A finished JoinHandle must not be polled twice.
        if !election.is_finished() {
            let _ = election.await;
        }
        if !drain.is_finished() {
            let _ = drain.await;
        }
        let _ = bridge.await;
        if stopped {
            return;
        }
        loop {
            let Some(delay) = budget.next_delay() else {
                return;
            };
            tokio::select! {
                biased;
                () = stopping(&mut shutdown) => return,
                () = tokio::time::sleep(delay) => {},
            }
            let mut store = PgDispatchStore::new(read.clone());
            let attempt = async {
                let next = Dispatch::initialize(&mut store, hub.clone())
                    .await
                    .map_err(|_| ())?;
                let session = super::election::ElectionSession::open(&settings)
                    .await
                    .map_err(|_| ())?;
                Ok::<_, ()>((next, session))
            };
            let restarted = tokio::select! {
                biased;
                () = stopping(&mut shutdown) => return,
                result = attempt => result,
            };
            match restarted {
                Ok((next, session)) => {
                    dispatch = next;
                    initial = Some(session);
                    break;
                }
                Err(_) => continue,
            }
        }
        hub.set_available(true, Terminal::Unavailable);
    }
}
