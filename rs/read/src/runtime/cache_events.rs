//! Bounded invalidation consumption. NOTIFY is a wake, never freshness proof.
use crate::runtime::cache_authority::CacheAuthority;
use sqlx::{
    Row,
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
};
use std::sync::Arc;

pub const CACHE_CHANNEL: &str = "verdaxis_public_read_cache";
pub const EVENT_BATCH: i64 = 256;

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

/// Deliberately not started by AppState: the listener needs an independently
/// attested budget slot and activation owner before trading caching can run.
pub struct CacheEvents {
    listener: PgListener,
    pool: sqlx::PgPool,
    authority: Arc<CacheAuthority>,
}

impl CacheEvents {
    pub async fn connect(
        options: &PgConnectOptions,
        timezone: &str,
        authority: Arc<CacheAuthority>,
    ) -> Result<Self, sqlx::Error> {
        authority.revoke();
        // PgListener owns this connection, rather than borrowing an extra pool
        // slot. The caller must account for exactly one dedicated connection.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await?;
        let mut listener = PgListener::connect_with(&pool).await?;
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
        listener.listen(CACHE_CHANNEL).await?;
        Ok(Self {
            listener,
            pool,
            authority,
        })
    }

    /// A lost connection must reach the owner before SQLx silently reconnects.
    /// The owner retires old pools/fills and clears public payloads before retry.
    pub async fn wake(&mut self) -> Result<(), sqlx::Error> {
        match self.listener.try_recv().await {
            Ok(Some(notification))
                if notification.channel() == CACHE_CHANNEL
                    && notification.payload() == "trading" =>
            {
                Ok(())
            }
            Ok(_) => {
                self.authority.revoke();
                Err(sqlx::Error::Protocol(
                    "cache listener requires resynchronization".into(),
                ))
            }
            Err(error) => {
                self.authority.revoke();
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
        self.authority.revoke();
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
