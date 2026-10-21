//! Public payload cache. Response headers and request admission are never stored.
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// Decision 69 permits reference data to be at most 60 seconds old. The entry
// and byte limits bound retained payloads independently of query diversity.
pub const REFERENCE_TTL: Duration = Duration::from_secs(60);
pub const REFERENCE_ENTRIES: usize = 128;
pub const REFERENCE_BYTES: usize = 8 * 1024 * 1024;

struct Entry<K> {
    key: K,
    payload: Vec<u8>,
    inserted: Instant,
}

pub struct ReferenceCache<K> {
    entries: Mutex<VecDeque<Entry<K>>>,
    max_entries: usize,
    max_bytes: usize,
}

impl<K: Eq> ReferenceCache<K> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            max_entries,
            max_bytes,
        }
    }

    pub fn get(&self, key: &K, now: Instant) -> Option<Vec<u8>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|entry| now.saturating_duration_since(entry.inserted) < REFERENCE_TTL);
        entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.payload.clone())
    }

    pub fn insert(&self, key: K, payload: Vec<u8>, now: Instant) {
        if self.max_entries == 0 || payload.len() > self.max_bytes {
            return;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|entry| {
            entry.key != key && now.saturating_duration_since(entry.inserted) < REFERENCE_TTL
        });
        while entries.len() >= self.max_entries
            || entries
                .iter()
                .map(|entry| entry.payload.len())
                .sum::<usize>()
                + payload.len()
                > self.max_bytes
        {
            entries.pop_front();
        }
        entries.push_back(Entry {
            key,
            payload,
            inserted: now,
        });
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

impl<K: Eq> Default for ReferenceCache<K> {
    fn default() -> Self {
        Self::new(REFERENCE_ENTRIES, REFERENCE_BYTES)
    }
}

pub fn cached_payload(state: &crate::AppState, key: &ReferenceKey) -> Option<crate::Json> {
    let bytes = state.reference_cache.get(key, Instant::now())?;
    serde_json::from_slice(&bytes).ok()
}

pub fn store_payload(state: &crate::AppState, key: ReferenceKey, payload: &crate::Json) {
    if let Ok(bytes) = serde_json::to_vec(payload) {
        state.reference_cache.insert(key, bytes, Instant::now());
    }
}

/// Typed route identity, never an authentication or organization identifier.
/// The cache belongs to one AppState, binding environment, policy and schema
/// identity to the process; a new release/policy creates an empty cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceKey {
    Products,
    DeliveryPoints,
    Ports {
        region: Option<String>,
        search: Option<String>,
    },
    Port(String),
    News {
        limit: i64,
        category: Option<String>,
        min_relevance: i32,
    },
}
