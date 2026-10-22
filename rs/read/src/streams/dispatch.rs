//! Ordered read-side drain. The runtime owns one instance for the hub lifetime.
//!
//! The store seam returns bounded, same-snapshot pages. It must inspect database
//! size metadata before fetching JSON. This module neither assigns sequences nor
//! writes producer state. Public obligations live beside the cursor, not inside
//! a scan future, so a cancelled or failed scan cannot discard them.

use std::{future::Future, sync::Arc};

use tokio::sync::{mpsc, watch};

use super::{
    bus::{HubError, PublicChannel, StoredEvent, StreamHub, Terminal},
    election::POLL_INTERVAL,
    policy::{MAX_BATCHES_PER_TURN, MAX_EVENT_BYTES, MAX_PAGE_BYTES, MAX_PAGE_ROWS, MAX_WAKES_PER_TURN},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    Store,
    InvalidPage,
    Publish,
}

/// Explicit coverage barrier and assigned high-water from one read snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Boundary {
    floor: i64,
    high_water: i64,
}

impl Boundary {
    pub fn new(floor: i64, high_water: i64) -> Result<Self, DispatchError> {
        if floor < 0 || high_water < floor {
            return Err(DispatchError::InvalidPage);
        }
        Ok(Self { floor, high_water })
    }

    pub fn high_water(self) -> i64 {
        self.high_water
    }
}

/// An unavailable row carries only its sequence, never an unbounded payload.
#[derive(Debug)]
pub struct ScanRow {
    seq: i64,
    bytes: usize,
    event: Option<Arc<StoredEvent>>,
}

impl ScanRow {
    pub fn event(event: Arc<StoredEvent>, bytes: usize) -> Result<Self, DispatchError> {
        if bytes == 0 || bytes > MAX_EVENT_BYTES {
            return Err(DispatchError::InvalidPage);
        }
        Ok(Self { seq: event.seq(), bytes, event: Some(event) })
    }

    pub fn invalid(seq: i64) -> Result<Self, DispatchError> {
        if seq <= 0 {
            return Err(DispatchError::InvalidPage);
        }
        Ok(Self { seq, bytes: 0, event: None })
    }
}

#[derive(Debug)]
pub struct ScanPage {
    boundary: Boundary,
    rows: Vec<ScanRow>,
    more: bool,
}

impl ScanPage {
    pub fn new(boundary: Boundary, rows: Vec<ScanRow>, more: bool) -> Result<Self, DispatchError> {
        if rows.len() > MAX_PAGE_ROWS || (rows.is_empty() && more) {
            return Err(DispatchError::InvalidPage);
        }
        let mut previous = 0;
        let mut bytes = 0usize;
        for row in &rows {
            bytes = bytes.checked_add(row.bytes).ok_or(DispatchError::InvalidPage)?;
            if row.seq <= previous || row.seq > boundary.high_water || bytes > MAX_PAGE_BYTES {
                return Err(DispatchError::InvalidPage);
            }
            previous = row.seq;
        }
        Ok(Self { boundary, rows, more })
    }
}

/// Short read-only snapshots, with no connection retained between calls.
///
/// `scan` must return all sequenced rows after `after` in order, stopping at
/// either page bound. `more` requests another bounded page without a poll wait.
/// Missing/corrupt retention state is an error, never an inferred floor.
pub trait DispatchStore: Send {
    fn boundary(&mut self) -> impl Future<Output = Result<Boundary, DispatchError>> + Send;
    fn scan(&mut self, after: i64) -> impl Future<Output = Result<ScanPage, DispatchError>> + Send;
}

/// Synchronous bounded enqueue only; never wait for client consumption.
pub trait DispatchSink: Send {
    fn private(&mut self, event: Arc<StoredEvent>) -> Result<(), HubError>;
    fn public(&mut self, channel: PublicChannel) -> Result<(), HubError>;
    fn terminate(&mut self, terminal: Terminal);
}

