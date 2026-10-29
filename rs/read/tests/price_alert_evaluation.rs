//! Offline contracts of the price-alert evaluator: which events qualify, that
//! dispatch is never held up, and how failed evaluations are retried.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use verdaxis_read::streams::{
    bus::{HubError, PublicChannel, StoredEvent, Terminal},
    dispatch::DispatchSink,
    price_alerts::{
        AlertEvaluator, EvaluationError, PriceAlertFeed, PriceAlertSink, TRIGGERED_EVENT_TYPE,
        executed_trade_id, run_worker,
    },
};

const TRADE: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001);
const OTHER_TRADE: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0002);

fn event(event_type: &str, payload: Value) -> StoredEvent {
    StoredEvent::new(1, vec![Uuid::new_v4()], event_type, payload).unwrap()
}

#[test]
fn auto_matched_and_confirmed_trades_qualify() {
    let matched = event(
        "trade_auto_matched",
        json!({"trade_id": TRADE.to_string(), "price": "700"}),
    );
    let confirmed = event(
        "trade_confirmed",
        json!({"id": OTHER_TRADE.to_string(), "price": "700"}),
    );
    assert_eq!(executed_trade_id(&matched), Some(TRADE));
    assert_eq!(executed_trade_id(&confirmed), Some(OTHER_TRADE));
}

#[test]
fn other_event_types_never_qualify() {
    for event_type in [
        "trade_created",
        "trade_declined",
        "trade_delivered",
        "trade_paid",
        "order_created",
        "orders_matched",
        TRIGGERED_EVENT_TYPE,
    ] {
        let candidate = event(
            event_type,
            json!({"id": TRADE.to_string(), "trade_id": TRADE.to_string()}),
        );
        assert_eq!(executed_trade_id(&candidate), None, "{event_type}");
    }
}

#[test]
fn the_alert_event_it_appends_is_not_an_execution() {
    let own = event(
        TRIGGERED_EVENT_TYPE,
        json!({"alert_id": Uuid::new_v4().to_string(), "price": "700"}),
    );
    let (feed, mut queue) = PriceAlertFeed::channel(4);
    feed.offer(&own);
    assert!(queue.try_recv().is_err());
}

#[test]
fn payloads_that_are_not_real_only_are_skipped() {
    for status in ["DEMO_ONLY", "UNKNOWN"] {
        let candidate = event(
            "trade_confirmed",
            json!({"id": TRADE.to_string(), "demo_status": status}),
        );
        assert_eq!(executed_trade_id(&candidate), None, "{status}");
    }
    let real = event(
        "trade_confirmed",
        json!({"id": TRADE.to_string(), "demo_status": "REAL_ONLY"}),
    );
    assert_eq!(executed_trade_id(&real), Some(TRADE));
}

#[test]
fn a_missing_or_malformed_trade_id_is_ignored() {
    assert_eq!(
        executed_trade_id(&event("trade_confirmed", json!({"price": "1"}))),
        None
    );
    assert_eq!(
        executed_trade_id(&event("trade_confirmed", json!({"id": "not-a-uuid"}))),
        None
    );
    assert_eq!(
        executed_trade_id(&event("trade_confirmed", json!({"id": 7}))),
        None
    );
}

#[derive(Default)]
struct Recorder {
    delivered: Vec<String>,
}

impl DispatchSink for Recorder {
    fn private(&mut self, event: Arc<StoredEvent>) -> Result<(), HubError> {
        self.delivered.push(event.event_type().to_owned());
        Ok(())
    }
    fn public(&mut self, _channel: PublicChannel) -> Result<(), HubError> {
        Ok(())
    }
    fn terminate(&mut self, _terminal: Terminal) {}
}

#[test]
fn the_sink_delivers_every_event_and_offers_only_executions() {
    let (feed, mut queue) = PriceAlertFeed::channel(8);
    let mut sink = PriceAlertSink::new(Recorder::default(), feed);
    for (event_type, payload) in [
        ("order_created", json!({"order_id": "x"})),
        ("trade_auto_matched", json!({"trade_id": TRADE.to_string()})),
        ("trade_created", json!({"id": OTHER_TRADE.to_string()})),
    ] {
        sink.private(Arc::new(event(event_type, payload))).unwrap();
    }
    assert_eq!(queue.try_recv(), Ok(TRADE));
    assert!(queue.try_recv().is_err());
}

