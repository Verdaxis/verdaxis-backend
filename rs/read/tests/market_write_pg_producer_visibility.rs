//! Watchlist event admission: one trusted-source rule for the feed, the
//! acknowledgement and the container counts.
//!
//! A best-price or went-quiet row is admitted only when the trusted producer
//! wrote it and attested its scope at or after the activation instant the
//! release recorded; rows of any other producer stay hidden for those classes.
//! A row that names a source order needs that order public, except a
//! went-quiet row, which names the order that just ended. A PIN needs a public
//! pinned order. The three surfaces must give the same answer for every row.
//!
//! The source rules run in every build. The database cases need a migrated
//! disposable template (`RUST_PRODUCER_*`); each case clones its own database.
//! The schema owner seeds every row through `fixtures/phase8_producer_visibility.sql`;
//! every read and acknowledgement runs as the application role.

#[path = "support/market_database.rs"]
mod market_database;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::crud::watchlists::{
    EventAdmission, EventQuery, Summary, VisibleSnapshot, WatchlistEvent, WatchlistSettings,
    acknowledge, capture_visible, decode_cursor, feed_page, router,
};
use verdaxis_read::{AppState, compiled_sha, Config, MarketPolicy, PointPolicy, ProductPolicy, runtime::artifact::ALLOWED_REVISIONS, VisibilityClock};

const BEST_PRICE: &str = "SLICE_BEST_PRICE_MOVED";
const PRODUCT_CODE: &str = "VISIBILITY";
const SECRET: &str = "synthetic-visibility-test-signing-key-32";
const ISSUER: &str = "visibility-tests";

/// The activation instant this release recorded for the trusted producer.
fn activation() -> DateTime<Utc> {
    "2026-10-20T00:00:00Z".parse().unwrap()
}

fn recorded() -> EventAdmission {
    EventAdmission::new(Some(activation()))
}

fn unrecorded() -> EventAdmission {
    EventAdmission::new(None)
}

// ------------------------------------------------------------- source rules

fn read_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("a readable source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

fn source(relative: &str) -> String {
    std::fs::read_to_string(read_root().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(read_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The runtime role may write every column of `watchlist_events`, so the
/// database cannot stop a forged trusted row. The code boundary is the control:
/// only the trusted producer's own module may name the trusted identity in a
/// write, and the admission relation names it only to read it.
#[test]
fn trusted_identity_is_read_only_by_admission_and_written_only_by_the_producer() {
    let mut sources = Vec::new();
    rust_sources(&read_root().join("src"), &mut sources);
    assert!(sources.len() > 50, "the production sources are present");
    let naming: Vec<String> = sources
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .expect("a readable source")
                .contains("rust-market-v1")
        })
        .map(|path| relative(path))
        .collect();
    let producer = "src/order_write/producer";
    let unexpected: Vec<&String> = naming
        .iter()
        .filter(|path| *path != "src/crud/watchlists/admission.rs" && !path.starts_with(producer))
        .collect();
    assert!(
        unexpected.is_empty(),
        "only the admission relation and the trusted producer name the trusted identity: {unexpected:?}"
    );
    assert!(
        naming.contains(&"src/crud/watchlists/admission.rs".to_owned()),
        "the admission relation reads the trusted identity"
    );
    let admission = source("src/crud/watchlists/admission.rs").to_ascii_lowercase();
    for write in [
        "insert into",
        "update watchlist",
        "delete from",
        "set is_read",
    ] {
        assert!(
            !admission.contains(write),
            "the admission relation issues `{write}`"
        );
    }
}

/// The feed, the acknowledgement read and its update, and the counts each
/// embed the one relation. No other watchlist source spells the guarded
/// classes or the trusted identity, so no second predicate can drift, and the
/// handlers cannot build an admission of their own: the settings build the
/// only one, from the release value, with no fixed instant in its place.
#[test]
fn feed_acknowledgement_and_metrics_share_one_relation() {
    let relation = "event_visible_sql!(";
    let events = source("src/crud/watchlists/events.rs");
    assert_eq!(
        events.matches(relation).count(),
        3,
        "the feed, the single read and the acknowledgement update"
    );
    let metrics = source("src/crud/watchlists/metrics.rs");
    assert_eq!(metrics.matches(relation).count(), 1, "the counts");

    let mut sources = Vec::new();
    rust_sources(&read_root().join("src"), &mut sources);
    for path in &sources {
        let name = relative(path);
        let text = std::fs::read_to_string(path).expect("a readable source");
        assert!(
            !text.contains("fn event_visible("),
            "{name} keeps a second visibility predicate"
        );
        if name.starts_with("src/crud/watchlists/") && name != "src/crud/watchlists/admission.rs" {
            for literal in [BEST_PRICE, "SLICE_WENT_QUIET", "rust-market-v1"] {
                assert!(!text.contains(literal), "{name} spells `{literal}`");
            }
        }
        let builds = text.matches("EventAdmission::new(").count();
        if name == "src/crud/watchlists/http.rs" {
            assert_eq!(builds, 1, "the settings build the one admission");
        } else {
            assert_eq!(builds, 0, "{name} builds an admission");
        }
    }
    for name in [
        "src/crud/watchlists/admission.rs",
        "src/crud/watchlists/http.rs",
    ] {
        let text = source(name);
        for fixed in ["MAX_UTC", "MIN_UTC", "UNIX_EPOCH", "with_ymd_and_hms"] {
            assert!(!text.contains(fixed), "{name} takes `{fixed}`");
        }
    }
}

// ------------------------------------------------------------- PostgreSQL

/// The public order scope reads only the runtime KYC switch from the
/// configuration; the other values are inert.
fn read_config(database_url: String) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
        database_url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().expect("a loopback address"),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 0,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        phase2: Default::default(),
    }
}

