//! Scenario proofs for the durable replay floor read store.
//!
//! The pure tests cover barrier interpretation, page planning and participant
//! decoding. The `postgres_` tests run only when
//! `STREAMS_STORE_TEST_DATABASE_URL` names a disposable read-only database;
//! `tests/postgres/test_phase6_retention.py` seeds those fixtures and invokes
//! them so the page SQL and the deployed schema are proved together.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use verdaxis_read::streams::store::{
    EventMetadata, PageLimits, ReplayBoundary, StoreError, cursor_is_retained, decode_participants,
    floor_from_rows, plan_page, replay_boundary, replay_page,
};

const ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_00a1);
const FOREIGN_ORG: Uuid = Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_00b2);

fn metadata(seq: i64, payload_bytes: i64, participants: i64) -> EventMetadata {
    EventMetadata {
        seq,
        payload_bytes,
        participants,
    }
}

async fn test_pool() -> Option<PgPool> {
    let url = std::env::var("STREAMS_STORE_TEST_DATABASE_URL").ok()?;
    Some(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("disposable stream store database"),
    )
}

#[tokio::test]
async fn postgres_invalid_row_is_classified() {
    let Some(pool) = test_pool().await else {
        return;
    };
    assert_eq!(
        replay_page(&pool, ORG, 0, 10, PageLimits::DEFAULT)
            .await
            .unwrap_err(),
        StoreError::InvalidEvent(10)
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_pool_acquisition_is_bounded() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let first = pool.acquire().await.unwrap();
    let second = pool.acquire().await.unwrap();
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(3), replay_boundary(&pool)).await;
    drop((first, second));
    assert_eq!(
        result.expect("store acquisition must finish within its bound"),
        Err(StoreError::Unavailable)
    );
    pool.close().await;
}

// --- barrier interpretation -------------------------------------------------

#[test]
fn floor_requires_exactly_one_row() {
    assert_eq!(floor_from_rows(&[0]).unwrap(), 0);
    assert_eq!(floor_from_rows(&[42]).unwrap(), 42);
    assert_eq!(floor_from_rows(&[]).unwrap_err(), StoreError::Unavailable);
    assert_eq!(
        floor_from_rows(&[1, 2]).unwrap_err(),
        StoreError::Unavailable
    );
}

#[test]
fn negative_floor_row_is_unavailable() {
    assert_eq!(floor_from_rows(&[-1]).unwrap_err(), StoreError::Unavailable);
}

#[test]
fn cursor_equal_to_floor_is_retained() {
    let boundary = ReplayBoundary {
        discarded_through: 7,
        high_water: 20,
    };
    assert!(cursor_is_retained(7, boundary));
    assert!(cursor_is_retained(8, boundary));
}

#[test]
fn cursor_below_floor_is_not_retained() {
    let boundary = ReplayBoundary {
        discarded_through: 7,
        high_water: 20,
    };
    assert!(!cursor_is_retained(6, boundary));
    assert!(!cursor_is_retained(0, boundary));
}

#[test]
fn sequence_holes_do_not_change_retention() {
    // A missing global identifier is not evidence of a deleted retained row.
    let boundary = ReplayBoundary {
        discarded_through: 4,
        high_water: 30,
    };
    assert!(cursor_is_retained(4, boundary));
    assert!(cursor_is_retained(17, boundary));
}

// --- page planning ----------------------------------------------------------

#[test]
fn page_plan_stops_before_cumulative_payload_bound() {
    let limits = PageLimits {
        payload_bytes: 350,
        payload_bytes_per_event: 350,
        ..PageLimits::DEFAULT
    };
    let (seqs, more) = plan_page(&[metadata(10, 100, 2), metadata(20, 300, 2)], limits).unwrap();
    assert_eq!(seqs, vec![10]);
    assert!(more);
}

#[test]
fn page_plan_stops_at_row_limit_and_reports_more() {
    let limits = PageLimits {
        rows: 2,
        ..PageLimits::DEFAULT
    };
    let (seqs, more) = plan_page(
        &[metadata(1, 10, 1), metadata(2, 10, 1), metadata(3, 10, 1)],
        limits,
    )
    .unwrap();
    assert_eq!(seqs, vec![1, 2]);
    assert!(more);
}

#[test]
fn page_plan_rejects_oversized_or_unscoped_row() {
    let oversized = plan_page(&[metadata(9, 262_145, 1)], PageLimits::DEFAULT).unwrap_err();
    assert_eq!(oversized, StoreError::InvalidEvent(9));

    let too_many = plan_page(&[metadata(9, 10, 257)], PageLimits::DEFAULT).unwrap_err();
    assert_eq!(too_many, StoreError::InvalidEvent(9));

    let empty = plan_page(&[metadata(9, 10, 0)], PageLimits::DEFAULT).unwrap_err();
    assert_eq!(empty, StoreError::InvalidEvent(9));
}

#[test]
fn page_plan_keeps_an_empty_page_valid() {
    let (seqs, more) = plan_page(&[], PageLimits::DEFAULT).unwrap();
    assert!(seqs.is_empty());
    assert!(!more);
}

