//! Trading cache races against disposable PostgreSQL. Nothing here measures
//! speed; every check is a correctness schedule.
//!
//! The mixed schedule is driven from pytest: Python commits a real business
//! change while this process keeps its listener attached but does not consume
//! the wake. A post-commit read must return the newer generation before any
//! event is delivered, and the delivered event must then clear the entry.
#[path = "support/cache_owner.rs"]
mod cache_owner;

use cache_owner::{Record, authority, bind, generation, reset, set_generation};
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use verdaxis_read::{
    ApiError, AppState, Boundary, Config, Deadline, MarketPolicy, RollingWindows, TemporalFacts,
    TradingCache, TradingCacheRuntime, TradingClock, TradingEntry, TradingFill, compiled_sha,
    connect,
    market::cache_dependencies::{
        certificate_boundary, kyc_grace_boundary, order_expiry_boundary, rolling_window_departure,
    },
    run_trading_read,
    runtime::{
        admission::{TradingReadObserver, TradingReadStage},
        cache_authority::{
            AUTHORITY_LEASE, Authority, CacheAuthority, FillTicket, admit_connection,
        },
        cache_events::{CacheEvents, ListenerBudget},
    },
};

/// Every database test rebinds the shared incarnation row and the shared
/// record path, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn budget() -> ListenerBudget {
    ListenerBudget::new(50, 100, 100)
}

fn options(url: &str) -> PgConnectOptions {
    PgConnectOptions::from_str(url).unwrap()
}

fn state_config(url: &str) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "prc_20261018_cache".into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        // The reviewed switch for this test: one budgeted listener slot.
        prepared_budget: Some("{\"cache_listener\":1}".into()),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
    }
}

fn policy() -> MarketPolicy {
    MarketPolicy {
        source_sha: compiled_sha().into(),
        products: vec![],
        points: vec![],
    }
}

async fn writer() -> Option<(String, PgConnection)> {
    let url = std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL").ok()?;
    let connection = PgConnection::connect_with(&options(&url)).await.unwrap();
    Some((url, connection))
}

/// Publish the owner record where the runtime looks for it. A real deploy
/// writes the same file atomically beside the binary.
fn publish_record(runtime: &TradingCacheRuntime, record: &Record) {
    let target = runtime.record_path();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::copy(record.path(), target).unwrap();
}

/// The loaded value is the trading generation row itself, so a hit that
/// survives a commit is immediately visible as the prior number.
async fn read_generation(
    state: &AppState,
    cache: &Arc<TradingCache<&'static str>>,
    runs: Arc<AtomicUsize>,
) -> i64 {
    // The generation row changes only through a write, like a price reference
    // over explicit dates. A moving price summary must derive its window.
    read_family(
        state,
        cache,
        "generation",
        "prices_fixed",
        TemporalFacts::proven_written_only(),
        runs,
    )
    .await
}

