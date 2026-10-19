//! Two disposable database incarnations on one disposable server. Nothing here
//! promotes or restores a live database; "promotion" and "restore" are the owner
//! rebinding which incarnation holds the externally active authority.
//!
//! The cache itself arrives with Task 8. These tests express its hit and fill
//! rules with the public authority API, so the rules are proved before any
//! entry exists: an entry is served only when its fence token is still accepted
//! and its generation equals the generation read in the request's own snapshot.
#[path = "support/cache_owner.rs"]
mod cache_owner;

use cache_owner::{Record, authority, bind, generation, reset, set_generation};
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use verdaxis_read::runtime::{
    cache_authority::{
        AUTHORITY_LEASE, Authority, CacheAuthority, FenceToken, FillTicket, admit_connection,
        database_state,
    },
    cache_events::{CacheEvents, ListenerBudget},
};

/// Every test rebinds the two shared incarnation rows, so they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Longer than the lease, as the owner's drain wait is.
const DRAIN: Duration = Duration::from_secs(10);

fn budget() -> ListenerBudget {
    ListenerBudget::new(50, 100, 100)
}

struct World {
    options_a: PgConnectOptions,
    options_b: PgConnectOptions,
    a: PgConnection,
    b: PgConnection,
    record: Record,
    /// Equal in both incarnations at the start: the reused generation.
    base: i64,
    clock: Instant,
}

/// A cached payload bound to the fence and generation it was computed under.
struct Entry {
    ticket: FillTicket,
    generation: i64,
    payload: &'static str,
}

impl Entry {
    fn new(token: &FenceToken, generation: i64, payload: &'static str) -> Self {
        Self {
            ticket: FillTicket::new(token.clone(), generation),
            generation,
            payload,
        }
    }
}

impl World {
    async fn open() -> Option<Self> {
        let first = std::env::var("PUBLIC_CACHE_TEST_DATABASE_URL").ok()?;
        let second = std::env::var("PUBLIC_CACHE_TEST_SECOND_DATABASE_URL").ok()?;
        let options_a = PgConnectOptions::from_str(&first).unwrap();
        let options_b = PgConnectOptions::from_str(&second).unwrap();
        let mut a = PgConnection::connect_with(&options_a).await.unwrap();
        let mut b = PgConnection::connect_with(&options_b).await.unwrap();
        // Move both past every event row either database already holds.
        let base = generation(&mut a).await.max(generation(&mut b).await) + 1000;
        set_generation(&mut a, base).await;
        set_generation(&mut b, base).await;
        Some(Self {
            options_a,
            options_b,
            a,
            b,
            record: Record::new(),
            base,
            clock: Instant::now(),
        })
    }

    fn after(&mut self, elapsed: Duration) -> Instant {
        self.clock += elapsed;
        self.clock
    }

    async fn finish(mut self) {
        reset(&mut self.a).await;
        reset(&mut self.b).await;
        self.a.close().await.unwrap();
        self.b.close().await.unwrap();
    }
}

fn process() -> Arc<CacheAuthority> {
    Arc::new(CacheAuthority::default())
}

/// The owner's order: publish revocation, wait out the lease, bind the
/// successor, publish it. The process re-reads after each published record.
/// Incarnation B is bound to the successor only when `bind_successor_on_b` is
/// set; otherwise only the record changes, as when a restore is not yet bound.
async fn rotate(
    world: &mut World,
    process: &CacheAuthority,
    successor: &Authority,
    bind_successor_on_b: bool,
) {
    world.record.revoke(successor.revision - 1);
    let seen = world.after(Duration::from_secs(1));
    process.refresh(world.record.path(), seen);
    let drained = world.after(DRAIN);
    if bind_successor_on_b {
        bind(&mut world.b, successor, true).await;
    }
    world.record.activate(successor);
    process.refresh(world.record.path(), drained);
}

/// A hit needs the request's own admission, an accepted fence token, and the
/// same generation read in the request's snapshot.
fn serve(
    process: &CacheAuthority,
    entry: &Entry,
    admitted: Option<&(FenceToken, i64)>,
    now: Instant,
) -> Option<&'static str> {
    let (_, latest) = admitted?;
    process
        .may_publish(&entry.ticket, *latest, now)
        .then_some(entry.payload)
}

