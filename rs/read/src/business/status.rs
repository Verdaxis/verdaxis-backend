use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    Pending,
    Approved,
    Rejected,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }
}

pub struct StatusFact {
    pub id: Uuid,
    pub user_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub role: Option<String>,
    pub from: Option<AccountStatus>,
    pub to: AccountStatus,
    pub at: DateTime<Utc>,
}

/// The caller holds the user lock and owns the status update and completion.
pub async fn append_status_transition(
    conn: &mut PgConnection,
    fact: &StatusFact,
) -> Result<Option<Uuid>, sqlx::Error> {
    if fact.from == Some(fact.to) {
        return Ok(None);
    }
    sqlx::query("INSERT INTO user_status_transitions (id,user_id,organization_id,role,from_status,to_status,effective_at,provenance) VALUES($1,$2,$3,$4,$5,$6,$7,'workflow')")
        .bind(fact.id)
        .bind(fact.user_id)
        .bind(fact.organization_id)
        .bind(&fact.role)
        .bind(fact.from.map(AccountStatus::as_str))
        .bind(fact.to.as_str())
        .bind(fact.at)
        .execute(conn)
        .await?;
    Ok(Some(fact.id))
}
