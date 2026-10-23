//! Task 15: the watchlist event feed (W07) and read state (W08).
//!
//! The cursor grammar, query bounds, the PD4-G guard and the payload projection
//! run without a database. The feed order, guard-before-limit pagination,
//! committed read state and the missing-target path need disposable PostgreSQL;
//! they provision their own fixture schema and skip when it is absent, so an
//! offline `cargo test --workspace` stays green.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;
use verdaxis_read::crud::watchlists::{
    EventCursor, EventQuery, EventsPage, StoredEvent, TargetColumns, VisibleSnapshot, acknowledge,
    decode_cursor, encode_cursor, event_visible, feed_page, parse_event_query,
};

fn instant(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, 0).single().unwrap()
}

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn snapshot(order_ids: &[Uuid]) -> VisibleSnapshot {
    VisibleSnapshot::from_orders(
        order_ids
            .iter()
            .map(|id| (*id, "B30".to_owned(), uuid(900), "SPOT".to_owned()))
            .collect(),
    )
}

// --------------------------------------------------------------- cursor grammar

#[test]
fn cursors_round_trip_and_accept_python_spellings() {
    let cursor = EventCursor::new(instant(1_792_407_780), uuid(7));
    let encoded = encode_cursor(cursor);
    assert_eq!(
        encoded,
        "2026-10-19T11:03:00.000000+00:00|00000000-0000-0000-0000-000000000007"
    );
    assert_eq!(decode_cursor(&encoded).unwrap(), cursor);

    // `datetime.fromisoformat` also accepts `Z`, a numeric offset, a space
    // separator and a naive timestamp. A Python-issued cursor uses the first
    // and every deployed host runs its session in UTC for the rest.
    for text in [
        "2026-10-19T11:03:00Z",
        "2026-10-19T19:03:00+08:00",
        "2026-10-19 11:03:00",
        "2026-10-19T11:03:00",
    ] {
        let decoded = decode_cursor(&format!("{text}|00000000-0000-0000-0000-000000000007"))
            .unwrap_or_else(|_| panic!("{text} must parse"));
        assert_eq!(decoded, cursor, "{text}");
    }

    // Fractional seconds survive the round trip at microsecond precision.
    let fractional =
        decode_cursor("2026-10-19T11:03:00.250000+00:00|00000000-0000-0000-0000-000000000007")
            .unwrap();
    assert_eq!(
        fractional.created_at,
        instant(1_792_407_780) + chrono::Duration::milliseconds(250)
    );
    assert_eq!(
        decode_cursor(&encode_cursor(fractional)).unwrap(),
        fractional
    );
}

#[test]
fn malformed_cursors_are_refused_with_the_route_error() {
    for cursor in [
        "not-a-cursor",
        "2026-10-19T00:00:00+00:00|not-a-uuid",
        "2026-10-19T00:00:00+00:00",
        "2026-10-19T00:00:00+00:00|00000000-0000-4000-8000-000000000001|extra",
    ] {
        let error = decode_cursor(cursor).unwrap_err();
        assert_eq!(error.status.as_u16(), 422, "{cursor}");
        assert_eq!(
            error.detail,
            Value::String("Invalid watchlist event cursor".into()),
            "{cursor}"
        );
    }
}

#[test]
fn query_defaults_and_bounds_are_the_route_contract() {
    assert_eq!(
        parse_event_query("").unwrap(),
        EventQuery {
            limit: 20,
            cursor: None
        }
    );
    assert_eq!(parse_event_query("cursor=").unwrap().cursor, None);
    assert_eq!(parse_event_query("limit=100").unwrap().limit, 100);

    for (raw, kind) in [
        ("limit=0", "greater_than_equal"),
        ("limit=101", "less_than_equal"),
        ("limit=abc", "int_parsing"),
    ] {
        let error = parse_event_query(raw).unwrap_err();
        assert_eq!(error.status.as_u16(), 422, "{raw}");
        assert_eq!(error.detail[0]["type"], kind, "{raw}");
    }
}

// ---------------------------------------------------------------- PD4-G guard

