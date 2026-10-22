use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use uuid::Uuid;
use verdaxis_read::streams::{
    bus::{Delivery, PublicChannel, StoredEvent, StreamHub, Terminal},
    policy::StreamPolicy,
};

fn event(seq: i64, participants: Vec<Uuid>) -> Arc<StoredEvent> {
    Arc::new(
        StoredEvent::new(
            seq,
            participants,
            "trade_created",
            json!({"quantity": "12", "_public_market_invalidation": true}),
        )
        .unwrap(),
    )
}

async fn next(subscription: &mut verdaxis_read::streams::bus::Subscription) -> Option<Delivery> {
    tokio::time::timeout(Duration::from_secs(2), subscription.recv())
        .await
        .unwrap()
}

#[tokio::test]
async fn t01() {
    let hub = StreamHub::default();
    let org = Uuid::new_v4();
    let mut subscription = hub.subscribe_org(org).unwrap();
    for seq in 1..=100 {
        hub.publish_private(event(seq, vec![org])).unwrap();
    }
    assert_eq!(hub.subscriber_count(), 1);
    hub.publish_private(event(101, vec![org])).unwrap();
    assert_eq!(hub.subscriber_count(), 0);
    assert!(matches!(
        next(&mut subscription).await,
        Some(Delivery::Terminal(Terminal::Overflow))
    ));
    assert!(next(&mut subscription).await.is_none());
}

#[test]
fn t02_through_t09() {
    let cases = [
        (
            Terminal::Overflow,
            "reset",
            "subscriber_overflow",
            true,
            false,
        ),
        (
            Terminal::Retention,
            "reset",
            "replay_retention",
            true,
            false,
        ),
        (Terminal::CursorAhead, "reset", "cursor_ahead", true, false),
        (
            Terminal::AuthExpired,
            "auth_expired",
            "token_expired",
            false,
            true,
        ),
        (
            Terminal::AuthRevoked,
            "auth_revoked",
            "authorization_changed",
            true,
            true,
        ),
        (
            Terminal::AuthorizationUnavailable,
            "auth_revoked",
            "authorization_unavailable",
            true,
            true,
        ),
        (
            Terminal::Unavailable,
            "error",
            "stream_unavailable",
            true,
            true,
        ),
        (
            Terminal::Capacity,
            "error",
            "connection_capacity",
            false,
            true,
        ),
        (
            Terminal::InvalidEvent,
            "reset",
            "event_unavailable",
            true,
            false,
        ),
        (
            Terminal::Shutdown,
            "reset",
            "service_restarting",
            true,
            false,
        ),
    ];
    for (terminal, name, reason, resync, error) in cases {
        assert_eq!(terminal.event_name(), name);
        let payload = terminal.payload();
        assert_eq!(payload["schema_version"], 1);
        assert_eq!(payload["reason"], reason);
        assert_eq!(payload["resync_required"], resync);
        assert_eq!(payload.get("error").is_some(), error);
        assert_eq!(
            payload.as_object().unwrap().len(),
            if error { 4 } else { 3 }
        );
        assert!(!format!("{:?}", terminal.into_event()).contains("id:"));
    }
}

#[test]
fn b01() {
    let hub = StreamHub::default();
    let mut subscriptions = Vec::new();
    for _ in 0..200 {
        subscriptions.push(hub.subscribe_public(PublicChannel::Prices).unwrap());
    }
    assert!(hub.subscribe_public(PublicChannel::Prices).is_err());
    assert_eq!(hub.subscriber_count(), 200);
    drop(subscriptions.pop());
    subscriptions.push(hub.subscribe_public(PublicChannel::Prices).unwrap());
    for _ in 0..9 {
        let org = Uuid::new_v4();
        for _ in 0..200 {
            subscriptions.push(hub.subscribe_org(org).unwrap());
        }
    }
    assert_eq!(hub.subscriber_count(), 2000);
    assert!(hub.subscribe_org(Uuid::new_v4()).is_err());
    drop(subscriptions);
    assert_eq!(hub.subscriber_count(), 0);
    assert!(hub.subscribe_org(Uuid::new_v4()).is_ok());
}

#[tokio::test]
async fn b02() {
    let hub = StreamHub::default();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut sa = hub.subscribe_org(a).unwrap();
    let mut sb = hub.subscribe_org(b).unwrap();
    let mut prices = hub.subscribe_public(PublicChannel::Prices).unwrap();
    let mut book = hub.subscribe_public(PublicChannel::Orderbook).unwrap();
    let shared = event(1, vec![a, a]);
    hub.publish_private(shared.clone()).unwrap();
    match next(&mut sa).await.unwrap() {
        Delivery::Private(received) => {
            assert!(Arc::ptr_eq(&shared, &received));
            assert!(
                received
                    .payload()
                    .get("_public_market_invalidation")
                    .is_none()
            );
            assert_eq!(received.payload()["schema_version"], 1);
        }
        other => panic!("unexpected {other:?}"),
    }
    hub.invalidate_public(PublicChannel::Prices).unwrap();
    assert!(matches!(
        next(&mut prices).await,
        Some(Delivery::PublicInvalidation)
    ));
    for subscription in [&mut sa, &mut sb, &mut prices, &mut book] {
        assert!(
            tokio::time::timeout(Duration::from_millis(10), subscription.recv())
                .await
                .is_err()
        );
    }
    hub.terminate_all(Terminal::Shutdown);
    for subscription in [&mut sa, &mut sb, &mut prices, &mut book] {
        assert!(matches!(
            next(subscription).await,
            Some(Delivery::Terminal(Terminal::Shutdown))
        ));
        assert!(next(subscription).await.is_none());
    }
}

