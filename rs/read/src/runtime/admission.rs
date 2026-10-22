use crate::market::cache_dependencies::{TemporalFacts, TradingClock, cacheable};
use crate::runtime::cache::{TradingCache, TradingEntry};
use crate::runtime::cache_authority::{FillTicket, admit_connection};
use crate::{ApiError, AppState, busy};
use sqlx::Connection;
use std::sync::Arc;

/// Rust-owned default (founder decision 88); readiness has one extra connection.
pub const DEFAULT_READ_ADMISSION_SLOTS: u32 = 16;

tokio::task_local! {
    static REQUEST_PERMIT: std::sync::Mutex<Option<tokio::sync::OwnedSemaphorePermit>>;
}

pub async fn middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !selected_read(request.method().as_str(), request.uri().path()) {
        return next.run(request).await;
    }
    let permit = match try_read(&state) {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    REQUEST_PERMIT
        .scope(std::sync::Mutex::new(Some(permit)), next.run(request))
        .await
}

pub fn try_read(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state.reads.clone().try_acquire_owned().map_err(|_| busy())
}

/// Run one unit of admitted database work.
///
/// The permit and the pooled connection are both owned by a spawned task, so a
/// client cancellation only detaches the caller. The task pings the connection
/// (draining every pending response), completes the pool return, and only then
/// releases the permit. A restarted caller can therefore never find a slot free
/// while its previous query is still running. Failed work closes the connection
/// instead of returning it to the pool.
pub async fn run_admitted<T: Send + 'static>(
    state: &AppState,
    work: impl for<'c> FnOnce(
        &'c mut sqlx::PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'c>,
    > + Send
    + 'static,
) -> Result<T, ApiError> {
    // The request middleware already admitted selected GETs; reuse its permit so
    // one request never occupies two slots. MCP tools and tests acquire here.
    let inherited = REQUEST_PERMIT
        .try_with(|permit| permit.lock().expect("request permit lock").take())
        .ok()
        .flatten();
    let permit = match inherited {
        Some(permit) => permit,
        None => try_read(state)?,
    };
    let pool = state.pools.read.clone();
    // Dropping the caller's JoinHandle detaches, rather than cancels, this owner.
    tokio::spawn(async move {
        let mut connection = pool.acquire().await.map_err(|error| {
            log_sql_error("acquire", &error);
            ApiError::database()
        })?;
        let result = work(&mut connection).await;
        let reusable = if result.is_ok() {
            match connection.ping().await {
                Ok(()) => true,
                Err(error) => {
                    log_sql_error("cleanup", &error);
                    false
                }
            }
        } else {
            // SQL errors are logged by the route before conversion to ApiError.
            tracing::warn!(
                stage = "work",
                error_class = "api_error",
                sqlstate = "",
                "admitted_work_failed"
            );
            false
        };
        if reusable {
            // `return_to_pool` is the drop handler run eagerly, so the pool
            // counts the connection as free before admission opens again.
            connection.return_to_pool().await;
            drop(connection);
            drop(permit);
            result
        } else {
            if let Err(error) = connection.close().await {
                log_sql_error("cleanup", &error);
            }
            drop(permit);
            match result {
                Err(error) => Err(error),
                // The work succeeded but the connection is unusable.
                Ok(_) => Err(ApiError::database()),
            }
        }
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// Observable boundaries of one trading read. Observers do not decide admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradingReadStage {
    SnapshotEstablished,
    BeforeInsert,
    BeforePublish,
}

/// Per-runtime clock and scheduler seam. Serving uses the real clock and no
/// waits; disposable schedules can hold a read at an exact lifecycle boundary.
pub trait TradingReadObserver: Send + Sync {
    fn now(&self) -> TradingClock {
        TradingClock::now()
    }

    fn checkpoint(&self, _stage: TradingReadStage) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(std::future::ready(()))
    }
}

pub struct RealtimeTradingRead;
impl TradingReadObserver for RealtimeTradingRead {}

/// What a route returns from its uncached query: the public value and every
/// time dependency the same snapshot read.
pub struct TradingFill<T> {
    pub value: T,
    pub facts: TemporalFacts,
}

/// One admitted trading read with a commit-safe cache.
///
/// The permit and the pooled connection stay owned by `run_admitted` for the
/// whole query, so a cancelled caller cannot free a slot early. Caching off
/// runs exactly the loader: no generation, incarnation or cache query is sent,
/// so the response and its SQL are identical to the uncached service. Caching
/// on captures one clock, opens one read-only REPEATABLE READ snapshot, reads
/// the generation in that snapshot, and serves a cached payload only when the
/// authority token, fence, generation, policy identity and time boundary all
/// still match. An unknown authority returns the existing database error.
/// An underivable boundary bypasses storage, but never the publication fence.
pub async fn run_trading_read<K, T>(
    state: &AppState,
    cache: &Arc<TradingCache<K>>,
    key: K,
    family: &'static str,
    load: impl for<'c> FnOnce(
        &'c mut sqlx::PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<TradingFill<T>, ApiError>> + Send + 'c>,
    > + Send
    + 'static,
) -> Result<T, ApiError>
where
    K: Eq + Send + 'static,
    T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
{
    let Some(runtime) = state.trading_cache.clone() else {
        return run_admitted(state, move |connection| {
            Box::pin(async move { Ok(load(connection).await?.value) })
        })
        .await;
    };
    let observer = state.trading_read_observer.clone();
    let authority = runtime.authority().clone();
    let policy = state.policy_identity.clone();
    // The work closure is spawned and may outlive this call, so it owns its
    // handle to the family cache rather than borrowing it.
    let cache = cache.clone();
    run_admitted(state, move |connection| {
        Box::pin(async move {
            let clock = observer.now();
            let mut transaction = connection.begin().await.map_err(|_| ApiError::database())?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *transaction)
                .await
                .map_err(|_| ApiError::database())?;
            let admitted = admit_connection(&authority, &mut transaction, clock.monotonic)
                .await
                .map_err(|_| ApiError::database())?;
            // Even an uncached answer needs an authoritative connection: without
            // a token there is no fence to check before publication.
            let Some((token, generation)) = admitted else {
                return Err(ApiError::database());
            };
            observer
                .checkpoint(TradingReadStage::SnapshotEstablished)
                .await;
            // Independent snapshots can finish out of order. Retain the
            // high-water mark for storage without revoking an older read.
            if !authority.observe_snapshot(&token, generation, clock.monotonic) {
                transaction
                    .rollback()
                    .await
                    .map_err(|_| ApiError::database())?;
                return Err(ApiError::database());
            }
            let ticket = FillTicket::new(token, generation);
            if let Some(entry) = cache.hit(
                &authority,
                &key,
                &ticket,
                generation,
                &policy,
                clock.monotonic,
            ) && entry.valid_until.is_live(observer.now().monotonic)
            {
                match serde_json::from_slice(&entry.payload) {
                    Ok(value) => {
                        transaction
                            .rollback()
                            .await
                            .map_err(|_| ApiError::database())?;
                        observer.checkpoint(TradingReadStage::BeforePublish).await;
                        if !authority.accepts(ticket.token(), observer.now().monotonic) {
                            return Err(ApiError::database());
                        }
                        return Ok(value);
                    }
                    // A self-produced payload that cannot be read is a bug;
                    // drop it and answer from the database instead.
                    Err(_) => cache.remove(&key),
                }
            }
            let fill = load(&mut transaction).await?;
            // Only the facts this family's query reads can end the entry; a
            // fact it does not read stays `Unknown` and must not block storage.
            let boundary = fill.facts.boundary_for(family, &clock);
            let payload = serde_json::to_vec(&fill.value).ok();
            transaction
                .rollback()
                .await
                .map_err(|_| ApiError::database())?;
            observer.checkpoint(TradingReadStage::BeforeInsert).await;
            // Stale-fill barriers. A fill started under one authority and
            // fence may neither be stored nor answered after either moved.
            if !authority.accepts(ticket.token(), observer.now().monotonic) {
                return Err(ApiError::database());
            }
            // A newer generation verified by this process since the fill
            // started also retires it from the cache. The value itself is a
            // valid snapshot answer, so it is still returned; only the
            // insertion is refused.
            let latest = authority
                .highest_generation()
                .unwrap_or(ticket.generation());
            if latest == ticket.generation()
                && let (Some(payload), true) = (payload, cacheable(family, &fill.facts, &clock))
            {
                // The boundary is a monotonic instant, so insertion checks
                // it against elapsed time, not the pre-query capture.
                cache.insert(
                    key,
                    TradingEntry::new(&ticket, payload, boundary, policy),
                    observer.now().monotonic,
                );
            }
            observer.checkpoint(TradingReadStage::BeforePublish).await;
            // Publishing the response repeats the fence check, so a change
            // after the insertion cannot let an old authority answer.
            if !authority.accepts(ticket.token(), observer.now().monotonic) {
                return Err(ApiError::database());
            }
            Ok(fill.value)
        })
    })
    .await
}