#[test]
fn the_guard_hides_best_price_and_orders_outside_the_public_scope() {
    let public = uuid(1);
    let hidden = uuid(2);
    let visible = snapshot(&[public]);
    let slice = target("SLICE", None, None);
    let pinned = |order: Option<Uuid>| target("PIN", order, None);

    assert!(!event_visible(
        "SLICE_BEST_PRICE_MOVED",
        Some(public),
        Some(&slice),
        &visible
    ));
    assert!(!event_visible(
        "SLICE_BEST_PRICE_MOVED",
        None,
        Some(&slice),
        &visible
    ));
    assert!(!event_visible(
        "SLICE_NEW_ORDER",
        Some(hidden),
        Some(&slice),
        &visible
    ));
    assert!(!event_visible(
        "PIN_PRICE_CHANGED",
        None,
        Some(&pinned(Some(hidden))),
        &visible
    ));
    assert!(!event_visible(
        "PIN_PRICE_CHANGED",
        None,
        Some(&pinned(None)),
        &visible
    ));

    // A deleted target has no PIN context; the event type and source order
    // still decide, and W08 reports the absent target separately.
    assert!(event_visible("SLICE_NEW_ORDER", None, None, &visible));
    assert!(!event_visible(
        "SLICE_BEST_PRICE_MOVED",
        None,
        None,
        &visible
    ));

    assert!(event_visible(
        "SLICE_NEW_ORDER",
        Some(public),
        Some(&slice),
        &visible
    ));
    assert!(event_visible(
        "SLICE_BENCHMARK_MOVED",
        None,
        Some(&slice),
        &visible
    ));
    assert!(event_visible(
        "PIN_PRICE_CHANGED",
        Some(public),
        Some(&pinned(Some(public))),
        &visible
    ));
}

// ----------------------------------------------------------- payload projection

fn target(kind: &str, order: Option<Uuid>, live_point_name: Option<&str>) -> TargetColumns {
    TargetColumns {
        target_type: kind.into(),
        product: Some("B30".into()),
        point: Some(uuid(900)),
        order,
        window: Some("SPOT".into()),
        snapshot_point_name: Some("Snapshot Point".into()),
        live_point_name: live_point_name.map(str::to_owned),
    }
}

fn record(payload: Value, target: TargetColumns) -> StoredEvent {
    StoredEvent {
        id: uuid(60),
        watchlist_id: uuid(61),
        target_id: uuid(62),
        event_type: "SLICE_NEW_ORDER".into(),
        payload: payload.as_object().cloned().unwrap_or_else(Map::new),
        is_read: false,
        created_at: instant(1_792_407_780),
        source_order: None,
        target: Some(target),
    }
}

#[test]
fn payload_fields_fall_back_independently() {
    let mut event = record(
        json!({
            "market_product_code": "VLSFO",
            "delivery_point_id": "65000000-0000-0000-0000-000000000005",
            "availability_window_code": "2026-Q1",
            "order_id": "65000000-0000-0000-0000-000000000006",
        }),
        target("SLICE", None, None),
    );
    // The feed query reads the source order; a canonical payload value is it.
    event.source_order = Some(uuid(0x6500_0000_0000_0000_0000_0000_0000_0006));
    let response = event.project().unwrap();
    assert_eq!(response.market_product_code.as_deref(), Some("VLSFO"));
    assert_eq!(
        response.delivery_point_id,
        Some(uuid(0x6500_0000_0000_0000_0000_0000_0000_0005))
    );
    assert_eq!(
        response.availability_window_code.as_deref(),
        Some("2026-Q1")
    );
    assert_eq!(
        response.order_id,
        Some(uuid(0x6500_0000_0000_0000_0000_0000_0000_0006))
    );

    // Invalid payload values never fail the page; each field falls back alone.
    let event = record(
        json!({
            "market_product_code": "",
            "delivery_point_id": "not-a-uuid",
            "availability_window_code": 7,
            "order_id": 7,
        }),
        target("PIN", Some(uuid(3)), None),
    );
    let response = event.project().unwrap();
    assert_eq!(response.market_product_code.as_deref(), Some("B30"));
    assert_eq!(response.delivery_point_id, Some(uuid(900)));
    assert_eq!(response.availability_window_code.as_deref(), Some("SPOT"));
    assert_eq!(response.order_id, Some(uuid(3)));
}

#[test]
fn the_response_order_is_the_source_order_the_guard_checked() {
    // The guard reads a payload order only through the database UUID rule. A
    // spelling that rule does not read is no source order, so the response
    // must not name the order either; it falls back to the target's order.
    let spelled = format!(
        "urn:uuid:{}",
        uuid(0x6400_0000_0000_0000_0000_0000_0000_0002)
    );
    let response = record(json!({"order_id": spelled}), target("SLICE", None, None))
        .project()
        .unwrap();
    assert_eq!(response.order_id, None);
    let response = record(
        json!({"order_id": spelled}),
        target("PIN", Some(uuid(3)), None),
    )
    .project()
    .unwrap();
    assert_eq!(response.order_id, Some(uuid(3)));
}