/// A runtime-enabled state whose authority is active and whose listener is
/// attached. The returned record keeps the owner's file alive.
async fn admitted_state(
    url: &str,
    writer: &mut PgConnection,
) -> (AppState, Arc<TradingCacheRuntime>, Record) {
    let record = Record::new();
    let identity = authority(1);
    bind(writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(url)).await.unwrap();
    let state = AppState::new(state_config(url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().unwrap().clone();
    publish_record(&runtime, &record);
    runtime.tick(Instant::now()).await;
    (state, runtime, record)
}

fn utc(value: SystemTime) -> chrono::DateTime<chrono::Utc> {
    value.into()
}

fn instant_at(text: &str) -> SystemTime {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&chrono::Utc)
        .into()
}

/// One resting order under the shared order rule's currency predicate
/// (`expires_at > $2`), with the expiry fact derived from the rows it kept.
async fn resting_order(
    connection: &mut PgConnection,
    expires_at: SystemTime,
    clock: &TradingClock,
) -> Result<TradingFill<i64>, ApiError> {
    let (count, earliest): (i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT count(*), min(expires_at) FROM (VALUES ($1::timestamptz)) AS orders(expires_at) \
         WHERE expires_at > $2::timestamptz",
    )
    .bind(utc(expires_at))
    .bind(utc(clock.wall))
    .fetch_one(connection)
    .await
    .map_err(|_| ApiError::database())?;
    let expiry = order_expiry_boundary(earliest.map(SystemTime::from), clock);
    Ok(TradingFill {
        value: count,
        facts: orderbook_facts(expiry, Deadline::None),
    })
}

/// A KYC grace row under the owner-admission predicate (`expires_at > $2`).
async fn grace_owner(
    connection: &mut PgConnection,
    expires_at: SystemTime,
    clock: &TradingClock,
) -> Result<TradingFill<i64>, ApiError> {
    let (count, earliest): (i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT count(*), min(expires_at) FROM (VALUES ($1::timestamptz)) AS grace(expires_at) \
         WHERE expires_at > $2::timestamptz",
    )
    .bind(utc(expires_at))
    .bind(utc(clock.wall))
    .fetch_one(connection)
    .await
    .map_err(|_| ApiError::database())?;
    let grace = kyc_grace_boundary(earliest.map(SystemTime::from), clock);
    Ok(TradingFill {
        value: count,
        facts: orderbook_facts(Deadline::None, grace),
    })
}

/// A FAME certificate under the declaration predicate
/// (`certificate_valid_until >= $3`, the Singapore business date).
async fn certified_ask(
    connection: &mut PgConnection,
    valid_until: chrono::NaiveDate,
    clock: &TradingClock,
) -> Result<TradingFill<i64>, ApiError> {
    let (count, earliest): (i64, Option<String>) = sqlx::query_as(
        "SELECT count(*), min(valid_until) FROM (VALUES ($1::text)) AS certificate(valid_until) \
         WHERE valid_until >= $2::text",
    )
    .bind(valid_until.format("%Y-%m-%d").to_string())
    .bind(clock.singapore_date().format("%Y-%m-%d").to_string())
    .fetch_one(connection)
    .await
    .map_err(|_| ApiError::database())?;
    let earliest = earliest
        .map(|text| chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d"))
        .transpose()
        .map_err(|_| ApiError::internal())?;
    let mut facts = orderbook_facts(Deadline::None, Deadline::None);
    facts.declaration_date = certificate_boundary(earliest, clock);
    Ok(TradingFill {
        value: count,
        facts,
    })
}

/// One trade under a seven-day lookback (`confirmed_at >= $2 - 7 days`, the
/// tape's own predicate), with the departure derived from the rows it kept.
async fn trade_in_window(
    connection: &mut PgConnection,
    confirmed_at: SystemTime,
    clock: &TradingClock,
) -> Result<TradingFill<i64>, ApiError> {
    let (count, oldest): (i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT count(*), min(confirmed_at) FROM (VALUES ($1::timestamptz)) AS trades(confirmed_at) \
         WHERE confirmed_at >= $2::timestamptz - interval '7 days'",
    )
    .bind(utc(confirmed_at))
    .bind(utc(clock.wall))
    .fetch_one(connection)
    .await
    .map_err(|_| ApiError::database())?;
    let rolling_windows = match rolling_window_departure(oldest.map(SystemTime::from), 7, clock) {
        Deadline::At(departure) => RollingWindows::Derived(vec![departure]),
        Deadline::None => RollingWindows::Derived(vec![]),
        Deadline::Unknown => RollingWindows::Unknown,
    };
    Ok(TradingFill {
        value: count,
        facts: TemporalFacts {
            rolling_windows,
            ..TemporalFacts::default()
        },
    })
}

/// Load the generation row as one route family that reports `facts`. `runs`
/// counts loads, so a hit is visible as an unchanged count.
async fn read_family(
    state: &AppState,
    cache: &Arc<TradingCache<&'static str>>,
    key: &'static str,
    family: &'static str,
    facts: TemporalFacts,
    runs: Arc<AtomicUsize>,
) -> i64 {
    let clock = state.trading_read_observer.now();
    run_trading_read(
        state,
        cache,
        key,
        family,
        clock,
        move |connection, _clock| {
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                let value: i64 = sqlx::query_scalar(
                    "SELECT generation FROM public.public_read_cache_generations \
                 WHERE domain = 'trading'",
                )
                .fetch_one(connection)
                .await
                .map_err(|_| ApiError::database())?;
                Ok(TradingFill { value, facts })
            })
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn paused_listener_never_serves_the_prior_generation() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let identity = authority(1);
    bind(&mut writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().expect("explicitly enabled");
    publish_record(runtime, &record);
    runtime.tick(Instant::now()).await;
    let cache = Arc::new(TradingCache::new(16, 1 << 20));
    state.trading_caches.register(&cache);
    let runs = Arc::new(AtomicUsize::new(0));

    let before = read_generation(&state, &cache, runs.clone()).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(cache.len(), 1);

    // The listener stays attached but is not woken: no event is delivered.
    sqlx::query("UPDATE public.products SET is_active = is_active")
        .execute(&mut writer)
        .await
        .unwrap();
    let committed = generation(&mut writer).await;
    assert!(committed > before);

    let after = read_generation(&state, &cache, runs.clone()).await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "a post-commit read must not answer from the prior generation"
    );
    assert_eq!(after, committed);
    assert_ne!(after, before);

    // Deliver the buffered wake through the same loop the service runs.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            runtime.tick(Instant::now()).await;
            if cache.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the delivered event clears the entry");
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

/// Long enough for two reads on a slow host, short enough to wait out. The
/// authority lease (five seconds) is renewed before each read regardless.
const ORDER_LIFETIME: Duration = Duration::from_secs(3);

/// An orderbook route reports only the facts its own query reads. Facts it
/// does not read stay `Unknown`, which must not stop it from being stored.
fn orderbook_facts(order_expiry: Deadline, kyc_grace: Deadline) -> TemporalFacts {
    TemporalFacts {
        order_expiry,
        kyc_grace,
        declaration_date: Deadline::None,
        ..TemporalFacts::default()
    }
}

#[tokio::test]
async fn an_orderbook_entry_serves_only_until_its_own_expiry() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let identity = authority(1);
    bind(&mut writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().expect("explicitly enabled");
    publish_record(runtime, &record);
    runtime.tick(Instant::now()).await;
    let cache = Arc::new(TradingCache::new(16, 1 << 20));
    state.trading_caches.register(&cache);
    let renew = || {
        runtime
            .authority()
            .refresh(runtime.record_path(), Instant::now());
    };

    // The earliest included order expires soon; nothing writes before then.
    let expiry = SystemTime::now() + ORDER_LIFETIME;
    let facts = orderbook_facts(Deadline::At(expiry), Deadline::None);
    let runs = Arc::new(AtomicUsize::new(0));
    renew();
    let first = read_family(
        &state,
        &cache,
        "book",
        "orderbook",
        facts.clone(),
        runs.clone(),
    )
    .await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        cache.len(),
        1,
        "a family is stored with only the facts its query reads"
    );
    renew();
    let hit = read_family(
        &state,
        &cache,
        "book",
        "orderbook",
        facts.clone(),
        runs.clone(),
    )
    .await;
    assert_eq!(runs.load(Ordering::SeqCst), 1, "a live entry is a hit");
    assert_eq!(hit, first);

    // The order leaves the book at its expiry without a write or an event.
    while SystemTime::now() <= expiry {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    renew();
    let expired = read_family(&state, &cache, "book", "orderbook", facts, runs.clone()).await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "an entry past its expiry boundary must load again"
    );
    assert_eq!(expired, first, "no write happened in between");

    // A fact the family does read but could not derive is never stored.
    let unproved = orderbook_facts(Deadline::None, Deadline::Unknown);
    let loads = runs.load(Ordering::SeqCst);
    for _ in 0..2 {
        renew();
        read_family(
            &state,
            &cache,
            "unproved",
            "orderbook",
            unproved.clone(),
            runs.clone(),
        )
        .await;
    }
    assert_eq!(runs.load(Ordering::SeqCst), loads + 2);
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn prices_require_a_known_rolling_departure() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let identity = authority(1);
    bind(&mut writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().unwrap();
    publish_record(runtime, &record);
    runtime.tick(Instant::now()).await;
    let cache = Arc::new(TradingCache::new(4, 1024));
    let runs = Arc::new(AtomicUsize::new(0));
    let facts = TemporalFacts {
        rolling_windows: verdaxis_read::RollingWindows::Unknown,
        ..TemporalFacts::default()
    };
    for _ in 0..2 {
        read_family(
            &state,
            &cache,
            "moving",
            "prices",
            facts.clone(),
            runs.clone(),
        )
        .await;
    }
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "an unknown moving-price boundary must bypass storage"
    );
    assert!(cache.is_empty());
}

/// "No window" is not a derivation for a family whose query always reads a
/// moving window, so neither it nor an unknown departure may be stored.
#[tokio::test]
async fn moving_windows_without_a_derived_departure_bypass_storage() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (state, runtime, _record) = admitted_state(&url, &mut writer).await;
    let mut stored = Vec::new();
    for family in ["prices", "trade_tape"] {
        for (name, rolling_windows) in [
            ("no window", RollingWindows::None),
            ("unknown", RollingWindows::Unknown),
        ] {
            let cache = Arc::new(TradingCache::new(4, 1024));
            let runs = Arc::new(AtomicUsize::new(0));
            let facts = TemporalFacts {
                rolling_windows,
                ..TemporalFacts::proven_written_only()
            };
            for _ in 0..2 {
                runtime
                    .authority()
                    .refresh(runtime.record_path(), Instant::now());
                read_family(
                    &state,
                    &cache,
                    "moving",
                    family,
                    facts.clone(),
                    runs.clone(),
                )
                .await;
            }
            if runs.load(Ordering::SeqCst) != 2 || !cache.is_empty() {
                stored.push(format!("{family}: {name}"));
            }
        }
    }
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert!(
        stored.is_empty(),
        "moving windows stored without a departure: {stored:?}"
    );
}

/// The tape reads `confirmed_at >= now - 7 days`. A trade leaves the tape, and
/// its `total`, when it turns seven days old, with no write. The entry must
/// end there.
#[tokio::test]
async fn a_trade_ages_out_of_the_tape_window_without_a_write() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (mut state, runtime, _record) = admitted_state(&url, &mut writer).await;
    let start = instant_at("2026-10-22T08:00:00Z");
    let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
        Instant::now(),
        start,
    ))));
    state.trading_read_observer = clock.clone();
    // Half a second short of seven days old at the first read.
    let confirmed_at = start - Duration::from_secs(7 * 86_400) + Duration::from_millis(500);
    let initial_generation = generation(&mut writer).await;
    let cache = Arc::new(TradingCache::new(4, 1024));
    let runs = Arc::new(AtomicUsize::new(0));
    let mut totals = Vec::new();
    for read in 0..3 {
        runtime
            .authority()
            .refresh(runtime.record_path(), clock.now().monotonic);
        let loads = runs.clone();
        let total = run_trading_read(
            &state,
            &cache,
            "tape",
            "trade_tape",
            clock.now(),
            move |connection, clock| {
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    trade_in_window(connection, confirmed_at, &clock).await
                })
            },
        )
        .await
        .unwrap();
        totals.push(total);
        if read == 1 {
            clock.advance(Duration::from_secs(1));
        }
    }
    assert_eq!(
        totals,
        [1, 1, 0],
        "the trade must leave the tape at seven days without a writer"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "one hit, then one window-departure miss"
    );
    assert_eq!(generation(&mut writer).await, initial_generation);
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

