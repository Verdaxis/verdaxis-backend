//! Bounded replay and live delivery using explicit snapshot and clock controls.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::auth::VerificationSettings;
use verdaxis_read::streams::{
    auth::{StreamError, StreamPrincipal, verify_stream},
    bus::{StoredEvent as LiveEvent, StreamHub, Terminal},
    replay::{Replay, ReplayClock, ReplaySource},
    store::{PageLimits, ReplayBoundary, ReplayPage, StoreError, StoredEvent},
};

const SECRET: &str = "replay-test-secret-longer-than-thirty-two-characters";
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_792_497_600, 0).unwrap()
}
fn org() -> Uuid {
    Uuid::from_u128(7)
}
fn principal(user: Uuid, organization: Uuid) -> StreamPrincipal {
    let claims = json!({"sub":user,"org_id":organization,"environment":"test",
        "type":"stream","iss":"test","aud":"test","iat":now().timestamp(),
        "exp":(now()+Duration::seconds(60)).timestamp()});
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    verify_stream(
        &token,
        &VerificationSettings::new(SECRET.into(), None, "test".into(), "test".into()),
        "test",
        now(),
    )
    .unwrap()
}
#[derive(Clone)]
struct Clock(Arc<Mutex<DateTime<Utc>>>);
impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(now())))
    }
    fn advance(&self, seconds: i64) {
        *self.0.lock().unwrap() += Duration::seconds(seconds);
    }
}
impl ReplayClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
fn boundary(floor: i64, high: i64) -> ReplayBoundary {
    ReplayBoundary {
        discarded_through: floor,
        high_water: high,
    }
}
fn event(seq: i64) -> StoredEvent {
    StoredEvent {
        seq,
        participants: vec![org()],
        event_type: "trade_created".into(),
        payload: json!({"schema_version":1,"quantity":"5"}),
        public_invalidated: false,
    }
}
fn page(floor: i64, high: i64, ids: impl IntoIterator<Item = i64>, more: bool) -> ReplayPage {
    ReplayPage {
        boundary: boundary(floor, high),
        events: ids.into_iter().map(event).collect(),
        more,
    }
}
#[derive(Clone)]
struct Source {
    state: Arc<Mutex<State>>,
}
struct State {
    boundary: ReplayBoundary,
    pages: VecDeque<ReplayPage>,
    auth: Result<(), StreamError>,
    calls: Vec<(i64, i64)>,
    on_boundary: Option<Box<dyn FnOnce() + Send>>,
}
impl Source {
    fn new(floor: i64, high: i64, pages: Vec<ReplayPage>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                boundary: boundary(floor, high),
                pages: pages.into(),
                auth: Ok(()),
                calls: vec![],
                on_boundary: None,
            })),
        }
    }
}
impl ReplaySource for Source {
    async fn boundary(&self) -> Result<ReplayBoundary, StoreError> {
        let mut state = self.state.lock().unwrap();
        if let Some(action) = state.on_boundary.take() {
            action();
        }
        Ok(state.boundary)
    }
    async fn page(
        &self,
        organization: Uuid,
        after: i64,
        through: i64,
        _limits: PageLimits,
    ) -> Result<ReplayPage, StoreError> {
        assert_eq!(organization, org());
        let mut state = self.state.lock().unwrap();
        state.calls.push((after, through));
        Ok(state
            .pages
            .pop_front()
            .unwrap_or_else(|| page(0, through, [], false)))
    }
    async fn revalidate(&self, _principal: &StreamPrincipal) -> Result<(), StreamError> {
        self.state.lock().unwrap().auth
    }
}
async fn open(
    source: Source,
    hub: &StreamHub,
    cursor: Option<i64>,
    clock: Clock,
) -> Replay<Source, Clock> {
    Replay::open(
        source,
        hub,
        principal(Uuid::from_u128(8), org()),
        cursor,
        clock,
    )
    .await
    .unwrap()
}
async fn id<S: ReplaySource, C: ReplayClock>(replay: &mut Replay<S, C>) -> i64 {
    replay
        .next()
        .await
        .unwrap()
        .sequence()
        .expect("business frame")
}
async fn terminal<S: ReplaySource, C: ReplayClock>(replay: &mut Replay<S, C>, expected: Terminal) {
    let frame = replay.next().await.unwrap();
    assert_eq!(frame.terminal(), Some(expected));
    let response = axum::response::IntoResponse::into_response(axum::response::sse::Sse::new(
        futures_util::stream::iter([Ok::<_, std::convert::Infallible>(frame.into_event())]),
    ));
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.lines().any(|line| line.starts_with("id:")));
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(
        payload["resync_required"],
        expected.payload()["resync_required"]
    );
    assert_eq!(payload["reason"], expected.payload()["reason"]);
    assert!(replay.next().await.is_none());
}
fn publish(hub: &StreamHub, seq: i64) {
    hub.publish_private(Arc::new(
        LiveEvent::new(seq, vec![org()], "trade_created", json!({"quantity":"5"})).unwrap(),
    ))
    .unwrap();
}

