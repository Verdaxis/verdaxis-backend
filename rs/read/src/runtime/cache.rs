//! Public payload cache. Response headers and request admission are never stored.
use crate::market::cache_dependencies::Boundary;
use crate::runtime::cache_authority::{CacheAuthority, FenceToken, FillTicket};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
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

// Trading entries are bounded independently of route diversity. The byte
// budget is larger than the reference cache because a curve or orderbook
// payload can be several hundred kilobytes; the entry count bounds keys.
pub const TRADING_ENTRIES: usize = 256;
pub const TRADING_BYTES: usize = 16 * 1024 * 1024;

/// One computed trading payload and every fact needed to decide whether it is
/// still the right answer. An entry never exists without the authority that
/// produced it and the generation its snapshot observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradingEntry {
    pub payload: Vec<u8>,
    pub database_epoch: String,
    pub primary_identity: String,
    pub authority_revision: u64,
    pub fence_counter: u64,
    pub generation: i64,
    pub valid_until: Boundary,
    pub policy_identity: String,
}

impl TradingEntry {
    /// Bind a payload to the fill ticket it was computed under. Copying the
    /// identity fields here, rather than at insertion, means a fill cannot be
    /// stored under a different authority than the one it started with.
    pub fn new(
        ticket: &FillTicket,
        payload: Vec<u8>,
        valid_until: Boundary,
        policy_identity: String,
    ) -> Self {
        let token = ticket.token();
        let authority = token.authority();
        Self {
            payload,
            database_epoch: authority.database_epoch.clone(),
            primary_identity: authority.primary_identity.clone(),
            authority_revision: authority.revision,
            fence_counter: token.fence(),
            generation: ticket.generation(),
            valid_until,
            policy_identity,
        }
    }

    /// Identity, fence, generation and policy must all equal the request's own
    /// snapshot. The time boundary is checked separately so a caller can see
    /// whether the payload merely expired.
    pub fn matches(
        &self,
        token: &FenceToken,
        latest_generation: i64,
        policy_identity: &str,
    ) -> bool {
        let authority = token.authority();
        self.database_epoch == authority.database_epoch
            && self.primary_identity == authority.primary_identity
            && self.authority_revision == authority.revision
            && self.fence_counter == token.fence()
            && self.generation == latest_generation
            && self.policy_identity == policy_identity
    }

    /// Order of production: a later authority revision, then a later fence,
    /// then a later generation is newer. Tuples compare in that order.
    fn recency(&self) -> (u64, u64, i64) {
        (self.authority_revision, self.fence_counter, self.generation)
    }
}

struct TradingStored<K> {
    key: K,
    entry: TradingEntry,
}

/// Bounded trading cache. Every lookup still performs the authority and
/// generation check; this is not a database-free hit.
pub struct TradingCache<K> {
    entries: Mutex<VecDeque<TradingStored<K>>>,
    max_entries: usize,
    max_bytes: usize,
}

