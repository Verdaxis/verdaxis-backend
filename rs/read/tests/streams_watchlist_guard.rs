//! The watchlist read guard stays in force beside the stream transport.
//!
//! The stream transport carries committed market events. It never writes
//! watchlist events, and no Rust source produces them before a trusted
//! producer is activated. Until then the feed and the counts apply one rule:
//! every `SLICE_BEST_PRICE_MOVED` row is hidden whatever the cutoff value, and
//! a row whose source order or pinned order is outside the current public
//! order scope is hidden too. The rule applies before feed pagination and
//! before unread and latest-event aggregation.
//!
//! The source and pure tests run in every build of this target. The database
//! tests are ignored here: `tests/postgres/test_phase6_watchlist_guard.py`
//! seeds their market rows and runs them on the disposable PostgreSQL harness,
//! on the migrated schema and with the deployed application role, and requires
//! every one of them to pass.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;
use verdaxis_read::crud::watchlists::{
    Event, EventQuery, Metrics, Slice, Summary, Target, VisibleSnapshot, WatchlistEvent,
    acknowledge, capture_visible, decode_cursor, event_visible, feed_page,
};
use verdaxis_read::{
    Config, MarketPolicy, PointPolicy, ProductPolicy, VisibilityClock, compiled_sha,
};

const BEST_PRICE: &str = "SLICE_BEST_PRICE_MOVED";

fn instant(text: &str) -> DateTime<Utc> {
    text.parse().expect("an RFC 3339 instant")
}

// ------------------------------------------------------------- source rules

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Every file below `directory` with `extension`, in a stable order.
fn files(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let entries = std::fs::read_dir(&next)
            .unwrap_or_else(|error| panic!("read {}: {error}", next.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn source_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// The lowercase words of SQL text. Quotes, string concatenation, schema dots
/// and line breaks become single spaces, so they cannot hide a statement.
fn sql_words(text: &str) -> String {
    let separated: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = separated.split_whitespace().collect();
    format!(" {} ", words.join(" "))
}

/// True when `text` contains a statement that adds watchlist event rows.
fn inserts_watchlist_events(text: &str) -> bool {
    let words = sql_words(text);
    ["insert into", "merge into", "copy"].iter().any(|verb| {
        ["watchlist_events", "public watchlist_events"]
            .iter()
            .any(|table| words.contains(&format!(" {verb} {table} ")))
    })
}

/// The watchlist tables; `watchlists` is also the read module's name.
const WATCHLIST_TABLES: [&str; 4] = [
    "watchlists",
    "watchlist_entries",
    "watchlist_targets",
    "watchlist_events",
];

/// True when `text` names a watchlist table or the watchlist module path.
fn names_watchlist_state(text: &str) -> bool {
    let words = sql_words(text);
    WATCHLIST_TABLES
        .iter()
        .any(|table| words.contains(&format!(" {table} ")))
}

/// Stream transport sources that name a watchlist table or module.
fn stream_sources_naming_watchlists(root: &Path) -> Vec<String> {
    let mut sources = files(&root.join("rs/read/src/streams"), "rs");
    sources.extend(
        files(&root.join("rs/read/sql"), "sql")
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("streams"))
            }),
    );
    assert!(!sources.is_empty(), "the stream module sources are present");
    sources
        .iter()
        .filter(|path| names_watchlist_state(&source_text(path)))
        .map(|path| path.display().to_string())
        .collect()
}

#[test]
fn source_rules_reject_each_forbidden_form() {
    for statement in [
        "INSERT INTO watchlist_events (id) VALUES ($1)",
        "insert into watchlist_events(id) values ($1)",
        "INSERT INTO public.watchlist_events (id) VALUES ($1)",
        "INSERT INTO \"public\".\"watchlist_events\" (id) VALUES ($1)",
        "concat!(\"INSERT INTO \", \"watchlist_events (id) VALUES ($1)\")",
        "\"INSERT INTO \\\n     watchlist_events (id) VALUES ($1)\"",
        "MERGE INTO watchlist_events USING source ON true",
        "COPY watchlist_events FROM STDIN",
    ] {
        assert!(inserts_watchlist_events(statement), "{statement}");
    }
    for statement in [
        "UPDATE watchlist_events SET is_read=true WHERE id=$1",
        "SELECT id FROM watchlist_events WHERE watchlist_id=$1",
        "INSERT INTO watchlist_targets (id) VALUES ($1)",
        "INSERT INTO watchlist_events_archive (id) VALUES ($1)",
    ] {
        assert!(!inserts_watchlist_events(statement), "{statement}");
    }
    for text in [
        "use crate::crud::watchlists::feed_page;",
        "SELECT id FROM \"watchlist_targets\"",
        "UPDATE watchlist_entries SET product_id=$1",
        "DELETE FROM public.watchlists WHERE id=$1",
    ] {
        assert!(names_watchlist_state(text), "{text}");
    }
    for text in [
        "SELECT stream_seq FROM market_event_outbox",
        "// Watchlist rows belong to their own producer.",
    ] {
        assert!(!names_watchlist_state(text), "{text}");
    }
}

