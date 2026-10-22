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
    ApiError, AppState, Boundary, Config, Deadline, MarketPolicy, TemporalFacts, TradingCache,
    TradingCacheRuntime, TradingEntry, TradingFill, compiled_sha, connect, run_trading_read,
    runtime::{
        cache_authority::{AUTHORITY_LEASE, CacheAuthority, FillTicket, admit_connection},
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
    // The generation row changes only through a write.
    read_family(
        state,
        cache,
        "generation",
        "prices",
        TemporalFacts::proven_written_only(),
        runs,
    )
    .await
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
    run_trading_read(state, cache, key, family, move |connection| {
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
    })
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
        Boundary::Never,
        "policy".into(),
    );
    assert!(cache.insert("key", entry_a.clone(), clock));
    assert!(
        cache
            .hit(&process, &"key", &ticket_a, generation_a, "policy", clock)
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
            .hit(&process, &"key", &ticket_b, generation_b, "policy", clock)
            .is_none()
    );
    let entry_b = TradingEntry::new(
        &ticket_b,
        b"rows-from-b".to_vec(),
        Boundary::Never,
        "policy".into(),
    );
    assert!(cache.insert("key", entry_b, clock));
    assert_eq!(
        cache
            .hit(&process, &"key", &ticket_b, generation_b, "policy", clock)
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

    let result = run_trading_read(&state, &cache, "stale", "prices", move |connection| {
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
    })
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
                process.synchronize(Some(identity.clone()), Instant::now() - AUTHORITY_LEASE).unwrap();
            }
            "mismatched" => bind(&mut writer, &authority(3), true).await,
            "missing" => {
                sqlx::query("UPDATE public.public_read_cache_incarnation SET active = false")
                    .execute(&mut writer).await.unwrap();
            }
            _ => unreachable!(),
        }
        let cache = Arc::new(TradingCache::new(4, 1024));
        let result = run_trading_read(&state, &cache, reason, "prices", move |connection| {
            Box::pin(async move {
                let value: i64 = sqlx::query_scalar("SELECT 42::bigint")
                    .fetch_one(connection).await.map_err(|_| ApiError::database())?;
                if reason == "rotation" {
                    // This loader started without a token. A later activation
                    // must not authorize its old-connection response.
                    process.synchronize(Some(authority(3)), Instant::now()).unwrap();
                }
                Ok(TradingFill { value, facts: TemporalFacts::proven_written_only() })
            })
        }).await;
        if result.is_ok() {
            published.push(reason);
        }
        assert!(cache.is_empty());
        state.pools.read.close().await;
    }
    reset(&mut writer).await;
    writer.close().await.unwrap();
    assert!(published.is_empty(), "unadmitted responses published: {published:?}");
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
