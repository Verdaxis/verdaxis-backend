//! Disposable PostgreSQL only; the Python fixture owns migrations and cleanup.
#[path = "support/cache_owner.rs"]
mod cache_owner;

use cache_owner::{Record, authority, bind, generation, reset};
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use verdaxis_read::runtime::{
    cache_authority::{AUTHORITY_REFRESH, Authority, CacheAuthority, admit_connection},
    cache_events::{CacheEvents, EventBatch, ListenerBudget, dependency_triggers_complete},
};

/// Both tests rebind the one shared incarnation row, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn budget() -> ListenerBudget {
    ListenerBudget::new(50, 100, 100)
}

async fn writer() -> Option<(PgConnectOptions, PgConnection)> {
    let url = std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL").ok()?;
    let options = PgConnectOptions::from_str(&url).unwrap();
    let writer = PgConnection::connect_with(&options).await.unwrap();
    Some((options, writer))
}

/// What the owner does before a process may cache: bind the database row, then
/// publish the record. The process re-reads the record once.
async fn activate(
    writer: &mut PgConnection,
    record: &Record,
    authority: &Authority,
    process: &CacheAuthority,
    now: Instant,
) {
    bind(writer, authority, true).await;
    record.activate(authority);
    process.refresh(record.path(), now);
}