impl DispatchSink for StreamHub {
    fn private(&mut self, event: Arc<StoredEvent>) -> Result<(), HubError> {
        self.publish_private(event)
    }
    fn public(&mut self, channel: PublicChannel) -> Result<(), HubError> {
        self.invalidate_public(channel)
    }
    fn terminate(&mut self, terminal: Terminal) {
        self.terminate_all(terminal);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub more: bool,
}

pub struct Dispatch<S> {
    sink: S,
    cursor: i64,
    // Index order is Prices, Orderbook. Clear only after that channel accepts.
    pending: [bool; 2],
}

impl<S: DispatchSink> Dispatch<S> {
    /// Complete before publishing the runtime handle or admitting subscribers.
    pub async fn initialize(store: &mut impl DispatchStore, sink: S) -> Result<Self, DispatchError> {
        let boundary = store.boundary().await?;
        Ok(Self { sink, cursor: boundary.high_water, pending: [false; 2] })
    }

    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    pub fn pending_channels(&self) -> [bool; 2] {
        self.pending
    }

    fn flush_public(&mut self) -> Result<(), DispatchError> {
        for (index, channel) in [PublicChannel::Prices, PublicChannel::Orderbook].into_iter().enumerate() {
            if self.pending[index] {
                self.sink.public(channel).map_err(|_| DispatchError::Publish)?;
                self.pending[index] = false;
            }
        }
        Ok(())
    }

    /// At most four pages, then public controls and a scheduling yield.
    pub async fn turn(&mut self, store: &mut impl DispatchStore) -> Result<Turn, DispatchError> {
        // A previous query failure must not make recovery depend on a new query.
        self.flush_public()?;
        let mut more = false;
        for _ in 0..MAX_BATCHES_PER_TURN {
            let page = store.scan(self.cursor).await?;
            if page.boundary.high_water < self.cursor
                || page.rows.first().is_some_and(|row| row.seq <= self.cursor)
            {
                return Err(DispatchError::InvalidPage);
            }
            if self.cursor < page.boundary.floor {
                // Explicit coverage recovery. No page business row is delivered.
                // Terminal delivery also covers public subscribers on this hub.
                self.sink.terminate(Terminal::Retention);
                self.pending = [true; 2];
                self.cursor = page.boundary.high_water;
                more = false;
                break;
            }
            more = page.more;
            for row in page.rows {
                match row.event {
                    Some(event) => {
                        if event.public_invalidated() {
                            self.pending = [true; 2];
                        }
                        self.sink.private(event).map_err(|_| DispatchError::Publish)?;
                    }
                    None => {
                        // Participant scope is unknown. Detach all rather than
                        // let any subscriber silently continue over this row.
                        self.sink.terminate(Terminal::InvalidEvent);
                        self.pending = [true; 2];
                    }
                }
                self.cursor = row.seq;
            }
            if !more {
                break;
            }
        }
        self.flush_public()?;
        tokio::task::yield_now().await;
        Ok(Turn { more })
    }

    /// Notifications are lossy hints; the two-second poll is authoritative.
    /// The supervisor owns restart policy after a store/publish error.
    pub async fn run(
        &mut self,
        store: &mut impl DispatchStore,
        wakes: &mut mpsc::Receiver<()>,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Result<(), DispatchError> {
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut more = true;
        loop {
            tokio::select! {
                biased;
                _ = stopping(shutdown) => {
                    self.sink.terminate(Terminal::Shutdown);
                    return Ok(());
                }
                result = async {
                    if !more {
                        tokio::select! {
                            _ = poll.tick() => {},
                            Some(()) = wakes.recv() => {},
                        }
                    }
                    for _ in 0..MAX_WAKES_PER_TURN {
                        if wakes.try_recv().is_err() { break; }
                    }
                    self.turn(store).await
                } => {
                    match result {
                        Ok(turn) => more = turn.more,
                        Err(error) => {
                            // Do not reconstruct this drain on a transient
                            // retry. If the supervisor does replace it, all old
                            // subscriptions have already received a terminal.
                            self.sink.terminate(Terminal::Unavailable);
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
}

async fn stopping(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow_and_update() || shutdown.changed().await.is_err() {
            return;
        }
    }
}
