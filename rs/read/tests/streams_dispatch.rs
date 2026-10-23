//! Ordered scan, bounded scheduling, and channel control contracts.

use serde_json::json;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;
use verdaxis_read::streams::{
    bus::{Delivery, HubError, PublicChannel, StoredEvent, StreamHub, Terminal},
    dispatch::{Boundary, Dispatch, DispatchError, DispatchSink, DispatchStore, ScanPage, ScanRow},
    policy::{MAX_EVENT_BYTES, MAX_PAGE_BYTES, MAX_PAGE_ROWS, MAX_PARTICIPANTS},
};

struct Store {
    boundary: Boundary,
    pages: VecDeque<Result<ScanPage, DispatchError>>,
    reads: usize,
}
impl Store {
    fn new(pages: Vec<Result<ScanPage, DispatchError>>) -> Self {
        Self {
            boundary: Boundary::new(0, 0).unwrap(),
            pages: pages.into(),
            reads: 0,
        }
    }
}
impl DispatchStore for Store {
    async fn boundary(&mut self) -> Result<Boundary, DispatchError> {
        Ok(self.boundary)
    }
    async fn scan(&mut self, _after: i64) -> Result<ScanPage, DispatchError> {
        self.reads += 1;
        self.pages
            .pop_front()
            .unwrap_or_else(|| Ok(page(0, 10000, vec![], false)))
    }
}
fn event(seq: i64, org: Uuid, marked: bool) -> Arc<StoredEvent> {
    Arc::new(
        StoredEvent::new(
            seq,
            vec![org],
            "trade_created",
            json!({"quantity": "2", "_public_market_invalidation": marked}),
        )
        .unwrap(),
    )
}
fn row(seq: i64, org: Uuid, marked: bool) -> ScanRow {
    ScanRow::event(event(seq, org, marked), 100).unwrap()
}
fn page(floor: i64, high: i64, rows: Vec<ScanRow>, more: bool) -> ScanPage {
    ScanPage::new(Boundary::new(floor, high).unwrap(), rows, more).unwrap()
}
async fn ready(store: &mut Store, hub: StreamHub) -> Dispatch<StreamHub> {
    Dispatch::initialize(store, hub).await.unwrap()
}
async fn private_seq(sub: &mut verdaxis_read::streams::bus::Subscription) -> i64 {
    match tokio::time::timeout(Duration::from_secs(1), sub.recv())
        .await
        .unwrap()
        .unwrap()
    {
        Delivery::Private(e) => e.seq(),
        other => panic!("expected private event: {other:?}"),
    }
}

#[tokio::test]
async fn sorted_org_scope() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let hub = StreamHub::default();
    let mut store = Store::new(vec![Ok(page(
        0,
        9,
        vec![row(2, a, false), row(7, b, true), row(9, a, false)],
        false,
    ))]);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut sa = hub.subscribe_org(a).unwrap();
    let mut sb = hub.subscribe_org(b).unwrap();
    let mut prices = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let mut book = hub.subscribe_public(PublicChannel::Orderbook).unwrap();
    drain.turn(&mut store).await.unwrap();
    assert_eq!(private_seq(&mut sa).await, 2);
    assert_eq!(private_seq(&mut sa).await, 9);
    assert_eq!(private_seq(&mut sb).await, 7);
    // The other organization's rows never cross the scope boundary.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), sb.recv())
            .await
            .is_err()
    );
    assert!(matches!(
        prices.recv().await,
        Some(Delivery::PublicInvalidation)
    ));
    assert!(matches!(
        book.recv().await,
        Some(Delivery::PublicInvalidation)
    ));
    assert_eq!(drain.cursor(), 9);
}

#[tokio::test]
async fn unsorted_repeated_participants_scope() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let unrelated = Uuid::new_v4();
    let hub = StreamHub::default();
    // One row whose stored participant list is unsorted and repeats an org.
    let duplicated = Arc::new(
        StoredEvent::new(5, vec![b, a, a], "trade_created", json!({"quantity": "2"})).unwrap(),
    );
    let mut store = Store::new(vec![Ok(page(
        0,
        9,
        vec![ScanRow::event(duplicated, 100).unwrap()],
        false,
    ))]);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut sa = hub.subscribe_org(a).unwrap();
    let mut sb = hub.subscribe_org(b).unwrap();
    let mut sc = hub.subscribe_org(unrelated).unwrap();
    drain.turn(&mut store).await.unwrap();
    // Each listed organization receives the row exactly once.
    assert_eq!(private_seq(&mut sa).await, 5);
    assert_eq!(private_seq(&mut sb).await, 5);
    for sub in [&mut sa, &mut sb] {
        assert!(
            tokio::time::timeout(Duration::from_millis(50), sub.recv())
                .await
                .is_err(),
            "a repeated participant must not deliver twice"
        );
    }
    // An organization outside the participant list receives nothing.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), sc.recv())
            .await
            .is_err()
    );
    assert_eq!(drain.cursor(), 5);
}