#[tokio::test]
async fn notifications_follow_commit_and_listener_retires_cleanly() {
    let _serial = SERIAL.lock().await;
    let Some((options, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let process = Arc::new(CacheAuthority::default());
    let identity = authority(1);
    activate(&mut writer, &record, &identity, &process, Instant::now()).await;
    let mut listener = CacheEvents::connect(&options, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let generation = generation(&mut writer).await;
    sqlx::query("BEGIN").execute(&mut writer).await.unwrap();
    sqlx::query("UPDATE products SET is_active=is_active")
        .execute(&mut writer)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.wake())
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK").execute(&mut writer).await.unwrap();
    assert_eq!(
        CacheEvents::read_batch(&mut writer, generation)
            .await
            .unwrap(),
        EventBatch::Current
    );
    sqlx::query("UPDATE products SET is_active=is_active")
        .execute(&mut writer)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), listener.wake())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        CacheEvents::read_batch(&mut writer, generation)
            .await
            .unwrap(),
        EventBatch::Advanced(generation + 1)
    );
    let now = Instant::now();
    assert!(
        admit_connection(&process, &mut writer, now)
            .await
            .unwrap()
            .is_some()
    );
    tokio::time::timeout(Duration::from_secs(5), listener.close())
        .await
        .unwrap()
        .unwrap();
    // A retired listener leaves nothing attached, so no hit is possible.
    assert!(
        admit_connection(&process, &mut writer, now)
            .await
            .unwrap()
            .is_none()
    );
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn listener_refuses_a_database_that_is_not_the_active_authority() {
    let _serial = SERIAL.lock().await;
    let Some((options, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let process = Arc::new(CacheAuthority::default());
    let now = Instant::now();
    let expected = authority(1);
    bind(&mut writer, &expected, true).await;
    // No external record yet: database metadata alone proves nothing.
    assert!(
        CacheEvents::connect(&options, "UTC", process.clone(), budget())
            .await
            .is_err()
    );
    // The record names a different identity than this database reports.
    record.activate(&authority(1));
    process.refresh(record.path(), now);
    assert!(
        CacheEvents::connect(&options, "UTC", process.clone(), budget())
            .await
            .is_err()
    );
    // Same epoch and primary but another revision is another authority.
    let mut stale = expected.clone();
    stale.revision = 2;
    record.activate(&stale);
    process.refresh(record.path(), now);
    assert!(
        CacheEvents::connect(&options, "UTC", process.clone(), budget())
            .await
            .is_err()
    );
    // An inactive database cannot back a cache even when the record matches.
    record.activate(&expected);
    process.refresh(record.path(), now);
    bind(&mut writer, &expected, false).await;
    assert!(
        CacheEvents::connect(&options, "UTC", process.clone(), budget())
            .await
            .is_err()
    );
    bind(&mut writer, &expected, true).await;
    let listener = CacheEvents::connect(&options, "UTC", process.clone(), budget())
        .await
        .unwrap();
    listener.close().await.unwrap();
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn lost_listener_connection_detaches_and_reconnect_catches_up() {
    let _serial = SERIAL.lock().await;
    let Some((options, mut writer)) = writer().await else {
        return;
    };
    let options = options.application_name("verdaxis_cache_listener_test");
    let record = Record::new();
    let process = Arc::new(CacheAuthority::default());
    let identity = authority(1);
    let now = Instant::now();
    activate(&mut writer, &record, &identity, &process, now).await;
    let mut listener = CacheEvents::connect(&options, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let before = generation(&mut writer).await;
    assert!(
        admit_connection(&process, &mut writer, now)
            .await
            .unwrap()
            .is_some()
    );
    // Cut the listener's own backend. The owner must see the loss, hits stop,
    // and the commit made while nothing listens is still found on reconnect.
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE application_name = 'verdaxis_cache_listener_test' AND pid <> pg_backend_pid()",
    )
    .execute(&mut writer)
    .await
    .unwrap();
    let lost = tokio::time::timeout(Duration::from_secs(5), listener.wake())
        .await
        .unwrap();
    assert!(lost.is_err());
    assert!(
        admit_connection(&process, &mut writer, now)
            .await
            .unwrap()
            .is_none()
    );
    // A detached listener never wakes the owner again by itself.
    assert!(listener.wake().await.is_err());
    sqlx::query("UPDATE products SET is_active=is_active")
        .execute(&mut writer)
        .await
        .unwrap();
    listener.close().await.unwrap();
    let successor = CacheEvents::connect(&options, "UTC", process.clone(), budget())
        .await
        .unwrap();
    assert_eq!(
        CacheEvents::read_batch(&mut writer, before).await.unwrap(),
        EventBatch::Advanced(before + 1)
    );
    // Reconnecting starts a new fence: the generation read before is not reused.
    let admitted = admit_connection(&process, &mut writer, now + AUTHORITY_REFRESH)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.1, before + 1);
    successor.close().await.unwrap();
    reset(&mut writer).await;
    writer.close().await.unwrap();
}

#[tokio::test]
async fn dependency_triggers_must_fire_for_every_session_role() {
    let _serial = SERIAL.lock().await;
    let Some((options, mut writer)) = writer().await else {
        return;
    };
    let record = Record::new();
    let process = Arc::new(CacheAuthority::default());
    let identity = authority(1);
    activate(&mut writer, &record, &identity, &process, Instant::now()).await;
    assert!(
        dependency_triggers_complete(&mut writer).await.unwrap(),
        "a migrated database carries every trigger"
    );
    // A disabled trigger, and one enabled only for origin sessions, would let a
    // write made under another replication role skip the generation bump.
    for weaker in ["DISABLE TRIGGER", "ENABLE TRIGGER"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE public.products {weaker} public_read_cache_changed_update"
        )))
        .execute(&mut writer)
        .await
        .unwrap();
        assert!(!dependency_triggers_complete(&mut writer).await.unwrap());
        let refused = CacheEvents::connect(&options, "UTC", process.clone(), budget()).await;
        assert!(refused.is_err(), "{weaker}");
    }
    sqlx::query(
        "ALTER TABLE public.products ENABLE ALWAYS TRIGGER public_read_cache_changed_update",
    )
    .execute(&mut writer)
    .await
    .unwrap();
    assert!(dependency_triggers_complete(&mut writer).await.unwrap());
    let listener = CacheEvents::connect(&options, "UTC", process.clone(), budget())
        .await
        .unwrap();
    listener.close().await.unwrap();
    reset(&mut writer).await;
    writer.close().await.unwrap();
}
