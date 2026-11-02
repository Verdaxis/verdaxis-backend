//! The market write serving composition.
//!
//! The 43 market operations (orders, inventory, trades, trade lifecycle,
//! freeze administration, commissions, match suggestions and market support)
//! are mounted only when the Phase 2 verification settings exist, the same
//! condition as the Phase 4 composition. Mounting does not route traffic:
//! Caddy keeps every operation on Python until the reviewed market switch
//! imports `deploy/caddy/rust-market-write.caddy`.
//!
//! One [`MarketWriteSettings`] value, built from the deployed environment at
//! startup, configures every component. A malformed key refuses the start, as
//! Python's settings model refuses it. No pool or background task is added:
//! every component uses the shared application pool, and the conversion
//! sender only schedules bounded best-effort deliveries after a commit.
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use axum::Router;
use tokio::sync::Semaphore;

use crate::analytics::sender::AnalyticsSender;
use crate::auth::limits::SharedLimits;
use crate::market_support::SupportSettings;
use crate::providers::umami::{UmamiConfig, UmamiPolicy};
use crate::routes::market_write::{
    amend, freeze, inventory, matchmaking, order_create, orders, trade_lifecycle, trades,
};
use crate::subscriptions::FeeConfig;
use crate::{AppState, Config};

/// Python's bounds for `MARKET_SUPPORT_MAX_TTL_HOURS` (`app/config.py`).
const SUPPORT_MAX_TTL_HOURS: RangeInclusive<i64> = 1..=720;
/// Python's bounds for `MARKET_SUPPORT_CONTEXT_TTL_MINUTES` (`app/config.py`).
const SUPPORT_CONTEXT_TTL_MINUTES: RangeInclusive<i64> = 1..=1440;
/// Python's placeholder consent metadata. A deployed environment that enables
/// market support must replace both values.
const PLACEHOLDER_CONSENT_VERSION: &str = "v1";
const PLACEHOLDER_CONSENT_REFERENCE: &str = "market-support/v1";
/// Environments where the placeholder consent metadata is refused.
const DEPLOYED_ENVIRONMENTS: [&str; 2] = ["staging", "production"];
/// Conversion events delivered at once. A create can schedule one order event
/// plus one trade event per inline match; the sender drops (never queues)
/// events beyond this bound.
const ANALYTICS_CONCURRENCY: usize = 8;
/// Concurrent administrator commands per component (freeze, support and
/// commission each hold their own bound of the same size).
const ADMIN_COMMAND_PERMITS: usize = 4;

/// The seven Python market settings and the conversion collector, as the
/// deployed environment states them. Field defaults are Python's defaults.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct MarketWriteOptions {
    /// `AUTO_MATCHING_ENABLED`: match on insert after create, amend and publish.
    pub auto_matching_enabled: bool,
    /// `MARKET_SUPPORT_ENABLED`.
    pub support_enabled: bool,
    /// `MARKET_SUPPORT_MAX_TTL_HOURS`. Only the retired legacy authorization
    /// routes read it in Python; it is validated so both services accept the
    /// same environment.
    pub support_max_ttl_hours: i64,
    /// `MARKET_SUPPORT_CONTEXT_TTL_MINUTES`.
    pub support_context_ttl_minutes: i64,
    /// `MARKET_SUPPORT_CONSENT_VERSION`.
    pub support_consent_version: String,
    /// `MARKET_SUPPORT_CONSENT_REFERENCE`.
    pub support_consent_reference: String,
    /// `MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS`: comma-separated; entries that
    /// are not UUIDs are ignored, as in Python.
    pub support_bootstrap_admin_user_ids: String,
    /// `UMAMI_ENABLED`, `UMAMI_BASE_URL` and `UMAMI_WEBSITE_ID`: where the
    /// post-commit conversion events go. Disabled or incomplete means none.
    pub analytics_enabled: bool,
    pub analytics_base_url: Option<String>,
    pub analytics_website_id: Option<String>,
}

impl Default for MarketWriteOptions {
    fn default() -> Self {
        Self {
            auto_matching_enabled: true,
            support_enabled: false,
            support_max_ttl_hours: 168,
            support_context_ttl_minutes: 60,
            support_consent_version: PLACEHOLDER_CONSENT_VERSION.into(),
            support_consent_reference: PLACEHOLDER_CONSENT_REFERENCE.into(),
            support_bootstrap_admin_user_ids: String::new(),
            analytics_enabled: false,
            analytics_base_url: None,
            analytics_website_id: None,
        }
    }
}

