use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;
use verdaxis_read::crud::watchlists::{Event, Metrics, Target, project_summary};

fn target(id: u128, kind: &str) -> Target {
    serde_json::from_value(json!({"id":Uuid::from_u128(id),"target_type":kind,
        "market_product_code":"TEST_PRODUCT","delivery_point_id":Uuid::from_u128(99),
        "availability_window_code":"SPOT","order_id":if kind=="PIN" {Some(Uuid::from_u128(id+1000))}else{None},
        "created_at":"2026-10-19T00:00:00Z"})).unwrap()
}

#[test]
fn totals_are_computed_before_slice_truncation() {
    let targets = (1..=101).map(|id| target(id, "SLICE")).collect::<Vec<_>>();
    let metrics = Metrics::default();
    let summary = project_summary(
        Uuid::nil(),
        "Market Radar".into(),
        "RADAR_DEFAULT".into(),
        Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap(),
        targets,
        &metrics,
    );
    assert_eq!(summary.total_slice_count, 101);
    assert_eq!(summary.slices.len(), 100);
    assert!(summary.has_more_slices);
}

#[test]
fn hidden_pins_and_old_best_price_events_do_not_change_counts_or_latest_time() {
    let cutoff = Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
    let targets = vec![target(1, "SLICE"), target(2, "PIN"), target(3, "PIN")];
    let events = vec![
        Event {
            target_id: Uuid::from_u128(1),
            event_type: "SLICE_BEST_PRICE_MOVED".into(),
            order_id: None,
            is_read: false,
            created_at: cutoff - chrono::Duration::microseconds(1),
        },
        Event {
            target_id: Uuid::from_u128(2),
            event_type: "PIN_PRICE_CHANGED".into(),
            order_id: Some(Uuid::from_u128(1002)),
            is_read: false,
            created_at: cutoff,
        },
        Event {
            target_id: Uuid::from_u128(3),
            event_type: "PIN_PRICE_CHANGED".into(),
            order_id: Some(Uuid::from_u128(1003)),
            is_read: false,
            created_at: cutoff + chrono::Duration::seconds(1),
        },
    ];
    let visible = std::collections::HashSet::from([Uuid::from_u128(1002)]);
    let metrics = Metrics::from_visible_events(&targets, &events, &visible, cutoff);
    let targets = targets
        .into_iter()
        .filter(|t| t.target_type == "SLICE" || t.order_id.is_some_and(|id| visible.contains(&id)))
        .collect();
    let summary = project_summary(
        Uuid::nil(),
        "Market Radar".into(),
        "RADAR_DEFAULT".into(),
        cutoff,
        targets,
        &metrics,
    );
    assert_eq!(summary.unread_event_count, 1);
    assert_eq!(summary.latest_event_at, Some(cutoff));
    assert_eq!(summary.slices[0].pins.len(), 1);
    assert_eq!(summary.slices[0].pins[0].id, Uuid::from_u128(2));
}

#[test]
fn empty_projection_keeps_declared_nulls_and_default_fields() {
    let now = Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
    let summary = project_summary(
        Uuid::nil(),
        "empty".into(),
        "CUSTOM".into(),
        now,
        vec![],
        &Metrics::default(),
    );
    let wire = serde_json::to_value(summary).unwrap();
    assert_eq!(wire["latest_event_at"], serde_json::Value::Null);
    assert_eq!(wire["unread_event_count"], 0);
    assert_eq!(wire["slices"], json!([]));
}
