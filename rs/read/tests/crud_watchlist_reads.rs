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
    let mut events = events;
    events.push(Event {target_id:Uuid::from_u128(1),event_type:"SLICE_BEST_PRICE_MOVED".into(),
        order_id:Some(Uuid::from_u128(1002)),is_read:false,created_at:cutoff+chrono::Duration::days(1)});
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

#[tokio::test]
async fn database_detail_is_actor_scoped_and_does_not_mutate_rows() {
    use verdaxis_read::crud::watchlists::load_detail;
    let Ok(url) = std::env::var("AUTH_INTEROP_ADMIN_URL") else {
        return;
    };
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    use sqlx::Connection;
    let owner = Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap();
    let id = Uuid::from_u128(700);
    sqlx::query("INSERT INTO watchlists(id,user_id,name,kind,created_at) VALUES($1,$2,'fixture','CUSTOM','2026-10-19T00:00:00Z')")
        .bind(id).bind(owner).execute(&mut connection).await.unwrap();
    let before: String =
        sqlx::query_scalar("SELECT row_to_json(w)::text FROM watchlists w WHERE id=$1")
            .bind(id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let cutoff = Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap();
    let visible = std::collections::HashSet::new();
    assert!(
        load_detail(&mut connection, Uuid::nil(), id, &visible, &[], cutoff)
            .await
            .unwrap()
            .is_none()
    );
    let summary = load_detail(&mut connection, owner, id, &visible, &[], cutoff)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.total_slice_count, 0);
    assert_eq!(summary.name, "fixture");
    let legacy = verdaxis_read::crud::watchlists::load_legacy_list(&mut connection, owner)
        .await
        .unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].id, id);
    assert_eq!(legacy[0].entry_count, 0);
    assert!(
        verdaxis_read::crud::watchlists::load_legacy_list(&mut connection, Uuid::nil())
            .await
            .unwrap()
            .is_empty()
    );
    // Keep the database query count independent of target count: target and
    // event loading are one batch each, never a query per SLICE/PIN.
    let point = Uuid::from_u128(701);
    sqlx::query("INSERT INTO delivery_points(id,name,region,is_active) VALUES($1,'Fixture point','Fixture',true)")
        .bind(point).execute(&mut connection).await.unwrap();
    sqlx::query("INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,availability_window_code,created_at) SELECT gen_random_uuid(),$1,'SLICE','TEST_PRODUCT',$2,'2026-'||lpad(n::text,2,'0'),'2026-10-19T00:00:00Z'::timestamptz FROM generate_series(1,10) n")
        .bind(id).bind(point).execute(&mut connection).await.unwrap();
    let populated = load_detail(&mut connection, owner, id, &visible, &[], cutoff)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(populated.total_slice_count, 10);
    sqlx::query("DELETE FROM watchlist_targets WHERE watchlist_id=$1")
        .bind(id)
        .execute(&mut connection)
        .await
        .unwrap();
    let after: String =
        sqlx::query_scalar("SELECT row_to_json(w)::text FROM watchlists w WHERE id=$1")
            .bind(id)
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(before, after);
    sqlx::query("DELETE FROM watchlists WHERE id=$1")
        .bind(id)
        .execute(&mut connection)
        .await
        .unwrap();
}

#[test]
fn public_visibility_snapshot_owns_ids_and_count_keys_together() {
    use verdaxis_read::crud::watchlists::VisibleSnapshot;
    let snapshot = VisibleSnapshot::from_orders(vec![(
        Uuid::from_u128(1),
        "TEST".into(),
        Uuid::from_u128(2),
        "SPOT".into(),
    )]);
    assert!(snapshot.contains(Uuid::from_u128(1)));
    assert!(!snapshot.contains(Uuid::from_u128(3)));
    assert_eq!(snapshot.order_count(), 1);
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