/// Activate incarnation A, attach its listener and make the first fill.
async fn first_fill(
    world: &mut World,
    process: &Arc<CacheAuthority>,
    primary: &Authority,
) -> (CacheEvents, Entry, FenceToken) {
    bind(&mut world.a, primary, true).await;
    world.record.activate(primary);
    process.refresh(world.record.path(), world.clock);
    let listener = CacheEvents::connect(&world.options_a, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let admitted = admit_connection(process, &mut world.a, world.clock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.1, world.base);
    let entry = Entry::new(&admitted.0, admitted.1, "rows-from-a");
    assert_eq!(
        serve(process, &entry, Some(&admitted), world.clock),
        Some("rows-from-a")
    );
    (listener, entry, admitted.0)
}

async fn admit(
    world: &mut World,
    process: &CacheAuthority,
    on_b: bool,
) -> Option<(FenceToken, i64)> {
    let now = world.clock;
    let connection = if on_b { &mut world.b } else { &mut world.a };
    admit_connection(process, connection, now).await.unwrap()
}

#[tokio::test]
async fn promotion_reuses_generation_without_reusing_cache() {
    let _serial = SERIAL.lock().await;
    let Some(mut world) = World::open().await else {
        return;
    };
    let process = process();
    let (listener_a, entry, _) = first_fill(&mut world, &process, &authority(1)).await;
    // Incarnation B holds different rows but the same generation number.
    rotate(&mut world, &process, &authority(3), true).await;
    // A still answers queries, but is no longer the externally active primary.
    assert!(admit(&mut world, &process, false).await.is_none());
    // B reports the active identity but has no bound listener yet: no hit.
    assert!(admit(&mut world, &process, true).await.is_none());
    let listener_b = CacheEvents::connect(&world.options_b, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let admitted_b = admit(&mut world, &process, true).await.unwrap();
    // The generation numbers are equal, yet A's entry is never served.
    assert_eq!(admitted_b.1, entry.generation);
    assert_eq!(
        serve(&process, &entry, Some(&admitted_b), world.clock),
        None
    );
    // An entry made under B is served under B.
    let fresh = Entry::new(&admitted_b.0, admitted_b.1, "rows-from-b");
    assert_eq!(
        serve(&process, &fresh, Some(&admitted_b), world.clock),
        Some("rows-from-b")
    );
    // The retired listener on A cannot take B's listener down with it.
    listener_a.close().await.unwrap();
    assert!(admit(&mut world, &process, true).await.is_some());
    listener_b.close().await.unwrap();
    world.finish().await;
}

#[tokio::test]
async fn restore_rotates_authority_before_activation() {
    let _serial = SERIAL.lock().await;
    let Some(mut world) = World::open().await else {
        return;
    };
    let process = process();
    let primary = authority(1);
    let (listener_a, entry, _) = first_fill(&mut world, &process, &primary).await;
    // A restored copy carries the same metadata and the same generation. Nothing
    // inside the database tells it from the original, which is why the owner
    // must rotate before traffic returns.
    bind(&mut world.b, &primary, true).await;
    let copied = database_state(&mut world.b).await.unwrap().unwrap();
    assert_eq!(copied.authority, primary);
    assert_eq!(copied.generation, world.base);
    // The owner publishes a fresh identity but has not bound the restored copy.
    rotate(&mut world, &process, &authority(3), false).await;
    assert!(admit(&mut world, &process, true).await.is_none());
    let refused = CacheEvents::connect(&world.options_b, "UTC", process.clone(), budget()).await;
    assert!(refused.is_err());
    // Binding the restored copy to the fresh identity is the activation.
    let rotated = authority(3);
    rotate(&mut world, &process, &rotated, true).await;
    let listener_b = CacheEvents::connect(&world.options_b, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let admitted_b = admit(&mut world, &process, true).await.unwrap();
    assert_eq!(admitted_b.1, entry.generation);
    assert_eq!(
        serve(&process, &entry, Some(&admitted_b), world.clock),
        None
    );
    listener_a.close().await.unwrap();
    listener_b.close().await.unwrap();
    world.finish().await;
}

#[tokio::test]
async fn old_primary_connection_is_fenced() {
    let _serial = SERIAL.lock().await;
    let Some(mut world) = World::open().await else {
        return;
    };
    let process = process();
    let (mut listener_a, _, in_flight) = first_fill(&mut world, &process, &authority(1)).await;
    // A request on the old primary computes an uncached result, then pauses.
    rotate(&mut world, &process, &authority(3), true).await;
    let now = world.clock;
    assert!(!process.accepts(&in_flight, now));
    // The old listener is still connected but is told it no longer belongs.
    assert!(listener_a.wake().await.is_err());
    // The old connection stays open and still answers, but admits nothing,
    // before and after the successor starts serving.
    assert!(admit(&mut world, &process, false).await.is_none());
    let listener_b = CacheEvents::connect(&world.options_b, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let admitted_b = admit(&mut world, &process, true).await.unwrap();
    assert!(admit(&mut world, &process, false).await.is_none());
    assert!(!process.accepts(&in_flight, now));
    assert!(process.accepts(&admitted_b.0, now));
    // Closing the old listener late leaves the successor serving.
    listener_a.close().await.unwrap();
    assert!(process.accepts(&admitted_b.0, now));
    listener_b.close().await.unwrap();
    world.finish().await;
}

async fn stale_fill_case(b_generation_offset: i64) {
    let Some(mut world) = World::open().await else {
        return;
    };
    let process = process();
    let (listener_a, entry, token_a) = first_fill(&mut world, &process, &authority(1)).await;
    // The fill is paused after its query, before cache insertion and response.
    let paused = Entry::new(&token_a, entry.generation, "paused-fill");
    set_generation(&mut world.b, world.base + b_generation_offset).await;
    rotate(&mut world, &process, &authority(3), true).await;
    let listener_b = CacheEvents::connect(&world.options_b, "UTC", process.clone(), budget())
        .await
        .unwrap();
    let admitted_b = admit(&mut world, &process, true).await.unwrap();
    assert_eq!(admitted_b.1, world.base + b_generation_offset);
    let now = world.clock;
    // Neither the insertion check nor the response check passes, whether the
    // successor's generation is equal, ahead or behind.
    assert!(!process.may_publish(&paused.ticket, admitted_b.1, now));
    assert!(!process.may_publish(&paused.ticket, paused.generation, now));
    assert!(!process.accepts(&token_a, now));
    listener_a.close().await.unwrap();
    listener_b.close().await.unwrap();
    world.finish().await;
}

#[tokio::test]
async fn old_epoch_fill_cannot_publish() {
    let _serial = SERIAL.lock().await;
    if std::env::var("PUBLIC_CACHE_TEST_SECOND_DATABASE_URL").is_err() {
        return;
    }
    // Equal generation, a later generation, and a generation rollback.
    for offset in [0, 5, -1] {
        stale_fill_case(offset).await;
    }
}

#[tokio::test]
async fn uncertain_authority_disables_hits() {
    let _serial = SERIAL.lock().await;
    let Some(mut world) = World::open().await else {
        return;
    };
    let primary = authority(1);
    let refreshing = process();
    let stuck = process();
    let (listener, _, _) = first_fill(&mut world, &refreshing, &primary).await;
    stuck.refresh(world.record.path(), world.clock);
    let stuck_listener = CacheEvents::connect(&world.options_a, "UTC", stuck.clone(), budget())
        .await
        .unwrap();
    assert!(admit(&mut world, &stuck, false).await.is_some());

    // One process stops re-reading the record. Past its lease it serves nothing
    // by itself, while the process that keeps reading continues. No routing
    // change is needed to stop the stuck process.
    let later = world.after(AUTHORITY_LEASE + Duration::from_secs(1));
    refreshing.refresh(world.record.path(), later);
    assert!(admit(&mut world, &stuck, false).await.is_none());
    assert!(admit(&mut world, &refreshing, false).await.is_some());

    // A missing record, then an unreadable one, then a symlink, disable hits.
    world.record.remove();
    refreshing.refresh(world.record.path(), world.clock);
    assert!(admit(&mut world, &refreshing, false).await.is_none());
    world.record.write_raw("not a record");
    refreshing.refresh(world.record.path(), world.clock);
    assert!(admit(&mut world, &refreshing, false).await.is_none());
    world.record.replace_with_symlink(&primary);
    refreshing.refresh(world.record.path(), world.clock);
    assert!(admit(&mut world, &refreshing, false).await.is_none());

    // Restoring the record is not enough: the lost listener must be replaced.
    world.record.activate(&primary);
    refreshing.refresh(world.record.path(), world.clock);
    assert!(admit(&mut world, &refreshing, false).await.is_none());
    listener.close().await.unwrap();
    let listener = CacheEvents::connect(&world.options_a, "UTC", refreshing.clone(), budget())
        .await
        .unwrap();
    assert!(admit(&mut world, &refreshing, false).await.is_some());

    // The database reporting a different epoch, primary, revision, or being
    // inactive is mismatched metadata: no hit while the record is unchanged.
    for (mismatch, active) in [
        (authority(1), true),
        (
            Authority {
                revision: 2,
                ..primary.clone()
            },
            true,
        ),
        (primary.clone(), false),
    ] {
        bind(&mut world.a, &mismatch, active).await;
        assert!(admit(&mut world, &refreshing, false).await.is_none());
    }
    bind(&mut world.a, &primary, true).await;
    assert!(admit(&mut world, &refreshing, false).await.is_some());

    listener.close().await.unwrap();
    stuck_listener.close().await.unwrap();
    world.finish().await;
}

#[tokio::test]
async fn missing_generation_infrastructure_disables_caching() {
    let _serial = SERIAL.lock().await;
    let Some(mut world) = World::open().await else {
        return;
    };
    let primary = authority(1);
    bind(&mut world.a, &primary, true).await;
    assert!(database_state(&mut world.a).await.unwrap().is_some());
    // An inactive database cannot back a cache even with metadata present.
    bind(&mut world.a, &primary, false).await;
    assert!(database_state(&mut world.a).await.unwrap().is_none());
    bind(&mut world.a, &primary, true).await;
    // A schema that predates the checkpoint has neither table. DDL is
    // transactional, so hide each table briefly and roll back.
    for table in [
        "public_read_cache_incarnation",
        "public_read_cache_generations",
    ] {
        let mut transaction = world.a.begin().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE public.{table} RENAME TO hidden_{table}"
        )))
        .execute(&mut *transaction)
        .await
        .unwrap();
        assert!(database_state(&mut transaction).await.unwrap().is_none());
        transaction.rollback().await.unwrap();
    }
    assert!(database_state(&mut world.a).await.unwrap().is_some());
    world.finish().await;
}
