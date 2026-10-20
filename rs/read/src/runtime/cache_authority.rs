//! Fail-closed cache authority. Database metadata alone is not activation proof.
//!
//! A trading-cache hit or fill is allowed only while all four facts hold at once:
//!
//! 1. The external owner record names an active authority. The deploy/recovery
//!    owner writes that file outside every database, so a restored copy of a
//!    database cannot carry it.
//! 2. This process re-read that record within [`AUTHORITY_LEASE`]. A process that
//!    stops refreshing (stopped, starved or cut off) loses its hits by itself.
//! 3. The database connection serving the request reports the same identity
//!    and is a primary.
//! 4. A change listener bound to that same identity is attached.
//!
//! Every change of authority, listener or lease bumps a fence counter. Entries
//! and fills carry the fence they were made under, so an old one is never
//! accepted again, even when the identity and generation numbers repeat.
use serde::Deserialize;
use sqlx::{PgConnection, Row};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Longest a process may rely on an owner record it has not re-read. The owner
/// waits twice this long after publishing a revocation before it binds a new
/// authority (`FENCE_DRAIN_SECONDS` in `scripts/rust_cache_authority.py`), so
/// every process has expired or re-read before the successor can serve.
pub const AUTHORITY_LEASE: Duration = Duration::from_secs(5);

/// How often the runtime re-reads the record. Several attempts fit in one lease,
/// so one slow read does not drop hits.
pub const AUTHORITY_REFRESH: Duration = Duration::from_secs(1);

/// The record is a few hundred bytes; a larger file is not the owner's record.
const MAX_RECORD_BYTES: u64 = 4096;

const RECORD_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    pub database_epoch: String,
    pub primary_identity: String,
    pub revision: u64,
}

impl Authority {
    fn is_proved(&self) -> bool {
        is_identity(&self.database_epoch)
            && is_identity(&self.primary_identity)
            && self.revision != 0
    }
}