impl MarketWriteOptions {
    /// Read the keys from the flat deployed environment. An absent key keeps
    /// Python's default; a present key that Python would refuse refuses here.
    pub fn from_deployed_environment(
        environment: &HashMap<String, String>,
        deployment: &str,
    ) -> Result<Self, String> {
        let defaults = Self::default();
        let text = |key: &str, default: &str| {
            environment
                .get(key)
                .cloned()
                .unwrap_or_else(|| default.to_owned())
        };
        let optional = |key: &str| {
            environment
                .get(key)
                .filter(|value| !value.trim().is_empty())
                .cloned()
        };
        let options = Self {
            auto_matching_enabled: deployed_bool(
                environment,
                "AUTO_MATCHING_ENABLED",
                defaults.auto_matching_enabled,
            )?,
            support_enabled: deployed_bool(
                environment,
                "MARKET_SUPPORT_ENABLED",
                defaults.support_enabled,
            )?,
            support_max_ttl_hours: deployed_integer(
                environment,
                "MARKET_SUPPORT_MAX_TTL_HOURS",
                defaults.support_max_ttl_hours,
            )?,
            support_context_ttl_minutes: deployed_integer(
                environment,
                "MARKET_SUPPORT_CONTEXT_TTL_MINUTES",
                defaults.support_context_ttl_minutes,
            )?,
            support_consent_version: text(
                "MARKET_SUPPORT_CONSENT_VERSION",
                PLACEHOLDER_CONSENT_VERSION,
            ),
            support_consent_reference: text(
                "MARKET_SUPPORT_CONSENT_REFERENCE",
                PLACEHOLDER_CONSENT_REFERENCE,
            ),
            support_bootstrap_admin_user_ids: text("MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS", ""),
            analytics_enabled: deployed_bool(environment, "UMAMI_ENABLED", false)?,
            analytics_base_url: optional("UMAMI_BASE_URL"),
            analytics_website_id: optional("UMAMI_WEBSITE_ID"),
        };
        options.validated(deployment)
    }

    /// Python's bounds and its activation rule for the consent metadata. The
    /// consent values are kept trimmed, as Python stores them.
    pub fn validated(mut self, deployment: &str) -> Result<Self, String> {
        if !SUPPORT_MAX_TTL_HOURS.contains(&self.support_max_ttl_hours) {
            return Err("MARKET_SUPPORT_MAX_TTL_HOURS must be from 1 to 720".into());
        }
        if !SUPPORT_CONTEXT_TTL_MINUTES.contains(&self.support_context_ttl_minutes) {
            return Err("MARKET_SUPPORT_CONTEXT_TTL_MINUTES must be from 1 to 1440".into());
        }
        if !self.support_enabled {
            return Ok(self);
        }
        let version = self.support_consent_version.trim().to_owned();
        let reference = self.support_consent_reference.trim().to_owned();
        if version.is_empty() || reference.is_empty() {
            return Err(
                "Market Support consent version and reference are required when enabled".into(),
            );
        }
        if DEPLOYED_ENVIRONMENTS.contains(&deployment)
            && (version == PLACEHOLDER_CONSENT_VERSION
                || reference == PLACEHOLDER_CONSENT_REFERENCE)
        {
            return Err("Market Support consent metadata must be explicitly configured".into());
        }
        self.support_consent_version = version;
        self.support_consent_reference = reference;
        Ok(self)
    }
}

/// Pydantic's boolean spellings. An absent key takes Python's default.
fn deployed_bool(
    environment: &HashMap<String, String>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match environment.get(key) {
        None => Ok(default),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "y" | "yes" | "1" | "on" => Ok(true),
            "false" | "f" | "n" | "no" | "0" | "off" => Ok(false),
            _ => Err(format!("{key} must be true or false")),
        },
    }
}

/// A decimal integer, surrounding blanks allowed, as Pydantic reads it.
fn deployed_integer(
    environment: &HashMap<String, String>,
    key: &str,
    default: i64,
) -> Result<i64, String> {
    match environment.get(key) {
        None => Ok(default),
        Some(value) => value
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{key} must be an integer")),
    }
}

/// The validated settings every market component receives.
#[derive(Clone)]
pub struct MarketWriteSettings {
    auto_matching_enabled: bool,
    kyc_execution_gate_enabled: bool,
    support: SupportSettings,
    consent_version: String,
    consent_reference: String,
    fees: FeeConfig,
    analytics: AnalyticsSender,
}