#[test]
fn unknown_payload_enums_use_schema_defaults() {
    let event = record(
        json!({"source_kind": "NOT_A_KIND", "demo_status": "DEMO", "scope": 7}),
        target("SLICE", None, None),
    );
    let response = event.project().unwrap();
    assert_eq!(response.source_kind, "UNKNOWN");
    assert_eq!(response.demo_status, "UNKNOWN");
    assert_eq!(response.scope, "DELIVERY_POINT");

    let event = record(
        json!({"source_kind": "LIVE_ORDER", "demo_status": "REAL_ONLY", "scope": "REGION"}),
        target("PIN", Some(uuid(3)), None),
    );
    let response = event.project().unwrap();
    assert_eq!(response.source_kind, "LIVE_ORDER");
    assert_eq!(response.demo_status, "REAL_ONLY");
    assert_eq!(response.scope, "REGION");

    // With no delivery point the default scope is UNKNOWN.
    let mut no_point = target("SLICE", None, None);
    no_point.point = None;
    assert_eq!(
        record(json!({}), no_point).project().unwrap().scope,
        "UNKNOWN"
    );
}

#[test]
fn observed_time_accepts_iso_and_epoch_values() {
    let at = instant(1_792_407_780);
    for payload in [
        json!({"observed_at": "2026-10-19T11:03:00Z"}),
        json!({"observed_at": "2026-10-19T11:03:00+00:00"}),
        json!({"observed_at": 1_792_407_780}),
    ] {
        assert_eq!(
            record(payload, target("SLICE", None, None))
                .project()
                .unwrap()
                .observed_at,
            Some(at)
        );
    }
    for payload in [
        json!({}),
        json!({"observed_at": ""}),
        json!({"observed_at": "yesterday"}),
    ] {
        assert_eq!(
            record(payload, target("SLICE", None, None))
                .project()
                .unwrap()
                .observed_at,
            None
        );
    }
}

#[test]
fn the_point_name_prefers_payload_then_live_then_snapshot() {
    let payload_name = record(
        json!({"delivery_point_name": "Payload Point"}),
        target("SLICE", None, Some("Live Point")),
    );
    assert_eq!(
        payload_name
            .project()
            .unwrap()
            .delivery_point_name
            .as_deref(),
        Some("Payload Point")
    );

    let live_name = record(json!({}), target("SLICE", None, Some("Live Point")));
    assert_eq!(
        live_name.project().unwrap().delivery_point_name.as_deref(),
        Some("Live Point")
    );

    let snapshot_name = record(json!({}), target("SLICE", None, None));
    assert_eq!(
        snapshot_name
            .project()
            .unwrap()
            .delivery_point_name
            .as_deref(),
        Some("Snapshot Point")
    );
}

// ------------------------------------------------------------ client contract

const CONTRACT: &str = include_str!("../../../tests/rust_contract/phase2/watchlist-events.json");

/// The exact field set `src/types.ts` declares. A field renamed or dropped here
/// breaks the frontend feed silently, so the fixture pins every key and its
/// nullability.
#[test]
fn the_serialized_event_has_exactly_the_frontend_fields() {
    let contract: Value = serde_json::from_str(CONTRACT).unwrap();
    let expected = contract["event_fields"].as_object().unwrap();
    let value = serde_json::to_value(
        record(json!({}), target("SLICE", None, None))
            .project()
            .unwrap(),
    )
    .unwrap();
    let actual = value.as_object().unwrap();

    let mut wanted: Vec<&str> = expected.keys().map(String::as_str).collect();
    let mut found: Vec<&str> = actual.keys().map(String::as_str).collect();
    wanted.sort_unstable();
    found.sort_unstable();
    assert_eq!(
        found, wanted,
        "the event field set differs from the frontend contract"
    );

    for (field, kind) in expected {
        let present = actual.get(field).unwrap();
        let matches = match kind.as_str().unwrap() {
            "uuid" => present.is_string(),
            "string" => present.is_string(),
            "boolean" => present.is_boolean(),
            "object" => present.is_object(),
            "array" => present.is_array(),
            "SLICE|PIN" => matches!(present.as_str(), Some("SLICE" | "PIN")),
            "instant" => present.is_string(),
            "instant|null" => present.is_null() || present.is_string(),
            "uuid|null" => present.is_null() || present.is_string(),
            "string|null" => present.is_null() || present.is_string(),
            other => panic!("unknown contract kind {other}"),
        };
        assert!(matches, "{field} does not match {kind:?}");
    }
}

#[test]
fn the_serialized_page_and_bounds_match_the_contract() {
    let contract: Value = serde_json::from_str(CONTRACT).unwrap();
    let page = EventsPage {
        items: vec![
            record(json!({}), target("SLICE", None, None))
                .project()
                .unwrap(),
        ],
        next_cursor: None,
    };
    let value = serde_json::to_value(&page).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["items", "next_cursor"]);
    assert!(value["next_cursor"].is_null());

    assert_eq!(
        parse_event_query("").unwrap(),
        EventQuery {
            limit: contract["page"]["default_limit"].as_i64().unwrap(),
            cursor: None
        }
    );
    // The declared bounds are the ones the query validates.
    assert_eq!(
        parse_event_query("limit=0").unwrap_err().detail[0]["ctx"]["ge"],
        contract["page"]["minimum_limit"].as_i64().unwrap()
    );
    assert_eq!(
        parse_event_query("limit=101").unwrap_err().detail[0]["ctx"]["le"],
        contract["page"]["maximum_limit"].as_i64().unwrap()
    );
}

