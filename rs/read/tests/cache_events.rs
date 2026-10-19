use verdaxis_read::runtime::cache_events::{EventBatch, check_batch, listener_budget};

#[test]
fn invalidation_batches_preserve_every_committed_generation() {
    assert_eq!(check_batch(4, &[]), EventBatch::Current);
    assert_eq!(
        check_batch(4, &[("trading".into(), 5), ("trading".into(), 6)]),
        EventBatch::Advanced(6)
    );
    for rows in [
        vec![("trading".into(), 6)],
        vec![("private".into(), 5)],
        vec![("trading".into(), 4)],
    ] {
        assert_eq!(check_batch(4, &rows), EventBatch::Resynchronize);
    }
}

#[test]
fn listener_requires_one_explicit_budget_slot() {
    assert_eq!(listener_budget(20, 20, false), Ok(20));
    assert!(listener_budget(20, 20, true).is_err());
    assert_eq!(listener_budget(20, 21, true), Ok(21));
    assert!(listener_budget(u32::MAX, u32::MAX, true).is_err());
}

#[tokio::test]
async fn an_unbudgeted_listener_is_refused_before_any_connection() {
    use sqlx::postgres::PgConnectOptions;
    use std::sync::Arc;
    use verdaxis_read::runtime::{
        cache_authority::CacheAuthority,
        cache_events::{CacheEvents, ListenerBudget},
    };

    // Port 1 is never listening: a budget refusal must come back as a protocol
    // error naming the budget, not as a failed connection.
    let options = PgConnectOptions::new().host("127.0.0.1").port(1);
    for budget in [
        ListenerBudget::new(100, 100, 100),
        ListenerBudget::new(70, 100, 70),
        ListenerBudget::new(u32::MAX, u32::MAX, u32::MAX),
    ] {
        let refused =
            CacheEvents::connect(&options, "UTC", Arc::new(CacheAuthority::default()), budget)
                .await;
        assert!(
            matches!(refused, Err(sqlx::Error::Protocol(ref message)) if message.contains("budget")),
            "{budget:?}"
        );
    }
}
