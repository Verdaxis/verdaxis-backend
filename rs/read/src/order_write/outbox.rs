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

/// Explicit participant activity fields; never an internal row or target snapshot.
#[derive(Clone, Debug)]
pub struct OrderActivity {
    pub id: Uuid,
    pub side: String,
    pub product_name: String,
    pub fuel_type: String,
    pub region: String,
    pub source_kind: String,
    pub scope: String,
    pub demo_status: String,
    pub is_demo_listing: bool,
    pub unknown_count: u32,
    /// Absent for a product outside the market policy.
    pub market_product: Option<String>,
    /// Absent for an order without a delivery point.
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub delivery_point: Option<String>,
}
impl OrderActivity {
    fn payload(&self) -> Value {
        json!({"id":self.id,"side":self.side,"product_name":self.product_name,"fuel_type":self.fuel_type,
            "region":self.region,"source_kind":self.source_kind,"scope":self.scope,"demo_status":self.demo_status,
            "is_demo_listing":self.is_demo_listing,"unknown_count":self.unknown_count,"market_product":self.market_product,
            "delivery_point_id":self.delivery_point_id,"availability_window":self.availability_window,
            "observed_at":self.observed_at,"delivery_point":self.delivery_point})
    }
}

/// Provenance fields common to the two matching activity projections.
#[derive(Clone, Debug)]
pub struct MatchActivity {
    pub source_kind: String,
    pub scope: String,
    pub demo_status: String,
    pub unknown_count: u32,
    pub market_product: Option<String>,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
    pub observed_at: DateTime<Utc>,
}
impl MatchActivity {
    fn payload(&self) -> Value {
        json!({"source_kind":self.source_kind,"scope":self.scope,"demo_status":self.demo_status,
            "unknown_count":self.unknown_count,"market_product":self.market_product,
            "delivery_point_id":self.delivery_point_id,"availability_window":self.availability_window,
            "observed_at":self.observed_at})
    }
}

/// Explicit transport projection. Subscription identities cannot enter this type.
#[derive(Clone, Debug)]
pub enum ParticipantFields {
    TradeCreated {
        activity: MatchActivity,
        product_id: Option<Uuid>,
        id: Uuid,
        status: String,
        quantity: String,
        price: String,
        product_name: String,
        fuel_type: String,
        region: String,
    },
    TradeAutoMatched {
        activity: MatchActivity,
        trade_id: Uuid,
        product_id: Uuid,
        product_name: String,
        fuel_type: String,
        quantity: String,
        price: String,
        is_anonymous: bool,
    },
    OrdersMatched {
        activity: MatchActivity,
        order_id: Uuid,
        matches: usize,
        is_demo_listing: bool,
        delivery_point: Option<String>,
    },
    OrderCancelled(OrderActivity),
    OrderUpdated(OrderActivity),
    OrderCreated {
        activity: OrderActivity,
        price: String,
        quantity: String,
    },
    OrderExpired {
        id: Uuid,
    },
    Inventory {
        id: Uuid,
    },
}

impl ParticipantEvent {
    pub(super) fn payload(&self) -> Value {
        let mut value = match &self.fields {
            ParticipantFields::TradeCreated {
                activity,
                product_id,
                id,
                status,
                quantity,
                price,
                product_name,
                fuel_type,
                region,
            } => {
                let mut value = activity.payload();
                value["product_id"] = json!(product_id);
                value["id"] = json!(id);
                value["status"] = json!(status);
                value["quantity"] = json!(quantity);
                value["price"] = json!(price);
                value["product_name"] = json!(product_name);
                value["fuel_type"] = json!(fuel_type);
                value["region"] = json!(region);
                value
            }
            ParticipantFields::TradeAutoMatched {
                activity,
                trade_id,
                product_id,
                product_name,
                fuel_type,
                quantity,
                price,
                is_anonymous,
            } => {
                let mut value = activity.payload();
                value["trade_id"] = json!(trade_id);
                value["product_id"] = json!(product_id);
                value["product_name"] = json!(product_name);
                value["fuel_type"] = json!(fuel_type);
                value["quantity"] = json!(quantity);
                value["price"] = json!(price);
                value["is_anonymous"] = json!(is_anonymous);
                value
            }
            ParticipantFields::OrdersMatched {
                activity,
                order_id,
                matches,
                is_demo_listing,
                delivery_point,
            } => {
                let mut value = activity.payload();
                value["order_id"] = json!(order_id);
                value["matches"] = json!(matches);
                value["is_demo_listing"] = json!(is_demo_listing);
                value["delivery_point"] = json!(delivery_point);
                value
            }
            ParticipantFields::OrderCancelled(activity) => activity.payload(),
            ParticipantFields::OrderUpdated(activity) => activity.payload(),
            ParticipantFields::OrderCreated {
                activity,
                price,
                quantity,
            } => {
                let mut value = activity.payload();
                value["price"] = json!(price);
                value["quantity"] = json!(quantity);
                value
            }
            ParticipantFields::OrderExpired { id } => json!({"id":id,"status":"EXPIRED"}),
            ParticipantFields::Inventory { id } => json!({"id":id}),
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
    if participants.is_empty()
        || participants.len() > 256
        || event.event_type.is_empty()
        || event.event_type.len() > 80
        || !event
            .event_type
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        || event.aggregate_type.is_empty()
        || event.aggregate_id.is_empty()
    {
        return Err(invalid_event());
    }
    let payload = event.payload();
    if serde_json::to_vec(&payload)
        .map_err(|_| invalid_event())?
        .len()
        > 256 * 1024
    {
        return Err(invalid_event());
    }
    let event_id = Uuid::new_v4();
    super::transaction::append_market_effect(
        conn,
        event,
        &participants,
        &payload,
        &evidence.database_value(),
        event_id,
        evidence.captured_at,
    )
    .await?;
    Ok(event_id)
}

fn invalid_event() -> MarketWriteError {
    MarketWriteError::new(MarketErrorCode::ValidationFailed)
}