impl<K: Eq> TradingCache<K> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            max_entries,
            max_bytes,
        }
    }

    /// Store a fill only while its boundary is live. `Boundary::Unknown` and an
    /// already-passed boundary are refused, so a guessed TTL cannot enter.
    pub fn insert(&self, key: K, entry: TradingEntry, now: Instant) -> bool {
        if self.max_entries == 0
            || entry.payload.len() > self.max_bytes
            || !entry.valid_until.is_live(now)
        {
            return false;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The helper's high-water check and this mutex are separate. A newer
        // fill can arrive between them; never let an older fill replace it.
        if entries
            .iter()
            .any(|stored| stored.key == key && stored.entry.recency() > entry.recency())
        {
            return false;
        }
        entries.retain(|stored| stored.key != key && stored.entry.valid_until.is_live(now));
        while entries.len() >= self.max_entries
            || entries
                .iter()
                .map(|stored| stored.entry.payload.len())
                .sum::<usize>()
                + entry.payload.len()
                > self.max_bytes
        {
            entries.pop_front();
        }
        entries.push_back(TradingStored { key, entry });
        true
    }

    /// Commit-safe hit check: the token must still be accepted and the entry
    /// must name the same authority, fence, generation, policy and a live
    /// boundary. `None` means the caller must load the payload again.
    pub fn hit(
        &self,
        authority: &CacheAuthority,
        key: &K,
        ticket: &FillTicket,
        latest_generation: i64,
        policy_identity: &str,
        now: Instant,
    ) -> Option<TradingEntry> {
        if !authority.may_publish(ticket, latest_generation, now) {
            return None;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.retain(|stored| stored.entry.valid_until.is_live(now));
        entries
            .iter()
            .find(|stored| {
                &stored.key == key
                    && stored
                        .entry
                        .matches(ticket.token(), latest_generation, policy_identity)
            })
            .map(|stored| stored.entry.clone())
    }

    pub fn remove(&self, key: &K) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|stored| &stored.key != key);
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Eq> Default for TradingCache<K> {
    fn default() -> Self {
        Self::new(TRADING_ENTRIES, TRADING_BYTES)
    }
}

/// Type-erased handle the runtime uses to clear every route family's cache.
pub trait TradingCacheControl: Send + Sync {
    fn clear(&self);
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Eq + Send + 'static> TradingCacheControl for TradingCache<K> {
    fn clear(&self) {
        <TradingCache<K>>::clear(self);
    }

    fn len(&self) -> usize {
        <TradingCache<K>>::len(self)
    }
}

/// Registry of the route caches this process owns. A weak reference means the
/// runtime never keeps a route's cache alive after the route drops it.
#[derive(Default)]
pub struct TradingCaches {
    caches: Mutex<Vec<Weak<dyn TradingCacheControl>>>,
}

impl TradingCaches {
    /// Register one family cache. Repeated registration of the same cache is a
    /// no-op, so a route may call this on every request.
    pub fn register<K: Eq + Send + 'static>(&self, cache: &Arc<TradingCache<K>>) {
        let erased: Arc<dyn TradingCacheControl> = cache.clone();
        let pointer = Arc::as_ptr(&erased) as *const ();
        let mut caches = self
            .caches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        caches.retain(|cache| cache.strong_count() > 0);
        if !caches
            .iter()
            .any(|cache| cache.as_ptr() as *const () == pointer)
        {
            caches.push(Arc::downgrade(&erased));
        }
    }

    pub fn clear(&self) {
        let mut caches = self
            .caches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        caches.retain(|cache| cache.strong_count() > 0);
        for cache in caches.iter().filter_map(Weak::upgrade) {
            cache.clear();
        }
    }

    pub fn entries(&self) -> usize {
        let mut caches = self
            .caches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        caches.retain(|cache| cache.strong_count() > 0);
        caches
            .iter()
            .filter_map(Weak::upgrade)
            .map(|cache| cache.len())
            .sum()
    }
}

#[cfg(test)]
mod trading_tests {
    use super::*;
    use crate::runtime::cache_authority::{Authority, FenceToken};
    use std::time::{Duration, Instant};

    fn identity(revision: u64) -> Authority {
        Authority {
            database_epoch: "a".repeat(64),
            primary_identity: "b".repeat(64),
            revision,
        }
    }

    /// A process with an active record, one attached listener and one token.
    /// This is the same state the runtime reaches after `tick` on a migrated
    /// database, without a database.
    struct Process {
        authority: CacheAuthority,
        identity: Authority,
        token: FenceToken,
        now: Instant,
    }

    fn process() -> Process {
        let now = Instant::now();
        let identity = identity(1);
        let authority = CacheAuthority::default();
        authority.synchronize(Some(identity.clone()), now).unwrap();
        authority.attach_listener(&identity).unwrap();
        let token = authority.token(&identity, now).unwrap();
        Process {
            authority,
            identity,
            token,
            now,
        }
    }

    fn entry_for(token: &FenceToken, generation: i64, valid_until: Boundary) -> TradingEntry {
        TradingEntry::new(
            &FillTicket::new(token.clone(), generation),
            b"payload".to_vec(),
            valid_until,
            "policy".into(),
        )
    }