/// Owner-allocated identities are lowercase hex of 16 to 64 random bytes.
fn is_identity(value: &str) -> bool {
    (32..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Proof that a request started under one authority, listener and fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FenceToken {
    authority: Authority,
    fence: u64,
}

/// A computed payload waiting to be stored, with the generation it observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillTicket {
    token: FenceToken,
    generation: i64,
}

impl FillTicket {
    pub fn new(token: FenceToken, generation: i64) -> Self {
        Self { token, generation }
    }
}

/// Opaque handle of one attached change listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerId(u64);

#[derive(Default)]
struct State {
    authority: Option<Authority>,
    confirmed: Option<Instant>,
    fence: u64,
    listener: Option<(ListenerId, Authority)>,
    last_listener: u64,
    highest_generation: Option<i64>,
}

impl State {
    /// A saturated counter would stop distinguishing old tokens from new ones,
    /// so it refuses every token instead.
    fn fence_exhausted(&self) -> bool {
        self.fence == u64::MAX
    }

    fn bump(&mut self) {
        self.fence = self.fence.saturating_add(1);
    }

    fn clear(&mut self) {
        if self.authority.is_some() || self.listener.is_some() {
            self.bump();
        }
        self.authority = None;
        self.confirmed = None;
        self.listener = None;
        self.highest_generation = None;
    }

    fn admits(&self, database: &Authority, now: Instant) -> bool {
        let lease_valid = self
            .confirmed
            .is_some_and(|at| now.saturating_duration_since(at) < AUTHORITY_LEASE);
        let listener_bound = self
            .listener
            .as_ref()
            .is_some_and(|(_, bound)| bound == database);
        !self.fence_exhausted()
            && lease_valid
            && listener_bound
            && self.authority.as_ref() == Some(database)
    }
}

#[derive(Default)]
pub struct CacheAuthority(Mutex<State>);

impl CacheAuthority {
    /// A poisoned lock means a panic while changing the state; no token is
    /// issued or accepted afterwards.
    fn lock(&self) -> Option<MutexGuard<'_, State>> {
        self.0.lock().ok()
    }

    /// Apply the owner record. `None` means revoked, absent or unreadable.
    /// The same record again only renews the lease; a different one starts a
    /// new fence and drops the listener bound to the old identity.
    pub fn synchronize(
        &self,
        external: Option<Authority>,
        now: Instant,
    ) -> Result<(), &'static str> {
        let mut state = self.lock().ok_or("cache authority poisoned")?;
        match external {
            None => {
                state.clear();
                Ok(())
            }
            Some(authority) if !authority.is_proved() => {
                state.clear();
                Err("unproved cache authority")
            }
            Some(authority) if state.authority.as_ref() == Some(&authority) => {
                state.confirmed = Some(now);
                Ok(())
            }
            Some(authority) => {
                state.clear();
                state.bump();
                state.authority = Some(authority);
                state.confirmed = Some(now);
                Ok(())
            }
        }
    }

    /// Re-read the owner record from disk. Any record that cannot be proved
    /// (missing, symlink, oversized, malformed or revoked) disables hits.
    pub fn refresh(&self, path: &Path, now: Instant) {
        let external = read_record(path).ok().flatten();
        // An unproved record has already cleared the state; nothing else to do.
        let _ = self.synchronize(external, now);
    }

    pub fn revoke(&self) {
        if let Some(mut state) = self.lock() {
            state.clear();
        }
    }

    /// Counter that changes on every authority, listener or revocation change.
    /// Reference caches compare it to drop entries when the database changes.
    pub fn current_fence(&self) -> u64 {
        self.lock().map_or(u64::MAX, |state| state.fence)
    }

    /// Bind a freshly connected listener to the identity its own connection
    /// reported. Refused unless that identity is the externally active one.
    pub fn attach_listener(&self, bound: &Authority) -> Option<ListenerId> {
        let mut state = self.lock()?;
        if state.authority.as_ref() != Some(bound) {
            return None;
        }
        state.last_listener = state.last_listener.checked_add(1)?;
        let id = ListenerId(state.last_listener);
        state.bump();
        state.listener = Some((id, bound.clone()));
        state.highest_generation = None;
        Some(id)
    }

    /// A listener that is gone, lost its connection or saw an unknown event
    /// disables hits. Detaching a replaced listener changes nothing.
    pub fn detach_listener(&self, id: ListenerId) {
        if let Some(mut state) = self.lock()
            && state
                .listener
                .as_ref()
                .is_some_and(|(current, _)| *current == id)
        {
            state.listener = None;
            state.highest_generation = None;
            state.bump();
        }
    }

    pub fn listener_is_current(&self, id: ListenerId) -> bool {
        self.lock().is_some_and(|state| {
            state
                .listener
                .as_ref()
                .is_some_and(|(current, _)| *current == id)
        })
    }

    /// Issue a token for a request whose own database connection reported
    /// `database`. `None` means serve no cached data.
    pub fn token(&self, database: &Authority, now: Instant) -> Option<FenceToken> {
        let state = self.lock()?;
        state.admits(database, now).then(|| FenceToken {
            authority: database.clone(),
            fence: state.fence,
        })
    }

    pub fn accepts(&self, token: &FenceToken, now: Instant) -> bool {
        self.lock()
            .is_some_and(|state| state.fence == token.fence && state.admits(&token.authority, now))
    }

    /// Generations only move forward inside one authority. A smaller one means
    /// the database went backwards (an unrotated restore): revoke everything.
    pub fn observe_generation(&self, token: &FenceToken, generation: i64, now: Instant) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        if state.fence != token.fence || !state.admits(&token.authority, now) {
            return false;
        }
        match state.highest_generation {
            Some(highest) if generation < highest => {
                state.clear();
                false
            }
            _ if generation < 0 => {
                state.clear();
                false
            }
            highest => {
                state.highest_generation = Some(highest.map_or(generation, |h| h.max(generation)));
                true
            }
        }
    }

    /// A fill may be stored or answered only under the authority, listener and
    /// fence it started with, and only while the generation it computed from is
    /// still the latest one the caller has verified.
    pub fn may_publish(&self, ticket: &FillTicket, latest_generation: i64, now: Instant) -> bool {
        ticket.generation == latest_generation && self.accepts(&ticket.token, now)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    version: u32,
    state: String,
    authority_revision: u64,
    database_epoch: Option<String>,
    primary_identity: Option<String>,
}