/// No Rust source can start a watchlist event producer: no production
/// statement adds watchlist event rows, and the serving entry point and its
/// runtime composition do not name the watchlist event table.
///
/// The one reviewed writer is the order-write boundary's own effect module
/// (`rs/read/src/order_write/events.rs`). It records the same withdrawal and
/// price rows the Python order service writes, inside the caller's order
/// transaction, and it starts no producer; the boundary scanner names that
/// file and its exact insert targets. Every other source stays refused.
#[test]
fn no_producer_startup() {
    let root = repository_root();
    let order_write_effects = root.join("rs/read/src/order_write/events.rs");
    let mut production = Vec::new();
    for directory in ["rs/read/src", "rs/contract/src"] {
        production.extend(files(&root.join(directory), "rs"));
    }
    production.extend(files(&root.join("rs/read/sql"), "sql"));
    assert!(production.len() > 50, "the production sources are present");
    assert!(
        order_write_effects.is_file(),
        "the reviewed writer is present"
    );

    let mut startup = vec![
        root.join("rs/read/src/main.rs"),
        root.join("rs/read/src/lib.rs"),
    ];
    startup.extend(files(&root.join("rs/read/src/runtime"), "rs"));

    let mut violations = Vec::new();
    for path in &production {
        if path == &order_write_effects {
            continue;
        }
        if inserts_watchlist_events(&source_text(path)) {
            violations.push(format!("{}: adds watchlist event rows", path.display()));
        }
    }
    for path in &startup {
        if source_text(path)
            .to_ascii_lowercase()
            .contains("watchlist_events")
        {
            violations.push(format!(
                "{}: startup names watchlist events",
                path.display()
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "a watchlist event producer is reachable:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------- pure rules

fn pure_target(id: Uuid, kind: &str, order: Option<Uuid>) -> Target {
    serde_json::from_value(json!({
        "id": id,
        "target_type": kind,
        "market_product_code": "GUARD",
        "delivery_point_id": Uuid::from_u128(9),
        "availability_window_code": "SPOT",
        "order_id": order,
        "created_at": "2026-10-01T00:00:00Z",
    }))
    .expect("a projected target")
}

/// The guard has no activation instant. No cutoff value, including the
/// default instant, the current clock and both representable extremes, lets a
/// best-price row count toward unread or latest-event metrics. The feed rule
/// takes no cutoff at all.
#[test]
fn no_default_activation_cutoff() {
    let public_order = Uuid::from_u128(1);
    let slice = Uuid::from_u128(10);
    let pin = Uuid::from_u128(11);
    let targets = vec![
        pure_target(slice, "SLICE", None),
        pure_target(pin, "PIN", Some(public_order)),
    ];
    let visible_orders = HashSet::from([public_order]);
    let ordinary = instant("2026-10-19T00:00:00Z");
    let now = Utc::now();

    let mut events = vec![Event {
        target_id: slice,
        event_type: "SLICE_NEW_ORDER".into(),
        order_id: Some(public_order),
        is_read: false,
        created_at: ordinary,
    }];
    let best_price_times = [
        DateTime::<Utc>::MIN_UTC,
        DateTime::<Utc>::default(),
        ordinary + Duration::seconds(1),
        now,
        DateTime::<Utc>::MAX_UTC,
    ];
    for (index, created_at) in best_price_times.into_iter().enumerate() {
        events.push(Event {
            target_id: if index % 2 == 0 { slice } else { pin },
            event_type: BEST_PRICE.into(),
            order_id: Some(public_order),
            is_read: false,
            created_at,
        });
    }

    for cutoff in [
        DateTime::<Utc>::default(),
        DateTime::<Utc>::MIN_UTC,
        ordinary,
        now,
        DateTime::<Utc>::MAX_UTC,
    ] {
        let metrics = Metrics::from_visible_events(&targets, &events, &visible_orders, cutoff);
        assert_eq!(metrics.unread.get(&slice), Some(&1), "cutoff {cutoff}");
        assert_eq!(metrics.unread.get(&pin), None, "cutoff {cutoff}");
        assert_eq!(
            metrics.latest.get(&slice),
            Some(&ordinary),
            "cutoff {cutoff}"
        );
        assert_eq!(metrics.latest.get(&pin), None, "cutoff {cutoff}");
    }

    let scope = VisibleSnapshot::from_orders(vec![(
        public_order,
        "GUARD".into(),
        Uuid::from_u128(9),
        "SPOT".into(),
    )]);
    assert!(!event_visible(BEST_PRICE, Some(public_order), None, &scope));
    assert!(!event_visible(BEST_PRICE, None, None, &scope));
    assert!(event_visible(
        "SLICE_NEW_ORDER",
        Some(public_order),
        None,
        &scope
    ));
}

// ------------------------------------------------------------- PostgreSQL

const APP_URL: &str = "WATCHLIST_GUARD_APP_URL";
const FIXTURE_URL: &str = "WATCHLIST_GUARD_FIXTURE_URL";
const MARKET: &str = "WATCHLIST_GUARD_MARKET";
const PRODUCT_CODE: &str = "GUARD";
const SPOT: &str = "SPOT";
const LATER_WINDOW: &str = "2027-01";
/// The shared sequencer lock key, written out here independently.
const SEQUENCER_KEY: i64 = 7_264_537_260_226_625;

/// The application role reads; the schema owner seeds watchlist fixture rows.
async fn connect(variable: &str) -> PgConnection {
    let url = std::env::var(variable).unwrap_or_else(|_| {
        panic!("{variable} is required: tests/postgres/test_phase6_watchlist_guard.py supplies it")
    });
    PgConnection::connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect with {variable}: {error}"))
}

/// The public order scope reads only the runtime KYC switch from the
/// configuration; the other values are inert.
fn read_config() -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "unused".into(),
        database_url: "postgres://unused@127.0.0.1/unused".into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().expect("a loopback address"),
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 0,
        configured_max_connections: 100,
        phase2: Default::default(),
    }
}

/// The market the PostgreSQL driver seeds through the schema owner, as one
/// JSON document. Rust test code writes no order-book rows: the repository
/// keeps every order-book write inside the write boundary.
#[derive(serde::Deserialize)]
struct SeededMarket {
    organization: Uuid,
    owner: Uuid,
    public_order: Uuid,
    closed_order: Uuid,
    product: Uuid,
    product_name: String,
    fuel_type: String,
    fuel_grade: String,
    point: Uuid,
    point_name: String,
    region: String,
}

/// One market: a REAL organization whose open order is in the public scope
/// and whose cancelled order is not, both in one SPOT slice. The policy names
/// only this market's product and point.
struct Market {
    organization: Uuid,
    owner: Uuid,
    public_order: Uuid,
    closed_order: Uuid,
    point: Uuid,
    policy: MarketPolicy,
}

fn seeded_market() -> Market {
    let document = std::env::var(MARKET).unwrap_or_else(|_| {
        panic!("{MARKET} is required: tests/postgres/test_phase6_watchlist_guard.py supplies it")
    });
    let seeded: SeededMarket = serde_json::from_str(&document).expect("the seeded market document");
    let policy = MarketPolicy {
        source_sha: compiled_sha().into(),
        products: vec![ProductPolicy {
            id: seeded.product,
            code: PRODUCT_CODE.into(),
            name: seeded.product_name,
            fuel_type: seeded.fuel_type,
            fuel_grade: seeded.fuel_grade,
            execution_mode: "ORDERBOOK".into(),
            lanes: vec![seeded.point],
            qualification_profile: "scalar".into(),
            specification_standard: None,
            display_order: 1,
        }],
        points: vec![PointPolicy {
            id: seeded.point,
            name: seeded.point_name,
            region: seeded.region,
            timezone: "UTC".into(),
            display_order: 1,
        }],
    };
    Market {
        organization: seeded.organization,
        owner: seeded.owner,
        public_order: seeded.public_order,
        closed_order: seeded.closed_order,
        point: seeded.point,
        policy,
    }
}

/// One watchlist of the market owner: a SPOT slice and a later-window slice
/// of the market's product and point, and pins on its public and cancelled
/// orders.
struct Watch {
    container: Uuid,
    spot_slice: Uuid,
    later_slice: Uuid,
    public_pin: Uuid,
    closed_pin: Uuid,
}

async fn seed_watch(fixture: &mut PgConnection, market: &Market) -> Watch {
    let watch = Watch {
        container: Uuid::new_v4(),
        spot_slice: Uuid::new_v4(),
        later_slice: Uuid::new_v4(),
        public_pin: Uuid::new_v4(),
        closed_pin: Uuid::new_v4(),
    };
    let created = instant("2026-10-01T00:00:00Z");
    sqlx::query(
        "INSERT INTO watchlists (id,user_id,name,kind,created_at) \
         VALUES ($1,$2,'Guard','CUSTOM',$3)",
    )
    .bind(watch.container)
    .bind(market.owner)
    .bind(created)
    .execute(&mut *fixture)
    .await
    .expect("seed the watchlist");
    for (offset, target, kind, window, order) in [
        (1, watch.spot_slice, "SLICE", SPOT, None),
        (2, watch.later_slice, "SLICE", LATER_WINDOW, None),
        (3, watch.public_pin, "PIN", SPOT, Some(market.public_order)),
        (4, watch.closed_pin, "PIN", SPOT, Some(market.closed_order)),
    ] {
        sqlx::query(
            "INSERT INTO watchlist_targets \
             (id,watchlist_id,target_type,market_product_code,delivery_point_id,\
              availability_window_code,order_id,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(target)
        .bind(watch.container)
        .bind(kind)
        .bind(PRODUCT_CODE)
        .bind(market.point)
        .bind(window)
        .bind(order)
        .bind(created + Duration::minutes(offset))
        .execute(&mut *fixture)
        .await
        .expect("seed a watchlist target");
    }
    watch
}

/// Store one watchlist event as the existing producer stores it. `source` is
/// the order named in the payload.
async fn insert_event(
    fixture: &mut PgConnection,
    watch: &Watch,
    target: Uuid,
    event_type: &str,
    source: Option<Uuid>,
    created_at: DateTime<Utc>,
    is_read: bool,
) -> Uuid {
    let id = Uuid::new_v4();
    let mut payload = json!({"schema_version": 1, "source_kind": "LIVE_ORDER"});
    if let Some(order) = source {
        payload["order_id"] = json!(order.to_string());
    }
    sqlx::query(
        "INSERT INTO watchlist_events \
         (id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) \
         VALUES ($1,$2,$3,$4,$5::json,$6,$7)",
    )
    .bind(id)
    .bind(watch.container)
    .bind(target)
    .bind(event_type)
    .bind(payload)
    .bind(is_read)
    .bind(created_at)
    .execute(&mut *fixture)
    .await
    .expect("seed a watchlist event");
    id
}

/// Capture the shared public order relation through the application role.
async fn public_scope(app: &mut PgConnection, market: &Market) -> VisibleSnapshot {
    let scope = capture_visible(
        app,
        &market.policy,
        &VisibilityClock::at(Utc::now()),
        &read_config(),
    )
    .await
    .expect("capture the public order scope");
    assert!(
        scope.contains(market.public_order),
        "the open order is public"
    );
    assert!(
        !scope.contains(market.closed_order),
        "the cancelled order is outside the public scope"
    );
    scope
}

/// Every feed row, page by page through the issued cursors.
async fn whole_feed(
    app: &mut PgConnection,
    watch: &Watch,
    scope: &VisibleSnapshot,
    limit: i64,
) -> Vec<WatchlistEvent> {
    let mut rows = Vec::new();
    let mut cursor = None;
    for _ in 0..100 {
        let page = feed_page(app, watch.container, scope, EventQuery { limit, cursor })
            .await
            .expect("a feed page");
        let page_size = i64::try_from(page.items.len()).expect("a small page");
        assert!(page_size <= limit, "a page holds at most the limit");
        rows.extend(page.items);
        match page.next_cursor {
            Some(text) => cursor = Some(decode_cursor(&text).expect("an issued cursor")),
            None => return rows,
        }
    }
    panic!("the feed did not end within 100 pages");
}

fn ids(rows: &[WatchlistEvent]) -> Vec<Uuid> {
    rows.iter().map(|row| row.id).collect()
}

async fn summary_at(
    app: &mut PgConnection,
    market: &Market,
    watch: &Watch,
    scope: &VisibleSnapshot,
    cutoff: DateTime<Utc>,
) -> Summary {
    scope
        .detail(app, market.owner, watch.container, cutoff)
        .await
        .expect("the detail read")
        .expect("the owner's watchlist")
}

fn slice_of(summary: &Summary, id: Uuid) -> &Slice {
    summary
        .slices
        .iter()
        .find(|slice| slice.id == id)
        .unwrap_or_else(|| panic!("slice {id} is in the summary"))
}

/// A guarded row cannot be acknowledged, and the refusal writes nothing.
async fn assert_refused_without_write(
    app: &mut PgConnection,
    watch: &Watch,
    scope: &VisibleSnapshot,
    event: Uuid,
) {
    let state = "SELECT xmin::text, is_read FROM watchlist_events WHERE id=$1";
    let before: (String, bool) = sqlx::query_as(state)
        .bind(event)
        .fetch_one(&mut *app)
        .await
        .expect("the stored row");
    let error = acknowledge(app, watch.container, event, scope)
        .await
        .expect_err("a guarded row cannot be acknowledged");
    assert_eq!(error.status.as_u16(), 404, "event {event}");
    let after: (String, bool) = sqlx::query_as(state)
        .bind(event)
        .fetch_one(&mut *app)
        .await
        .expect("the stored row");
    assert_eq!(after, before, "event {event} is unchanged");
}

/// The feed and the counts come from one rule: each slice and pin count
/// exactly the visible feed rows of that target, and nothing else.
fn assert_feed_matches_counts(feed: &[WatchlistEvent], summary: &Summary) {
    let mut slice_of_target = HashMap::new();
    for slice in &summary.slices {
        slice_of_target.insert(slice.id, slice.id);
        for pin in &slice.pins {
            slice_of_target.insert(pin.id, slice.id);
        }
    }
    // A slice counts its own rows and the rows of its pins.
    let mut unread = HashMap::<Uuid, i64>::new();
    let mut latest = HashMap::<Uuid, DateTime<Utc>>::new();
    for row in feed {
        assert_ne!(row.event_type, BEST_PRICE, "feed row {}", row.id);
        let slice = *slice_of_target
            .get(&row.watchlist_target_id)
            .unwrap_or_else(|| panic!("feed row {} belongs to a counted target", row.id));
        if !row.is_read {
            *unread.entry(slice).or_default() += 1;
        }
        let newest = latest.entry(slice).or_insert(row.created_at);
        *newest = (*newest).max(row.created_at);
    }
    for slice in &summary.slices {
        assert_eq!(
            slice.unread_event_count,
            unread.get(&slice.id).copied().unwrap_or(0),
            "slice {} unread",
            slice.id
        );
        assert_eq!(
            slice.latest_event_at,
            latest.get(&slice.id).copied(),
            "slice {} latest",
            slice.id
        );
        for pin in &slice.pins {
            let own_unread = feed
                .iter()
                .filter(|row| row.watchlist_target_id == pin.id && !row.is_read)
                .count();
            assert_eq!(
                pin.unread_event_count,
                i64::try_from(own_unread).expect("a small count"),
                "pin {} unread",
                pin.id
            );
            let own_latest = feed
                .iter()
                .filter(|row| row.watchlist_target_id == pin.id)
                .map(|row| row.created_at)
                .max();
            assert_eq!(pin.latest_event_at, own_latest, "pin {} latest", pin.id);
        }
    }
    let total_unread = feed.iter().filter(|row| !row.is_read).count();
    assert_eq!(
        summary.unread_event_count,
        i64::try_from(total_unread).expect("a small count")
    );
    assert_eq!(
        summary.latest_event_at,
        feed.iter().map(|row| row.created_at).max()
    );
}

/// Best-price rows sourced from an order that is public now stay hidden for a
/// cutoff before, at and after them, in the feed, the counts and the read
/// state. Rows dated after this run are hidden too: the rule is not a clock
/// cutoff that would reveal newer rows.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn best_price_guard_before_at_after_cutoff() {
    let mut fixture = connect(FIXTURE_URL).await;
    let mut app = connect(APP_URL).await;
    let market = seeded_market();
    let watch = seed_watch(&mut fixture, &market).await;
    let cutoff = instant("2026-10-19T00:00:00Z");
    let microsecond = Duration::microseconds(1);
    let future = Utc::now() + Duration::days(365);

    let mut hidden = Vec::new();
    for (target, source, created_at) in [
        (
            watch.spot_slice,
            Some(market.public_order),
            cutoff - microsecond,
        ),
        (watch.spot_slice, Some(market.public_order), cutoff),
        (
            watch.public_pin,
            Some(market.public_order),
            cutoff + microsecond,
        ),
        (
            watch.spot_slice,
            Some(market.public_order),
            cutoff + Duration::days(1),
        ),
        (watch.spot_slice, None, cutoff + Duration::days(2)),
        (watch.spot_slice, Some(market.public_order), future),
        (
            watch.public_pin,
            Some(market.public_order),
            future + microsecond,
        ),
    ] {
        let id = insert_event(
            &mut fixture,
            &watch,
            target,
            BEST_PRICE,
            source,
            created_at,
            false,
        )
        .await;
        hidden.push(id);
    }
    let new_order = insert_event(
        &mut fixture,
        &watch,
        watch.spot_slice,
        "SLICE_NEW_ORDER",
        Some(market.public_order),
        cutoff - Duration::days(1),
        false,
    )
    .await;
    let price_change = insert_event(
        &mut fixture,
        &watch,
        watch.public_pin,
        "PIN_PRICE_CHANGED",
        Some(market.public_order),
        cutoff - Duration::days(2),
        true,
    )
    .await;

    let scope = public_scope(&mut app, &market).await;
    let feed = whole_feed(&mut app, &watch, &scope, 100).await;
    assert_eq!(ids(&feed), vec![new_order, price_change]);

    for activation in [
        cutoff - Duration::days(3),
        cutoff - microsecond,
        cutoff,
        cutoff + microsecond,
        cutoff + Duration::days(3),
    ] {
        let summary = summary_at(&mut app, &market, &watch, &scope, activation).await;
        assert_eq!(summary.unread_event_count, 1, "cutoff {activation}");
        assert_eq!(
            summary.latest_event_at,
            Some(cutoff - Duration::days(1)),
            "cutoff {activation}"
        );
        let spot = slice_of(&summary, watch.spot_slice);
        assert_eq!(spot.unread_event_count, 1, "cutoff {activation}");
        assert_eq!(
            spot.latest_event_at,
            Some(cutoff - Duration::days(1)),
            "cutoff {activation}"
        );
        let pins: Vec<Uuid> = spot.pins.iter().map(|pin| pin.id).collect();
        assert_eq!(pins, vec![watch.public_pin], "cutoff {activation}");
        assert_eq!(spot.pins[0].unread_event_count, 0, "cutoff {activation}");
        assert_eq!(
            spot.pins[0].latest_event_at,
            Some(cutoff - Duration::days(2)),
            "cutoff {activation}"
        );
        assert_feed_matches_counts(&feed, &summary);
    }

    for event in hidden {
        assert_refused_without_write(&mut app, &watch, &scope, event).await;
    }
}

/// Hidden rows are the newest rows, so a guard applied after `LIMIT` would
/// fill the first page with them and a guard applied after aggregation would
/// change counts, latest times and slice order.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn guard_precedes_pagination_and_counts() {
    let mut fixture = connect(FIXTURE_URL).await;
    let mut app = connect(APP_URL).await;
    let market = seeded_market();
    let watch = seed_watch(&mut fixture, &market).await;
    let base = instant("2026-10-19T00:00:00Z");
    let at = |seconds: i64| base + Duration::seconds(seconds);

    for (target, event_type, source, seconds) in [
        (watch.spot_slice, BEST_PRICE, Some(market.public_order), 9),
        (
            watch.spot_slice,
            "SLICE_NEW_ORDER",
            Some(market.closed_order),
            8,
        ),
        (
            watch.closed_pin,
            "PIN_PRICE_CHANGED",
            Some(market.closed_order),
            7,
        ),
        (watch.later_slice, BEST_PRICE, None, 6),
        (watch.spot_slice, BEST_PRICE, Some(market.public_order), 4),
        (watch.later_slice, BEST_PRICE, Some(market.public_order), 1),
    ] {
        insert_event(
            &mut fixture,
            &watch,
            target,
            event_type,
            source,
            at(seconds),
            false,
        )
        .await;
    }
    let newest_visible = insert_event(
        &mut fixture,
        &watch,
        watch.later_slice,
        "SLICE_NEW_ORDER",
        Some(market.public_order),
        at(5),
        false,
    )
    .await;
    let quantity_change = insert_event(
        &mut fixture,
        &watch,
        watch.public_pin,
        "PIN_QUANTITY_CHANGED",
        Some(market.public_order),
        at(3),
        true,
    )
    .await;
    let quiet = insert_event(
        &mut fixture,
        &watch,
        watch.spot_slice,
        "SLICE_WENT_QUIET",
        None,
        at(2),
        true,
    )
    .await;

    let scope = public_scope(&mut app, &market).await;
    let first = feed_page(
        &mut app,
        watch.container,
        &scope,
        EventQuery {
            limit: 2,
            cursor: None,
        },
    )
    .await
    .expect("the first page");
    assert_eq!(ids(&first.items), vec![newest_visible, quantity_change]);
    let cursor = decode_cursor(
        first
            .next_cursor
            .as_deref()
            .expect("a third visible row remains"),
    )
    .expect("an issued cursor");
    assert_eq!(cursor.id, quantity_change);
    let second = feed_page(
        &mut app,
        watch.container,
        &scope,
        EventQuery {
            limit: 2,
            cursor: Some(cursor),
        },
    )
    .await
    .expect("the second page");
    assert_eq!(ids(&second.items), vec![quiet]);
    assert_eq!(second.next_cursor, None);

    let summary = summary_at(&mut app, &market, &watch, &scope, base).await;
    assert_eq!(summary.unread_event_count, 1);
    assert_eq!(summary.latest_event_at, Some(at(5)));
    assert_eq!(summary.total_slice_count, 2);
    // Only a visible unread row moves a slice to the front.
    let order: Vec<Uuid> = summary.slices.iter().map(|slice| slice.id).collect();
    assert_eq!(order, vec![watch.later_slice, watch.spot_slice]);
    let later = slice_of(&summary, watch.later_slice);
    assert_eq!(later.unread_event_count, 1);
    assert_eq!(later.latest_event_at, Some(at(5)));
    assert_eq!(later.active_order_count, 0);
    let spot = slice_of(&summary, watch.spot_slice);
    assert_eq!(spot.unread_event_count, 0);
    assert_eq!(spot.latest_event_at, Some(at(3)));
    assert_eq!(spot.active_order_count, 1);
    let pins: Vec<Uuid> = spot.pins.iter().map(|pin| pin.id).collect();
    assert_eq!(pins, vec![watch.public_pin]);
}

/// Feed rows and counts agree for a mixed set of visible and hidden rows,
/// and they stay in agreement after a visible row is acknowledged.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn watchlist_feed_metrics_contract() {
    let mut fixture = connect(FIXTURE_URL).await;
    let mut app = connect(APP_URL).await;
    let market = seeded_market();
    let watch = seed_watch(&mut fixture, &market).await;
    let base = instant("2026-10-19T00:00:00Z");
    let public = Some(market.public_order);
    let closed = Some(market.closed_order);

    let mut visible = HashSet::new();
    let mut hidden = Vec::new();
    for (seconds, target, event_type, source, is_read, shown) in [
        (1, watch.spot_slice, "SLICE_NEW_ORDER", public, false, true),
        (2, watch.spot_slice, BEST_PRICE, public, false, false),
        (
            3,
            watch.public_pin,
            "PIN_PRICE_CHANGED",
            public,
            false,
            true,
        ),
        (4, watch.public_pin, "PIN_FILLED", None, true, true),
        (5, watch.closed_pin, "PIN_WITHDRAWN", closed, false, false),
        (
            6,
            watch.later_slice,
            "SLICE_BENCHMARK_MOVED",
            None,
            false,
            true,
        ),
        (
            7,
            watch.later_slice,
            "SLICE_NEW_ORDER",
            closed,
            false,
            false,
        ),
        (8, watch.later_slice, BEST_PRICE, None, true, false),
        (9, watch.spot_slice, "SLICE_WENT_QUIET", None, true, true),
    ] {
        let id = insert_event(
            &mut fixture,
            &watch,
            target,
            event_type,
            source,
            base + Duration::seconds(seconds),
            is_read,
        )
        .await;
        if shown {
            visible.insert(id);
        } else {
            hidden.push(id);
        }
    }

    let scope = public_scope(&mut app, &market).await;
    let feed = whole_feed(&mut app, &watch, &scope, 1).await;
    assert_eq!(ids(&feed).into_iter().collect::<HashSet<_>>(), visible);
    assert_eq!(feed.len(), visible.len(), "no feed row repeats");
    let summary = summary_at(&mut app, &market, &watch, &scope, base).await;
    assert_feed_matches_counts(&feed, &summary);
    assert_eq!(summary.unread_event_count, 3);

    let first_unread = feed
        .iter()
        .find(|row| row.watchlist_target_id == watch.spot_slice && !row.is_read)
        .map(|row| row.id)
        .expect("a visible unread slice row");
    let mut transaction = app.begin().await.expect("a read-state transaction");
    let acknowledged = acknowledge(&mut transaction, watch.container, first_unread, &scope)
        .await
        .expect("a visible row can be acknowledged")
        .expect("its target exists");
    assert!(acknowledged.is_read);
    transaction.commit().await.expect("commit the read state");

    let feed = whole_feed(&mut app, &watch, &scope, 1).await;
    let summary = summary_at(&mut app, &market, &watch, &scope, base).await;
    assert_feed_matches_counts(&feed, &summary);
    assert_eq!(summary.unread_event_count, 2);

    for event in hidden {
        assert_refused_without_write(&mut app, &watch, &scope, event).await;
    }
}

/// Row counts and content digests of every watchlist table.
async fn watchlist_digest(app: &mut PgConnection) -> String {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(\
           'watchlists', (SELECT jsonb_build_array(count(*), \
              md5(coalesce(string_agg(t::text, '|' ORDER BY t.id), ''))) FROM watchlists t),\
           'watchlist_entries', (SELECT jsonb_build_array(count(*), \
              md5(coalesce(string_agg(t::text, '|' ORDER BY t.id), ''))) FROM watchlist_entries t),\
           'watchlist_targets', (SELECT jsonb_build_array(count(*), \
              md5(coalesce(string_agg(t::text, '|' ORDER BY t.id), ''))) FROM watchlist_targets t),\
           'watchlist_events', (SELECT jsonb_build_array(count(*), \
              md5(coalesce(string_agg(t::text, '|' ORDER BY t.id), ''))) FROM watchlist_events t)\
         )::text",
    )
    .fetch_one(app)
    .await
    .expect("the watchlist digest")
}

