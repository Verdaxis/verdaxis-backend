//! Bounded invalidation consumption. NOTIFY is a wake, never freshness proof.
use crate::runtime::cache::{ReferenceCache, ReferenceKey, TradingCaches};
use crate::runtime::cache_authority::{
    AUTHORITY_REFRESH, CacheAuthority, ListenerId, database_state,
};
use sqlx::{
    Acquire, Row,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

pub const CACHE_CHANNEL: &str = "verdaxis_public_read_cache";
pub const EVENT_BATCH: i64 = 256;

/// Tables whose every write must advance the trading generation in its own
/// transaction. This is the Rust copy of `DEPENDENCIES` in the cache checkpoint
/// migration; a test compares the two, and `docs/rust-phase1/cache-dependencies.md`
/// explains why each table is here.
pub const TRADING_DEPENDENCY_TABLES: [&str; 16] = [
    "orderbook_orders",
    "trades",
    "products",
    "delivery_points",
    "users",
    "organizations",
    "kyc_execution_grace",
    "market_row_quarantines",
    "organization_market_approvals",
    "market_indications",
    "fair_price_bands",
    "physical_stems",
    "market_signal_ingestion_runs",
    "benchmarks",
    "live_slice_benchmarks",
    "market_event_outbox",
];

/// True only when every dependency table still carries the generation trigger,
/// enabled for every session role: after, per statement, for INSERT, UPDATE,
/// DELETE and TRUNCATE. A missing or disabled trigger is a silent freshness hole,
/// so the listener refuses to attach and the caller keeps caching off.
pub async fn dependency_triggers_complete(
    connection: &mut sqlx::PgConnection,
) -> Result<bool, sqlx::Error> {
    // tgtype bits: 1 row, 2 before, 4 insert, 8 delete, 16 update, 32 truncate,
    // 64 instead. 60 with 1, 2 and 64 clear is "after, statement, all four".
    let present: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM unnest($1::text[]) AS dependency(name) WHERE EXISTS ( \
         SELECT 1 FROM pg_catalog.pg_trigger AS tg \
         WHERE tg.tgrelid = to_regclass(format('public.%I', dependency.name)) \
           AND tg.tgname = 'public_read_cache_changed' \
           AND tg.tgenabled = 'A' \
           AND tg.tgfoid = to_regprocedure('public.bump_public_read_cache_generation()') \
           AND (tg.tgtype & 127) = 60)",
    )
    .bind(TRADING_DEPENDENCY_TABLES.map(str::to_owned).to_vec())
    .fetch_one(connection)
    .await?;
    Ok(usize::try_from(present).is_ok_and(|count| count == TRADING_DEPENDENCY_TABLES.len()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum EventBatch {
    Current,
    Advanced(i64),
    Resynchronize,
}

/// A gap or unknown domain clears synchronization instead of claiming coverage.
pub fn check_batch(current: i64, rows: &[(String, i64)]) -> EventBatch {
    if current < 0 || rows.len() > EVENT_BATCH as usize {
        return EventBatch::Resynchronize;
    }
    let mut next = current;
    for (domain, generation) in rows {
        if domain != "trading" || next.checked_add(1) != Some(*generation) {
            return EventBatch::Resynchronize;
        }
        next = *generation;
    }
    if next == current {
        EventBatch::Current
    } else {
        EventBatch::Advanced(next)
    }
}

/// The connection allowance the attested budget leaves for the listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerBudget {
    total_without_listener: u32,
    available: u32,
}

impl ListenerBudget {
    /// `total_without_listener` is the Python budget plus the Rust pool the
    /// existing attestation already counts. `available` is the smaller of the
    /// configured and the observed `max_connections`.
    pub fn new(total_without_listener: u32, configured_max: u32, observed_max: u32) -> Self {
        Self {
            total_without_listener,
            available: configured_max.min(observed_max),
        }
    }

    fn check(self) -> Result<u32, &'static str> {
        listener_budget(self.total_without_listener, self.available, true)
    }
}

/// Deliberately not started by AppState: the listener needs an independently
/// attested budget slot and an externally activated authority before trading
/// caching can run.
pub struct CacheEvents {
    listener: PgListener,
    pool: sqlx::PgPool,
    authority: Arc<CacheAuthority>,
    id: ListenerId,
}

impl CacheEvents {
    /// Open the one dedicated listener connection. The listener is bound to the
    /// identity its own connection reports, and only when that is the identity
    /// the owner activated. Connecting to an old or restored database fails here.
    pub async fn connect(
        options: &PgConnectOptions,
        timezone: &str,
        authority: Arc<CacheAuthority>,
        budget: ListenerBudget,
    ) -> Result<Self, sqlx::Error> {
        budget
            .check()
            .map_err(|message| sqlx::Error::Protocol(message.into()))?;
        // PgListener owns this connection, rather than borrowing an extra pool
        // slot. The budget above accounts for exactly one dedicated connection.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        match Self::attach(pool.clone(), timezone, authority).await {
            Ok(events) => Ok(events),
            Err(error) => {
                pool.close().await;
                Err(error)
            }
        }
    }

    async fn attach(
        pool: sqlx::PgPool,
        timezone: &str,
        authority: Arc<CacheAuthority>,
    ) -> Result<Self, sqlx::Error> {
        let mut listener = PgListener::connect_with(&pool).await?;
        // A lost connection must reach the owner before SQLx reconnects.
        listener.eager_reconnect(false);
        sqlx::query("SELECT set_config('TimeZone', $1, false)")
            .bind(timezone)
            .execute(&mut listener)
            .await?;
        let actual: String = sqlx::query_scalar("SHOW TimeZone")
            .fetch_one(&mut listener)
            .await?;
        if actual != timezone {
            return Err(sqlx::Error::Protocol(
                "cache listener timezone mismatch".into(),
            ));
        }
        // LISTEN first: a commit between the identity read and LISTEN would be
        // missed, one after LISTEN is buffered and reaches the first wake.
        listener.listen(CACHE_CHANNEL).await?;
        if !dependency_triggers_complete(listener.acquire().await?).await? {
            return Err(sqlx::Error::Protocol(
                "cache dependency triggers are missing or not always enabled".into(),
            ));
        }
        let state = database_state(listener.acquire().await?).await?;
        let id = state
            .and_then(|state| authority.attach_listener(&state.authority))
            .ok_or_else(|| {
                sqlx::Error::Protocol("cache listener database is not the active authority".into())
            })?;
        Ok(Self {
            listener,
            pool,
            authority,
            id,
        })
    }

    /// A lost connection or unknown wake detaches the listener, which disables
    /// hits and fences every entry made before. The owner must call `connect`
    /// again; nothing retries silently.
    pub async fn wake(&mut self) -> Result<(), sqlx::Error> {
        if !self.authority.listener_is_current(self.id) {
            return Err(sqlx::Error::Protocol(
                "cache listener is no longer bound to the active authority".into(),
            ));
        }
        match self.listener.try_recv().await {
            Ok(Some(notification))
                if notification.channel() == CACHE_CHANNEL
                    && notification.payload() == "trading" =>
            {
                Ok(())
            }
            Ok(_) => {
                self.authority.detach_listener(self.id);
                Err(sqlx::Error::Protocol(
                    "cache listener requires resynchronization".into(),
                ))
            }
            Err(error) => {
                self.authority.detach_listener(self.id);
                Err(error)
            }
        }
    }

    pub async fn read_batch(
        connection: &mut sqlx::PgConnection,
        after: i64,
    ) -> Result<EventBatch, sqlx::Error> {
        let rows = sqlx::query("SELECT domain,generation FROM public.public_read_cache_events WHERE domain='trading' AND generation > $1 ORDER BY generation LIMIT $2")
            .bind(after).bind(EVENT_BATCH).fetch_all(connection).await?;
        let values: Vec<(String, i64)> = rows
            .into_iter()
            .map(|row| (row.get("domain"), row.get("generation")))
            .collect();
        Ok(check_batch(after, &values))
    }

    /// Read the durable event rows on the listener's own connection, so the
    /// reader never takes a second budgeted slot.
    pub async fn read_batch_here(&mut self, after: i64) -> Result<EventBatch, sqlx::Error> {
        let connection = self.listener.acquire().await?;
        Self::read_batch(connection, after).await
    }

    /// The generation the listener's own connection reports. A gap between
    /// this and a wake means the reader must resynchronize.
    pub async fn trading_generation(&mut self) -> Result<i64, sqlx::Error> {
        let state = database_state(self.listener.acquire().await?).await?;
        state.map(|state| state.generation).ok_or_else(|| {
            sqlx::Error::Protocol("cache listener database is not the active authority".into())
        })
    }

    pub async fn close(self) -> Result<(), sqlx::Error> {
        self.authority.detach_listener(self.id);
        drop(self.listener);
        self.pool.close().await;
        Ok(())
    }
}

