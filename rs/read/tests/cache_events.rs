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
