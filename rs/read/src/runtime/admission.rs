use crate::routes::market_data_error;
use crate::{ApiError, ApiErrorKind, AppState, busy};
use sqlx::Connection;

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
        Err(_) if market_data_read(request.method().as_str(), request.uri().path()) => {
            return market_data_error::unavailable().into_response();
        }
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
    admitted(state, Failures::Existing, work).await
}

/// Run one Phase 5 public market read.
///
/// The work runs in one READ ONLY transaction whose statements stop after five
/// seconds. `SET LOCAL` keeps the limit inside the transaction, so the pooled
/// session keeps its defaults. The owner task of `run_admitted` holds the
/// permit and the connection through commit, rollback and cleanup; a client
/// cancellation only detaches the caller, and nothing is retried. Admission,
/// pool, transaction and owner failures use the typed market-data envelope;
/// the route maps its own SQL errors with `market_data_error::database_failure`.
pub async fn run_market_read<T: Send + 'static>(
    state: &AppState,
    work: impl for<'c> FnOnce(
        &'c mut sqlx::PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'c>,
    > + Send
    + 'static,
) -> Result<T, ApiError> {
    admitted(state, Failures::MarketData, move |connection| {
        Box::pin(async move {
            let mut transaction = connection
                .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY")
                .await
                .map_err(|error| market_data_error::database_failure("begin", &error))?;
            sqlx::query("SET LOCAL statement_timeout = '5s'")
                .execute(&mut *transaction)
                .await
                .map_err(|error| market_data_error::database_failure("begin", &error))?;
            // An error drops the transaction, which queues its rollback; the
            // owner then closes the connection instead of reusing it.
            let value = work(&mut transaction).await?;
            transaction
                .commit()
                .await
                .map_err(|error| market_data_error::database_failure("commit", &error))?;
            Ok(value)
        })
    })
    .await
}

/// How an admitted read reports a failure outside the route's own work.
#[derive(Clone, Copy)]
enum Failures {
    /// Existing reads: string details, unchanged.
    Existing,
    /// Phase 5 market reads: typed `READ_UNAVAILABLE` or `READ_FAILED`.
    MarketData,
}

impl Failures {
    /// Every admission slot is in use.
    fn refused(self) -> ApiError {
        match self {
            Self::Existing => busy(),
            Self::MarketData => market_data_error::unavailable(),
        }
    }

    /// Log bounded metadata once and classify a pool or connection failure.
    fn database(self, stage: &'static str, error: &sqlx::Error) -> ApiError {
        match self {
            Self::Existing => {
                log_sql_error(stage, error);
                ApiError::database()
            }
            Self::MarketData => market_data_error::database_failure(stage, error),
        }
    }

    /// The owner task ended without a result: its work panicked or the
    /// runtime is shutting down.
    fn owner_lost(self) -> ApiError {
        match self {
            Self::Existing => ApiError::internal(),
            Self::MarketData => {
                // SQLx's worker-crash class selects the generic typed 500 and
                // one bounded diagnostic event; no error text is formatted.
                let mut error =
                    market_data_error::database_failure("owner", &sqlx::Error::WorkerCrashed);
                error.kind = ApiErrorKind::Unexpected;
                error
            }
        }
    }
}

async fn admitted<T: Send + 'static>(
    state: &AppState,
    failures: Failures,
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
        None => try_read(state).map_err(|_| failures.refused())?,
    };
    let pool = state.pools.read.clone();
    // Dropping the caller's JoinHandle detaches, rather than cancels, this owner.
    tokio::spawn(async move {
        let mut connection = pool
            .acquire()
            .await
            .map_err(|error| failures.database("acquire", &error))?;
        let outcome = match work(&mut connection).await {
            Ok(value) => match connection.ping().await {
                Ok(()) => Ok(value),
                // The work succeeded but the connection is unusable.
                Err(error) => Err(failures.database("cleanup", &error)),
            },
            Err(error) => {
                // SQL errors are logged by the route before conversion to ApiError.
                tracing::warn!(
                    stage = "work",
                    error_class = "api_error",
                    sqlstate = "",
                    "admitted_work_failed"
                );
                Err(error)
            }
        };
        match outcome {
            Ok(value) => {
                // `return_to_pool` is the drop handler run eagerly, so the pool
                // counts the connection as free before admission opens again.
                connection.return_to_pool().await;
                drop(connection);
                drop(permit);
                Ok(value)
            }
            Err(error) => {
                // A failed unit never returns its connection to the pool.
                if let Err(close_error) = connection.close().await {
                    log_sql_error("cleanup", &close_error);
                }
                drop(permit);
                Err(error)
            }
        }
    })
    .await
    .map_err(|_| failures.owner_lost())?
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

/// The Phase 5 public market reads. Their admission and database failures use
/// the typed `{"detail": {"code", "message"}}` envelope; every other admitted
/// read keeps its string detail.
pub fn market_data_read(method: &str, path: &str) -> bool {
    method == "GET"
        && matches!(
            path,
            "/api/availability" | "/api/benchmarks" | "/api/demand" | "/api/producers"
        )
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