/// Log only bounded database failure metadata, never SQL text or values.
pub fn log_sql_error(stage: &'static str, error: &sqlx::Error) {
    let error_class = match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::Decode(_) | sqlx::Error::ColumnDecode { .. } => "decode",
        sqlx::Error::RowNotFound => "row_not_found",
        _ => "sqlx_error",
    };
    let code = error.as_database_error().and_then(|error| error.code());
    let sqlstate = code
        .as_deref()
        .filter(|code| {
            code.len() == 5
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        .unwrap_or("");
    tracing::warn!(stage, error_class, sqlstate, "admitted_database_failure");
}

pub fn selected_read(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    let Some(path) = path.strip_prefix("/api") else {
        return false;
    };
    matches!(
        path,
        "/availability"
            | "/benchmarks"
            | "/catalog/products"
            | "/catalog/delivery-points"
            | "/curves/forward"
            | "/curves/forward/table"
            | "/curves/forward/slice"
            | "/curves/forward/board"
            | "/curves/forward/export"
            | "/demand"
            | "/listings"
            | "/orderbook"
            | "/orderbook/snapshot"
            | "/orderbook/bids"
            | "/orderbook/asks"
            | "/orderbook/with-ci"
            | "/orderbook/aggregated"
            | "/orderbook/product-counts"
            | "/orderbook/map-summary"
            | "/orderbook/map-summary/compact"
            | "/orderbook/products"
            | "/orderbook/regions"
            | "/orderbook/fuel-types"
            | "/ports"
            | "/prices"
            | "/prices/reference"
            | "/prices/reference/export"
            | "/producers"
            | "/trade-tape"
    ) || path
        .strip_prefix("/ports/")
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[derive(Clone)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn acquire_and_cleanup_logs_do_not_include_error_values() {
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = Capture(bytes.clone());
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        log_sql_error("acquire", &sqlx::Error::PoolTimedOut);
        log_sql_error("cleanup", &sqlx::Error::Protocol("secret-sentinel".into()));
        let logs = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(!logs.contains("secret-sentinel"));
        let events: Vec<serde_json::Value> = logs
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["fields"]["stage"], "acquire");
        assert_eq!(events[0]["fields"]["error_class"], "pool_timeout");
        assert_eq!(events[1]["fields"]["stage"], "cleanup");
        assert_eq!(events[1]["fields"]["error_class"], "protocol");
    }
}