/// The route captures its clock before admission and binds it into SQL. When
/// admission takes longer than the time left to an included order's expiry,
/// the payload is already stale when the read finishes. It may answer this
/// request (it is current at the route's clock) but must never be stored.
#[tokio::test]
async fn an_expiry_between_the_route_clock_and_the_read_is_not_stored() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (mut state, runtime, _record) = admitted_state(&url, &mut writer).await;
    let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
        Instant::now(),
        instant_at("2026-10-22T08:00:00Z"),
    ))));
    state.trading_read_observer = clock.clone();
    let cache = Arc::new(TradingCache::new(4, 1024));
    let runs = Arc::new(AtomicUsize::new(0));

    let route_clock = clock.now();
    let expires_at = route_clock.wall + Duration::from_secs(1);
    // Admission and the pool wait take two seconds.
    clock.advance(Duration::from_secs(2));
    runtime
        .authority()
        .refresh(runtime.record_path(), clock.now().monotonic);
    let loads = runs.clone();
    let first = run_trading_read(
        &state,
        &cache,
        "book",
        "orderbook",
        route_clock,
        move |connection, clock| {
            Box::pin(async move {
                loads.fetch_add(1, Ordering::SeqCst);
                resting_order(connection, expires_at, &clock).await
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(first, 1, "the route's own clock still includes the order");
    assert!(
        cache.is_empty(),
        "an expiry that passed before the read finished must not be stored"
    );

    // The next request captures its clock after the expiry.
    runtime
        .authority()
        .refresh(runtime.record_path(), clock.now().monotonic);
    let loads = runs.clone();
    let second = run_trading_read(
        &state,
        &cache,
        "book",
        "orderbook",
        clock.now(),
        move |connection, clock| {
            Box::pin(async move {
                loads.fetch_add(1, Ordering::SeqCst);
                resting_order(connection, expires_at, &clock).await
            })
        },
    )
    .await
    .unwrap();
    assert_eq!(second, 0, "the expired order must not be served again");
    assert_eq!(runs.load(Ordering::SeqCst), 2);

    // A loader that ignores the clock it is given and binds the route's
    // earlier one reports an expiry that has already passed at the runtime's
    // clock. The boundary fails closed: nothing is stored, the next read loads.
    let mismatched = Arc::new(TradingCache::new(4, 1024));
    for _ in 0..2 {
        runtime
            .authority()
            .refresh(runtime.record_path(), clock.now().monotonic);
        let loads = runs.clone();
        run_trading_read(
            &state,
            &mismatched,
            "book",
            "orderbook",
            clock.now(),
            move |connection, _ignored| {
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    resting_order(connection, expires_at, &route_clock).await
                })
            },
        )
        .await
        .unwrap();
        assert!(
            mismatched.is_empty(),
            "a fill from an earlier clock must not be stored"
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 4);
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn an_entry_from_the_prior_incarnation_is_never_a_hit() {
    let _serial = SERIAL.lock().await;
    let (Ok(first), Ok(second)) = (
        std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL"),
        std::env::var("PUBLIC_CACHE_TEST_SECOND_DATABASE_URL"),
    ) else {
        return;
    };
    let options_a = options(&first);
    let options_b = options(&second);
    let mut a = PgConnection::connect_with(&options_a).await.unwrap();
    let mut b = PgConnection::connect_with(&options_b).await.unwrap();
    // Both incarnations start at the same generation: the reused number.
    let base = generation(&mut a).await.max(generation(&mut b).await) + 1000;
    set_generation(&mut a, base).await;
    set_generation(&mut b, base).await;
    let record = Record::new();
    let process = Arc::new(CacheAuthority::default());
    let mut clock = Instant::now();

    let primary_a = authority(1);
    bind(&mut a, &primary_a, true).await;
    record.activate(&primary_a);
    process.refresh(record.path(), clock);
    let listener_a = CacheEvents::connect(&options_a, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let (generation_a, ticket_a) = {
        let (token, generation) = admit_connection(&process, &mut a, clock)
            .await
            .unwrap()
            .unwrap();
        (generation, FillTicket::new(token, generation))
    };
    assert_eq!(generation_a, base);
    let cache = TradingCache::new(4, 1024);
    let entry_a = TradingEntry::new(
        &ticket_a,
        b"rows-from-a".to_vec(),
        TradingClock::new(clock, SystemTime::now()),
        Boundary::Never,
        "policy".into(),
    );
    assert!(cache.insert("key", entry_a.clone(), clock));
    assert!(
        cache
            .hit(
                &process,
                &"key",
                &ticket_a,
                "policy",
                TradingClock::new(clock, SystemTime::now()),
                clock
            )
            .is_some()
    );

    // The owner revokes, drains, binds the successor and activates it.
    let primary_b = authority(3);
    record.revoke(2);
    process.refresh(record.path(), clock);
    clock += AUTHORITY_LEASE + Duration::from_secs(1);
    bind(&mut b, &primary_b, true).await;
    record.activate(&primary_b);
    process.refresh(record.path(), clock);
    let listener_b = CacheEvents::connect(&options_b, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let (generation_b, ticket_b) = {
        let (token, generation) = admit_connection(&process, &mut b, clock)
            .await
            .unwrap()
            .unwrap();
        (generation, FillTicket::new(token, generation))
    };
    // Equal generation numbers are not equal data: the epoch and fence differ.
    assert_eq!(generation_b, base);
    assert!(
        cache
            .hit(
                &process,
                &"key",
                &ticket_b,
                "policy",
                TradingClock::new(clock, SystemTime::now()),
                clock
            )
            .is_none()
    );
    let entry_b = TradingEntry::new(
        &ticket_b,
        b"rows-from-b".to_vec(),
        TradingClock::new(clock, SystemTime::now()),
        Boundary::Never,
        "policy".into(),
    );
    assert!(cache.insert("key", entry_b, clock));
    assert_eq!(
        cache
            .hit(
                &process,
                &"key",
                &ticket_b,
                "policy",
                TradingClock::new(clock, SystemTime::now()),
                clock
            )
            .map(|entry| entry.payload),
        Some(b"rows-from-b".to_vec())
    );

    listener_a.close().await.unwrap();
    listener_b.close().await.unwrap();
    reset(&mut a).await;
    reset(&mut b).await;
    a.close().await.unwrap();
    b.close().await.unwrap();
}

fn barrier(directory: &Path, reader: &str, step: &str) -> PathBuf {
    directory.join(format!("{reader}.{step}"))
}

/// A fill that loses its authority while the query runs is neither stored nor
/// answered. The revocation happens inside the loader, after the snapshot and
/// generation were read but before the result is published.
#[tokio::test]
async fn a_fill_that_loses_its_authority_is_not_stored_or_answered() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let identity = authority(1);
    bind(&mut writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().expect("explicitly enabled");
    publish_record(runtime, &record);
    runtime.tick(Instant::now()).await;
    let cache = Arc::new(TradingCache::new(16, 1 << 20));
    state.trading_caches.register(&cache);
    let authority = runtime.authority().clone();

    let clock = state.trading_read_observer.now();
    let result = run_trading_read(
        &state,
        &cache,
        "stale",
        "prices_fixed",
        clock,
        move |connection, _clock| {
            Box::pin(async move {
                let value: i64 = sqlx::query_scalar(
                    "SELECT generation FROM public.public_read_cache_generations \
                 WHERE domain = 'trading'",
                )
                .fetch_one(connection)
                .await
                .map_err(|_| ApiError::database())?;
                // The owner revokes while this fill is in flight.
                authority.revoke();
                Ok(TradingFill {
                    value,
                    facts: TemporalFacts::proven_written_only(),
                })
            })
        },
    )
    .await;

    assert!(
        result.is_err(),
        "a fill from a revoked authority must not be answered"
    );
    assert!(
        cache.is_empty(),
        "a fill from a revoked authority must not be stored"
    );
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn unadmitted_connections_never_publish() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let mut published = Vec::new();
    for reason in ["revoked", "expired", "mismatched", "missing", "rotation"] {
        let record = Record::new();
        let identity = authority(1);
        bind(&mut writer, &identity, true).await;
        record.activate(&identity);
        let config = state_config(&url);
        let pools = connect(&config).await.unwrap();
        let state = AppState::new(config, pools, policy()).unwrap();
        let runtime = state.trading_cache.as_ref().unwrap();
        publish_record(runtime, &record);
        runtime.tick(Instant::now()).await;
        let process = runtime.authority().clone();
        match reason {
            "revoked" | "rotation" => process.revoke(),
            "expired" => {
                process
                    .synchronize(Some(identity.clone()), Instant::now() - AUTHORITY_LEASE)
                    .unwrap();
            }
            "mismatched" => bind(&mut writer, &authority(3), true).await,
            "missing" => {
                sqlx::query("UPDATE public.public_read_cache_incarnation SET active = false")
                    .execute(&mut writer)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let cache = Arc::new(TradingCache::new(4, 1024));
        let clock = state.trading_read_observer.now();
        let result = run_trading_read(
            &state,
            &cache,
            reason,
            "prices_fixed",
            clock,
            move |connection, _clock| {
                Box::pin(async move {
                    let value: i64 = sqlx::query_scalar("SELECT 42::bigint")
                        .fetch_one(connection)
                        .await
                        .map_err(|_| ApiError::database())?;
                    if reason == "rotation" {
                        // This loader started without a token. A later activation
                        // must not authorize its old-connection response.
                        process
                            .synchronize(Some(authority(3)), Instant::now())
                            .unwrap();
                    }
                    Ok(TradingFill {
                        value,
                        facts: TemporalFacts::proven_written_only(),
                    })
                })
            },
        )
        .await;
        if result.is_ok() {
            published.push(reason);
        }
        assert!(cache.is_empty());
        drop(state);
    }
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert!(
        published.is_empty(),
        "unadmitted responses published: {published:?}"
    );
}

struct ReadBarrier {
    stage: TradingReadStage,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl TradingReadObserver for ReadBarrier {
    fn checkpoint(
        &self,
        stage: TradingReadStage,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if stage == self.stage {
                self.entered.notify_one();
                self.release.notified().await;
            }
        })
    }
}

#[tokio::test]
async fn overlapping_snapshots_publish_without_replacing_the_newer_entry() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let identity = authority(1);
    bind(&mut writer, &identity, true).await;
    record.activate(&identity);
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().unwrap();
    publish_record(runtime, &record);
    runtime.tick(Instant::now()).await;
    let cache = Arc::new(TradingCache::new(4, 1024));
    let before = generation(&mut writer).await;
    let gate = Arc::new(ReadBarrier {
        stage: TradingReadStage::SnapshotEstablished,
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let mut older = state.clone();
    older.trading_read_observer = gate.clone();
    let old_cache = cache.clone();
    let old = tokio::spawn(async move {
        let clock = older.trading_read_observer.now();
        run_trading_read(&older, &old_cache, "overlap", "prices_fixed", clock, |connection, _clock| Box::pin(async move {
            let value = sqlx::query_scalar::<_, i64>("SELECT generation FROM public_read_cache_generations WHERE domain='trading'")
                .fetch_one(connection).await.map_err(|_| ApiError::database())?;
            Ok(TradingFill { value, facts: TemporalFacts::proven_written_only() })
        })).await
    });
    tokio::time::timeout(Duration::from_secs(30), gate.entered.notified())
        .await
        .unwrap();
    sqlx::query("UPDATE products SET is_active = is_active")
        .execute(&mut writer)
        .await
        .unwrap();
    let newer = generation(&mut writer).await;
    let runs = Arc::new(AtomicUsize::new(0));
    assert_eq!(
        read_family(
            &state,
            &cache,
            "overlap",
            "prices_fixed",
            TemporalFacts::proven_written_only(),
            runs.clone()
        )
        .await,
        newer
    );
    gate.release.notify_one();
    let old_result = old.await.unwrap();
    assert!(
        old_result.is_ok(),
        "an overlapping old snapshot is not a restore"
    );
    assert_eq!(old_result.unwrap(), before);
    assert_eq!(
        read_family(
            &state,
            &cache,
            "overlap",
            "prices_fixed",
            TemporalFacts::proven_written_only(),
            runs.clone()
        )
        .await,
        newer
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the older fill must not replace the newer entry"
    );
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

/// Hold only the delayed request; other requests share its advancing clock.
struct ClockBarrier {
    clock: Arc<ControlledClock>,
    gate: ReadBarrier,
}

impl TradingReadObserver for ClockBarrier {
    fn now(&self) -> TradingClock {
        self.clock.now()
    }

    fn checkpoint(
        &self,
        stage: TradingReadStage,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        self.gate.checkpoint(stage)
    }
}

#[tokio::test]
async fn delayed_requests_load_at_their_own_clock_without_replacing_later_entries() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (mut state, runtime, _record) = admitted_state(&url, &mut writer).await;
    let initial_generation = generation(&mut writer).await;
    let mut results = Vec::new();
    for (kind, family) in [("expiry", "orderbook"), ("rolling", "trade_tape")] {
        let start = instant_at("2026-10-22T08:00:00Z");
        let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
            Instant::now(),
            start,
        ))));
        state.trading_read_observer = clock.clone();
        let gate = Arc::new(ClockBarrier {
            clock: clock.clone(),
            gate: ReadBarrier {
                stage: TradingReadStage::SnapshotEstablished,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            },
        });
        let cache = Arc::new(TradingCache::new(4, 1024));
        let runs = Arc::new(AtomicUsize::new(0));
        let mut earlier = state.clone();
        earlier.trading_read_observer = gate.clone();
        let early_cache = cache.clone();
        let loads = runs.clone();
        let captured = clock.now();
        let delayed = tokio::spawn(async move {
            run_trading_read(
                &earlier,
                &early_cache,
                kind,
                family,
                captured,
                move |connection, clock| {
                    Box::pin(async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        clock_case(connection, kind, start, &clock).await
                    })
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(30), gate.gate.entered.notified())
            .await
            .unwrap();
        // B overtakes A after the row disappears, without any database write.
        clock.advance(Duration::from_secs(2));
        runtime
            .authority()
            .refresh(runtime.record_path(), clock.now().monotonic);
        let loads = runs.clone();
        let later = run_trading_read(
            &state,
            &cache,
            kind,
            family,
            clock.now(),
            move |connection, clock| {
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    clock_case(connection, kind, start, &clock).await
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(later, 0);
        assert_eq!(cache.len(), 1, "B stores its empty result");
        gate.gate.release.notify_one();
        let early = tokio::time::timeout(Duration::from_secs(30), delayed)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let loads = runs.clone();
        let retained = run_trading_read(
            &state,
            &cache,
            kind,
            family,
            clock.now(),
            move |connection, clock| {
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    clock_case(connection, kind, start, &clock).await
                })
            },
        )
        .await
        .unwrap();
        results.push((kind, early, retained, runs.load(Ordering::SeqCst)));
    }
    assert_eq!(generation(&mut writer).await, initial_generation);
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert_eq!(
        results,
        [("expiry", 1, 0, 2), ("rolling", 1, 0, 2)],
        "A must load its earlier row; the next request must still hit B's empty entry"
    );
}

struct ControlledClock(std::sync::Mutex<TradingClock>);
impl TradingReadObserver for ControlledClock {
    fn now(&self) -> TradingClock {
        *self.0.lock().unwrap()
    }
}
impl ControlledClock {
    fn advance(&self, duration: Duration) {
        self.advance_separately(duration, duration);
    }

    fn advance_separately(&self, monotonic: Duration, wall: Duration) {
        let mut clock = self.0.lock().unwrap();
        clock.monotonic += monotonic;
        clock.wall += wall;
    }
}

#[tokio::test]
async fn wall_deadlines_expire_entries_before_their_monotonic_boundary() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (mut state, runtime, _record) = admitted_state(&url, &mut writer).await;
    let initial_generation = generation(&mut writer).await;
    let mut results = Vec::new();
    for (kind, family) in [("expiry", "orderbook"), ("rolling", "trade_tape")] {
        let start = instant_at("2026-10-22T08:00:00Z");
        let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
            Instant::now(),
            start,
        ))));
        state.trading_read_observer = clock.clone();
        let cache = Arc::new(TradingCache::new(4, 1024));
        let runs = Arc::new(AtomicUsize::new(0));
        let mut totals = Vec::new();
        for read in 0..4 {
            runtime
                .authority()
                .refresh(runtime.record_path(), clock.now().monotonic);
            let loads = runs.clone();
            let total = run_trading_read(
                &state,
                &cache,
                kind,
                family,
                clock.now(),
                move |connection, clock| {
                    Box::pin(async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        clock_case(connection, kind, start, &clock).await
                    })
                },
            )
            .await
            .unwrap();
            totals.push(total);
            if read == 0 {
                assert_eq!(cache.len(), 1);
                // A distinct forward clock inside both bounds must still hit.
                clock.advance(Duration::from_millis(100));
            } else if read == 1 {
                // Both request/current monotonic instants remain before the
                // stored boundary, but the SQL wall clock crosses departure.
                clock.advance_separately(Duration::from_millis(100), Duration::from_secs(2));
            }
        }
        results.push((kind, totals, runs.load(Ordering::SeqCst)));
    }
    assert_eq!(generation(&mut writer).await, initial_generation);
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert_eq!(
        results,
        [
            ("expiry", vec![1, 1, 0, 0], 2),
            ("rolling", vec![1, 1, 0, 0], 2),
        ],
        "wall expiry must reload; ordinary and post-departure reads must still hit"
    );
}