#[test]
fn marker_removed() {
    let marked = StoredEvent::new(
        1,
        vec![Uuid::new_v4()],
        "trade_created",
        json!({"_public_market_invalidation": true, "quantity": "2"}),
    )
    .unwrap();
    assert!(marked.public_invalidated());
    assert!(
        marked
            .payload()
            .get("_public_market_invalidation")
            .is_none()
    );
    let plain = StoredEvent::new(
        2,
        vec![Uuid::new_v4()],
        "trade_created",
        json!({"_public_market_invalidation": false}),
    )
    .unwrap();
    assert!(!plain.public_invalidated());
    assert!(plain.payload().get("_public_market_invalidation").is_none());
}

#[test]
fn legacy_schema_version() {
    let legacy = StoredEvent::new(
        1,
        vec![Uuid::new_v4()],
        "trade_created",
        json!({"quantity": "2"}),
    )
    .unwrap();
    assert_eq!(legacy.payload()["schema_version"], 1);
    let current = StoredEvent::new(
        2,
        vec![Uuid::new_v4()],
        "trade_created",
        json!({"schema_version": 1, "quantity": "2"}),
    )
    .unwrap();
    assert_eq!(current.payload()["schema_version"], 1);
}

#[tokio::test]
async fn scan_failure_preserves_pending_channels() {
    let hub = StreamHub::default();
    let org = Uuid::new_v4();
    let mut store = Store::new(vec![
        Ok(page(0, 2, vec![row(1, org, true)], true)),
        Err(DispatchError::Store),
    ]);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut p = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let mut b = hub.subscribe_public(PublicChannel::Orderbook).unwrap();
    assert_eq!(drain.turn(&mut store).await, Err(DispatchError::Store));
    assert_eq!(drain.cursor(), 1);
    drain.turn(&mut store).await.unwrap();
    assert!(matches!(p.recv().await, Some(Delivery::PublicInvalidation)));
    assert!(matches!(b.recv().await, Some(Delivery::PublicInvalidation)));
}

