//! Disposable PostgreSQL only; the Python fixture owns migrations and cleanup.
#[tokio::test]
async fn notifications_follow_commit_and_listener_retires_cleanly() {
    use sqlx::{Connection, postgres::PgConnectOptions};
    use std::{str::FromStr, sync::Arc, time::Duration};
    use verdaxis_read::runtime::{
        cache_authority::CacheAuthority,
        cache_events::{CacheEvents, EventBatch},
    };

    let Ok(url) = std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL") else {
        return;
    };
    let options = PgConnectOptions::from_str(&url).unwrap();
    let mut writer = sqlx::PgConnection::connect_with(&options).await.unwrap();
    let authority = Arc::new(CacheAuthority::default());
    let mut listener = CacheEvents::connect(&options, "UTC", authority)
        .await
        .unwrap();
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM public_read_cache_generations WHERE domain='trading'",
    )
    .fetch_one(&mut writer)
    .await
    .unwrap();
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
    tokio::time::timeout(Duration::from_secs(5), listener.close())
        .await
        .unwrap()
        .unwrap();
    writer.close().await.unwrap();
}