/// One market, one watchlist and its four targets on a fresh database.
struct Fixture {
    admin: PgConnection,
    app: PgConnection,
    runtime_url: String,
    policy: MarketPolicy,
    owner: Uuid,
    public_order: Uuid,
    closed_order: Uuid,
    container: Uuid,
    spot_slice: Uuid,
    later_slice: Uuid,
    public_pin: Uuid,
    closed_pin: Uuid,
    _database: market_database::MarketDatabase,
}

impl Fixture {
    async fn new() -> Self {
        let database = market_database::MarketDatabase::new("RUST_PRODUCER").await;
        let mut admin = PgConnection::connect(&database.admin_url).await.unwrap();
        sqlx::raw_sql(include_str!("fixtures/phase8_producer_visibility.sql"))
            .execute(&mut admin)
            .await
            .unwrap();
        let ids: Vec<Uuid> = (0..11).map(|_| Uuid::new_v4()).collect();
        let (product, point, organization, owner) = (ids[0], ids[1], ids[2], ids[3]);
        let (public_order, closed_order, container) = (ids[4], ids[5], ids[6]);
        let (spot_slice, later_slice, public_pin, closed_pin) = (ids[7], ids[8], ids[9], ids[10]);
        let label = product.simple().to_string();
        let product_name = format!("Visibility methanol {label}");
        let fuel_grade = format!("Visibility {label}");
        let point_name = format!("Visibility point {label}");
        sqlx::query(
            "SELECT producer_visibility_seed($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
        )
        .bind(product)
        .bind(&product_name)
        .bind("Methanol")
        .bind(&fuel_grade)
        .bind(point)
        .bind(&point_name)
        .bind("Visibility region")
        .bind(organization)
        .bind(owner)
        .bind(public_order)
        .bind(closed_order)
        .bind(container)
        .bind(spot_slice)
        .bind(later_slice)
        .bind(public_pin)
        .bind(closed_pin)
        .bind(PRODUCT_CODE)
        .execute(&mut admin)
        .await
        .unwrap();
        let policy = MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![ProductPolicy {
                id: product,
                code: PRODUCT_CODE.into(),
                name: product_name,
                fuel_type: "Methanol".into(),
                fuel_grade,
                execution_mode: "ORDERBOOK".into(),
                lanes: vec![point],
                qualification_profile: "scalar".into(),
                specification_standard: None,
                display_order: 1,
            }],
            points: vec![PointPolicy {
                id: point,
                name: point_name,
                region: "Visibility region".into(),
                timezone: "UTC".into(),
                display_order: 1,
            }],
        };
        let app = PgConnection::connect(&database.runtime_url).await.unwrap();
        let mut fixture = Self {
            admin,
            app,
            runtime_url: database.runtime_url.clone(),
            policy,
            owner,
            public_order,
            closed_order,
            container,
            spot_slice,
            later_slice,
            public_pin,
            closed_pin,
            _database: database,
        };
        let scope = fixture.scope().await;
        assert!(scope.contains(public_order), "the open order is public");
        assert!(
            !scope.contains(closed_order),
            "the cancelled order is outside the public scope"
        );
        fixture
    }

    /// Store one event through the fixture. `trusted` is the attestation
    /// instant of a trusted row; `None` stores a legacy row.
    #[allow(clippy::too_many_arguments)]
    async fn event_keyed(
        &mut self,
        target: Uuid,
        kind: &str,
        source: Option<Uuid>,
        created: DateTime<Utc>,
        read: bool,
        trusted: Option<DateTime<Utc>>,
        key: Option<&str>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let key = key.map(str::to_owned).unwrap_or_else(|| id.to_string());
        sqlx::query("SELECT producer_visibility_event($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(id)
            .bind(self.container)
            .bind(target)
            .bind(kind)
            .bind(source)
            .bind(created)
            .bind(read)
            .bind(trusted.is_some())
            .bind(trusted)
            .bind(key)
            .execute(&mut self.admin)
            .await
            .unwrap();
        id
    }

    async fn event(
        &mut self,
        target: Uuid,
        kind: &str,
        source: Option<Uuid>,
        created: DateTime<Utc>,
        trusted: Option<DateTime<Utc>>,
    ) -> Uuid {
        self.event_keyed(target, kind, source, created, false, trusted, None)
            .await
    }

    /// The shared public order relation, captured through the application role.
    async fn scope(&mut self) -> VisibleSnapshot {
        capture_visible(
            &mut self.app,
            &self.policy,
            &VisibilityClock::at(Utc::now()),
            &read_config(self.runtime_url.clone()),
        )
        .await
        .expect("capture the public order scope")
    }

    /// Every feed row, page by page through the issued cursors.
    async fn feed(
        &mut self,
        scope: &VisibleSnapshot,
        admission: EventAdmission,
        limit: i64,
    ) -> Vec<WatchlistEvent> {
        let mut rows = Vec::new();
        let mut cursor = None;
        for _ in 0..100 {
            let page = feed_page(
                &mut self.app,
                self.container,
                scope,
                admission,
                EventQuery { limit, cursor },
            )
            .await
            .expect("a feed page");
            assert!(
                page.items.len() <= usize::try_from(limit).unwrap(),
                "a page holds at most the limit"
            );
            rows.extend(page.items);
            match page.next_cursor {
                Some(text) => cursor = Some(decode_cursor(&text).expect("an issued cursor")),
                None => return rows,
            }
        }
        panic!("the feed did not end within 100 pages");
    }

    async fn summary(&mut self, scope: &VisibleSnapshot, admission: EventAdmission) -> Summary {
        scope
            .detail(&mut self.app, self.owner, self.container, admission)
            .await
            .expect("the detail read")
            .expect("the owner's watchlist")
    }

    /// Acknowledge one event in its own committed transaction.
    async fn acknowledge(
        &mut self,
        scope: &VisibleSnapshot,
        admission: EventAdmission,
        event: Uuid,
    ) -> Result<Option<WatchlistEvent>, u16> {
        let mut transaction = self.app.begin().await.unwrap();
        let result = acknowledge(&mut transaction, self.container, event, scope, admission).await;
        transaction.commit().await.unwrap();
        result.map_err(|error| error.status.as_u16())
    }

    async fn rows(&mut self) -> Value {
        sqlx::query_scalar("SELECT producer_visibility_rows($1)")
            .bind(self.container)
            .fetch_one(&mut self.admin)
            .await
            .unwrap()
    }

    /// The slice a target counts toward: the spot slice holds the public pin.
    fn slice_of(&self, target: Uuid) -> Uuid {
        if target == self.public_pin || target == self.closed_pin {
            self.spot_slice
        } else {
            target
        }
    }

    /// The counts agree with exactly `visible`, given as (target, is_read,
    /// created_at): each slice counts its own rows and its visible pins' rows,
    /// the public pin counts its own, and the hidden pin is not listed.
    fn assert_counts(&self, summary: &Summary, visible: &[(Uuid, bool, DateTime<Utc>)]) {
        let mut unread = HashMap::<Uuid, i64>::new();
        let mut latest = HashMap::<Uuid, DateTime<Utc>>::new();
        for (target, read, created) in visible {
            let slice = self.slice_of(*target);
            let mut counted = vec![*target];
            if slice != *target {
                counted.push(slice);
            }
            for owner in counted {
                if !read {
                    *unread.entry(owner).or_default() += 1;
                }
                let newest = latest.entry(owner).or_insert(*created);
                *newest = (*newest).max(*created);
            }
        }
        let slices: HashSet<Uuid> = summary.slices.iter().map(|slice| slice.id).collect();
        assert_eq!(slices, HashSet::from([self.spot_slice, self.later_slice]));
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
            let pins: Vec<Uuid> = slice.pins.iter().map(|pin| pin.id).collect();
            if slice.id == self.spot_slice {
                assert_eq!(pins, vec![self.public_pin], "only the public pin is listed");
            } else {
                assert!(pins.is_empty());
            }
            for pin in &slice.pins {
                assert_eq!(pin.latest_event_at, latest.get(&pin.id).copied());
                let own = visible
                    .iter()
                    .filter(|(target, read, _)| *target == pin.id && !read)
                    .count();
                assert_eq!(pin.unread_event_count, i64::try_from(own).unwrap());
            }
        }
        let total = visible.iter().filter(|(_, read, _)| !read).count();
        assert_eq!(summary.unread_event_count, i64::try_from(total).unwrap());
        assert_eq!(
            summary.latest_event_at,
            visible.iter().map(|(_, _, created)| *created).max()
        );
    }
}

