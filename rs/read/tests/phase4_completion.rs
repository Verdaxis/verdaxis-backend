#![cfg(feature = "contract-tests")]

use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use verdaxis_read::business::transaction::{CompletionControl, CompletionPoint};

#[tokio::test]
async fn p4_t2_c08() {
    let control = CompletionControl::new(CompletionPoint::AfterCommit, true);
    let count = Arc::new(AtomicUsize::new(0));
    let task_control = control.clone();
    let calls = count.clone();
    let owner = tokio::spawn(async move {
        task_control.wait_at(CompletionPoint::BeforeCommit).await;
        calls.fetch_add(1, Ordering::SeqCst);
        task_control.wait_at(CompletionPoint::AfterCommit).await;
    });
    control.reached().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    control.release();
    owner.await.unwrap();
    assert!(control.lose_ack());
}