struct Sink {
    hub: StreamHub,
    fail: Option<PublicChannel>,
    fail_private: bool,
}
impl DispatchSink for Sink {
    fn private(&mut self, event: Arc<StoredEvent>) -> Result<(), HubError> {
        if self.fail_private {
            self.fail_private = false;
            return Err(HubError);
        }
        self.hub.publish_private(event)
    }
    fn public(&mut self, channel: PublicChannel) -> Result<(), HubError> {
        if self.fail == Some(channel) {
            self.fail = None;
            return Err(HubError);
        }
        self.hub.invalidate_public(channel)
    }
    fn terminate(&mut self, terminal: Terminal) {
        self.hub.terminate_all(terminal);
    }
}
#[tokio::test]
async fn partial_public_acceptance_retains_other_channel() {
    let hub = StreamHub::default();
    let org = Uuid::new_v4();
    let mut store = Store::new(vec![Ok(page(0, 1, vec![row(1, org, true)], false))]);
    let sink = Sink {
        hub: hub.clone(),
        fail: Some(PublicChannel::Orderbook),
        fail_private: false,
    };
    let mut drain = Dispatch::initialize(&mut store, sink).await.unwrap();
    let mut p = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let mut b = hub.subscribe_public(PublicChannel::Orderbook).unwrap();
    assert_eq!(drain.turn(&mut store).await, Err(DispatchError::Publish));
    assert_eq!(drain.pending_channels(), [false, true]);
    drain.turn(&mut store).await.unwrap();
    assert_eq!(drain.pending_channels(), [false, false]);
    assert!(matches!(p.recv().await, Some(Delivery::PublicInvalidation)));
    assert!(matches!(b.recv().await, Some(Delivery::PublicInvalidation)));
}
#[tokio::test]
async fn private_failure_records_public_obligation_before_cursor() {
    let hub = StreamHub::default();
    let org = Uuid::new_v4();
    let mut store = Store::new(vec![Ok(page(0, 1, vec![row(1, org, true)], false))]);
    let sink = Sink {
        hub,
        fail: None,
        fail_private: true,
    };
    let mut drain = Dispatch::initialize(&mut store, sink).await.unwrap();
    assert_eq!(drain.turn(&mut store).await, Err(DispatchError::Publish));
    assert_eq!(drain.cursor(), 0);
    assert_eq!(drain.pending_channels(), [true, true]);
}
#[test]
fn event_size_boundary() {
    let org = Uuid::new_v4();
    let e = event(1, org, false);
    assert!(ScanRow::event(e.clone(), MAX_EVENT_BYTES).is_ok());
    assert!(ScanRow::event(e, MAX_EVENT_BYTES + 1).is_err());
}
#[test]
fn participant_count_boundary() {
    let payload = json!({"x":1});
    assert!(
        StoredEvent::new(
            1,
            vec![Uuid::nil(); MAX_PARTICIPANTS],
            "trade_created",
            payload.clone()
        )
        .is_ok()
    );
    assert!(
        StoredEvent::new(
            1,
            vec![Uuid::nil(); MAX_PARTICIPANTS + 1],
            "trade_created",
            payload
        )
        .is_err()
    );
}
#[test]
fn page_byte_boundary() {
    let org = Uuid::new_v4();
    let rows = || {
        (1..=8)
            .map(|seq| ScanRow::event(event(seq, org, false), MAX_EVENT_BYTES).unwrap())
            .collect()
    };
    assert_eq!(MAX_PAGE_BYTES, 8 * MAX_EVENT_BYTES);
    assert!(ScanPage::new(Boundary::new(0, 9).unwrap(), rows(), true).is_ok());
    let mut excess = rows();
    excess.push(row(9, org, false));
    assert!(ScanPage::new(Boundary::new(0, 9).unwrap(), excess, false).is_err());
    let rows = (1..=MAX_PAGE_ROWS + 1)
        .map(|s| row(s as i64, org, false))
        .collect();
    assert!(ScanPage::new(Boundary::new(0, 1000).unwrap(), rows, false).is_err());
}
#[test]
fn unordered_or_out_of_boundary_pages_are_rejected() {
    let org = Uuid::new_v4();
    for seqs in [[2, 1], [1, 1], [1, 4]] {
        assert!(
            ScanPage::new(
                Boundary::new(0, 3).unwrap(),
                seqs.into_iter().map(|s| row(s, org, false)).collect(),
                false
            )
            .is_err()
        );
    }
    assert!(ScanPage::new(Boundary::new(0, 3).unwrap(), vec![], true).is_err());
    assert!(Boundary::new(4, 3).is_err());
}
#[tokio::test]
async fn hub_turn_control_fairness() {
    let org = Uuid::new_v4();
    let hub = StreamHub::default();
    let pages = (1..=6)
        .map(|s| Ok(page(0, 6, vec![row(s, org, true)], true)))
        .collect();
    let mut store = Store::new(pages);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut p = hub.subscribe_public(PublicChannel::Prices).unwrap();
    assert!(drain.turn(&mut store).await.unwrap().more);
    assert_eq!(store.reads, 4);
    assert_eq!(drain.cursor(), 4);
    assert!(matches!(p.recv().await, Some(Delivery::PublicInvalidation)));
}
#[tokio::test]
async fn invalid_row_resets_and_scan_can_continue() {
    let org = Uuid::new_v4();
    let hub = StreamHub::default();
    let mut store = Store::new(vec![Ok(page(
        0,
        3,
        vec![ScanRow::invalid(2).unwrap(), row(3, org, true)],
        false,
    ))]);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut sub = hub.subscribe_org(org).unwrap();
    drain.turn(&mut store).await.unwrap();
    assert!(matches!(
        sub.recv().await,
        Some(Delivery::Terminal(Terminal::InvalidEvent))
    ));
    assert!(sub.recv().await.is_none());
    assert_eq!(drain.cursor(), 3);
}
#[tokio::test]
async fn floor_overtaken_hub_resets_before_new_rows() {
    let org = Uuid::new_v4();
    let hub = StreamHub::default();
    let mut store = Store::new(vec![Ok(page(4, 8, vec![row(8, org, false)], false))]);
    let mut drain = ready(&mut store, hub.clone()).await;
    let mut sub = hub.subscribe_org(org).unwrap();
    drain.turn(&mut store).await.unwrap();
    assert!(matches!(
        sub.recv().await,
        Some(Delivery::Terminal(Terminal::Retention))
    ));
    assert_eq!(drain.cursor(), 8);
}
#[tokio::test]
async fn baseline_is_captured_before_live_scans() {
    let mut store = Store::new(vec![]);
    store.boundary = Boundary::new(3, 9).unwrap();
    let mut drain = ready(&mut store, StreamHub::default()).await;
    assert_eq!(drain.cursor(), 9);
    store
        .pages
        .push_back(Ok(page(3, 10, vec![row(8, Uuid::nil(), false)], false)));
    assert_eq!(
        drain.turn(&mut store).await,
        Err(DispatchError::InvalidPage)
    );
    assert_eq!(drain.cursor(), 9);
}
#[tokio::test]
async fn poll_and_spurious_wakes_read_committed_state() {
    let mut store = Store::new(vec![]);
    let hub = StreamHub::default();
    let mut drain = ready(&mut store, hub).await;
    let (wake_tx, mut wakes) = mpsc::channel(64);
    for _ in 0..40 {
        wake_tx.try_send(()).unwrap();
    }
    let (stop_tx, mut stop) = watch::channel(false);
    let task = tokio::spawn(async move {
        drain.run(&mut store, &mut wakes, &mut stop).await.unwrap();
        store.reads
    });
    tokio::task::yield_now().await;
    stop_tx.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn public_frames_have_no_cursor_or_private_fields() {
    use axum::{
        body::to_bytes,
        response::{IntoResponse, Sse},
    };
    use futures_util::stream;
    let response = Sse::new(stream::once(async {
        Ok::<_, std::convert::Infallible>(Delivery::PublicInvalidation.into_event())
    }))
    .into_response();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.lines().any(|line| line.starts_with("id:")));
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(payload, json!({"schema_version":1,"resync_required":true}));
}