#[tokio::test]
async fn stale_cursor_receives_terminal_reset() {
    let hub = StreamHub::default();
    let mut r = open(Source::new(10, 20, vec![]), &hub, Some(9), Clock::new()).await;
    terminal(&mut r, Terminal::Retention).await;
    assert_eq!(hub.subscriber_count(), 0);
}
#[tokio::test]
async fn equal_cursor_replays_above_floor() {
    let source = Source::new(10, 12, vec![page(10, 12, [12], false)]);
    let hub = StreamHub::default();
    let mut r = open(source, &hub, Some(10), Clock::new()).await;
    assert_eq!(id(&mut r).await, 12);
}
#[tokio::test]
async fn future_cursor_receives_terminal_reset() {
    let hub = StreamHub::default();
    let mut r = open(
        Source::new(0, 20, vec![]),
        &hub,
        Some(i64::MAX),
        Clock::new(),
    )
    .await;
    terminal(&mut r, Terminal::CursorAhead).await;
}
#[tokio::test]
async fn replay_601_rows_with_foreign_sequence_holes() {
    let source = Source::new(
        0,
        2000,
        vec![
            page(0, 2000, (1..=500).map(|n| n * 3), true),
            page(0, 2000, (501..=601).map(|n| n * 3), false),
        ],
    );
    let hub = StreamHub::default();
    let mut r = open(source.clone(), &hub, Some(0), Clock::new()).await;
    for n in 1..=601 {
        assert_eq!(id(&mut r).await, n * 3);
    }
    assert_eq!(
        source.state.lock().unwrap().calls,
        vec![(0, 2000), (1500, 2000)]
    );
}
#[tokio::test]
async fn subscribe_before_capture_merges_live_overlap() {
    let hub = StreamHub::default();
    let source = Source::new(0, 2, vec![page(0, 2, [1, 2], false)]);
    let publisher = hub.clone();
    source.state.lock().unwrap().on_boundary = Some(Box::new(move || {
        publish(&publisher, 2);
        publish(&publisher, 3);
    }));
    let mut r = open(source, &hub, Some(0), Clock::new()).await;
    for expected in [1, 2, 3] {
        assert_eq!(id(&mut r).await, expected);
    }
}
#[tokio::test]
async fn no_cursor_uses_post_subscription_baseline() {
    let hub = StreamHub::default();
    let source = Source::new(8, 10, vec![]);
    let publisher = hub.clone();
    source.state.lock().unwrap().on_boundary = Some(Box::new(move || {
        publish(&publisher, 10);
        publish(&publisher, 11);
    }));
    let mut r = open(source.clone(), &hub, None, Clock::new()).await;
    assert_eq!(id(&mut r).await, 11);
    assert!(source.state.lock().unwrap().calls.is_empty());
}
#[tokio::test]
async fn prune_before_page_snapshot_resets() {
    let hub = StreamHub::default();
    let mut r = open(
        Source::new(0, 10, vec![page(5, 10, [6], false)]),
        &hub,
        Some(0),
        Clock::new(),
    )
    .await;
    terminal(&mut r, Terminal::Retention).await;
}
#[tokio::test]
async fn prune_between_pages_resets() {
    let hub = StreamHub::default();
    let mut r = open(
        Source::new(0, 10, vec![page(0, 10, [2], true), page(5, 10, [6], false)]),
        &hub,
        Some(0),
        Clock::new(),
    )
    .await;
    assert_eq!(id(&mut r).await, 2);
    terminal(&mut r, Terminal::Retention).await;
}
#[tokio::test]
async fn prune_after_snapshot_keeps_captured_page() {
    let hub = StreamHub::default();
    let source = Source::new(
        0,
        10,
        vec![page(0, 10, [2, 4], true), page(4, 10, [8], false)],
    );
    let mut r = open(source.clone(), &hub, Some(0), Clock::new()).await;
    assert_eq!(id(&mut r).await, 2);
    source.state.lock().unwrap().boundary.discarded_through = 4;
    assert_eq!(id(&mut r).await, 4);
    assert_eq!(id(&mut r).await, 8);
}
#[tokio::test]
async fn empty_table_preserves_nonzero_floor() {
    let hub = StreamHub::default();
    let mut r = open(Source::new(9, 9, vec![]), &hub, Some(8), Clock::new()).await;
    terminal(&mut r, Terminal::Retention).await;
    let mut equal = open(Source::new(9, 9, vec![]), &hub, Some(9), Clock::new()).await;
    publish(&hub, 10);
    assert_eq!(id(&mut equal).await, 10);
}
#[tokio::test]
async fn expiry_mid_page_precedes_next_business_frame() {
    let hub = StreamHub::default();
    let clock = Clock::new();
    let mut r = open(
        Source::new(0, 2, vec![page(0, 2, [1, 2], false)]),
        &hub,
        Some(0),
        clock.clone(),
    )
    .await;
    assert_eq!(id(&mut r).await, 1);
    clock.advance(60);
    terminal(&mut r, Terminal::AuthExpired).await;
}
#[tokio::test]
async fn authorization_mid_page_never_rebinds_scope() {
    let hub = StreamHub::default();
    let clock = Clock::new();
    let source = Source::new(0, 2, vec![page(0, 2, [1, 2], false)]);
    let mut r = open(source.clone(), &hub, Some(0), clock.clone()).await;
    assert_eq!(id(&mut r).await, 1);
    source.state.lock().unwrap().auth = Err(StreamError::TokenInvalid);
    clock.advance(15);
    terminal(&mut r, Terminal::AuthRevoked).await;
}
#[tokio::test]
async fn authorization_store_failure_is_terminal() {
    let hub = StreamHub::default();
    let clock = Clock::new();
    let source = Source::new(0, 2, vec![page(0, 2, [1, 2], false)]);
    let mut r = open(source.clone(), &hub, Some(0), clock.clone()).await;
    assert_eq!(id(&mut r).await, 1);
    source.state.lock().unwrap().auth = Err(StreamError::AuthorizationUnavailable);
    clock.advance(15);
    terminal(&mut r, Terminal::AuthorizationUnavailable).await;
}
#[tokio::test]
async fn overflow_during_replay_discards_page() {
    let hub = StreamHub::default();
    let mut r = open(
        Source::new(0, 2, vec![page(0, 2, [1, 2], false)]),
        &hub,
        Some(0),
        Clock::new(),
    )
    .await;
    assert_eq!(id(&mut r).await, 1);
    for n in 3..=103 {
        publish(&hub, n);
    }
    terminal(&mut r, Terminal::Overflow).await;
    assert_eq!(hub.subscriber_count(), 0);
}
#[tokio::test]
async fn overflow_during_live_discards_queue() {
    let hub = StreamHub::default();
    let mut r = open(Source::new(0, 0, vec![]), &hub, None, Clock::new()).await;
    for n in 1..=101 {
        publish(&hub, n);
    }
    terminal(&mut r, Terminal::Overflow).await;
}
#[tokio::test]
async fn client_cancel_releases_subscription() {
    let hub = StreamHub::default();
    let mut r = open(Source::new(0, 0, vec![]), &hub, None, Clock::new()).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), r.next())
            .await
            .is_err()
    );
    assert_eq!(hub.subscriber_count(), 1);
    drop(r);
    assert_eq!(hub.subscriber_count(), 0);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL replay fixture"]