/// The sequencing statement scoped to this test's rows: the same assignment
/// a dispatcher runs, on the session that holds the shared sequencer lock.
const ASSIGN_SEQUENCES: &str = "\
    WITH pending AS (\
        SELECT id FROM market_event_outbox \
        WHERE stream_seq IS NULL AND id = ANY($1) \
        ORDER BY created_at, id \
        FOR UPDATE SKIP LOCKED\
    ) \
    UPDATE market_event_outbox AS outbox \
    SET stream_seq = nextval('market_event_stream_seq'), \
        dispatched_at = now(), \
        delivery_attempts = outbox.delivery_attempts + 1 \
    FROM pending \
    WHERE outbox.id = pending.id \
    RETURNING outbox.stream_seq";

/// Producing, sequencing and reading committed market events leaves every
/// watchlist table unchanged. No outbox trigger or rule can write watchlist
/// rows, and the stream transport sources name no watchlist table or module.
#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn dispatch_does_not_create_watchlist_rows() {
    let mut fixture = connect(FIXTURE_URL).await;
    let mut app = connect(APP_URL).await;
    let market = seeded_market();
    let watch = seed_watch(&mut fixture, &market).await;
    let base = instant("2026-10-19T00:00:00Z");
    insert_event(
        &mut fixture,
        &watch,
        watch.public_pin,
        "PIN_PRICE_CHANGED",
        Some(market.public_order),
        base,
        false,
    )
    .await;
    insert_event(
        &mut fixture,
        &watch,
        watch.spot_slice,
        BEST_PRICE,
        Some(market.public_order),
        base + Duration::seconds(1),
        false,
    )
    .await;
    let before = watchlist_digest(&mut app).await;

    // A producer commits participant-scoped envelopes and wakes the sequencer.
    let trade = Uuid::new_v4();
    let participants = json!([market.organization.to_string()]);
    let envelopes = [
        (
            "order_created",
            "order",
            market.public_order,
            json!({
                "schema_version": 1,
                "order_id": market.public_order.to_string(),
                "_public_market_invalidation": true,
            }),
        ),
        (
            "trade_created",
            "trade",
            trade,
            json!({"schema_version": 1, "trade_id": trade.to_string(), "quantity_mt": "10"}),
        ),
        (
            "trade_delivered",
            "trade",
            trade,
            json!({"schema_version": 1, "trade_id": trade.to_string(), "final_quantity_mt": "10"}),
        ),
    ];
    let mut envelope_ids = Vec::new();
    let mut producer = app.begin().await.expect("a producer transaction");
    for (event_type, aggregate_type, aggregate, payload) in envelopes {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO market_event_outbox \
             (id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload) \
             VALUES ($1,$2,$3,$4,$5::json,$6::json)",
        )
        .bind(id)
        .bind(event_type)
        .bind(aggregate_type)
        .bind(aggregate.to_string())
        .bind(&participants)
        .bind(payload)
        .execute(&mut *producer)
        .await
        .expect("enqueue an envelope");
        envelope_ids.push(id);
    }
    sqlx::query("SELECT pg_notify('verdaxis_market_events', '')")
        .execute(&mut *producer)
        .await
        .expect("wake the sequencer");
    producer.commit().await.expect("commit the producer");
    assert_eq!(
        watchlist_digest(&mut app).await,
        before,
        "after the producer"
    );

    // One elected session assigns the sequence after the producer commit.
    let mut sequencer = connect(APP_URL).await;
    let elected: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(SEQUENCER_KEY)
        .fetch_one(&mut sequencer)
        .await
        .expect("contend for the sequencer lock");
    assert!(elected, "no other sequencer holds the shared lock");
    let assigned: Vec<i64> = sqlx::query_scalar(ASSIGN_SEQUENCES)
        .bind(&envelope_ids)
        .fetch_all(&mut sequencer)
        .await
        .expect("assign sequence numbers");
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(SEQUENCER_KEY)
        .fetch_one(&mut sequencer)
        .await
        .expect("release the sequencer lock");
    assert!(released, "the session held the lock it released");
    sqlx::query("SELECT pg_notify('verdaxis_market_events', '')")
        .execute(&mut sequencer)
        .await
        .expect("wake the hubs");
    assert_eq!(assigned.len(), envelope_ids.len());
    assert_eq!(watchlist_digest(&mut app).await, before, "after sequencing");

    // A hub reads the participant's sequenced rows in one read-only snapshot.
    let mut reader = app.begin().await.expect("a replay transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *reader)
        .await
        .expect("a read-only snapshot");
    let delivered: Vec<(i64, String)> = sqlx::query_as(
        "SELECT stream_seq, event_type FROM market_event_outbox \
         WHERE stream_seq IS NOT NULL AND id = ANY($1) \
           AND participant_org_ids::jsonb ? $2 \
         ORDER BY stream_seq",
    )
    .bind(&envelope_ids)
    .bind(market.organization.to_string())
    .fetch_all(&mut *reader)
    .await
    .expect("read the sequenced rows");
    reader.commit().await.expect("end the snapshot");
    // Sequence order is authoritative; a single producer transaction does not
    // impose an event-type order on rows with equal creation timestamps.
    let mut delivered_types: Vec<&str> = delivered.iter().map(|(_, kind)| kind.as_str()).collect();
    delivered_types.sort_unstable();
    assert_eq!(
        delivered_types,
        vec!["order_created", "trade_created", "trade_delivered"]
    );
    assert!(delivered.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(
        watchlist_digest(&mut app).await,
        before,
        "after the replay read"
    );

    let trigger_writers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid \
         WHERE t.tgrelid = 'market_event_outbox'::regclass AND NOT t.tgisinternal \
           AND p.prosrc ILIKE '%watchlist%'",
    )
    .fetch_one(&mut app)
    .await
    .expect("inspect outbox triggers");
    assert_eq!(
        trigger_writers, 0,
        "an outbox trigger names watchlist tables"
    );
    let rules: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_rules WHERE tablename = 'market_event_outbox'")
            .fetch_one(&mut app)
            .await
            .expect("inspect outbox rules");
    assert_eq!(rules, 0, "the outbox has a rewrite rule");

    assert_eq!(
        stream_sources_naming_watchlists(&repository_root()),
        Vec::<String>::new()
    );
}