#[tokio::test]
async fn poll_without_notification_drains_again() {
    let mut store = Store::new(vec![]);
    let mut drain = ready(&mut store, StreamHub::default()).await;
    let (_wake_tx, mut wakes) = mpsc::channel(1);
    let (stop_tx, mut stop) = watch::channel(false);
    let task = tokio::spawn(async move {
        drain.run(&mut store, &mut wakes, &mut stop).await.unwrap();
        store.reads
    });
    // One immediate scan, the interval's first tick, then a real poll tick.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    stop_tx.send_replace(true);
    let reads = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(reads >= 3);
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection, Row};
    struct PgStore {
        connection: PgConnection,
    }
    impl DispatchStore for PgStore {
        async fn boundary(&mut self) -> Result<Boundary, DispatchError> {
            let high: i64 =
                sqlx::query_scalar("SELECT COALESCE(max(stream_seq),0) FROM market_event_outbox")
                    .fetch_one(&mut self.connection)
                    .await
                    .map_err(|_| DispatchError::Store)?;
            Boundary::new(0, high)
        }
        async fn scan(&mut self, after: i64) -> Result<ScanPage, DispatchError> {
            let boundary = self.boundary().await?;
            let rows=sqlx::query("SELECT stream_seq, participant_org_ids::text AS participants, payload::text AS payload, event_type FROM market_event_outbox WHERE stream_seq > $1 AND stream_seq <= $2 ORDER BY stream_seq LIMIT 500")
                .bind(after).bind(boundary.high_water()).fetch_all(&mut self.connection).await.map_err(|_|DispatchError::Store)?;
            let mut scanned = Vec::new();
            for row in rows {
                let payload: String = row.get("payload");
                let participants: String = row.get("participants");
                let event = StoredEvent::new(
                    row.get("stream_seq"),
                    serde_json::from_str(&participants).unwrap(),
                    row.get("event_type"),
                    serde_json::from_str(&payload).unwrap(),
                )
                .unwrap();
                scanned.push(ScanRow::event(Arc::new(event), payload.len()).unwrap());
            }
            ScanPage::new(boundary, scanned, false)
        }
    }
    #[tokio::test]
    #[ignore = "requires disposable PostgreSQL"]
    async fn committed_visibility_and_rollback() {
        let url = std::env::var("STREAM_DISPATCH_APP_URL").expect("disposable app URL required");
        let mut producer = PgConnection::connect(&url).await.unwrap();
        let mut store = PgStore {
            connection: PgConnection::connect(&url).await.unwrap(),
        };
        let hub = StreamHub::default();
        let mut drain = Dispatch::initialize(&mut store, hub.clone()).await.unwrap();
        let org = Uuid::new_v4();
        let mut sub = hub.subscribe_org(org).unwrap();
        let mut public = hub.subscribe_public(PublicChannel::Prices).unwrap();
        for commit in [false, true] {
            let mut tx = producer.begin().await.unwrap();
            let seq:i64=sqlx::query_scalar("INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,stream_seq,delivery_attempts,created_at) VALUES ($1,'trade_created','phase6_dispatch',$2,$3::json,$4::json,nextval('market_event_stream_seq'),0,now()) RETURNING stream_seq")
                .bind(Uuid::new_v4()).bind(Uuid::new_v4().to_string()).bind(json!([org]).to_string()).bind(json!({"_public_market_invalidation":true}).to_string()).fetch_one(&mut *tx).await.unwrap();
            let before = drain.cursor();
            drain.turn(&mut store).await.unwrap();
            assert_eq!(drain.cursor(), before);
            if commit {
                tx.commit().await.unwrap();
                drain.turn(&mut store).await.unwrap();
                assert_eq!(private_seq(&mut sub).await, seq);
                assert!(matches!(
                    public.recv().await,
                    Some(Delivery::PublicInvalidation)
                ));
            } else {
                tx.rollback().await.unwrap();
                drain.turn(&mut store).await.unwrap();
                assert_eq!(drain.cursor(), before);
            }
        }
        producer.close().await.unwrap();
        store.connection.close().await.unwrap();
    }
}