fn ids(rows: &[WatchlistEvent]) -> Vec<Uuid> {
    rows.iter().map(|row| row.id).collect()
}

/// The plan's first case: an attested trusted best-price row is listed,
/// counted and acknowledged; a legacy row of the same class and target stays
/// hidden and its acknowledgement writes nothing.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn trusted_best_price_row_is_visible_and_acknowledgeable() {
    let mut f = Fixture::new().await;
    let at = activation() + Duration::seconds(1);
    let trusted = f.event(f.spot_slice, BEST_PRICE, None, at, Some(at)).await;
    let legacy = f
        .event(
            f.spot_slice,
            BEST_PRICE,
            None,
            at + Duration::seconds(1),
            None,
        )
        .await;
    let scope = f.scope().await;

    let feed = f.feed(&scope, recorded(), 20).await;
    assert_eq!(ids(&feed), vec![trusted]);
    let summary = f.summary(&scope, recorded()).await;
    f.assert_counts(&summary, &[(f.spot_slice, false, at)]);

    let before = f.rows().await;
    assert_eq!(
        f.acknowledge(&scope, recorded(), legacy).await.err(),
        Some(404)
    );
    assert_eq!(
        f.rows().await,
        before,
        "a refused acknowledgement writes nothing"
    );

    let acknowledged = f
        .acknowledge(&scope, recorded(), trusted)
        .await
        .expect("a visible row can be acknowledged")
        .expect("its target exists");
    assert_eq!(acknowledged.id, trusted);
    assert!(acknowledged.is_read);
    let summary = f.summary(&scope, recorded()).await;
    f.assert_counts(&summary, &[(f.spot_slice, true, at)]);
    let feed = f.feed(&scope, recorded(), 20).await;
    assert_eq!(ids(&feed), vec![trusted]);
    assert!(feed[0].is_read);
}

