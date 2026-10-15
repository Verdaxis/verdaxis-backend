//! Included only by the feature-gated disposable server example.
use axum::{
    extract::{Query, State},
    response::IntoResponse,
};
use std::collections::HashMap;
use verdaxis_read::market::live_benchmarks::{LiveBenchmarkCache, LiveBenchmarkKey};
use verdaxis_read::{ApiError, AppState};

pub async fn project(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let now = params
        .get("as_of")
        .cloned()
        .unwrap_or_else(|| std::env::var("RUST_CONTRACT_CLOCK").expect("fixed clock"))
        .parse::<chrono::DateTime<chrono::Utc>>()
        .expect("test clock");
    let mut keys = Vec::new();
    for side in ["ASK", "BID"] {
        for product in &state.policy.products {
            for point in &state.policy.points {
                for window in ["SPOT", "2026-11", "2099-01"] {
                    keys.push(LiveBenchmarkKey {
                        side: side.into(),
                        product: product.code.clone(),
                        point: point.id,
                        window: window.into(),
                    });
                }
            }
        }
    }
    keys.sort_by_key(|key| {
        (
            key.side.clone(),
            key.product.clone(),
            key.point,
            key.window.clone(),
        )
    });
    match params.get("selection").map(String::as_str) {
        Some("empty") => keys.clear(),
        Some("sparse") => keys = keys.into_iter().step_by(7).collect(),
        _ => {}
    }
    let policy = state.policy.clone();
    let config = state.config.clone();
    let body = verdaxis_read::runtime::admission::run_admitted(&state, move |connection| Box::pin(async move {
        use sqlx::Connection;
        let mut tx = connection.begin().await.expect("test transaction");
        let mut cache = LiveBenchmarkCache::default();
        let result = cache.load(&mut tx, &policy, &config, &keys, now).await?;
        // Abort the transaction: any accidental cache re-query now fails.
        assert!(sqlx::query("SELECT 1 / 0").execute(&mut *tx).await.is_err());
        // Repeated keys, including absent keys, must use the request cache.
        let again = cache.load(&mut tx, &policy, &config, &keys, now).await?;
        assert_eq!(result.len(), again.len());
        let rows: Vec<_> = keys.iter().map(|key| {
            let value = result[key].as_ref();
            serde_json::json!({"side": key.side, "product": key.product, "point": key.point, "window": key.window,
                "price": value.map(|value| value.price.to_string()),
                "quantity": value.map(|value| value.quantity.to_string()),
                "count": value.map(|value| value.count)})
        }).collect();
        tx.rollback().await.expect("read-only rollback");
        Ok(rows)
    })).await?;
    Ok(axum::Json(body))
}