async fn postgres_replay_snapshots() {
    postgres::run().await;
}

mod postgres {
    use super::*;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use verdaxis_read::streams::replay::PgReplaySource;
    async fn execute(pool: &PgPool, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(pool)
            .await
            .unwrap();
    }
    async fn insert(pool: &PgPool, seq: i64, organization: Uuid) {
        sqlx::query("INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at,stream_seq) VALUES ($1,'trade_created','trade','fixture',$2,$3,now(),$4)")
            .bind(Uuid::new_v4()).bind(json!([organization])).bind(json!({"quantity":"5"})).bind(seq).execute(pool).await.unwrap();
    }
    pub async fn run() {
        let app = PgPoolOptions::new()
            .max_connections(1)
            .connect(&std::env::var("STREAM_REPLAY_APP_URL").expect("disposable app URL required"))
            .await
            .unwrap();
        let owner = PgPoolOptions::new()
            .max_connections(2)
            .connect(
                &std::env::var("STREAM_REPLAY_OWNER_URL").expect("disposable owner URL required"),
            )
            .await
            .unwrap();
        let user: Uuid = std::env::var("STREAM_REPLAY_USER")
            .unwrap()
            .parse()
            .unwrap();
        execute(&owner,"UPDATE market_event_retention_control SET mode='rust_active',ownership_epoch=ownership_epoch+1,environment='test',release_sha=repeat('a',40),selector_config_digest=repeat('b',64),drain_completed_at=now()").await;
        execute(&owner, "DELETE FROM market_event_outbox").await;
        let baseline: i64 =
            sqlx::query_scalar("SELECT discarded_through_seq FROM market_event_replay_state")
                .fetch_one(&owner)
                .await
                .unwrap();
        for n in 1..=601 {
            insert(&owner, baseline + n * 3, org()).await;
            insert(&owner, baseline + n * 3 - 1, Uuid::from_u128(9)).await;
        }
        let hub = StreamHub::default();
        let mut replay = Replay::open(
            PgReplaySource::new(app.clone()),
            &hub,
            principal(user, org()),
            Some(baseline),
            Clock::new(),
        )
        .await
        .unwrap();
        for n in 1..=601 {
            assert_eq!(id(&mut replay).await, baseline + n * 3);
            // A one-connection pool remains usable after every delivered row.
            assert_eq!(
                sqlx::query_scalar::<_, i32>("SELECT 1")
                    .fetch_one(&app)
                    .await
                    .unwrap(),
                1
            );
        }
        drop(replay);
        println!(
            "replay postgres: 601 authorized rows with foreign holes and released connections"
        );
        let mut replay = Replay::open(
            PgReplaySource::new(app.clone()),
            &hub,
            principal(user, org()),
            Some(baseline),
            Clock::new(),
        )
        .await
        .unwrap();
        assert_eq!(id(&mut replay).await, baseline + 3);
        sqlx::query("DELETE FROM market_event_outbox WHERE stream_seq=$1")
            .bind(baseline + 1503)
            .execute(&owner)
            .await
            .unwrap();
        for n in 2..=500 {
            assert_eq!(id(&mut replay).await, baseline + n * 3);
        }
        terminal(&mut replay, Terminal::Retention).await;
        println!("replay postgres: prune after captured page and before next snapshot");
        execute(&owner, "DELETE FROM market_event_outbox").await;
        let floor: i64 =
            sqlx::query_scalar("SELECT discarded_through_seq FROM market_event_replay_state")
                .fetch_one(&owner)
                .await
                .unwrap();
        let mut stale = Replay::open(
            PgReplaySource::new(app.clone()),
            &hub,
            principal(user, org()),
            Some(floor - 1),
            Clock::new(),
        )
        .await
        .unwrap();
        terminal(&mut stale, Terminal::Retention).await;
        let mut equal = Replay::open(
            PgReplaySource::new(app.clone()),
            &hub,
            principal(user, org()),
            Some(floor),
            Clock::new(),
        )
        .await
        .unwrap();
        insert(&owner, floor + 1, org()).await;
        publish(&hub, floor + 1);
        assert_eq!(id(&mut equal).await, floor + 1);
        drop(equal);
        println!("replay postgres: empty retained table keeps floor and equal cursor");
        let clock = Clock::new();
        let mut replay = Replay::open(
            PgReplaySource::new(app.clone()),
            &hub,
            principal(user, org()),
            Some(floor),
            clock.clone(),
        )
        .await
        .unwrap();
        assert_eq!(id(&mut replay).await, floor + 1);
        sqlx::query("UPDATE users SET authentication_revoked_at=$1 WHERE id=$2")
            .bind(now())
            .bind(user)
            .execute(&owner)
            .await
            .unwrap();
        clock.advance(15);
        publish(&hub, floor + 2);
        terminal(&mut replay, Terminal::AuthRevoked).await;
        println!("replay postgres: current account cutoff terminates live delivery");
        assert_eq!(hub.subscriber_count(), 0);
        app.close().await;
        owner.close().await;
    }
}
