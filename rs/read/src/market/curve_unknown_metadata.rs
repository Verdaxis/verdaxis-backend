//! Counts legacy UNKNOWN rows without exposing their price, quantity or owner.
use crate::market::visibility::VisibilityClock;
use crate::{ApiError, AppState, Json};
use sqlx::{PgConnection, Row};

pub async fn load(
    connection: &mut PgConnection,
    state: &AppState,
    clock: &VisibilityClock,
    keys: &Json,
) -> Result<Vec<Json>, ApiError> {
    let sql = crate::market::visibility::expand_shared_markers(
        &include_str!("../../sql/curve_unknown_metadata.sql").replace(
            "/* current_order_rule */",
            include_str!("../../sql/current_order_rule.sql"),
        ),
        "$5",
    );
    let policy = serde_json::to_string(state.policy.as_ref()).map_err(|_| ApiError::internal())?;
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(policy)
        .bind(clock.now)
        .bind(clock.singapore_date.to_string())
        .bind(keys.to_string())
        .bind(state.config.kyc_execution_gate_enabled)
        .fetch_all(connection)
        .await
        .map_err(|error| crate::routes::ports::query_error("/api/curves/forward/table", &error))?;
    rows.into_iter().map(|row| {
        Ok(serde_json::json!({
            "product_id": row.try_get::<uuid::Uuid, _>("product_id").map_err(|_| ApiError::internal())?,
            "delivery_point_id": row.try_get::<uuid::Uuid, _>("delivery_point_id").map_err(|_| ApiError::internal())?,
            "availability_window": row.try_get::<String, _>("availability_window").map_err(|_| ApiError::internal())?,
            "unknown_order_count": row.try_get::<i64, _>("unknown_order_count").map_err(|_| ApiError::internal())?,
        }))
    }).collect()
}