/// One expected table drives all three surfaces, with and without a recorded
/// activation: the feed lists exactly the admitted rows, the counts count
/// exactly them, and exactly they can be acknowledged. A refused
/// acknowledgement leaves its row's read state and version unchanged.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn event_visible_agrees_across_feed_acknowledgement_and_metrics() {
    let a = activation();
    let micro = Duration::microseconds(1);
    for admission in [recorded(), unrecorded()] {
        let mut f = Fixture::new().await;
        let public = Some(f.public_order);
        let closed = Some(f.closed_order);
        let (spot, later, public_pin, closed_pin) =
            (f.spot_slice, f.later_slice, f.public_pin, f.closed_pin);
        let far = Utc::now() + Duration::days(365);
        // (target, class, source order, is_read, trusted attestation,
        //  derived key, admitted with the activation, admitted without it)
        type Row = (
            Uuid,
            &'static str,
            Option<Uuid>,
            bool,
            Option<DateTime<Utc>>,
            Option<&'static str>,
            bool,
            bool,
        );
        let table: Vec<Row> = vec![
            (spot, BEST_PRICE, None, false, None, None, false, false),
            (
                spot,
                BEST_PRICE,
                None,
                false,
                Some(a - micro),
                None,
                false,
                false,
            ),
            (spot, BEST_PRICE, None, false, Some(a), None, true, false),
            (
                spot,
                BEST_PRICE,
                None,
                false,
                Some(a + Duration::hours(1)),
                Some("shared"),
                true,
                false,
            ),
            (
                spot,
                BEST_PRICE,
                None,
                false,
                Some(a + Duration::hours(2)),
                Some("shared"),
                true,
                false,
            ),
            (
                spot,
                BEST_PRICE,
                public,
                false,
                Some(a + Duration::hours(1)),
                None,
                true,
                false,
            ),
            (
                spot,
                BEST_PRICE,
                closed,
                false,
                Some(a + Duration::hours(1)),
                None,
                false,
                false,
            ),
            (
                spot,
                "SLICE_NEW_ORDER",
                public,
                false,
                None,
                None,
                true,
                true,
            ),
            (
                spot,
                "SLICE_NEW_ORDER",
                closed,
                false,
                None,
                None,
                false,
                false,
            ),
            (
                spot,
                "SLICE_NEW_ORDER",
                public,
                false,
                Some(a - Duration::days(1)),
                None,
                true,
                true,
            ),
            // A went-quiet row names the order that just ended.
            (
                spot,
                "SLICE_WENT_QUIET",
                closed,
                true,
                Some(a + Duration::hours(1)),
                None,
                true,
                false,
            ),
            (
                later,
                "SLICE_BENCHMARK_MOVED",
                None,
                false,
                None,
                None,
                true,
                true,
            ),
            (
                public_pin,
                "PIN_PRICE_CHANGED",
                public,
                true,
                None,
                None,
                true,
                true,
            ),
            (
                public_pin,
                "PIN_QUANTITY_CHANGED",
                public,
                false,
                Some(a + Duration::hours(1)),
                None,
                true,
                true,
            ),
            (
                closed_pin,
                "PIN_WITHDRAWN",
                closed,
                false,
                None,
                None,
                false,
                false,
            ),
            (
                closed_pin,
                "PIN_PRICE_CHANGED",
                None,
                false,
                Some(a + Duration::hours(1)),
                None,
                false,
                false,
            ),
            (later, BEST_PRICE, None, false, None, None, false, false),
            (
                later,
                BEST_PRICE,
                None,
                true,
                Some(a + Duration::hours(3)),
                None,
                true,
                false,
            ),
        ];
        let mut admitted = Vec::new();
        let mut refused = Vec::new();
        for (index, (target, kind, source, read, trusted, key, with, without)) in
            table.into_iter().enumerate()
        {
            // Legacy best-price rows dated after the activation, and one dated
            // far after this run, stay hidden: no clock reveals them.
            let created = if index == 16 {
                far
            } else {
                a + Duration::days(1) + Duration::seconds(i64::try_from(index).unwrap())
            };
            let id = f
                .event_keyed(target, kind, source, created, read, trusted, key)
                .await;
            let expected = if admission == recorded() {
                with
            } else {
                without
            };
            if expected {
                admitted.push((id, target, read, created));
            } else {
                refused.push(id);
            }
        }
        let scope = f.scope().await;

        let feed = f.feed(&scope, admission, 3).await;
        let listed: HashSet<Uuid> = ids(&feed).into_iter().collect();
        assert_eq!(listed.len(), feed.len(), "no feed row repeats");
        let expected: HashSet<Uuid> = admitted.iter().map(|row| row.0).collect();
        assert_eq!(listed, expected, "{admission:?}: feed");
        let counted: Vec<(Uuid, bool, DateTime<Utc>)> = admitted
            .iter()
            .map(|(_, target, read, created)| (*target, *read, *created))
            .collect();
        let summary = f.summary(&scope, admission).await;
        f.assert_counts(&summary, &counted);

        let before = f.rows().await;
        for id in &refused {
            assert_eq!(
                f.acknowledge(&scope, admission, *id).await.err(),
                Some(404),
                "{admission:?}: refused row {id}"
            );
        }
        assert_eq!(f.rows().await, before, "refusals write nothing");
        for (id, ..) in &admitted {
            let acknowledged = f
                .acknowledge(&scope, admission, *id)
                .await
                .unwrap_or_else(|status| panic!("{admission:?}: row {id} refused with {status}"))
                .expect("its target exists");
            assert!(acknowledged.is_read);
        }
        let after = f.rows().await;
        for id in &refused {
            let key = id.to_string();
            assert_eq!(after["events"][&key], before["events"][&key], "row {id}");
        }
        for (id, ..) in &admitted {
            assert_eq!(after["events"][id.to_string()][0], json!(true), "row {id}");
        }
        let read: Vec<(Uuid, bool, DateTime<Utc>)> = counted
            .iter()
            .map(|(target, _, created)| (*target, true, *created))
            .collect();
        let summary = f.summary(&scope, admission).await;
        f.assert_counts(&summary, &read);
        let feed = f.feed(&scope, admission, 3).await;
        assert_eq!(ids(&feed).into_iter().collect::<HashSet<_>>(), expected);
    }
}