    #[test]
    fn entries_stop_matching_at_their_boundary() {
        let process = process();
        let now = process.now;
        let entry = entry_for(
            &process.token,
            10,
            Boundary::At(now + Duration::from_secs(10)),
        );
        assert!(entry.matches(&process.token, 10, "policy"));
        assert!(entry.valid_until.is_live(now));
        assert!(!entry.valid_until.is_live(now + Duration::from_secs(10)));
        let cache = TradingCache::new(4, 1024);
        assert!(!cache.insert(
            "past",
            entry_for(&process.token, 10, Boundary::At(now)),
            now
        ));
        assert!(!cache.insert(
            "unknown",
            entry_for(&process.token, 10, Boundary::Unknown),
            now
        ));
        assert!(cache.insert("live", entry, now));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_changed_identity_fence_generation_or_policy_is_a_miss() {
        let process = process();
        let entry = entry_for(
            &process.token,
            10,
            Boundary::At(process.now + Duration::from_secs(10)),
        );
        assert!(entry.matches(&process.token, 10, "policy"));
        // Another revision is another authority even with equal identities.
        let mut revised = process.identity.clone();
        revised.revision = 2;
        process
            .authority
            .synchronize(Some(revised.clone()), process.now)
            .unwrap();
        process.authority.attach_listener(&revised).unwrap();
        let revised_token = process.authority.token(&revised, process.now).unwrap();
        assert!(!entry.matches(&revised_token, 10, "policy"));
        // A new listener bumps the process-local fence without changing identity.
        let original = process.identity.clone();
        process
            .authority
            .synchronize(Some(original.clone()), process.now)
            .unwrap();
        process.authority.attach_listener(&original).unwrap();
        let next_listener = process.authority.token(&original, process.now).unwrap();
        assert_ne!(revised_token.fence(), next_listener.fence());
        assert!(!entry.matches(&next_listener, 10, "policy"));
        assert!(!entry.matches(&process.token, 11, "policy"));
        assert!(!entry.matches(&process.token, 10, "another policy"));
    }

    #[test]
    fn the_same_key_replaces_and_the_registry_clears_every_family() {
        let process = process();
        let now = process.now;
        let first = Arc::new(TradingCache::new(4, 1024));
        let second = Arc::new(TradingCache::new(4, 1024));
        assert!(first.insert("key", entry_for(&process.token, 10, Boundary::Never), now));
        assert!(first.insert("key", entry_for(&process.token, 10, Boundary::Never), now));
        assert_eq!(first.len(), 1);
        assert!(second.insert("key", entry_for(&process.token, 10, Boundary::Never), now));
        let registry = TradingCaches::default();
        registry.register(&first);
        registry.register(&first);
        registry.register(&second);
        assert_eq!(registry.entries(), 2);
        registry.clear();
        assert_eq!(registry.entries(), 0);
        assert!(first.is_empty());
        assert!(second.is_empty());
    }

    #[test]
    fn an_older_fill_never_replaces_a_newer_entry() {
        let process = process();
        let now = process.now;
        let cache = TradingCache::new(4, 1024);
        assert!(cache.insert("key", entry_for(&process.token, 11, Boundary::Never), now));
        // An overlapping snapshot at the older generation finishes later.
        assert!(!cache.insert("key", entry_for(&process.token, 10, Boundary::Never), now));
        let current = FillTicket::new(process.token.clone(), 11);
        assert!(
            cache
                .hit(&process.authority, &"key", &current, 11, "policy", now)
                .is_some()
        );
        // The same or a later generation still replaces the entry.
        assert!(cache.insert("key", entry_for(&process.token, 11, Boundary::Never), now));
        assert!(cache.insert("key", entry_for(&process.token, 12, Boundary::Never), now));
        // A later fence outranks a higher generation from an earlier fence.
        process
            .authority
            .attach_listener(&process.identity)
            .unwrap();
        let later = process.authority.token(&process.identity, now).unwrap();
        assert!(cache.insert("key", entry_for(&later, 5, Boundary::Never), now));
        assert!(!cache.insert("key", entry_for(&process.token, 13, Boundary::Never), now));
        assert_eq!(cache.len(), 1);
    }
}