/// The owner record this process must re-read. It sits beside the running
/// binary, inside the deploy's private runtime directory.
const RECORD_NAME: &str = "cache-authority.json";

/// How often the runtime loop wakes. A wake later than a commit only delays
/// clearing; the generation check on every hit keeps correctness independent of
/// this delay.
const TICK: Duration = Duration::from_millis(100);

/// Longest one tick waits for a notification. A quiet channel is the normal
/// case; the wait is bounded so the authority refresh keeps its own interval.
const WAKE_POLL: Duration = TICK;

/// How long to wait before retrying an attach that failed. Attachment failure
/// is normal while the owner record is absent or revoked.
const ATTACH_RETRY: Duration = Duration::from_secs(1);

struct RuntimeState {
    listener: Option<CacheEvents>,
    last_fence: u64,
    /// `-1` reads every retained event until the first attach publishes the
    /// real generation; no committed generation starts below zero.
    last_generation: i64,
    next_refresh: Option<Instant>,
    next_attach: Option<Instant>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            listener: None,
            last_fence: 0,
            last_generation: -1,
            next_refresh: None,
            next_attach: None,
        }
    }
}

/// The process-local trading cache runtime. It refreshes the external authority
/// record, keeps one dedicated listener attached to the active incarnation,
/// clears every route cache when the fence moves or an event arrives, and keeps
/// the reference cache's approved 60-second staleness while dropping its
/// entries on an authority change. It never stores a payload itself.
pub struct TradingCacheRuntime {
    authority: Arc<CacheAuthority>,
    record: PathBuf,
    options: PgConnectOptions,
    timezone: String,
    budget: ListenerBudget,
    caches: Arc<TradingCaches>,
    reference: Arc<ReferenceCache<ReferenceKey>>,
    state: tokio::sync::Mutex<RuntimeState>,
}