/// Legacy best-price rows stay hidden in every surface for every activation
/// value, including none, and for rows dated before, at, after and far after
/// the activation. Only the trusted producer can make the class visible.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn legacy_row_is_never_visible_for_the_best_price_class() {
    let mut f = Fixture::new().await;
    let a = activation();
    let micro = Duration::microseconds(1);
    let public = Some(f.public_order);
    let ordinary_at = a - Duration::days(2);
    let ordinary = f
        .event(f.later_slice, "SLICE_NEW_ORDER", None, ordinary_at, None)
        .await;
    let mut legacy = Vec::new();
    for (index, created) in [
        a - Duration::days(1),
        a - micro,
        a,
        a + micro,
        a + Duration::days(1),
        Utc::now() + Duration::days(365),
    ]
    .into_iter()
    .enumerate()
    {
        let (target, source) = if index % 2 == 0 {
            (f.spot_slice, public)
        } else {
            (f.later_slice, None)
        };
        legacy.push(f.event(target, BEST_PRICE, source, created, None).await);
    }
    let scope = f.scope().await;
    let before = f.rows().await;
    for admission in [
        unrecorded(),
        EventAdmission::new(Some("1970-01-01T00:00:00Z".parse().unwrap())),
        EventAdmission::new(Some(a - Duration::days(1))),
        recorded(),
        EventAdmission::new(Some(a + Duration::days(1))),
        EventAdmission::new(Some("2100-01-01T00:00:00Z".parse().unwrap())),
    ] {
        let feed = f.feed(&scope, admission, 2).await;
        assert_eq!(ids(&feed), vec![ordinary], "{admission:?}");
        let summary = f.summary(&scope, admission).await;
        f.assert_counts(&summary, &[(f.later_slice, false, ordinary_at)]);
        for id in &legacy {
            assert_eq!(
                f.acknowledge(&scope, admission, *id).await.err(),
                Some(404),
                "{admission:?}: legacy row {id}"
            );
        }
    }
    assert_eq!(f.rows().await, before, "no refusal wrote a row");
}

