//! W02, W03 and W11: the default radar and custom container writes.
//!
//! Every handler holds the actor's user-row lock from authentication to commit,
//! so the radar uniqueness and the container cap need no further locking.
use super::http::{WatchlistState, WriteControls, not_found, refusal, run};
use super::validation::{parse_body, path_uuid, unprocessable, validate_create};
use super::{Summary, capture_visible};
use crate::ApiError;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

const MAX_WATCHLISTS_PER_USER: i64 = 10;
const DEFAULT_RADAR_NAME: &str = "Market Radar";

/// The radar write has committed by the time the summary is read. A failed or
/// empty summary is therefore a response, never a reason to undo the radar.
enum RadarOutcome {
    Summary(Summary),
    Missing,
    SummaryFailed,
}

/// The actor's one default radar. Python selects first and inserts only when
/// none exists; a concurrent insert resolves to the winner's row.
async fn ensure_radar(
    connection: &mut PgConnection,
    actor: Uuid,
    controls: &mut WriteControls,
) -> Result<Uuid, sqlx::Error> {
    const SELECT: &str =
        "SELECT id FROM watchlists WHERE user_id=$1 AND kind='RADAR_DEFAULT' LIMIT 1";
    if let Some(row) = sqlx::query(SELECT)
        .bind(actor)
        .fetch_optional(&mut *connection)
        .await?
    {
        return row.try_get("id");
    }
    let inserted = sqlx::query(
        "INSERT INTO watchlists(id,user_id,name,kind,created_at) \
         VALUES($1,$2,$3,'RADAR_DEFAULT',$4) ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(controls.next_id())
    .bind(actor)
    .bind(DEFAULT_RADAR_NAME)
    .bind(controls.now())
    .fetch_optional(&mut *connection)
    .await?;
    match inserted {
        Some(row) => row.try_get("id"),
        None => sqlx::query(SELECT)
            .bind(actor)
            .fetch_one(&mut *connection)
            .await?
            .try_get("id"),
    }
}

/// Read the radar summary inside a savepoint so a failed read leaves the
/// transaction (and the radar insert) intact.
async fn read_summary(
    connection: &mut PgConnection,
    state: &WatchlistState,
    now: DateTime<Utc>,
    actor: Uuid,
    radar: Uuid,
    fail: bool,
) -> RadarOutcome {
    if sqlx::query("SAVEPOINT watchlist_summary")
        .execute(&mut *connection)
        .await
        .is_err()
    {
        return RadarOutcome::SummaryFailed;
    }
    let read = async {
        if fail {
            return Err(sqlx::Error::PoolClosed);
        }
        let clock = crate::market::visibility::VisibilityClock::at(now);
        let visible = capture_visible(
            &mut *connection,
            &state.app.policy,
            &clock,
            &state.app.config,
        )
        .await?;
        visible
            .detail(
                &mut *connection,
                actor,
                radar,
                state.settings.best_price_cutoff(),
            )
            .await
    }
    .await;
    match read {
        Ok(found) => {
            let _ = sqlx::query("RELEASE SAVEPOINT watchlist_summary")
                .execute(&mut *connection)
                .await;
            found.map_or(RadarOutcome::Missing, RadarOutcome::Summary)
        }
        Err(_) => {
            let _ = sqlx::query("ROLLBACK TO SAVEPOINT watchlist_summary")
                .execute(&mut *connection)
                .await;
            RadarOutcome::SummaryFailed
        }
    }
}

/// W02: ensure the default radar, commit it, then return its summary.
pub(super) async fn radar(State(state): State<WatchlistState>, headers: HeaderMap) -> Response {
    let mut controls = state.settings.controls_for(&headers);
    let now = Utc::now();
    let worker = state.clone();
    let result = run(
        &state,
        &headers,
        "POST",
        "/api/watchlists/me",
        move |transaction| {
            Box::pin(async move {
                let actor = transaction.actor().id;
                let radar = ensure_radar(transaction.connection(), actor, &mut controls)
                    .await
                    .map_err(|_| ApiError::database())?;
                Ok(read_summary(
                    transaction.connection(),
                    &worker,
                    now,
                    actor,
                    radar,
                    controls.fail_summary,
                )
                .await)
            })
        },
    )
    .await;
    match result {
        Ok(RadarOutcome::Summary(summary)) => Json(summary).into_response(),
        Ok(RadarOutcome::Missing) => not_found("Watchlist not found").into_response(),
        Ok(RadarOutcome::SummaryFailed) => ApiError::database().into_response(),
        Err(response) => response,
    }
}

/// W03: a custom container, limited to ten per user of any kind.
pub(super) async fn create(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    raw: Bytes,
) -> Response {
    // JSON syntax is refused before authentication, as FastAPI reads the body first.
    let body = match parse_body(&raw) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let mut controls = state.settings.controls_for(&headers);
    let result = run(&state, &headers, "POST", "/api/watchlists", move |transaction| {
        Box::pin(async move {
            let name = validate_create(&body).map_err(unprocessable)?;
            let actor = transaction.actor().id;
            let existing: i64 =
                sqlx::query_scalar("SELECT count(id) FROM watchlists WHERE user_id=$1")
                    .bind(actor)
                    .fetch_one(&mut *transaction.connection())
                    .await
                    .map_err(|_| ApiError::database())?;
            if existing >= MAX_WATCHLISTS_PER_USER {
                return Err(refusal(
                    StatusCode::BAD_REQUEST,
                    "Maximum of 10 watchlists allowed",
                ));
            }
            let id = controls.next_id();
            let created_at: DateTime<Utc> = sqlx::query_scalar(
                "INSERT INTO watchlists(id,user_id,name,kind,created_at) \
                 VALUES($1,$2,$3,'CUSTOM',$4) RETURNING created_at",
            )
            .bind(id)
            .bind(actor)
            .bind(&name)
            .bind(controls.now())
            .fetch_one(&mut *transaction.connection())
            .await
            .map_err(|_| ApiError::database())?;
            Ok(json!({"id":id,"name":name,"entry_count":0,"entries":[],"created_at":created_at}))
        })
    })
    .await;
    match result {
        Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
        Err(response) => response,
    }
}

/// W11: delete a custom container and everything under it. Entries have no
/// database cascade, so every child table is cleared explicitly, children first.
pub(super) async fn delete(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let result = run(
        &state,
        &headers,
        "DELETE",
        "/api/watchlists/{watchlist_id}",
        move |transaction| {
            Box::pin(async move {
                let id =
                    path_uuid("watchlist_id", &raw).map_err(|item| unprocessable(vec![item]))?;
                let actor = transaction.actor().id;
                let kind: Option<String> = sqlx::query_scalar(
                    "SELECT kind::text FROM watchlists WHERE id=$1 AND user_id=$2",
                )
                .bind(id)
                .bind(actor)
                .fetch_optional(&mut *transaction.connection())
                .await
                .map_err(|_| ApiError::database())?;
                match kind.as_deref() {
                    None => return Err(not_found("Watchlist not found")),
                    Some("RADAR_DEFAULT") => {
                        return Err(refusal(
                            StatusCode::BAD_REQUEST,
                            "The Market Radar container cannot be deleted",
                        ));
                    }
                    Some(_) => {}
                }
                for statement in [
                    "DELETE FROM watchlist_events WHERE watchlist_id=$1",
                    "DELETE FROM watchlist_targets WHERE watchlist_id=$1",
                    "DELETE FROM watchlist_entries WHERE watchlist_id=$1",
                    "DELETE FROM watchlists WHERE id=$1",
                ] {
                    sqlx::query(statement)
                        .bind(id)
                        .execute(&mut *transaction.connection())
                        .await
                        .map_err(|_| ApiError::database())?;
                }
                Ok(())
            })
        },
    )
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => response,
    }
}