impl TradingCacheRuntime {
    pub fn from_config(
        config: &crate::Config,
        caches: Arc<TradingCaches>,
        reference: Arc<ReferenceCache<ReferenceKey>>,
    ) -> Result<Self, String> {
        let options = PgConnectOptions::from_str(&config.database_url)
            .map_err(|_| "invalid cache listener database URL")?
            .application_name("verdaxis_cache_listener");
        let total = config
            .python_budget
            .checked_add(config.pool_max)
            .ok_or("cache listener budget overflow")?;
        Ok(Self {
            authority: Arc::new(CacheAuthority::default()),
            record: record_path(config),
            options,
            timezone: config.db_session_time_zone.clone(),
            budget: ListenerBudget::new(
                total,
                config.configured_max_connections,
                config.configured_max_connections,
            ),
            caches,
            reference,
            state: tokio::sync::Mutex::new(RuntimeState::default()),
        })
    }

    pub fn authority(&self) -> &Arc<CacheAuthority> {
        &self.authority
    }

    pub fn record_path(&self) -> &Path {
        &self.record
    }

    /// One loop step: re-read the record, attach or retire the listener and
    /// clear obsolete entries. Public so a test drives the same code the loop
    /// runs instead of sleeping.
    pub async fn tick(&self, now: Instant) {
        let mut state = self.state.lock().await;
        if state.next_refresh.is_none_or(|next| now >= next) {
            self.authority.refresh(&self.record, now);
            state.next_refresh = Some(now + AUTHORITY_REFRESH);
        }
        // Any authority, listener or revocation change retires every entry and
        // the listener bound to the old identity. Reference entries keep their
        // own 60-second allowance but never survive an authority change.
        let fence = self.authority.current_fence();
        if fence != state.last_fence {
            state.last_fence = fence;
            self.clear_all();
            if let Some(listener) = state.listener.take() {
                let _ = listener.close().await;
            }
        }
        if state.listener.is_none() {
            if state.next_attach.is_some_and(|next| now < next) {
                return;
            }
            match CacheEvents::connect(
                &self.options,
                &self.timezone,
                self.authority.clone(),
                self.budget,
            )
            .await
            {
                Ok(mut listener) => match listener.trading_generation().await {
                    Ok(generation) => {
                        state.last_generation = generation;
                        state.last_fence = self.authority.current_fence();
                        state.listener = Some(listener);
                        self.clear_all();
                    }
                    Err(_) => {
                        let _ = listener.close().await;
                        state.next_attach = Some(now + ATTACH_RETRY);
                    }
                },
                Err(_) => state.next_attach = Some(now + ATTACH_RETRY),
            }
            return;
        }
        let after = state.last_generation;
        // `wake` waits on the socket, so bound it: no notification this tick is
        // normal and must not stall the authority refresh loop.
        let wake = match state.listener.as_mut() {
            Some(listener) => tokio::time::timeout(WAKE_POLL, listener.wake()).await,
            None => return,
        };
        match wake {
            // No notification inside the poll window: normal, keep the listener.
            Err(_) => {}
            Ok(Ok(())) => {
                let batch = match state.listener.as_mut() {
                    Some(listener) => listener.read_batch_here(after).await,
                    None => return,
                };
                // A trading event clears trading entries only. Reference data
                // keeps its separate, approved 60-second allowance.
                self.caches.clear();
                match batch {
                    Ok(EventBatch::Advanced(generation)) => {
                        state.last_generation = generation;
                    }
                    Ok(EventBatch::Current) => {}
                    _ => {
                        // A gap, an unknown event shape or a read failure never
                        // keeps synchronization; the next tick re-attaches.
                        if let Some(listener) = state.listener.as_mut() {
                            match listener.trading_generation().await {
                                Ok(generation) => state.last_generation = generation,
                                Err(_) => {
                                    if let Some(listener) = state.listener.take() {
                                        let _ = listener.close().await;
                                    }
                                    state.next_attach = Some(now + ATTACH_RETRY);
                                }
                            }
                        }
                    }
                }
            }
            Ok(Err(_)) => {
                // A lost or unknown listener cannot prove freshness. Trading
                // entries go immediately; reference data keeps its own timer
                // unless the authority itself changed.
                self.caches.clear();
                if let Some(listener) = state.listener.take() {
                    let _ = listener.close().await;
                }
                state.next_attach = Some(now + ATTACH_RETRY);
            }
        }
    }

    fn clear_all(&self) {
        self.caches.clear();
        self.reference.clear();
    }

    /// Interval the started loop should use between ticks.
    pub const fn interval() -> Duration {
        TICK
    }
}

/// The owner record path. A deploy places the binary and the record in the same
/// private runtime directory. Only the test environment may redirect the path,
/// so a configuration mistake cannot point a deployment at a friendlier file.
fn record_path(config: &crate::Config) -> PathBuf {
    if config.environment == "test"
        && let Some(path) = std::env::var_os("PUBLIC_CACHE_TEST_RECORD")
    {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(RECORD_NAME)
}

/// Refuse an unbudgeted listener; existing disabled-cache starts add no slot.
pub fn listener_budget(
    total_without_listener: u32,
    available: u32,
    enabled: bool,
) -> Result<u32, &'static str> {
    let total = total_without_listener
        .checked_add(u32::from(enabled))
        .ok_or("cache listener budget overflow")?;
    if total > available {
        Err("cache listener exceeds attested connection budget")
    } else {
        Ok(total)
    }
}
