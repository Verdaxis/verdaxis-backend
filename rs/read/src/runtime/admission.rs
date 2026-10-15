use crate::{ApiError, AppState, busy};
use sqlx::Connection;

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

pub async fn run_admitted<T: Send + 'static>(
    state: &AppState,
    work: impl for<'c> FnOnce(
        &'c mut sqlx::PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, ApiError>> + Send + 'c>,
    > + Send
    + 'static,
) -> Result<T, ApiError> {
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
        let mut connection = pool.acquire().await.map_err(|_| ApiError::internal())?;
        let result = work(&mut connection).await;
        if result.is_ok() && connection.ping().await.is_ok() {
            // Complete SQLx's pool return before releasing admission. A plain
            // drop schedules another ping and can leave a new read waiting.
            connection.return_to_pool().await;
            drop(connection);
            drop(permit);
            result
        } else {
            let _ = connection.close().await;
            drop(permit);
            match result {
                Err(error) => Err(error),
                Ok(_) => Err(ApiError::internal()),
            }
        }
    })
    .await
    .map_err(|_| ApiError::internal())?
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
