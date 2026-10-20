//! Bounded invalidation consumption. NOTIFY is a wake, never freshness proof.
use crate::runtime::cache_authority::{CacheAuthority, ListenerId, database_state};
use sqlx::{
    Acquire, Row,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
};
use std::sync::Arc;

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

    pub async fn close(self) -> Result<(), sqlx::Error> {
        self.authority.detach_listener(self.id);
        drop(self.listener);
        self.pool.close().await;
        Ok(())
    }
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