// ------------------------------------------------------------ mounted routes

#[path = "support/watchlist_app.rs"]
mod watchlist_app;

mod mounted {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::watchlist_app;

    const CONTAINER: &str = "70000000-0000-4000-8000-000000000011";

    async fn status(method: Method, path: &str) -> StatusCode {
        watchlist_app::app()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    /// Both routes verify the bearer before any path, query or database work.
    #[tokio::test]
    async fn the_feed_and_read_state_require_a_bearer_first() {
        for path in [
            format!("/api/watchlists/{CONTAINER}/events"),
            format!("/api/watchlists/{CONTAINER}/events?cursor=not-a-cursor&limit=0"),
            "/api/watchlists/not-a-uuid/events".to_owned(),
        ] {
            assert_eq!(
                status(Method::GET, &path).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        for path in [
            format!("/api/watchlists/{CONTAINER}/events/70000000-0000-4000-8000-000000000012"),
            "/api/watchlists/not-a-uuid/events/also-not-a-uuid".to_owned(),
        ] {
            assert_eq!(
                status(Method::PATCH, &path).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
    }
}

// ------------------------------------------------------------- PostgreSQL proof

mod postgres {
    use super::*;
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;

    const OWNER: u128 = 0x7000_0000_0000_0000_0000_0000_0000_0001;
    const CONTAINER: u128 = 0x7000_0000_0000_0000_0000_0000_0000_0011;

    const SLICE: u128 = 0x7200_0000_0000_0000_0000_0000_0000_0001;
    const PIN_PUBLIC: u128 = 0x7200_0000_0000_0000_0000_0000_0000_0002;
    const PIN_HIDDEN: u128 = 0x7200_0000_0000_0000_0000_0000_0000_0003;
    const SLICE_TWO: u128 = 0x7200_0000_0000_0000_0000_0000_0000_0004;

    const POINT: u128 = 0x6500_0000_0000_0000_0000_0000_0000_0001;
    const PUBLIC_ORDER: u128 = 0x6400_0000_0000_0000_0000_0000_0000_0001;
    const HIDDEN_ORDER: u128 = 0x6400_0000_0000_0000_0000_0000_0000_0002;

    async fn harness() -> Option<sqlx::PgPool> {
        let migrator_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL").ok()?;
        std::env::var("RUST_TEST_DATABASE_URL").ok()?;
        let migrator = PgPoolOptions::new()
            .max_connections(2)
            .connect(&migrator_url)
            .await
            .expect("connect the fixture migrator");
        provision(&migrator).await;
        Some(migrator)
    }

    async fn provision(migrator: &PgPool) {
        for statement in [
            "CREATE TABLE IF NOT EXISTS delivery_points (id uuid PRIMARY KEY, name text NOT NULL)",
            "CREATE TABLE IF NOT EXISTS watchlists (\
                 id uuid PRIMARY KEY, user_id uuid NOT NULL, name text NOT NULL, \
                 kind text NOT NULL DEFAULT 'CUSTOM', created_at timestamptz NOT NULL)",
            "CREATE TABLE IF NOT EXISTS watchlist_targets (\
                 id uuid PRIMARY KEY, watchlist_id uuid NOT NULL, target_type text NOT NULL, \
                 market_product_code text, delivery_point_id uuid, availability_window_code text, \
                 order_id uuid, snapshot_delivery_point_name text, created_at timestamptz NOT NULL)",
            "CREATE TABLE IF NOT EXISTS watchlist_events (\
                 id uuid PRIMARY KEY, watchlist_id uuid NOT NULL, watchlist_target_id uuid NOT NULL, \
                 event_type text NOT NULL, event_payload json, is_read boolean NOT NULL DEFAULT false, \
                 created_at timestamptz NOT NULL)",
            "GRANT SELECT, INSERT, UPDATE, DELETE ON delivery_points, watchlists, \
                 watchlist_targets, watchlist_events TO rust_app",
            "TRUNCATE watchlist_events, watchlist_targets, watchlists, delivery_points",
        ] {
            sqlx::query(statement)
                .execute(migrator)
                .await
                .unwrap_or_else(|error| panic!("provision failed: {error}\n{statement}"));
        }
        seed(migrator).await;
    }

    async fn seed(migrator: &PgPool) {
        sqlx::query("INSERT INTO delivery_points (id,name) VALUES ($1,'Dalian')")
            .bind(uuid(POINT))
            .execute(migrator)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO watchlists (id,user_id,name,kind,created_at) \
             VALUES ($1,$2,'Event feed','CUSTOM',$3)",
        )
        .bind(uuid(CONTAINER))
        .bind(uuid(OWNER))
        .bind(instant(1_792_370_000))
        .execute(migrator)
        .await
        .unwrap();
        for (id, kind, order, point, snapshot, created) in [
            (SLICE, "SLICE", None, Some(POINT), None, 1_792_370_100i64),
            (
                PIN_PUBLIC,
                "PIN",
                Some(PUBLIC_ORDER),
                Some(POINT),
                None,
                1_792_370_200,
            ),
            (
                PIN_HIDDEN,
                "PIN",
                Some(HIDDEN_ORDER),
                Some(POINT),
                None,
                1_792_370_300,
            ),
            (
                SLICE_TWO,
                "SLICE",
                None,
                None,
                Some("Snapshot Point"),
                1_792_370_400,
            ),
        ] {
            sqlx::query(
                "INSERT INTO watchlist_targets \
                 (id,watchlist_id,target_type,market_product_code,delivery_point_id,\
                  availability_window_code,order_id,snapshot_delivery_point_name,created_at) \
                 VALUES ($1,$2,$3,'B30',$4,'SPOT',$5,$6,$7)",
            )
            .bind(uuid(id))
            .bind(uuid(CONTAINER))
            .bind(kind)
            .bind(point.map(uuid))
            .bind(order.map(uuid))
            .bind(snapshot)
            .bind(instant(created))
            .execute(migrator)
            .await
            .unwrap();
        }
    }

    async fn insert_event(
        migrator: &PgPool,
        id: u128,
        target: u128,
        event_type: &str,
        payload: Value,
        created: DateTime<Utc>,
        is_read: bool,
    ) {
        sqlx::query(
            "INSERT INTO watchlist_events \
             (id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(uuid(id))
        .bind(uuid(CONTAINER))
        .bind(uuid(target))
        .bind(event_type)
        .bind(payload)
        .bind(is_read)
        .bind(created)
        .execute(migrator)
        .await
        .unwrap();
    }

    async fn is_read(migrator: &PgPool, id: u128) -> bool {
        sqlx::query_scalar("SELECT is_read FROM watchlist_events WHERE id=$1")
            .bind(uuid(id))
            .fetch_one(migrator)
            .await
            .unwrap()
    }

    async fn xmin(migrator: &PgPool, id: u128) -> String {
        sqlx::query_scalar("SELECT xmin::text FROM watchlist_events WHERE id=$1")
            .bind(uuid(id))
            .fetch_one(migrator)
            .await
            .unwrap()
    }

    fn visible() -> VisibleSnapshot {
        snapshot(&[uuid(PUBLIC_ORDER)])
    }

    fn query(limit: i64, cursor: Option<EventCursor>) -> EventQuery {
        EventQuery { limit, cursor }
    }

    #[tokio::test]
    async fn tied_timestamps_order_by_id_descending_and_pages_without_repeats() {
        let Some(migrator) = harness().await else {
            return;
        };
        let tied = instant(1_792_378_000);
        for (id, seconds) in [
            (0x7300_0000_0000_0000_0000_0000_0000_0001u128, 0),
            (0x7300_0000_0000_0000_0000_0000_0000_0002u128, 0),
            (0x7300_0000_0000_0000_0000_0000_0000_0003u128, 0),
            (0x7300_0000_0000_0000_0000_0000_0000_0004u128, -10),
            (0x7300_0000_0000_0000_0000_0000_0000_0005u128, -20),
        ] {
            insert_event(
                &migrator,
                id,
                SLICE,
                "SLICE_NEW_ORDER",
                json!({}),
                tied + chrono::Duration::seconds(seconds),
                false,
            )
            .await;
        }
        let mut connection = migrator.acquire().await.unwrap();

        let first = feed_page(&mut connection, uuid(CONTAINER), &visible(), query(2, None))
            .await
            .unwrap();
        let ids: Vec<Uuid> = first.items.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                uuid(0x7300_0000_0000_0000_0000_0000_0000_0003),
                uuid(0x7300_0000_0000_0000_0000_0000_0000_0002)
            ]
        );
        let cursor = decode_cursor(first.next_cursor.as_deref().unwrap()).unwrap();

        let second = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(2, Some(cursor)),
        )
        .await
        .unwrap();
        let ids: Vec<Uuid> = second.items.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                uuid(0x7300_0000_0000_0000_0000_0000_0000_0001),
                uuid(0x7300_0000_0000_0000_0000_0000_0000_0004)
            ]
        );
        let cursor = decode_cursor(second.next_cursor.as_deref().unwrap()).unwrap();

        let third = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(2, Some(cursor)),
        )
        .await
        .unwrap();
        let ids: Vec<Uuid> = third.items.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![uuid(0x7300_0000_0000_0000_0000_0000_0000_0005)]);
        assert_eq!(third.next_cursor, None);
    }

    #[tokio::test]
    async fn the_guard_filters_before_the_limit_so_pages_stay_full() {
        let Some(migrator) = harness().await else {
            return;
        };
        let at = instant(1_792_378_000);
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0001,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            at + chrono::Duration::seconds(6),
            false,
        )
        .await;
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0002,
            SLICE,
            "SLICE_BEST_PRICE_MOVED",
            json!({"order_id": uuid(PUBLIC_ORDER).to_string()}),
            at + chrono::Duration::seconds(5),
            false,
        )
        .await;
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0003,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({"order_id": uuid(HIDDEN_ORDER).to_string()}),
            at + chrono::Duration::seconds(4),
            false,
        )
        .await;
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0004,
            PIN_HIDDEN,
            "PIN_PRICE_CHANGED",
            json!({}),
            at + chrono::Duration::seconds(3),
            false,
        )
        .await;
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0005,
            PIN_PUBLIC,
            "PIN_PRICE_CHANGED",
            json!({}),
            at + chrono::Duration::seconds(2),
            false,
        )
        .await;
        insert_event(
            &migrator,
            0x7400_0000_0000_0000_0000_0000_0000_0006,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            at - chrono::Duration::seconds(1),
            false,
        )
        .await;
        let mut connection = migrator.acquire().await.unwrap();

        // Three visible rows, three hidden rows above the oldest visible one.
        // The limit counts visible rows only, so the third is still pending.
        let page = feed_page(&mut connection, uuid(CONTAINER), &visible(), query(2, None))
            .await
            .unwrap();
        let ids: Vec<Uuid> = page.items.iter().map(|item| item.id).collect();
        assert_eq!(
            ids,
            vec![
                uuid(0x7400_0000_0000_0000_0000_0000_0000_0001),
                uuid(0x7400_0000_0000_0000_0000_0000_0000_0005)
            ]
        );
        let cursor = decode_cursor(
            page.next_cursor
                .as_deref()
                .expect("a third visible row remains"),
        )
        .unwrap();

        let page = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(2, Some(cursor)),
        )
        .await
        .unwrap();
        let ids: Vec<Uuid> = page.items.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![uuid(0x7400_0000_0000_0000_0000_0000_0000_0006)]);
        assert_eq!(page.next_cursor, None);
    }

    #[tokio::test]
    async fn an_exactly_full_page_has_no_next_cursor() {
        let Some(migrator) = harness().await else {
            return;
        };
        let at = instant(1_792_378_000);
        for index in 0..2u128 {
            insert_event(
                &migrator,
                0x7500_0000_0000_0000_0000_0000_0000_0000 + index + 1,
                SLICE,
                "SLICE_NEW_ORDER",
                json!({}),
                at + chrono::Duration::seconds(index as i64),
                false,
            )
            .await;
        }
        let mut connection = migrator.acquire().await.unwrap();
        let page = feed_page(&mut connection, uuid(CONTAINER), &visible(), query(2, None))
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.next_cursor, None);
    }

    #[tokio::test]
    async fn acknowledging_an_event_commits_once_and_a_repeat_writes_nothing() {
        let Some(migrator) = harness().await else {
            return;
        };
        insert_event(
            &migrator,
            0x7600_0000_0000_0000_0000_0000_0000_0001,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_378_000),
            false,
        )
        .await;

        let mut transaction = migrator.begin().await.unwrap();
        let ack = acknowledge(
            &mut transaction,
            uuid(CONTAINER),
            uuid(0x7600_0000_0000_0000_0000_0000_0000_0001),
            &visible(),
        )
        .await
        .unwrap();
        assert!(matches!(ack, Some(ref event) if event.is_read));
        transaction.commit().await.unwrap();
        assert!(is_read(&migrator, 0x7600_0000_0000_0000_0000_0000_0000_0001).await);
        let before = xmin(&migrator, 0x7600_0000_0000_0000_0000_0000_0000_0001).await;

        let mut transaction = migrator.begin().await.unwrap();
        let ack = acknowledge(
            &mut transaction,
            uuid(CONTAINER),
            uuid(0x7600_0000_0000_0000_0000_0000_0000_0001),
            &visible(),
        )
        .await
        .unwrap();
        assert!(matches!(ack, Some(ref event) if event.is_read));
        transaction.commit().await.unwrap();
        assert_eq!(
            before,
            xmin(&migrator, 0x7600_0000_0000_0000_0000_0000_0000_0001).await
        );
    }

    #[tokio::test]
    async fn a_guarded_event_is_refused_without_a_write() {
        let Some(migrator) = harness().await else {
            return;
        };
        for (id, target_id, event_type, payload) in [
            (
                0x7700_0000_0000_0000_0000_0000_0000_0001u128,
                SLICE,
                "SLICE_BEST_PRICE_MOVED",
                json!({}),
            ),
            (
                0x7700_0000_0000_0000_0000_0000_0000_0002,
                SLICE,
                "SLICE_NEW_ORDER",
                json!({"order_id": uuid(HIDDEN_ORDER).to_string()}),
            ),
            (
                0x7700_0000_0000_0000_0000_0000_0000_0003,
                PIN_HIDDEN,
                "PIN_PRICE_CHANGED",
                json!({}),
            ),
        ] {
            insert_event(
                &migrator,
                id,
                target_id,
                event_type,
                payload,
                instant(1_792_378_000),
                false,
            )
            .await;
        }
        for id in [
            0x7700_0000_0000_0000_0000_0000_0000_0001u128,
            0x7700_0000_0000_0000_0000_0000_0000_0002,
            0x7700_0000_0000_0000_0000_0000_0000_0003,
        ] {
            let before = xmin(&migrator, id).await;
            let mut transaction = migrator.begin().await.unwrap();
            let error = acknowledge(&mut transaction, uuid(CONTAINER), uuid(id), &visible())
                .await
                .unwrap_err();
            transaction.rollback().await.unwrap();
            assert_eq!(error.status.as_u16(), 404);
            assert_eq!(error.detail, Value::String("Event not found".into()));
            assert!(!is_read(&migrator, id).await);
            assert_eq!(before, xmin(&migrator, id).await);
        }
    }

    /// Two readers acknowledge the same unread event at once. The row lock
    /// serializes them; the loser's conditional update matches no row, so the
    /// event is written exactly once and both readers succeed.
    #[tokio::test]
    async fn concurrent_acknowledgements_write_the_event_once() {
        let Some(migrator) = harness().await else {
            return;
        };
        install_ack_counter(&migrator).await;
        let id = 0x7c00_0000_0000_0000_0000_0000_0000_0001u128;
        insert_event(
            &migrator,
            id,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_378_000),
            false,
        )
        .await;

        let mut first = migrator.begin().await.unwrap();
        let ack = acknowledge(&mut first, uuid(CONTAINER), uuid(id), &visible())
            .await
            .unwrap();
        assert!(ack.is_some_and(|event| event.is_read));

        // The second reader starts while the first still holds the row lock.
        let pool = migrator.clone();
        let second = tokio::spawn(async move {
            let mut transaction = pool.begin().await.unwrap();
            let ack = acknowledge(&mut transaction, uuid(CONTAINER), uuid(id), &visible())
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            ack.is_some_and(|event| event.is_read)
        });

        first.commit().await.unwrap();
        assert!(
            second.await.unwrap(),
            "the blocked reader must still succeed"
        );

        assert!(is_read(&migrator, id).await);
        assert_eq!(
            ack_writes(&migrator).await,
            1,
            "the loser must not write the row"
        );
    }

    async fn install_ack_counter(migrator: &PgPool) {
        for statement in [
            "CREATE TABLE IF NOT EXISTS watchlist_event_ack_writes (writes bigint NOT NULL)",
            "CREATE OR REPLACE FUNCTION watchlist_event_ack_count() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN UPDATE watchlist_event_ack_writes SET writes = writes + 1; RETURN NEW; END $$",
            "DROP TRIGGER IF EXISTS watchlist_event_ack_count ON watchlist_events",
            "CREATE TRIGGER watchlist_event_ack_count AFTER UPDATE ON watchlist_events \
             FOR EACH ROW EXECUTE FUNCTION watchlist_event_ack_count()",
            "DELETE FROM watchlist_event_ack_writes",
            "INSERT INTO watchlist_event_ack_writes VALUES (0)",
        ] {
            sqlx::query(statement)
                .execute(migrator)
                .await
                .unwrap_or_else(|error| panic!("ack counter failed: {error}\n{statement}"));
        }
    }

    async fn ack_writes(migrator: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT writes FROM watchlist_event_ack_writes")
            .fetch_one(migrator)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_event_of_another_container_is_not_found() {
        let Some(migrator) = harness().await else {
            return;
        };
        insert_event(
            &migrator,
            0x7800_0000_0000_0000_0000_0000_0000_0001,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_378_000),
            false,
        )
        .await;
        let error = acknowledge(
            &mut migrator.acquire().await.unwrap(),
            uuid(CONTAINER + 1),
            uuid(0x7800_0000_0000_0000_0000_0000_0000_0001),
            &visible(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status.as_u16(), 404);
        assert!(!is_read(&migrator, 0x7800_0000_0000_0000_0000_0000_0000_0001).await);
    }

    #[tokio::test]
    async fn an_event_whose_target_is_gone_commits_the_read_then_reports_missing() {
        let Some(migrator) = harness().await else {
            return;
        };
        insert_event(
            &migrator,
            0x7900_0000_0000_0000_0000_0000_0000_0001,
            SLICE_TWO,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_378_000),
            false,
        )
        .await;
        sqlx::query("DELETE FROM watchlist_targets WHERE id=$1")
            .bind(uuid(SLICE_TWO))
            .execute(&migrator)
            .await
            .unwrap();

        let mut transaction = migrator.begin().await.unwrap();
        let ack = acknowledge(
            &mut transaction,
            uuid(CONTAINER),
            uuid(0x7900_0000_0000_0000_0000_0000_0000_0001),
            &visible(),
        )
        .await
        .unwrap();
        assert!(
            ack.is_none(),
            "a missing target is reported after the committed read"
        );
        transaction.commit().await.unwrap();
        assert!(is_read(&migrator, 0x7900_0000_0000_0000_0000_0000_0000_0001).await);
    }

    #[tokio::test]
    async fn old_read_events_are_never_pruned_by_a_read() {
        let Some(migrator) = harness().await else {
            return;
        };
        insert_event(
            &migrator,
            0x7a00_0000_0000_0000_0000_0000_0000_0001,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_000_000),
            true,
        )
        .await;
        let mut connection = migrator.acquire().await.unwrap();
        let page = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(20, None),
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page.items[0].is_read);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM watchlist_events")
            .fetch_one(&migrator)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn the_point_name_falls_back_to_the_stored_snapshot() {
        let Some(migrator) = harness().await else {
            return;
        };
        insert_event(
            &migrator,
            0x7b00_0000_0000_0000_0000_0000_0000_0001,
            SLICE_TWO,
            "SLICE_NEW_ORDER",
            json!({}),
            instant(1_792_378_000),
            false,
        )
        .await;
        let mut connection = migrator.acquire().await.unwrap();
        let page = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(20, None),
        )
        .await
        .unwrap();
        assert_eq!(
            page.items[0].delivery_point_name.as_deref(),
            Some("Snapshot Point")
        );
        assert_eq!(page.items[0].delivery_point_id, None);
    }

    /// The feed, the read state and the container counts read a payload's
    /// order with one rule, so a non-canonical spelling cannot make them
    /// disagree or put a non-public order into a response.
    #[tokio::test]
    async fn feed_and_counts_read_one_payload_order_rule() {
        let Some(migrator) = harness().await else {
            return;
        };
        let at = instant(1_792_378_000);
        // A URN spelling that only one of two parsers would read.
        let urn = format!("urn:uuid:{}", uuid(HIDDEN_ORDER));
        // A hyphen after every group of four digits, which only the other
        // parser would read.
        let simple = uuid(HIDDEN_ORDER).simple().to_string();
        let grouped = simple
            .as_bytes()
            .chunks(4)
            .map(|group| std::str::from_utf8(group).unwrap())
            .collect::<Vec<_>>()
            .join("-");
        const URN_EVENT: u128 = 0x7c00_0000_0000_0000_0000_0000_0000_0001;
        const GROUPED_EVENT: u128 = 0x7c00_0000_0000_0000_0000_0000_0000_0002;
        insert_event(
            &migrator,
            URN_EVENT,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({"order_id": urn}),
            at + chrono::Duration::seconds(2),
            false,
        )
        .await;
        insert_event(
            &migrator,
            GROUPED_EVENT,
            SLICE,
            "SLICE_NEW_ORDER",
            json!({"order_id": grouped}),
            at + chrono::Duration::seconds(1),
            false,
        )
        .await;
        let mut connection = migrator.acquire().await.unwrap();

        let page = feed_page(
            &mut connection,
            uuid(CONTAINER),
            &visible(),
            query(20, None),
        )
        .await
        .unwrap();
        let ids: Vec<Uuid> = page.items.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![uuid(URN_EVENT)]);
        assert_eq!(page.items[0].order_id, None);

        let summary = visible()
            .detail(&mut connection, uuid(OWNER), uuid(CONTAINER), at)
            .await
            .unwrap()
            .expect("the owner's container");
        assert_eq!(summary.unread_event_count, ids.len() as i64);
        assert_eq!(
            summary.latest_event_at,
            Some(at + chrono::Duration::seconds(2))
        );
    }
}
