//! The demo activity writer (Phase 10 Task 17; design revision 3,
//! `outputs/research/rust-phase10/task17-system-writer-design.md`).
//!
//! Ports `app/services/demo_activity.py` and `scripts/run_demo_activity.py`.
//! The job (`cli::jobs::demo`) runs the phases in order, each one transaction,
//! and stops at the first failure: provisioning (in the job module: it writes
//! organizations, not the order book), then here coverage (one transaction per
//! product), legacy expiry, prune and tick. Every phase that changed something
//! publishes one `demo_market_changed` event, as Python does.
use sqlx::PgConnection;
use uuid::Uuid;

use crate::cli::jobs::demo::model::{DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID};

/// The two demo activity organizations: tick, prune and legacy expiry write
/// only their rows.
pub(crate) const ACTIVITY_ORGANIZATIONS: [Uuid; 2] =
    [DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID];

/// Append Python's `demo_market_changed` event for one phase, in the
/// caller's transaction: participants the activity pair, no producer
/// evidence, the versioned payload with the public invalidation marker; the
/// dispatchers are woken on commit.
pub(crate) async fn append_demo_market_changed(
    conn: &mut PgConnection,
    operation: &str,
) -> Result<(), sqlx::Error> {
    let mut participants = ACTIVITY_ORGANIZATIONS.to_vec();
    participants.sort_unstable();
    let payload = serde_json::json!({
        "operation": operation,
        "schema_version": 1,
        "_public_market_invalidation": true,
    });
    sqlx::query(
        "INSERT INTO market_event_outbox \
           (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload, \
            producer_evidence, created_at) \
         VALUES ($1, 'demo_market_changed', 'demo_market', $2, $3::json, $4::json, NULL, now())",
    )
    .bind(Uuid::new_v4())
    .bind(operation)
    .bind(
        serde_json::to_value(&participants)
            .map_err(|error| sqlx::Error::Encode(Box::new(error)))?,
    )
    .bind(payload)
    .execute(&mut *conn)
    .await?;
    super::super::wake_market_event_dispatchers(conn).await
}