/// Parse the owner record. `Ok(None)` is a deliberate revocation.
pub fn parse_record(text: &str) -> Result<Option<Authority>, &'static str> {
    let raw: RawRecord =
        serde_json::from_str(text).map_err(|_| "invalid cache authority record")?;
    if raw.version != RECORD_VERSION {
        return Err("unsupported cache authority record version");
    }
    match (raw.state.as_str(), raw.database_epoch, raw.primary_identity) {
        ("revoked", None, None) => Ok(None),
        ("active", Some(database_epoch), Some(primary_identity)) => {
            let authority = Authority {
                database_epoch,
                primary_identity,
                revision: raw.authority_revision,
            };
            authority
                .is_proved()
                .then_some(Some(authority))
                .ok_or("unproved cache authority record")
        }
        _ => Err("invalid cache authority record state"),
    }
}

/// Read the record as a small regular file, never through a symlink.
pub fn read_record(path: &Path) -> Result<Option<Authority>, &'static str> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "cache authority record is unavailable")?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err("cache authority record must be a small regular file");
    }
    let text = std::fs::read_to_string(path).map_err(|_| "cache authority record is unreadable")?;
    parse_record(&text)
}

/// What one database connection reports about its own incarnation.
#[derive(Debug, PartialEq, Eq)]
pub struct DatabaseState {
    pub authority: Authority,
    pub generation: i64,
}

/// Read identity, primary role and the trading generation in one statement, so
/// both come from the same snapshot. `None` means the database cannot back a
/// cache: no metadata tables yet, not activated, a replica, or revision zero.
pub async fn database_state(
    connection: &mut PgConnection,
) -> Result<Option<DatabaseState>, sqlx::Error> {
    let present: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.public_read_cache_incarnation') IS NOT NULL \
         AND to_regclass('public.public_read_cache_generations') IS NOT NULL",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !present {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT i.database_epoch, i.primary_identity, i.authority_revision, i.active, \
         NOT pg_catalog.pg_is_in_recovery() AS is_primary, g.generation \
         FROM public.public_read_cache_incarnation AS i \
         CROSS JOIN public.public_read_cache_generations AS g \
         WHERE i.singleton AND g.domain = 'trading'",
    )
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let revision: i64 = row.try_get("authority_revision")?;
    let active: bool = row.try_get("active")?;
    let is_primary: bool = row.try_get("is_primary")?;
    let Ok(revision) = u64::try_from(revision) else {
        return Ok(None);
    };
    if !active || !is_primary || revision == 0 {
        return Ok(None);
    }
    Ok(Some(DatabaseState {
        authority: Authority {
            database_epoch: row.try_get("database_epoch")?,
            primary_identity: row.try_get("primary_identity")?,
            revision,
        },
        generation: row.try_get("generation")?,
    }))
}

/// Admit one request: its own connection must report the externally active
/// identity. Returns the token and the generation read in that snapshot.
pub async fn admit_connection(
    authority: &CacheAuthority,
    connection: &mut PgConnection,
    now: Instant,
) -> Result<Option<(FenceToken, i64)>, sqlx::Error> {
    let Some(state) = database_state(connection).await? else {
        return Ok(None);
    };
    Ok(authority
        .token(&state.authority, now)
        .map(|token| (token, state.generation)))
}
