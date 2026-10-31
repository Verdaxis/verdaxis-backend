//! Validated limits for stream subscriptions and persisted envelopes.

pub const MAX_CHANNEL_SUBSCRIBERS: usize = 200;
pub const MAX_PROCESS_SUBSCRIBERS: usize = 2_000;
pub const MAX_QUEUED_EVENTS: usize = 100;
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_PARTICIPANTS: usize = 256;
pub const MAX_PAGE_ROWS: usize = 500;
pub const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_BATCHES_PER_TURN: usize = 4;
pub const MAX_WAKES_PER_TURN: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct StreamPolicy {
    pub(crate) per_channel: usize,
    pub(crate) process: usize,
    pub(crate) queue: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPolicy;

impl StreamPolicy {
    pub fn new(per_channel: usize, process: usize, queue: usize) -> Result<Self, InvalidPolicy> {
        if !(1..=MAX_CHANNEL_SUBSCRIBERS).contains(&per_channel)
            || !(1..=MAX_PROCESS_SUBSCRIBERS).contains(&process)
            || !(1..=MAX_QUEUED_EVENTS).contains(&queue)
        {
            return Err(InvalidPolicy);
        }
        Ok(Self {
            per_channel,
            process,
            queue,
        })
    }
}

impl Default for StreamPolicy {
    fn default() -> Self {
        Self {
            per_channel: MAX_CHANNEL_SUBSCRIBERS,
            process: MAX_PROCESS_SUBSCRIBERS,
            queue: MAX_QUEUED_EVENTS,
        }
    }
}
