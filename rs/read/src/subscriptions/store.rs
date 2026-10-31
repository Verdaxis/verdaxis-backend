use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::fees::{SubscriptionTier, validate_rate};
use crate::{
    ApiError,
    business::{error::refusal, transaction::AttemptError},
};

fn invalid() -> ApiError {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_FAILED",
        "Invalid subscription update",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeUpdate {
    Omitted,
    Set(Option<Decimal>),
}

#[derive(Debug, Clone)]
pub struct SubscriptionUpdate {
    pub tier: SubscriptionTier,
    pub fee: FeeUpdate,
}

impl SubscriptionUpdate {
    pub fn parse(value: Value) -> Result<Self, ApiError> {
        let object = value.as_object().ok_or_else(invalid)?;
        let tier = object
            .get("tier")
            .and_then(Value::as_str)
            .and_then(|value| SubscriptionTier::parse(value).ok())
            .ok_or_else(invalid)?;
        let fee = match object.get("seller_fee_per_mt_usd") {
            None => FeeUpdate::Omitted,
            Some(Value::Null) => FeeUpdate::Set(None),
            Some(value) => {
                let raw = match value {
                    Value::String(value) => value.clone(),
                    Value::Number(value) => value.to_string(),
                    _ => return Err(invalid()),
                };
                // Parse exact text before SQL. Never convert through floating point.
                if raw.is_empty()
                    || !raw
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || byte == b'.')
                {
                    return Err(invalid());
                }
                let rate = Decimal::from_str_exact(&raw).map_err(|_| invalid())?;
                FeeUpdate::Set(Some(validate_rate(rate).map_err(|_| invalid())?))
            }
        };
        Ok(Self { tier, fee })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Page {
    limit: i64,
    offset: i64,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            limit: 100,
            offset: 0,
        }
    }
}
impl Page {
    pub fn new(limit: i64, offset: i64) -> Result<Self, ApiError> {
        if !(1..=500).contains(&limit) || !(0..=10_000).contains(&offset) {
            return Err(invalid());
        }
        Ok(Self { limit, offset })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Subscription {
    pub id: Uuid,
    pub org_id: Uuid,
    pub tier: SubscriptionTier,
    pub started_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub seller_fee_per_mt_usd: Option<Decimal>,
}

fn decode(row: sqlx::postgres::PgRow) -> Result<Subscription, AttemptError> {
    let tier: String = row.try_get("tier")?;
    Ok(Subscription {
        id: row.try_get("id")?,
        org_id: row.try_get("org_id")?,
        tier: SubscriptionTier::parse(&tier).map_err(|_| {
            refusal(
                StatusCode::CONFLICT,
                "SUBSCRIPTION_TIER_INVALID",
                "Stored subscription tier is invalid",
            )
        })?,
        started_at: row.try_get("started_at")?,
        expires_at: row.try_get("expires_at")?,
        is_active: row.try_get("is_active")?,
        seller_fee_per_mt_usd: row.try_get("seller_fee_per_mt_usd")?,
    })
}

pub async fn find(
    conn: &mut PgConnection,
    org: Uuid,
) -> Result<Option<Subscription>, AttemptError> {
    sqlx::query("SELECT id,org_id,tier,started_at,expires_at,is_active,seller_fee_per_mt_usd FROM subscriptions WHERE org_id=$1")
        .bind(org).fetch_optional(conn).await?.map(decode).transpose()
}

pub async fn list(
    conn: &mut PgConnection,
    page: Page,
) -> Result<(Vec<Subscription>, bool), AttemptError> {
    let rows = sqlx::query("SELECT id,org_id,tier,started_at,expires_at,is_active,seller_fee_per_mt_usd FROM subscriptions ORDER BY org_id,id LIMIT $1 OFFSET $2")
        .bind(page.limit + 1).bind(page.offset).fetch_all(conn).await?;
    let truncated = rows.len() > page.limit as usize;
    let rows = rows
        .into_iter()
        .take(page.limit as usize)
        .map(decode)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((rows, truncated))
}

pub async fn require_organization(conn: &mut PgConnection, org: Uuid) -> Result<(), AttemptError> {
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM organizations WHERE id=$1)")
        .bind(org)
        .fetch_one(conn)
        .await?
    {
        return Err(refusal(
            StatusCode::NOT_FOUND,
            "ORGANIZATION_NOT_FOUND",
            "Organization not found",
        )
        .into());
    }
    Ok(())
}

/// Caller already holds organization then actor locks. A competing Python
/// bootstrap may not hold the organization lock; the unique key still arbitrates.
pub async fn bootstrap(conn: &mut PgConnection, org: Uuid) -> Result<Subscription, AttemptError> {
    require_organization(conn, org).await?;
    sqlx::query("INSERT INTO subscriptions(id,org_id,tier,is_active) VALUES($1,$2,'free',true) ON CONFLICT(org_id) DO NOTHING")
        .bind(Uuid::new_v4()).bind(org).execute(&mut *conn).await?;
    find(conn, org)
        .await?
        .ok_or_else(|| sqlx::Error::RowNotFound.into())
}

/// Atomic upsert also handles a Python insert after the caller's preview. An
/// omitted fee preserves the winner's fee; explicit null clears it.
pub async fn update(
    conn: &mut PgConnection,
    org: Uuid,
    patch: &SubscriptionUpdate,
) -> Result<Subscription, AttemptError> {
    require_organization(conn, org).await?;
    let (provided, fee) = match patch.fee {
        FeeUpdate::Omitted => (false, None),
        FeeUpdate::Set(value) => (true, value),
    };
    let row = sqlx::query("INSERT INTO subscriptions(id,org_id,tier,is_active,seller_fee_per_mt_usd) VALUES($1,$2,$3,true,$4) ON CONFLICT(org_id) DO UPDATE SET tier=EXCLUDED.tier,seller_fee_per_mt_usd=CASE WHEN $5 THEN EXCLUDED.seller_fee_per_mt_usd ELSE subscriptions.seller_fee_per_mt_usd END RETURNING id,org_id,tier,started_at,expires_at,is_active,seller_fee_per_mt_usd")
        .bind(Uuid::new_v4()).bind(org).bind(patch.tier.as_str()).bind(fee).bind(provided).fetch_one(conn).await?;
    let row = decode(row)?;
    if row.tier == SubscriptionTier::Enterprise && row.seller_fee_per_mt_usd.is_none() {
        return Err(refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ENTERPRISE_FEE_REQUIRED",
            "Enterprise requires a negotiated seller fee",
        )
        .into());
    }
    Ok(row)
}
