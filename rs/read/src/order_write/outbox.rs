//! Transactional append adapter. Internal producer facts have no transport DTO.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use super::{MarketErrorCode, MarketWriteError};

/// Participant-facing fields are constructed independently of producer evidence.
#[derive(Clone, Debug)]
pub struct ParticipantEvent {
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub participant_org_ids: Vec<Uuid>,
    pub fields: ParticipantFields,
    pub public_invalidated: bool,
}

/// Explicit transport projection. Subscription identities cannot enter this type.
#[derive(Clone, Debug)]
pub enum ParticipantFields {
    Order { order_id: Uuid, status: String },
    Inventory { inventory_id: Uuid, action: String },
    Support { context_id: Uuid, status: String },
    Commission { commission_id: Uuid, status: String },
}

impl ParticipantEvent {
    pub(super) fn payload(&self) -> Value {
        let mut value = match &self.fields {
            ParticipantFields::Order { order_id, status } => json!({"order_id":order_id,"status":status}),
            ParticipantFields::Inventory { inventory_id, action } => json!({"inventory_id":inventory_id,"action":action}),
            ParticipantFields::Support { context_id, status } => json!({"context_id":context_id,"status":status}),
            ParticipantFields::Commission { commission_id, status } => json!({"commission_id":commission_id,"status":status}),
        };
        value["schema_version"] = json!(1);
        if self.public_invalidated {
            value["_public_market_invalidation"] = json!(true);
        }
        value
    }
}

/// Database-only envelope. Deliberately has neither Serialize nor a transport conversion.
pub struct ProducerEvidence {
    pub(super) mutation_id: Uuid,
    pub(super) captured_at: DateTime<Utc>,
    pub(super) orders: Vec<Value>,
    pub(super) aggregates_before: Vec<Value>,
    pub(super) aggregates_after: Vec<Value>,
    pub(super) targets: Vec<Value>,
}

impl ProducerEvidence {
    fn database_value(&self) -> Value {
        json!({"version":1,"mutation_id":self.mutation_id,"captured_at":self.captured_at,
            "orders":self.orders,"aggregates_before":self.aggregates_before,
            "aggregates_after":self.aggregates_after,"targets":self.targets})
    }
}

pub(super) async fn append(
    conn: &mut PgConnection,
    event: &ParticipantEvent,
    evidence: &ProducerEvidence,
) -> Result<Uuid, MarketWriteError> {
    let mut participants = event.participant_org_ids.clone();
    participants.sort_unstable();
    participants.dedup();
    // These are the accepted stream envelope bounds, independent of internal evidence.
    if participants.is_empty() || participants.len() > 256 || event.event_type.is_empty()
        || event.event_type.len() > 80
        || !event.event_type.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b,b'_'|b'-'|b'.'))
        || event.aggregate_type.is_empty() || event.aggregate_id.is_empty()
    {
        return Err(invalid_event());
    }
    let payload = event.payload();
    if serde_json::to_vec(&payload).map_err(|_| invalid_event())?.len() > 256 * 1024 {
        return Err(invalid_event());
    }
    super::transaction::append_market_effect(conn, event, &participants, &payload,
        &evidence.database_value(), evidence.mutation_id, evidence.captured_at).await?;
    Ok(evidence.mutation_id)
}

fn invalid_event() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