/// Hidden rows newer than a full page do not take places on it: the guard
/// runs before `LIMIT`, so the first page is full, the cursor pages without
/// repeats and the counts equal the listed rows.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn hidden_rows_do_not_shorten_a_visible_page() {
    let mut f = Fixture::new().await;
    let a = activation();
    let at = |seconds: i64| a + Duration::seconds(seconds);
    for seconds in 10..16 {
        f.event(f.spot_slice, BEST_PRICE, None, at(seconds), None)
            .await;
    }
    let mut visible = Vec::new();
    for seconds in 1..7 {
        let id = if seconds % 2 == 0 {
            f.event(
                f.spot_slice,
                BEST_PRICE,
                None,
                at(seconds),
                Some(at(seconds)),
            )
            .await
        } else {
            f.event(f.spot_slice, "SLICE_NEW_ORDER", None, at(seconds), None)
                .await
        };
        visible.push(id);
    }
    visible.reverse();
    let scope = f.scope().await;
    let first = feed_page(
        &mut f.app,
        f.container,
        &scope,
        recorded(),
        EventQuery {
            limit: 5,
            cursor: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&first.items), visible[..5].to_vec());
    let cursor = decode_cursor(first.next_cursor.as_deref().expect("one row remains")).unwrap();
    let second = feed_page(
        &mut f.app,
        f.container,
        &scope,
        recorded(),
        EventQuery {
            limit: 5,
            cursor: Some(cursor),
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&second.items), visible[5..].to_vec());
    assert_eq!(second.next_cursor, None);
    let summary = f.summary(&scope, recorded()).await;
    let counted: Vec<(Uuid, bool, DateTime<Utc>)> = (1..7)
        .map(|seconds| (f.spot_slice, false, at(seconds)))
        .collect();
    f.assert_counts(&summary, &counted);
}

