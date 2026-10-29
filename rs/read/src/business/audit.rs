use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

pub struct AuditFact {
    pub id: Uuid,
    pub actor_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub changes: Option<Value>,
    pub ip_address: Option<String>,
    pub request_id: Option<String>,
    pub at: DateTime<Utc>,
}

/// Append on the caller's transaction. The caller owns commit and rollback.
pub async fn append_audit(conn: &mut PgConnection, fact: &AuditFact) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_logs (id,user_id,action,resource_type,resource_id,changes,ip_address,request_id,timestamp) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(fact.id)
        .bind(fact.actor_id)
        .bind(&fact.action)
        .bind(&fact.resource_type)
        .bind(&fact.resource_id)
        .bind(&fact.changes)
        .bind(&fact.ip_address)
        .bind(&fact.request_id)
        .bind(fact.at)
        .execute(conn)
        .await?;
    Ok(())
}
