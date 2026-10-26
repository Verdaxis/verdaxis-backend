//! Organization-scoped advisory dismissal, composed with the ordered transaction owner.
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::{
    ApiError, ApiErrorKind, AppState,
    auth::{
        CurrentActor, VerifiedAccess,
        request::{RequestAdmission, validate_and_admit},
    },
    business::transaction::{
        AttemptError, BusinessCommand, CommandFuture, OrganizationWritePlan, run_organization_write,
    },
};

fn refusal(status: axum::http::StatusCode, code: &str, message: &str) -> ApiError {
    ApiError {
        kind: ApiErrorKind::Expected,
        status,
        detail: json!({"code":code,"message":message}),
        retry_after: None,
    }
}
pub async fn dismiss_suggestion(
    state: &AppState,
    access: VerifiedAccess,
    order: String,
    admission: RequestAdmission,
) -> Result<Value, ApiError> {
    run_organization_write(state, access, Dismiss { order, admission }).await
}
struct Dismiss {
    order: String,
    admission: RequestAdmission,
}
impl BusinessCommand for Dismiss {
    type Output = Value;
    fn preview<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError> {
        // The shared owner adds and revalidates the actor's organization and user.
        Box::pin(async { Ok(OrganizationWritePlan::new([], [], [], Value::Null)) })
    }
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Value, AttemptError> {
        Box::pin(async move {
            // Indicative admission uses the shared advisory branch, independent
            // of the execution switch. It introduces no execution obligation.
            let sql = format!(
                "WITH anchor AS (SELECT 1) {} SELECT EXISTS(SELECT 1 FROM eligible_owners WHERE user_id=$1 AND organization_id=$3)",
                crate::market::visibility::eligibility_sets_sql("false", "$2")
            );
            let admitted: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(actor.id)
                .bind(chrono::Utc::now())
                .bind(actor.organization_id)
                .fetch_one(&mut *conn)
                .await?;
            if !admitted {
                return Err(refusal(
                    axum::http::StatusCode::FORBIDDEN,
                    "EXECUTION_NOT_AVAILABLE",
                    "Market access is not available",
                )
                .into());
            }
            let order = validate_and_admit(Value::Null, &self.admission, |_| {
                self.order.parse::<Uuid>().map_err(|_| {
                    refusal(
                        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                        "VALIDATION_FAILED",
                        "Order identifier is invalid",
                    )
                })
            })?;
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orderbook_orders WHERE id=$1)")
                    .bind(order)
                    .fetch_one(&mut *conn)
                    .await?;
            if !exists {
                return Err(refusal(
                    axum::http::StatusCode::NOT_FOUND,
                    "ORDER_NOT_FOUND",
                    "Order not found",
                )
                .into());
            }
            // The organization lock serializes all members before this check.
            // EXISTS also accepts retained rows created by other writers.
            let existing: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM match_suggestions WHERE recipient_org_id=$1 AND status='DISMISSED' AND (bid_order_id=$2 OR ask_order_id=$2))").bind(actor.organization_id).bind(order).fetch_one(&mut *conn).await?;
            if existing {
                return Ok(json!({"status":"already_dismissed"}));
            }
            let row=sqlx::query("INSERT INTO match_suggestions(id,bid_order_id,ask_order_id,score,match_reasons,status,recipient_org_id) VALUES($1,$2,$2,0,'[]','DISMISSED',$3) RETURNING id").bind(Uuid::new_v4()).bind(order).bind(actor.organization_id).fetch_one(conn).await?;
            let _: Uuid = row.try_get("id")?;
            Ok(json!({"status":"dismissed"}))
        })
    }
}