/// Trusted rows that name an order follow that order's current public scope:
/// when it leaves the scope, the feed, the counts and the acknowledgement all
/// refuse them. A trusted best-price row that names no order is decided by its
/// attestation and the activation alone, so it stays visible.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn order_leaving_the_public_scope_hides_the_trusted_event() {
    let mut f = Fixture::new().await;
    let at = activation() + Duration::seconds(1);
    let public = Some(f.public_order);
    let new_order = f
        .event(f.spot_slice, "SLICE_NEW_ORDER", public, at, Some(at))
        .await;
    let pin_price = f
        .event(
            f.public_pin,
            "PIN_PRICE_CHANGED",
            public,
            at + Duration::seconds(1),
            Some(at),
        )
        .await;
    let best_price_without_order = f
        .event(
            f.spot_slice,
            BEST_PRICE,
            None,
            at + Duration::seconds(2),
            Some(at),
        )
        .await;

    let scope = f.scope().await;
    let feed = f.feed(&scope, recorded(), 20).await;
    assert_eq!(
        ids(&feed),
        vec![best_price_without_order, pin_price, new_order]
    );
    let summary = f.summary(&scope, recorded()).await;
    f.assert_counts(
        &summary,
        &[
            (f.spot_slice, false, at),
            (f.public_pin, false, at + Duration::seconds(1)),
            (f.spot_slice, false, at + Duration::seconds(2)),
        ],
    );

    sqlx::query("SELECT producer_visibility_close_order($1)")
        .bind(f.public_order)
        .execute(&mut f.admin)
        .await
        .unwrap();
    let scope = f.scope().await;
    assert!(
        !scope.contains(f.public_order),
        "the order left the public scope"
    );

    let feed = f.feed(&scope, recorded(), 20).await;
    assert_eq!(ids(&feed), vec![best_price_without_order]);
    let summary = f.summary(&scope, recorded()).await;
    let spot = summary
        .slices
        .iter()
        .find(|slice| slice.id == f.spot_slice)
        .expect("the spot slice");
    assert!(
        spot.pins.is_empty(),
        "the pin on a hidden order is not listed"
    );
    assert_eq!(spot.unread_event_count, 1);
    assert_eq!(spot.latest_event_at, Some(at + Duration::seconds(2)));
    assert_eq!(summary.unread_event_count, 1);
    let before = f.rows().await;
    for id in [new_order, pin_price] {
        assert_eq!(f.acknowledge(&scope, recorded(), id).await.err(), Some(404));
    }
    assert_eq!(f.rows().await, before);
    assert!(
        f.acknowledge(&scope, recorded(), best_price_without_order)
            .await
            .expect("the best-price row stays acknowledgeable")
            .is_some()
    );
}

/// A went-quiet row names the order that just ended, which is never public
/// again, so the order rule does not apply to this class. Instead the trusted
/// producer's attestation at or after the activation admits it: a realistic
/// trusted row is listed, counted and acknowledged, while untrusted rows (with
/// or without an order) and a row attested before the activation stay hidden,
/// and with no recorded activation every went-quiet row stays hidden.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn went_quiet_rows_need_trusted_attestation_after_activation() {
    let mut f = Fixture::new().await;
    let a = activation();
    let ended = Some(f.closed_order);
    let at = a + Duration::seconds(1);
    let trusted = f
        .event(f.spot_slice, "SLICE_WENT_QUIET", ended, at, Some(at))
        .await;
    let mut hidden = Vec::new();
    for (source, created, trust) in [
        (ended, at + Duration::seconds(1), None),
        (None, at + Duration::seconds(2), None),
        (
            ended,
            at + Duration::seconds(3),
            Some(a - Duration::microseconds(1)),
        ),
    ] {
        hidden.push(
            f.event(f.spot_slice, "SLICE_WENT_QUIET", source, created, trust)
                .await,
        );
    }
    let scope = f.scope().await;
    let before = f.rows().await;

    assert!(f.feed(&scope, unrecorded(), 20).await.is_empty());
    let summary = f.summary(&scope, unrecorded()).await;
    f.assert_counts(&summary, &[]);
    for id in hidden.iter().chain([&trusted]) {
        assert_eq!(
            f.acknowledge(&scope, unrecorded(), *id).await.err(),
            Some(404)
        );
    }
    assert_eq!(f.rows().await, before, "refusals write nothing");

    let feed = f.feed(&scope, recorded(), 20).await;
    assert_eq!(ids(&feed), vec![trusted]);
    let summary = f.summary(&scope, recorded()).await;
    f.assert_counts(&summary, &[(f.spot_slice, false, at)]);
    for id in &hidden {
        assert_eq!(
            f.acknowledge(&scope, recorded(), *id).await.err(),
            Some(404)
        );
    }
    assert_eq!(f.rows().await, before, "refusals write nothing");
    let acknowledged = f
        .acknowledge(&scope, recorded(), trusted)
        .await
        .expect("the trusted row can be acknowledged")
        .expect("its target exists");
    assert!(acknowledged.is_read);
}

