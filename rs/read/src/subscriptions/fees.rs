use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionTier {
    Free,
    Standard,
    Enterprise,
}

impl SubscriptionTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Standard => "standard",
            Self::Enterprise => "enterprise",
        }
    }

    pub fn parse(value: &str) -> Result<Self, FeeError> {
        match value {
            "free" => Ok(Self::Free),
            "standard" => Ok(Self::Standard),
            "enterprise" => Ok(Self::Enterprise),
            _ => Err(FeeError::InvalidTier),
        }
    }
}

#[derive(Debug)]
pub enum FeeError {
    Database(sqlx::Error),
    InvalidTier,
    InvalidRate,
    MissingNegotiatedRate,
}

impl From<sqlx::Error> for FeeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl std::fmt::Display for FeeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Database(_) => "Fee storage unavailable",
            Self::InvalidTier => "Subscription tier is invalid",
            Self::InvalidRate => "Fee rate is invalid",
            Self::MissingNegotiatedRate => "Enterprise fee is not configured",
        })
    }
}
impl std::error::Error for FeeError {}

pub fn validate_rate(rate: Decimal) -> Result<Decimal, FeeError> {
    if rate < Decimal::ZERO || rate > Decimal::from(100_000) || rate.scale() > 2 {
        return Err(FeeError::InvalidRate);
    }
    Ok(rate)
}

#[derive(Debug, Clone)]
pub struct FeeConfig {
    pilot: Decimal,
    professional: Decimal,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            pilot: Decimal::from(2),
            professional: Decimal::new(150, 2),
        }
    }
}

impl FeeConfig {
    pub fn new(pilot: Decimal, professional: Decimal) -> Result<Self, FeeError> {
        Ok(Self {
            pilot: validate_rate(pilot)?,
            professional: validate_rate(professional)?,
        })
    }

    pub fn schedule(&self) -> FeeSchedule {
        FeeSchedule {
            currency: "USD",
            buyer_fee_per_mt_usd: Decimal::ZERO.to_string(),
            seller_fee_per_mt_usd: std::collections::BTreeMap::from([
                ("free", Some(self.pilot.to_string())),
                ("standard", Some(self.professional.to_string())),
                ("enterprise", None),
            ]),
        }
    }

    fn snapshot(
        &self,
        tier: SubscriptionTier,
        negotiated: Option<Decimal>,
    ) -> Result<FeeSnapshot, FeeError> {
        let rate = match tier {
            SubscriptionTier::Free => self.pilot,
            SubscriptionTier::Standard => self.professional,
            SubscriptionTier::Enterprise => negotiated.ok_or(FeeError::MissingNegotiatedRate)?,
        };
        Ok(FeeSnapshot {
            plan: tier,
            seller_fee_per_mt_usd: validate_rate(rate)?,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct FeeSchedule {
    pub currency: &'static str,
    pub buyer_fee_per_mt_usd: String,
    pub seller_fee_per_mt_usd: std::collections::BTreeMap<&'static str, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSnapshot {
    pub plan: SubscriptionTier,
    pub seller_fee_per_mt_usd: Decimal,
}

/// Resolve on the trade creator's connection. No insert, row lock, commit, or
/// provider I/O: the caller freezes this value when it inserts the trade.
pub async fn resolve_seller_fee(
    conn: &mut PgConnection,
    seller_org: Uuid,
    now: DateTime<Utc>,
    config: &FeeConfig,
) -> Result<FeeSnapshot, FeeError> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT tier,is_active,expires_at,seller_fee_per_mt_usd FROM subscriptions WHERE org_id=$1",
    )
    .bind(seller_org)
    .fetch_optional(conn)
    .await?;
    let Some(row) = row else {
        return config.snapshot(SubscriptionTier::Free, None);
    };
    let active: bool = row.try_get("is_active")?;
    let expiry: Option<DateTime<Utc>> = row.try_get("expires_at")?;
    if !active || expiry.is_some_and(|expiry| expiry <= now) {
        return config.snapshot(SubscriptionTier::Free, None);
    }
    config.snapshot(
        SubscriptionTier::parse(row.try_get::<&str, _>("tier")?)?,
        row.try_get("seller_fee_per_mt_usd")?,
    )
}