#[tokio::test]
async fn b03() {
    let hub = StreamHub::default();
    let org = Uuid::new_v4();
    let mut subscription = hub.subscribe_org(org).unwrap();
    hub.publish_private(event(1, vec![org])).unwrap();
    subscription.terminate(Terminal::Retention);
    subscription.terminate(Terminal::Unavailable);
    assert_eq!(hub.subscriber_count(), 0);
    assert!(matches!(
        next(&mut subscription).await,
        Some(Delivery::Terminal(Terminal::Retention))
    ));
    assert!(next(&mut subscription).await.is_none());
}

#[tokio::test]
async fn b04() {
    let hub = StreamHub::default();
    let mut subscription = hub.subscribe_org(Uuid::new_v4()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), subscription.recv())
            .await
            .is_err()
    );
    assert_eq!(hub.subscriber_count(), 1);
    let task = tokio::spawn(async move { subscription.recv().await });
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(hub.subscriber_count(), 0);
}

#[tokio::test]
async fn b07() {
    use axum::{
        body::to_bytes,
        response::{IntoResponse, Sse},
    };
    use futures_util::stream;

    for terminal in [
        Terminal::Overflow,
        Terminal::Retention,
        Terminal::CursorAhead,
        Terminal::AuthExpired,
        Terminal::AuthRevoked,
        Terminal::AuthorizationUnavailable,
        Terminal::Unavailable,
        Terminal::Capacity,
        Terminal::InvalidEvent,
        Terminal::Shutdown,
    ] {
        let body = Sse::new(stream::once(async move {
            Ok::<_, std::convert::Infallible>(terminal.into_event())
        }))
        .into_response()
        .into_body();
        let bytes = to_bytes(body, 4096).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("event:"))
                .count(),
            1
        );
        assert!(!lines.iter().any(|line| line.starts_with("id:")));
        let data = lines
            .iter()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(data).unwrap(),
            terminal.payload()
        );
    }
}

#[test]
fn b08() {
    let org = Uuid::new_v4();
    assert!(StoredEvent::new(1, vec![org], &"a".repeat(80), json!({})).is_ok());
    assert!(StoredEvent::new(1, vec![org], &"a".repeat(81), json!({})).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b09() {
    let hub = StreamHub::new(StreamPolicy::new(1, 1, 1).unwrap());
    let org = Uuid::new_v4();
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let hub = hub.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            hub.subscribe_org(org)
        }));
    }
    barrier.wait().await;
    let mut accepted = Vec::new();
    for task in tasks {
        if let Ok(subscription) = task.await.unwrap() {
            accepted.push(subscription);
        }
    }
    assert_eq!(accepted.len(), 1);
    assert_eq!(hub.subscriber_count(), 1);
    let mut subscription = accepted.pop().unwrap();
    hub.publish_private(event(1, vec![org])).unwrap();
    hub.publish_private(event(2, vec![org])).unwrap();
    assert!(matches!(
        next(&mut subscription).await,
        Some(Delivery::Terminal(Terminal::Overflow))
    ));
    assert_eq!(hub.subscriber_count(), 0);
    let replacement = hub.subscribe_org(org).unwrap();
    drop(subscription);
    assert_eq!(hub.subscriber_count(), 1);
    drop(replacement);
    assert_eq!(hub.subscriber_count(), 0);
}

#[test]
fn b05() {
    for limits in [
        (0, 2000, 100),
        (201, 2000, 100),
        (200, 2001, 100),
        (200, 2000, 101),
        (200, 0, 100),
        (200, 2000, 0),
    ] {
        assert!(StreamPolicy::new(limits.0, limits.1, limits.2).is_err());
    }
    assert!(StreamPolicy::new(1, 1, 1).is_ok());
    let org = Uuid::new_v4();
    for name in ["", "trade\ncreated", "trade\rcreated", "trade\0created"] {
        assert!(StoredEvent::new(1, vec![org], name, json!({})).is_err());
    }
    assert!(StoredEvent::new(0, vec![org], "trade_created", json!({})).is_err());
    assert!(StoredEvent::new(1, vec![org], "trade_created", Value::Null).is_err());
    assert!(StoredEvent::new(1, vec![org; 257], "trade_created", json!({})).is_err());
    assert!(
        StoredEvent::new(
            1,
            vec![org],
            "trade_created",
            json!({"data": "a".repeat(256 * 1024)})
        )
        .is_err()
    );
}

#[tokio::test]
async fn b06() {
    let hub = StreamHub::default();
    let mut subscriptions = Vec::new();
    let org = Uuid::new_v4();
    for _ in 0..200 {
        subscriptions.push(hub.subscribe_org(org).unwrap());
    }
    let publisher = hub.clone();
    tokio::task::spawn_blocking(move || {
        for seq in 1..=101 {
            publisher.publish_private(event(seq, vec![org])).unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(hub.subscriber_count(), 0);
    for mut subscription in subscriptions {
        assert!(matches!(
            next(&mut subscription).await,
            Some(Delivery::Terminal(Terminal::Overflow))
        ));
        assert!(next(&mut subscription).await.is_none());
    }
}