#[test]
fn a_full_queue_never_blocks_or_fails_delivery() {
    let (feed, mut queue) = PriceAlertFeed::channel(1);
    let mut sink = PriceAlertSink::new(Recorder::default(), feed);
    for _ in 0..5 {
        let execution = event("trade_confirmed", json!({"id": TRADE.to_string()}));
        assert!(sink.private(Arc::new(execution)).is_ok());
    }
    assert_eq!(queue.try_recv(), Ok(TRADE));
    assert!(queue.try_recv().is_err());
}

#[test]
fn a_stopped_evaluator_never_fails_delivery() {
    let (feed, queue) = PriceAlertFeed::channel(1);
    drop(queue);
    let mut sink = PriceAlertSink::new(Recorder::default(), feed);
    let execution = event("trade_confirmed", json!({"id": TRADE.to_string()}));
    assert!(sink.private(Arc::new(execution)).is_ok());
}

/// Records each evaluation and fails the scheduled ones.
struct Script {
    calls: Arc<Mutex<Vec<Uuid>>>,
    failures: Vec<Uuid>,
}

impl AlertEvaluator for Script {
    async fn evaluate(&mut self, trade_id: Uuid) -> Result<usize, EvaluationError> {
        self.calls.lock().unwrap().push(trade_id);
        if let Some(position) = self.failures.iter().position(|failed| *failed == trade_id) {
            self.failures.remove(position);
            return Err(EvaluationError);
        }
        Ok(0)
    }
}

async fn wait_for_calls(calls: &Arc<Mutex<Vec<Uuid>>>, count: usize) {
    for _ in 0..200 {
        if calls.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("evaluator did not reach {count} calls");
}

#[tokio::test]
async fn a_failed_evaluation_is_retried_with_the_next_event() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let evaluator = Script {
        calls: calls.clone(),
        failures: vec![TRADE],
    };
    let (sender, queue) = mpsc::channel(8);
    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(run_worker(evaluator, queue, shutdown));

    sender.send(TRADE).await.unwrap();
    wait_for_calls(&calls, 1).await;
    sender.send(OTHER_TRADE).await.unwrap();
    wait_for_calls(&calls, 3).await;

    // The next event first retries the failed trade, then evaluates itself.
    assert_eq!(*calls.lock().unwrap(), vec![TRADE, TRADE, OTHER_TRADE]);
    stop.send_replace(true);
    worker.await.unwrap();
}

#[tokio::test]
async fn a_trade_is_abandoned_after_its_attempts() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let evaluator = Script {
        calls: calls.clone(),
        failures: vec![TRADE, TRADE, TRADE, TRADE],
    };
    let (sender, queue) = mpsc::channel(8);
    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(run_worker(evaluator, queue, shutdown));

    sender.send(TRADE).await.unwrap();
    wait_for_calls(&calls, 1).await;
    // Each later event retries the failed trade once, then evaluates itself.
    for expected_calls in [3, 5, 6] {
        sender.send(OTHER_TRADE).await.unwrap();
        wait_for_calls(&calls, expected_calls).await;
    }

    // Three attempts for the failing trade, then it is not tried again.
    let seen = calls.lock().unwrap().clone();
    assert_eq!(seen.iter().filter(|id| **id == TRADE).count(), 3);
    stop.send_replace(true);
    worker.await.unwrap();
}

#[tokio::test]
async fn the_worker_stops_on_shutdown() {
    let evaluator = Script {
        calls: Arc::new(Mutex::new(Vec::new())),
        failures: Vec::new(),
    };
    let (_sender, queue) = mpsc::channel(1);
    let (stop, shutdown) = watch::channel(false);
    let worker = tokio::spawn(run_worker(evaluator, queue, shutdown));
    stop.send_replace(true);
    tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .expect("worker stops")
        .unwrap();
}