/// One row that the clock removes one second after `start`, read through its
/// own predicate with the loader's clock. The fact comes from the rows kept.
async fn clock_case(
    connection: &mut PgConnection,
    kind: &str,
    start: SystemTime,
    clock: &TradingClock,
) -> Result<TradingFill<i64>, ApiError> {
    let edge = start + Duration::from_secs(1);
    match kind {
        "expiry" | "late-fill" => resting_order(connection, edge, clock).await,
        "grace" => grace_owner(connection, edge, clock).await,
        // A seven-day lookback (a price summary over 168 hours) keeps a row on
        // its edge, so this trade leaves half a second before `edge`.
        "rolling" => {
            let confirmed_at = edge - Duration::from_secs(7 * 86_400) - Duration::from_millis(500);
            trade_in_window(connection, confirmed_at, clock).await
        }
        // Valid through 22 October, the Singapore date of `start`.
        "singapore" => {
            let valid_until = chrono::NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
            certified_ask(connection, valid_until, clock).await
        }
        _ => unreachable!(),
    }
}

/// No case writes: each payload changes because the clock passes the
/// boundary that its own rows derived.
#[tokio::test]
async fn clock_boundaries_change_payloads_without_writes() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    let (mut state, runtime, _record) = admitted_state(&url, &mut writer).await;
    // One second before midnight in Singapore (16:00 UTC).
    let start = instant_at("2026-10-22T15:59:59Z");
    let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
        Instant::now(),
        start,
    ))));
    state.trading_read_observer = clock.clone();
    let initial_generation = generation(&mut writer).await;
    for kind in ["expiry", "grace", "rolling", "singapore", "late-fill"] {
        *clock.0.lock().unwrap() = TradingClock::new(Instant::now(), start);
        let cache = Arc::new(TradingCache::new(4, 1024));
        let runs = Arc::new(AtomicUsize::new(0));
        let family = if kind == "rolling" {
            "prices"
        } else {
            "orderbook"
        };
        for read in 0..3 {
            runtime
                .authority()
                .refresh(runtime.record_path(), clock.now().monotonic);
            let loader_clock = clock.clone();
            let loads = runs.clone();
            let value = run_trading_read(
                &state,
                &cache,
                kind,
                family,
                clock.now(),
                move |connection, now| {
                    Box::pin(async move {
                        loads.fetch_add(1, Ordering::SeqCst);
                        let fill = clock_case(connection, kind, start, &now).await;
                        if kind == "late-fill" && read == 0 {
                            // The read itself outlasts the order.
                            loader_clock.advance(Duration::from_secs(1));
                        }
                        fill
                    })
                },
            )
            .await
            .unwrap();
            if kind == "late-fill" && read == 0 {
                assert!(
                    cache.is_empty(),
                    "a fill past its controlled-clock boundary must not be stored"
                );
            } else if read < 2 && kind != "late-fill" {
                assert_eq!(value, 1, "{kind}: live row");
            } else {
                assert_eq!(value, 0, "{kind}: row must leave without a writer");
            }
            if read == 1 {
                clock.advance(Duration::from_secs(1));
            }
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "{kind}: one hit, then one boundary miss"
        );
    }
    assert_eq!(generation(&mut writer).await, initial_generation);
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