// -------------------------------------------------------------- served routes

fn token(actor: Uuid) -> String {
    let now = Utc::now().timestamp();
    let claims =
        json!({"sub":actor,"type":"access","iat":now,"exp":now+600,"iss":ISSUER,"aud":ISSUER});
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

/// The served watchlist routes with the activation the release recorded.
async fn served(f: &Fixture, activation: Option<DateTime<Utc>>) -> Router {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&f.runtime_url)
        .await
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&f.runtime_url)
        .unwrap();
    let state = AppState::new(
        read_config(f.runtime_url.clone()),
        verdaxis_read::runtime::db::Pools { read: pool, health },
        f.policy.clone(),
    )
    .unwrap();
    let settings = WatchlistSettings::new(
        verdaxis_read::auth::VerificationSettings::new(
            SECRET.into(),
            None,
            ISSUER.into(),
            ISSUER.into(),
        ),
        activation,
    );
    router(state, settings)
}

async fn call(router: &Router, method: &str, path: &str, actor: Uuid) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {}", token(actor)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Pausing the trusted producer (its switch back to the legacy value, the
/// recorded activation unchanged) stops consumption only. The served routes
/// take the activation from the release settings, never the switch, so the
/// stored trusted rows stay listed, counted and acknowledgeable, the legacy
/// best-price row stays hidden, and no read writes an event or moves the
/// producer watermark. Without a recorded activation the same routes hide the
/// trusted best-price row.
#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL"]
async fn producer_rollback_to_legacy_keeps_trusted_rows_readable() {
    let mut f = Fixture::new().await;
    sqlx::query("SELECT producer_visibility_pause()")
        .execute(&mut f.admin)
        .await
        .unwrap();
    let at = activation() + Duration::seconds(1);
    let trusted = f.event(f.spot_slice, BEST_PRICE, None, at, Some(at)).await;
    let legacy = f
        .event(
            f.spot_slice,
            BEST_PRICE,
            None,
            at + Duration::seconds(1),
            None,
        )
        .await;
    let before = f.rows().await;
    let feed_path = format!("/api/watchlists/{}/events", f.container);
    let detail_path = format!("/api/watchlists/{}", f.container);

    let unrecorded_routes = served(&f, None).await;
    let (status, body) = call(&unrecorded_routes, "GET", &feed_path, f.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"], json!([]));

    let routes = served(&f, Some(activation())).await;
    let (status, body) = call(&routes, "GET", &feed_path, f.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["id"], json!(trusted));
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let (status, body) = call(&routes, "GET", &detail_path, f.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unread_event_count"], 1);
    assert_eq!(f.rows().await, before, "reads write nothing");

    let (status, _) = call(&routes, "PATCH", &format!("{feed_path}/{legacy}"), f.owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(f.rows().await, before, "a refusal writes nothing");
    let (status, body) = call(&routes, "PATCH", &format!("{feed_path}/{trusted}"), f.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_read"], true);
    let (_, body) = call(&routes, "GET", &detail_path, f.owner).await;
    assert_eq!(body["unread_event_count"], 0);

    let after = f.rows().await;
    assert_eq!(
        after["producer"], before["producer"],
        "the watermark is untouched"
    );
    assert_eq!(
        after["events"][legacy.to_string()],
        before["events"][legacy.to_string()]
    );
    assert_eq!(after["events"][trusted.to_string()][0], json!(true));
    assert_eq!(
        after["events"].as_object().unwrap().len(),
        before["events"].as_object().unwrap().len(),
        "no event row was added"
    );
}