impl MarketWriteSettings {
    /// Build the settings once, at startup. Every refusal here stops the start.
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let options = config
            .phase2
            .market_write
            .clone()
            .validated(&config.environment)?;
        let support = SupportSettings::new(
            options.support_enabled,
            &options.support_bootstrap_admin_user_ids,
        )
        .with_context_ttl_minutes(options.support_context_ttl_minutes)
        .map_err(|_| "MARKET_SUPPORT_CONTEXT_TTL_MINUTES must be from 1 to 1440".to_owned())?;
        let collector = UmamiConfig {
            enabled: options.analytics_enabled,
            base_url: options.analytics_base_url.clone(),
            website_id: options.analytics_website_id.clone(),
            api_username: None,
            api_password: None,
            policy: UmamiPolicy::default(),
            catalog: Arc::default(),
        };
        let analytics = AnalyticsSender::new(
            &collector,
            config.phase2.monitor_token.clone(),
            config.phase2.frontend_url.as_deref().unwrap_or_default(),
            ANALYTICS_CONCURRENCY,
        )
        .map_err(str::to_owned)?;
        Ok(Self {
            auto_matching_enabled: options.auto_matching_enabled,
            kyc_execution_gate_enabled: config.kyc_execution_gate_enabled,
            support,
            consent_version: options.support_consent_version,
            consent_reference: options.support_consent_reference,
            fees: seller_fees()?,
            analytics,
        })
    }

    // The contract-test build adds a barrier field that only the default fills.
    #[allow(clippy::needless_update)]
    fn create(&self) -> order_create::CreateSettings {
        order_create::CreateSettings {
            auto_matching_enabled: self.auto_matching_enabled,
            kyc_execution_gate_enabled: self.kyc_execution_gate_enabled,
            support_enabled: self.support.enabled,
            consent_version: self.consent_version.clone(),
            consent_reference: self.consent_reference.clone(),
            fees: self.fees.clone(),
            analytics: Some(self.analytics.clone()),
            ..Default::default()
        }
    }

    fn amend(&self) -> amend::AmendSettings {
        amend::AmendSettings {
            auto_matching_enabled: self.auto_matching_enabled,
            kyc_execution_gate_enabled: self.kyc_execution_gate_enabled,
            support_enabled: self.support.enabled,
            fees: self.fees.clone(),
            analytics: Some(self.analytics.clone()),
        }
    }
}

/// The seller fee rates the deployed environment projects, read the same way
/// as the Phase 4 subscription routes read them.
fn seller_fees() -> Result<FeeConfig, String> {
    let rate = |key: &str, default: &str| {
        std::env::var(key)
            .unwrap_or_else(|_| default.into())
            .parse::<rust_decimal::Decimal>()
            .map_err(|_| format!("{key} must be a decimal number"))
    };
    FeeConfig::new(
        rate("SELLER_FEE_PILOT_PER_MT_USD", "2")?,
        rate("SELLER_FEE_PROFESSIONAL_PER_MT_USD", "1.50")?,
    )
    .map_err(|_| "seller fee configuration is invalid".to_owned())
}

/// True when the served router mounts the market operations.
pub fn mounts(app: &AppState) -> bool {
    app.phase2.is_some() && app.market_write.is_some()
}

/// Every market operation, composed into one router. `None` when the Phase 2
/// verification settings are absent, so Caddy keeps the operations on Python.
///
/// `PUT` (amend) and `DELETE` (cancel) on `/api/orderbook/{id}` come from two
/// components; axum merges their method routers on the one path.
pub(crate) fn routes(app: &AppState, limits: SharedLimits) -> Option<Router> {
    let phase2 = app.phase2.as_ref()?;
    let settings = app.market_write.as_ref()?;
    let verification = phase2.verification.clone();
    let pool = app.pools.read.clone();
    let create = settings.create();
    let orders = orders::router_with_create(
        app.clone(),
        verification.clone(),
        verification.clone(),
        create.clone(),
    );
    let amend = amend::router(app.clone(), verification.clone(), settings.amend());
    let inventory =
        inventory::router_with_publication(app.clone(), verification.clone(), create.clone());
    let trades = trades::router(
        app.clone(),
        verification.clone(),
        settings.support.enabled,
        settings.fees.clone(),
        Some(settings.analytics.clone()),
    );
    let lifecycle = trade_lifecycle::router(app.clone(), verification.clone());
    let freeze = freeze::router(freeze::FreezeState::new(pool.clone(), verification.clone()));
    let commissions =
        crate::commission_routes::routes(Arc::new(crate::commission_routes::RouteContext {
            pool: pool.clone(),
            permits: Arc::new(Semaphore::new(ADMIN_COMMAND_PERMITS)),
            settings: Arc::new(verification.clone()),
            limits: limits.clone(),
            policy: app.policy.clone(),
            config: app.config.clone(),
        }));
    let suggestions = matchmaking::routes(Arc::new(matchmaking::MatchmakingAuthority::new(
        verification.clone(),
        limits,
    )))
    .with_state(app.clone());
    let support = crate::market_support::router(crate::market_support::SupportState::new(
        pool,
        Arc::new(verification),
        settings.support.clone(),
        (*app.policy).clone(),
        (*app.config).clone(),
    ));
    Some(
        orders
            .merge(amend)
            .merge(inventory)
            .merge(trades)
            .merge(lifecycle)
            .merge(freeze)
            .merge(commissions)
            .merge(suggestions)
            .merge(support),
    )
}