#[test]
fn page_limits_reject_unbounded_values() {
    for limits in [
        PageLimits {
            rows: 0,
            ..PageLimits::DEFAULT
        },
        PageLimits {
            rows: 501,
            ..PageLimits::DEFAULT
        },
        PageLimits {
            payload_bytes: 2 * 1024 * 1024 + 1,
            ..PageLimits::DEFAULT
        },
        PageLimits {
            payload_bytes_per_event: 262_145,
            ..PageLimits::DEFAULT
        },
        PageLimits {
            participants: 257,
            ..PageLimits::DEFAULT
        },
        // A single event must always fit a page, otherwise a page can be empty
        // while it still reports more rows.
        PageLimits {
            payload_bytes: 1024,
            payload_bytes_per_event: 2048,
            ..PageLimits::DEFAULT
        },
    ] {
        assert_eq!(
            limits.validate().unwrap_err(),
            StoreError::InvalidPageRequest
        );
    }
    assert!(PageLimits::DEFAULT.validate().is_ok());
}

// --- participant decoding ---------------------------------------------------

#[test]
fn participants_decode_uuid_strings() {
    let raw = format!(r#"["{ORG}","{FOREIGN_ORG}"]"#);
    let decoded = decode_participants(3, &raw, 256).unwrap();
    assert_eq!(decoded, vec![ORG, FOREIGN_ORG]);
}

#[test]
fn participants_reject_non_array_empty_and_overflow() {
    assert_eq!(
        decode_participants(3, r#"{"org":"x"}"#, 256).unwrap_err(),
        StoreError::InvalidEvent(3)
    );
    assert_eq!(
        decode_participants(3, "[]", 256).unwrap_err(),
        StoreError::InvalidEvent(3)
    );
    assert_eq!(
        decode_participants(3, r#"["not-a-uuid"]"#, 256).unwrap_err(),
        StoreError::InvalidEvent(3)
    );
    assert_eq!(
        decode_participants(3, &format!(r#"["{ORG}"]"#), 0).unwrap_err(),
        StoreError::InvalidEvent(3)
    );
}

// --- deployed read path -----------------------------------------------------

#[tokio::test]
async fn postgres_empty_retained_history_boundary() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let boundary = replay_boundary(&pool).await.expect("retained boundary");
    assert_eq!(boundary.discarded_through, 25);
    assert_eq!(boundary.high_water, 25);
    assert!(cursor_is_retained(25, boundary));
    let page = replay_page(&pool, ORG, 25, 25, PageLimits::DEFAULT)
        .await
        .expect("empty retained page");
    assert!(page.events.is_empty());
    pool.close().await;
}

#[tokio::test]
async fn postgres_empty_store_boundary() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let boundary = replay_boundary(&pool).await.expect("empty replays pass");
    assert_eq!(
        boundary,
        ReplayBoundary {
            discarded_through: 0,
            high_water: 0,
        }
    );
    let page = replay_page(&pool, ORG, 0, boundary.high_water, PageLimits::DEFAULT)
        .await
        .expect("empty page");
    assert!(page.events.is_empty());
    assert!(!page.more);
    assert_eq!(page.boundary, boundary);
    pool.close().await;
}

#[tokio::test]
async fn postgres_sequence_holes_and_scope() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let boundary = replay_boundary(&pool).await.expect("boundary");
    assert_eq!(
        boundary,
        ReplayBoundary {
            discarded_through: 0,
            high_water: 30,
        }
    );
    let page = replay_page(&pool, ORG, 0, boundary.high_water, PageLimits::DEFAULT)
        .await
        .expect("scoped page");
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![10, 30]
    );
    assert!(page.events[0].public_invalidated);
    assert!(!page.events[1].public_invalidated);
    assert_eq!(page.events[0].participants, vec![ORG]);
    assert_eq!(page.events[0].event_type, "trade_created");
    assert_eq!(page.events[0].payload["schema_version"], 1);
    assert!(
        page.events[0]
            .payload
            .get("_public_market_invalidation")
            .is_none()
    );
    assert!(!page.more);

    let tail = replay_page(&pool, ORG, 10, boundary.high_water, PageLimits::DEFAULT)
        .await
        .expect("tail page");
    assert_eq!(
        tail.events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![30]
    );

    let foreign = replay_page(
        &pool,
        FOREIGN_ORG,
        0,
        boundary.high_water,
        PageLimits::DEFAULT,
    )
    .await
    .expect("foreign page");
    assert_eq!(
        foreign
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![20]
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_missing_state_is_unavailable() {
    let Some(pool) = test_pool().await else {
        return;
    };
    assert_eq!(
        replay_boundary(&pool).await.unwrap_err(),
        StoreError::Unavailable
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_prefix_retained() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let boundary = replay_boundary(&pool).await.expect("boundary");
    assert_eq!(
        boundary,
        ReplayBoundary {
            discarded_through: 0,
            high_water: 25,
        }
    );
    let page = replay_page(&pool, ORG, 0, boundary.high_water, PageLimits::DEFAULT)
        .await
        .expect("page");
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![5, 15, 25]
    );
    assert!(cursor_is_retained(0, boundary));
    pool.close().await;
}

#[tokio::test]
async fn postgres_prefix_discarded() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let boundary = replay_boundary(&pool).await.expect("boundary");
    assert_eq!(
        boundary,
        ReplayBoundary {
            discarded_through: 15,
            high_water: 25,
        }
    );
    assert!(!cursor_is_retained(14, boundary));
    assert!(cursor_is_retained(15, boundary));
    let page = replay_page(&pool, ORG, 15, boundary.high_water, PageLimits::DEFAULT)
        .await
        .expect("page");
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![25]
    );
    pool.close().await;
}
