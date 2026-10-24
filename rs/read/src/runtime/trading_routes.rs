//! Public route adapter. Unknown temporal facts never become a guessed TTL.
use crate::runtime::admission::run_trading_read;
use crate::{
    ApiError, AppState, Deadline, Json, RollingWindows, TemporalFacts, TradingClock, TradingFill,
};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row};
use std::{future::Future, pin::Pin, time::SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicTradingKey {
    pub route: &'static str,
    pub query: String,
}

pub fn capture(state: &AppState) -> Result<TradingClock, ApiError> {
    let clock = state.trading_read_observer.now();
    #[cfg(feature = "contract-tests")]
    if state.config.environment == "test" && std::env::var_os("RUST_CONTRACT_CLOCK").is_some() {
        return Ok(TradingClock::new(
            clock.monotonic,
            super::super::routes::prices::captured_now(state)?.into(),
        ));
    }
    let _ = state;
    Ok(clock)
}

pub async fn read<T>(
    state: &AppState,
    route: &'static str,
    query: String,
    family: &'static str,
    clock: TradingClock,
    hours: Option<i64>,
    load: impl for<'c> FnOnce(
        &'c mut PgConnection,
    ) -> Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'c>>
    + Send
    + 'static,
) -> Result<T, ApiError>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send + 'static,
{
    // Preserve the old SQL and transaction path exactly when the switch is off.
    if state.trading_cache.is_none() {
        return crate::run_admitted(state, load).await;
    }
    let cache = state.public_trading_cache.clone();
    state.trading_caches.register(&cache);
    // Alias windows and map compaction depend on the captured UTC date.
    let query = format!("{}:{query}", DateTime::<Utc>::from(clock.wall).date_naive());
    run_trading_read(
        state,
        &cache,
        PublicTradingKey { route, query },
        family,
        clock,
        move |connection, clock| {
            Box::pin(async move {
                let mut facts = facts(connection, family, &clock, hours).await?;
                let value = load(connection).await?;
                // The map loader uses None for its existing unhandled-error path.
                // A null result is never retained as a successful public payload.
                if serde_json::to_value(&value)
                    .map_err(|_| ApiError::internal())?
                    .is_null()
                {
                    facts = TemporalFacts::default();
                }
                Ok(TradingFill { value, facts })
            })
        },
    )
    .await
}

/// Global minima are conservative: rows outside the requested slice can shorten
/// storage, but pagination, ranking and excluded rows cannot hide a departure.
async fn facts(
    connection: &mut PgConnection,
    family: &str,
    clock: &TradingClock,
    hours: Option<i64>,
) -> Result<TemporalFacts, ApiError> {
    let now: DateTime<Utc> = clock.wall.into();
    if family == "prices_fixed" {
        return Ok(TemporalFacts::proven_written_only());
    }
    let mut facts = TemporalFacts::default();
    if family == "orderbook" || family == "curves" {
        let row = sqlx::query("SELECT (SELECT min(expires_at) FROM orderbook_orders WHERE expires_at > $1) AS expiry, (SELECT min(expires_at) FROM kyc_execution_grace WHERE expires_at > $1) AS grace")
            .bind(now).fetch_one(&mut *connection).await.map_err(|_| ApiError::database())?;
        let deadline = |name| -> Result<Deadline, ApiError> {
            let value: Option<DateTime<Utc>> =
                row.try_get(name).map_err(|_| ApiError::internal())?;
            Ok(value.map_or(Deadline::None, |value| Deadline::At(value.into())))
        };
        facts.order_expiry = deadline("expiry")?;
        facts.kyc_grace = deadline("grace")?;
        // Certificate strings use lexicographic Singapore dates. Midnight is a
        // sound bound even for legacy non-date strings; do not parse them here.
        facts.declaration_date = clock
            .next_singapore_midnight()
            .map_or(Deadline::Unknown, Deadline::At);
    }
    if family == "prices" {
        let Some(hours) = hours else {
            return Ok(facts);
        };
        facts.rolling_windows = rolling(connection, now, hours).await?;
    }
    if family == "curves" {
        // Include every potential selected observation, not only the current
        // winner: expiry can expose a lower-ranked evidence class.
        let departure: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT min(departure) FROM (\
             SELECT confirmed_at + interval '30 days' AS departure FROM trades \
             UNION ALL SELECT observed_at + interval '7 days' FROM market_indications \
             UNION ALL SELECT observed_at + interval '30 days' FROM fair_price_bands \
             UNION ALL SELECT observed_at + interval '90 days' FROM physical_stems \
             UNION ALL SELECT confirmed_at + interval '7 days' FROM trades \
             UNION ALL SELECT observed_at + interval '7 days' FROM fair_price_bands \
             UNION ALL SELECT observed_at + interval '7 days' FROM physical_stems \
             UNION ALL SELECT coalesce(updated_at, created_at) + interval '7 days' FROM benchmarks \
             UNION ALL SELECT coalesce(updated_at, created_at) + interval '7 days' FROM orderbook_orders\
             ) AS boundaries WHERE departure >= $1")
            .bind(now).fetch_one(&mut *connection).await.map_err(|_| ApiError::database())?;
        facts.rolling_windows = if departure == Some(now) {
            RollingWindows::Unknown
        } else {
            RollingWindows::Derived(departure.map(SystemTime::from).into_iter().collect())
        };
        // Persisted benchmarks have no time-expiry predicate; writes invalidate.
        facts.benchmark_expiry = Deadline::None;
        let utc_midnight = now
            .date_naive()
            .succ_opt()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|value| SystemTime::from(value.and_utc()));
        facts.calendar_window = match (utc_midnight, clock.next_singapore_midnight()) {
            (Some(utc), Some(singapore)) => Deadline::At(utc.min(singapore)),
            _ => Deadline::Unknown,
        };
    }
    Ok(facts)
}

async fn rolling(
    connection: &mut PgConnection,
    now: DateTime<Utc>,
    hours: i64,
) -> Result<RollingWindows, ApiError> {
    let oldest: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT min(confirmed_at) FROM trades WHERE confirmed_at >= $1 - ($2::bigint * interval '1 hour')")
        .bind(now).bind(hours).fetch_one(connection).await.map_err(|_| ApiError::database())?;
    match oldest {
        None => Ok(RollingWindows::Derived(vec![])),
        Some(oldest) => match oldest.checked_add_signed(chrono::Duration::hours(hours)) {
            Some(departure) if departure > now => {
                Ok(RollingWindows::Derived(vec![SystemTime::from(departure)]))
            }
            _ => Ok(RollingWindows::Unknown),
        },
    }
}

/// Only `generated_at` is request-owned. Other timestamps describe evidence.
pub fn generated_at(value: &mut Json, now: DateTime<Utc>, snapshot: bool) {
    match value {
        Json::Object(fields) => {
            for (name, value) in fields {
                if name == "generated_at" {
                    *value = Json::String(if snapshot {
                        crate::routes::ports::pydantic_datetime(now)
                    } else {
                        crate::routes::ports::utc_rfc3339(now, "Z")
                    });
                } else {
                    generated_at(value, now, snapshot);
                }
            }
        }
        Json::Array(items) => {
            for value in items {
                generated_at(value, now, snapshot);
            }
        }
        _ => {}
    }
}