async fn route_json(state: &AppState, uri: &str) -> (u16, serde_json::Value) {
    use tower::ServiceExt;
    let routes = axum::Router::new()
        .merge(verdaxis_read::routes::orderbook_snapshot::routes())
        .merge(verdaxis_read::routes::orderbook_lists::routes())
        .merge(verdaxis_read::routes::orderbook_map::routes())
        .merge(verdaxis_read::routes::prices::routes())
        .merge(verdaxis_read::routes::curves_canonical::routes())
        .with_state(state.clone());
    let response = routes
        .oneshot(
            axum::http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    fn remove_generated(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                fields.remove("generated_at");
                for value in fields.values_mut() {
                    remove_generated(value);
                }
            }
            serde_json::Value::Array(items) => {
                for value in items {
                    remove_generated(value);
                }
            }
            _ => {}
        }
    }
    remove_generated(&mut value);
    (status, value)
}

#[tokio::test]
async fn public_route_wrappers_check_generations_and_time_without_listener_wakes() {
    let _serial = SERIAL.lock().await;
    let Ok(url) = std::env::var("ROUTE_CACHE_TEST_DATABASE_URL") else {
        return;
    };
    let mut writer = PgConnection::connect_with(&options(&url)).await.unwrap();
    let (mut state, runtime, record) = admitted_state(&url, &mut writer).await;
    state.policy = Arc::new(
        serde_json::from_slice::<MarketPolicy>(include_bytes!(
            "../../../deploy/rust/market-policy.json"
        ))
        .unwrap(),
    );
    let start = instant_at("2026-10-23T08:00:00Z");
    let clock = Arc::new(ControlledClock(std::sync::Mutex::new(TradingClock::new(
        Instant::now(),
        start,
    ))));
    state.trading_read_observer = clock.clone();
    let product_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM products WHERE is_active ORDER BY id LIMIT 1")
            .fetch_one(&mut writer)
            .await
            .unwrap();
    let point_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM delivery_points WHERE is_active ORDER BY id LIMIT 1")
            .fetch_one(&mut writer)
            .await
            .unwrap();
    let product = state
        .policy
        .products
        .iter()
        .find(|p| p.id == product_id)
        .unwrap();
    let point = state
        .policy
        .points
        .iter()
        .find(|p| p.id == point_id)
        .unwrap();
    let paths = vec![
        format!(
            "/api/orderbook/snapshot?market_product={}&delivery_point_id={}&availability_window=SPOT",
            product.code, point.id
        ),
        "/api/orderbook/bids".into(),
        "/api/orderbook/asks".into(),
        "/api/orderbook".into(),
        "/api/orderbook/with-ci".into(),
        "/api/orderbook/aggregated".into(),
        "/api/orderbook/product-counts?side=BID".into(),
        "/api/orderbook/map-summary".into(),
        "/api/orderbook/map-summary/compact".into(),
        "/api/orderbook/regions".into(),
        "/api/orderbook/fuel-types".into(),
        "/api/prices".into(),
        "/api/prices/reference".into(),
        "/api/curves/forward/table".into(),
        "/api/curves/forward/board".into(),
        format!(
            "/api/curves/forward/slice?market_product={}&delivery_point_id={}&availability_window=SPOT",
            product.code, point.id
        ),
    ];
    let mut disabled = state.clone();
    disabled.trading_cache = None;
    let mut revision = 1;
    for path in paths {
        let expiry: chrono::DateTime<chrono::Utc> =
            (clock.now().wall + Duration::from_secs(1)).into();
        sqlx::query("UPDATE orderbook_orders SET expires_at = $1")
            .bind(expiry)
            .execute(&mut writer)
            .await
            .unwrap();
        runtime
            .authority()
            .refresh(runtime.record_path(), clock.now().monotonic);
        state.trading_caches.clear();
        let expected = route_json(&disabled, &path).await;
        let actual = route_json(&state, &path).await;
        assert_eq!(actual, expected, "cache off/on cold: {path}");
        assert_eq!(actual.0, 200, "{path}");
        assert!(
            state.trading_caches.entries() > 0,
            "route has no proven wrapper: {path}"
        );
        assert_eq!(route_json(&state, &path).await, expected, "warm: {path}");
        // No runtime tick consumes the event. The next route request must still
        // read the new committed generation and replace its old entry.
        sqlx::query("UPDATE orderbook_orders SET price_per_mt_usd = price_per_mt_usd + 1")
            .execute(&mut writer)
            .await
            .unwrap();
        let committed = route_json(&disabled, &path).await;
        let gate = Arc::new(ClockBarrier {
            clock: clock.clone(),
            gate: ReadBarrier {
                stage: TradingReadStage::BeforeInsert,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            },
        });
        let mut paused = state.clone();
        paused.trading_read_observer = gate.clone();
        let request_path = path.clone();
        let task = tokio::spawn(async move { route_json(&paused, &request_path).await });
        tokio::time::timeout(Duration::from_secs(30), gate.gate.entered.notified())
            .await
            .expect("post-commit route must miss without event delivery");
        gate.gate.release.notify_one();
        assert_eq!(task.await.unwrap(), committed, "post-commit: {path}");
        clock.advance(Duration::from_secs(1));
        runtime
            .authority()
            .refresh(runtime.record_path(), clock.now().monotonic);
        assert_eq!(
            route_json(&state, &path).await,
            route_json(&disabled, &path).await,
            "exact expiry: {path}"
        );
        // Reused and reduced generations retain new authority ownership.
        let before_rotation = generation(&mut writer).await;
        for offset in [0, -1] {
            let current = generation(&mut writer).await;
            revision += 1;
            record.revoke(revision);
            publish_record(&runtime, &record);
            runtime
                .authority()
                .refresh(runtime.record_path(), clock.now().monotonic);
            assert_eq!(route_json(&state, &path).await.0, 500, "revoked: {path}");
            revision += 1;
            let successor = authority(revision);
            bind(&mut writer, &successor, true).await;
            set_generation(&mut writer, current + offset).await;
            record.activate(&successor);
            publish_record(&runtime, &record);
            until_serving(&runtime, &successor).await;
            runtime
                .authority()
                .refresh(runtime.record_path(), clock.now().monotonic);
            assert_eq!(
                route_json(&state, &path).await,
                route_json(&disabled, &path).await,
                "rotated: {path}"
            );
        }
        // The disposable restore reused event keys too. Move beyond retained
        // events before the next economic write; do not leave a duplicate key.
        set_generation(&mut writer, before_rotation + 100).await;
    }
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

async fn incarnation_epoch(connection: &mut PgConnection) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT database_epoch FROM public.public_read_cache_incarnation WHERE singleton",
    )
    .fetch_one(connection)
    .await
    .map_err(|_| ApiError::database())
}

