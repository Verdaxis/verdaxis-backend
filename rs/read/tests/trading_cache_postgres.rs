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
    time::{Duration, Instant},
};
use verdaxis_read::{
    ApiError, AppState, Boundary, Config, MarketPolicy, TemporalFacts, TradingCache,
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
    run_trading_read(state, cache, "generation", "prices", move |connection| {
        let runs = runs.clone();
        Box::pin(async move {
            runs.fetch_add(1, Ordering::SeqCst);
            let value: i64 = sqlx::query_scalar(
                "SELECT generation FROM public.public_read_cache_generations \
                 WHERE domain = 'trading'",
            )
            .fetch_one(connection)
            .await
            .map_err(|_| ApiError::database())?;
            Ok(TradingFill {
                value,
                // The generation row changes only through a write.
                facts: TemporalFacts::proven_written_only(),
            })
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
    let reader = std::env::var("PUBLIC_CACHE_TEST_READER").unwrap_or_else(|_| "a".into());
    let directory =
        PathBuf::from(std::env::var("PUBLIC_CACHE_TEST_BARRIER_DIR").expect("barrier directory"));
    let pools = connect(&state_config(&url)).await.unwrap();
    let state = AppState::new(state_config(&url), pools, policy()).unwrap();
    let runtime = state.trading_cache.as_ref().expect("explicitly enabled");
    let cache = Arc::new(TradingCache::new(16, 1 << 20));
    state.trading_caches.register(&cache);

    // The Python owner wrote the record before starting this process.
    let mut probe = PgConnection::connect_with(&options(&url)).await.unwrap();
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