/// Load the incarnation's own epoch as an explicit-range family, so each
/// authority's payload is visibly its own.
async fn read_epoch(
    state: &AppState,
    cache: &Arc<TradingCache<&'static str>>,
    runs: Arc<AtomicUsize>,
) -> String {
    let clock = state.trading_read_observer.now();
    run_trading_read(
        state,
        cache,
        "rotation",
        "prices_fixed",
        clock,
        move |connection, _clock| {
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(TradingFill {
                    value: incarnation_epoch(connection).await?,
                    facts: TemporalFacts::proven_written_only(),
                })
            })
        },
    )
    .await
    .unwrap()
}

/// Drive the service loop until the successor's listener is attached and its
/// token is issued, as the loop does after an activation.
async fn until_serving(runtime: &TradingCacheRuntime, successor: &Authority) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            runtime.tick(Instant::now()).await;
            if runtime
                .authority()
                .token(successor, Instant::now())
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the successor listener attaches");
}

/// A fill pauses before insertion or before publication while the owner
/// revokes this authority and activates a successor. Before the release, a
/// successor read leaves this process's high-water mark equal to the paused
/// fill's generation (a promotion that reuses the number) or one below it (a
/// restore). Generation numbers alone accept the paused fill in the first case
/// and are lower in the second; the fence must reject it in both.
#[tokio::test]
async fn authority_rotation_at_each_publication_barrier_rejects_old_fills() {
    let _serial = SERIAL.lock().await;
    let Some((url, mut writer)) = writer().await else {
        return;
    };
    for stage in [
        TradingReadStage::BeforeInsert,
        TradingReadStage::BeforePublish,
    ] {
        for (variant, offset) in [("equal", 0), ("lower", -1)] {
            let (state, runtime, record) = admitted_state(&url, &mut writer).await;
            let cache = Arc::new(TradingCache::new(4, 1024));
            state.trading_caches.register(&cache);
            let gate = Arc::new(ReadBarrier {
                stage,
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let mut paused = state.clone();
            paused.trading_read_observer = gate.clone();
            let paused_cache = cache.clone();
            let old = tokio::spawn(async move {
                let clock = paused.trading_read_observer.now();
                run_trading_read(
                    &paused,
                    &paused_cache,
                    "rotation",
                    "prices_fixed",
                    clock,
                    |connection, _clock| {
                        Box::pin(async move {
                            Ok(TradingFill {
                                value: incarnation_epoch(connection).await?,
                                facts: TemporalFacts::proven_written_only(),
                            })
                        })
                    },
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(30), gate.entered.notified())
                .await
                .unwrap();
            let paused_generation = generation(&mut writer).await;

            // Revoke, then bind and activate the successor.
            record.revoke(2);
            publish_record(&runtime, &record);
            runtime
                .authority()
                .refresh(runtime.record_path(), Instant::now());
            runtime.tick(Instant::now()).await;
            let successor = authority(3);
            bind(&mut writer, &successor, true).await;
            set_generation(&mut writer, paused_generation + offset).await;
            record.activate(&successor);
            publish_record(&runtime, &record);
            runtime
                .authority()
                .refresh(runtime.record_path(), Instant::now());
            until_serving(&runtime, &successor).await;

            // The successor's first read sets the high-water mark.
            let runs = Arc::new(AtomicUsize::new(0));
            assert_eq!(
                read_epoch(&state, &cache, runs.clone()).await,
                successor.database_epoch
            );
            assert_eq!(
                runtime.authority().highest_generation(),
                Some(paused_generation + offset),
                "{stage:?}, {variant}"
            );

            gate.release.notify_one();
            assert!(
                old.await.unwrap().is_err(),
                "{stage:?}, {variant}: obsolete response"
            );
            // Only the successor's entry remains, and it still answers.
            assert_eq!(
                cache.len(),
                1,
                "{stage:?}, {variant}: obsolete cache insertion"
            );
            runtime
                .authority()
                .refresh(runtime.record_path(), Instant::now());
            assert_eq!(
                read_epoch(&state, &cache, runs.clone()).await,
                successor.database_epoch
            );
            assert_eq!(
                runs.load(Ordering::SeqCst),
                1,
                "{stage:?}, {variant}: the successor entry must still answer"
            );
            // Restore a generation above retained event keys for later writers.
            set_generation(&mut writer, paused_generation + 100).await;
            reset(&mut writer).await;
        }
    }
    writer.close().await.unwrap();
}

#[tokio::test]
async fn two_incarnations_read_distinct_rows_at_a_reused_generation() {
    let _serial = SERIAL.lock().await;
    let (Ok(first), Ok(second)) = (
        std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL"),
        std::env::var("PUBLIC_CACHE_TEST_SECOND_DATABASE_URL"),
    ) else {
        return;
    };
    let mut a = PgConnection::connect_with(&options(&first)).await.unwrap();
    let mut b = PgConnection::connect_with(&options(&second)).await.unwrap();
    let shared = generation(&mut a).await.max(generation(&mut b).await) + 1000;
    let cache = Arc::new(TradingCache::new(4, 4096));
    let runs = Arc::new(AtomicUsize::new(0));
    let mut payloads = Vec::new();
    for (url, writer, revision) in [(&first, &mut a, 1), (&second, &mut b, 3)] {
        set_generation(writer, shared).await;
        let identity = authority(revision);
        bind(writer, &identity, true).await;
        let record = Record::new();
        record.activate(&identity);
        let pools = connect(&state_config(url)).await.unwrap();
        let state = AppState::new(state_config(url), pools, policy()).unwrap();
        let runtime = state.trading_cache.as_ref().unwrap();
        publish_record(runtime, &record);
        runtime.tick(Instant::now()).await;
        let loads = runs.clone();
        let value: String = run_trading_read(
            &state,
            &cache,
            "incarnation",
            "prices_fixed",
            state.trading_read_observer.now(),
            move |connection, _clock| {
                Box::pin(async move {
                    loads.fetch_add(1, Ordering::SeqCst);
                    // This is an actual protected row, not an invented payload label.
                    let value: String = sqlx::query_scalar(
                        "SELECT database_epoch FROM public_read_cache_incarnation WHERE singleton",
                    )
                    .fetch_one(connection)
                    .await
                    .map_err(|_| ApiError::database())?;
                    Ok(TradingFill {
                        value,
                        facts: TemporalFacts::proven_written_only(),
                    })
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(value, identity.database_epoch);
        assert_eq!(generation(writer).await, shared);
        payloads.push(value);
        runtime.authority().revoke();
    }
    assert_ne!(payloads[0], payloads[1]);
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    reset(&mut a).await;
    reset(&mut b).await;
    a.close().await.unwrap();
    b.close().await.unwrap();
}

fn signal(directory: &Path, reader: &str, step: &str) {
    std::fs::write(barrier(directory, reader, step), b"1").unwrap();
}

/// Keep the authority lease fresh without waking the listener. The event stays
/// buffered, so the post-commit read below is proved before delivery.
async fn wait_without_delivery(runtime: &TradingCacheRuntime, path: &Path) {
    loop {
        if path.exists() {
            return;
        }
        runtime
            .authority()
            .refresh(runtime.record_path(), Instant::now());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn mixed_python_writer_schedule() {
    if std::env::var("PUBLIC_CACHE_TEST_SCHEDULE").as_deref() != Ok("mixed") {
        return;
    }
    let Ok(url) = std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL") else {
        return;
    };
    // Each reader may be pointed at its own disposable incarnation, so the two
    // processes prove the commit-safe check on two databases as well.
    let database =
        std::env::var("PUBLIC_CACHE_TEST_READER_DATABASE_URL").unwrap_or_else(|_| url.clone());
    let reader = std::env::var("PUBLIC_CACHE_TEST_READER").unwrap_or_else(|_| "a".into());
    let directory =
        PathBuf::from(std::env::var("PUBLIC_CACHE_TEST_BARRIER_DIR").expect("barrier directory"));
    let pools = connect(&state_config(&database)).await.unwrap();
    let state = AppState::new(state_config(&database), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().expect("explicitly enabled");
    let cache = Arc::new(TradingCache::new(16, 1 << 20));
    state.trading_caches.register(&cache);

    // The Python owner wrote the record before starting this process.
    let mut probe = PgConnection::connect_with(&options(&database))
        .await
        .unwrap();
    // Prove each reader really serves its own incarnation, so "two processes on
    // two databases" is not an assumption about the environment.
    if let Ok(expected) = std::env::var("PUBLIC_CACHE_TEST_EXPECT_DATABASE") {
        let actual: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&mut probe)
            .await
            .unwrap();
        assert_eq!(
            actual, expected,
            "reader {reader} is not on its named database"
        );
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            runtime.tick(Instant::now()).await;
            if admit_connection(runtime.authority(), &mut probe, Instant::now())
                .await
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the listener and authority become ready");

    let runs = Arc::new(AtomicUsize::new(0));
    let before = read_generation(&state, &cache, runs.clone()).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    signal(&directory, &reader, "warm.rust");
    wait_without_delivery(runtime, &barrier(&directory, &reader, "warm.python")).await;

    // Python has committed a real business change; the wake is still queued.
    let after = read_generation(&state, &cache, runs.clone()).await;
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "the reader must load after the commit, not answer from the prior generation"
    );
    assert!(after > before);
    std::fs::write(
        directory.join(format!("result_{reader}.json")),
        format!("{{\"before\":{before},\"after\":{after}}}"),
    )
    .unwrap();
    signal(&directory, &reader, "read.rust");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            runtime.tick(Instant::now()).await;
            if cache.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the delivered event clears the entry");
    signal(&directory, &reader, "cleared.rust");
    probe.close().await.unwrap();
}
