use std::{sync::Arc, time::Duration};

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;
use verdaxis_read::{
    analytics::{
        events::{AllowedServerEvent, ServerEvent},
        sender::{AnalyticsSender, RequestMetadata},
    },
    market_trades::{TradeCreate, TradePage, project_trade},
    providers::umami::{UmamiConfig, UmamiPolicy},
};

fn request() -> Value {
    json!({"order_id":Uuid::nil(),"quantity_mt":"1.25"})
}

#[test]
fn take_input_validates_each_scalar_domain() {
    let parsed = TradeCreate::parse(request()).unwrap();
    assert_eq!(parsed.quantity_mt.to_string(), "1.25");
    for quantity in [
        json!(null),
        json!(0),
        json!(-1),
        json!("0.001"),
        json!("100000.01"),
        json!("NaN"),
    ] {
        let mut body = request();
        body["quantity_mt"] = quantity;
        assert!(TradeCreate::parse(body).is_err());
    }
    for (key, value) in [
        ("order_id", json!("bad")),
        ("expected_order_version", json!(0)),
        ("expected_terms_digest", json!("A".repeat(64))),
        ("certification_declared", json!("yes")),
    ] {
        let mut body = request();
        body[key] = value;
        assert!(TradeCreate::parse(body).is_err(), "{key}");
    }
    let mut body = request();
    body["expected_order_version"] = json!(99);
    body["expected_terms_digest"] = json!("a".repeat(64));
    assert!(TradeCreate::parse(body).is_ok());
}

#[test]
fn take_hash_omits_nulls_and_false_declarations() {
    let plain = TradeCreate::parse(request()).unwrap();
    let mut body = request();
    for key in [
        "fame_terms",
        "lng_terms",
        "expected_order_version",
        "expected_terms_digest",
    ] {
        body[key] = Value::Null;
    }
    body["certification_declared"] = json!(false);
    body["msds_available"] = json!(false);
    assert_eq!(
        plain.request_hash().unwrap(),
        TradeCreate::parse(body.clone())
            .unwrap()
            .request_hash()
            .unwrap()
    );
    body["expected_order_version"] = json!(1);
    assert_ne!(
        plain.request_hash().unwrap(),
        TradeCreate::parse(body).unwrap().request_hash().unwrap()
    );
}

#[test]
fn trade_page_has_bounded_filters() {
    let page = TradePage::parse(None).unwrap();
    assert_eq!((page.skip, page.limit), (0, 20));
    for query in [
        "skip=-1",
        "limit=0",
        "limit=101",
        "action_required=maybe",
        "status_group=unknown",
    ] {
        assert!(TradePage::parse(Some(query)).is_err(), "{query}");
    }
    assert!(
        TradePage::parse(Some(
            "skip=2&limit=1&action_required=true&status_group=completed"
        ))
        .is_ok()
    );
}

#[test]
fn pending_projection_separates_fee_and_fuel_visibility() {
    let buyer = Uuid::new_v4();
    let seller = Uuid::new_v4();
    let row = json!({"id":Uuid::new_v4(),"buyer_id":buyer,"seller_id":seller,"buyer_name":"Buyer","seller_name":"Seller","status":"PENDING_CONFIRMATION","is_anonymous":true,"commission_fee_per_mt_usd":"2","commission_plan":"FREE","commission_amount_usd":"4","commission_rate_pct":"0","fame_terms_snapshot":null,"lng_terms_snapshot":null});
    let buy = project_trade(row.clone(), Some(buyer), false).unwrap();
    assert!(buy["seller_id"].is_null());
    assert_eq!(buy["seller_name"], "Anonymous");
    assert!(buy["commission_fee_per_mt_usd"].is_null());
    assert!(buy["commission_amount_usd"].is_null());
    assert!(buy["lng_terms_snapshot"]["ask"]["physical_supplier_name"].is_null());
    let sell = project_trade(row.clone(), Some(seller), false).unwrap();
    assert_eq!(sell["commission_fee_per_mt_usd"], "2");
    assert!(sell["lng_terms_snapshot"]["ask"]["physical_supplier_name"].is_null());
    let mut confirmed = row.clone();
    confirmed["status"] = json!("CONFIRMED");
    let confirmed = project_trade(confirmed, Some(buyer), false).unwrap();
    assert_eq!(confirmed["seller_id"], seller.to_string());

    let mut legacy = row;
    legacy["commission_fee_per_mt_usd"] = Value::Null;
    assert_eq!(
        project_trade(legacy, Some(buyer), false).unwrap()["commission_amount_usd"],
        "4"
    );
}

#[derive(Clone, Default)]
struct Receiver {
    rows: Arc<Mutex<Vec<Value>>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    hold: bool,
    fail: bool,
}
async fn receive(
    State(state): State<Receiver>,
    Json(value): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    state.rows.lock().await.push(value);
    state.entered.notify_one();
    if state.hold {
        state.release.notified().await;
    }
    (
        if state.fail {
            axum::http::StatusCode::BAD_GATEWAY
        } else {
            axum::http::StatusCode::OK
        },
        Json(json!({"ok":true})),
    )
}
async fn sender(state: Receiver) -> (AnalyticsSender, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/api/send", post(receive))
                .with_state(state),
        )
        .await
        .unwrap();
    });
    let config = UmamiConfig {
        enabled: true,
        base_url: Some(format!("http://{address}")),
        website_id: Some("synthetic-site".into()),
        api_username: None,
        api_password: None,
        policy: UmamiPolicy {
            timeout: Duration::from_millis(150),
        },
        catalog: Arc::new(Default::default()),
    };
    (
        AnalyticsSender::new(
            &config,
            Some("synthetic-monitor".into()),
            "https://app.example.test",
            1,
        )
        .unwrap(),
        task,
    )
}
fn event() -> ServerEvent {
    ServerEvent::new(AllowedServerEvent::TradeCreated, []).unwrap()
}

#[tokio::test]
async fn analytics_sender_posts_the_registered_payload() {
    let receiver = Receiver::default();
    let (sender, task) = sender(receiver.clone()).await;
    sender.schedule(
        event(),
        RequestMetadata::new(Some(" browser\nagent "), Some("API.EXAMPLE.TEST")),
        None,
    );
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    let rows = receiver.rows.lock().await;
    assert_eq!(
        rows[0],
        json!({"type":"event","payload":{"website":"synthetic-site","hostname":"api.example.test","url":"/server-events","title":"Verdaxis server event","name":"trade_created","data":{}}})
    );
    task.abort();
}

#[tokio::test]
async fn analytics_sender_skips_only_the_configured_monitor() {
    let receiver = Receiver::default();
    let (sender, task) = sender(receiver.clone()).await;
    sender.schedule(
        event(),
        RequestMetadata::default(),
        Some("synthetic-monitor"),
    );
    tokio::task::yield_now().await;
    assert!(receiver.rows.lock().await.is_empty());
    sender.schedule(event(), RequestMetadata::default(), Some("another-token"));
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    assert_eq!(receiver.rows.lock().await.len(), 1);
    task.abort();
}

#[tokio::test]
async fn analytics_sender_bounds_inflight_work_and_recovers_after_timeout() {
    let receiver = Receiver {
        hold: true,
        ..Default::default()
    };
    let (sender, task) = sender(receiver.clone()).await;
    sender.schedule(event(), RequestMetadata::default(), None);
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    for _ in 0..20 {
        sender.schedule(event(), RequestMetadata::default(), None);
    }
    tokio::task::yield_now().await;
    assert_eq!(receiver.rows.lock().await.len(), 1);
    tokio::time::sleep(Duration::from_millis(250)).await;
    sender.schedule(event(), RequestMetadata::default(), None);
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    assert_eq!(receiver.rows.lock().await.len(), 2);
    task.abort();
}

#[tokio::test]
async fn analytics_sender_drops_collector_failures_without_affecting_callers() {
    let receiver = Receiver {
        fail: true,
        ..Default::default()
    };
    let (sender, task) = sender(receiver.clone()).await;
    sender.schedule(event(), RequestMetadata::default(), None);
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    sender.schedule(event(), RequestMetadata::default(), None);
    tokio::time::timeout(Duration::from_secs(2), receiver.entered.notified())
        .await
        .unwrap();
    assert_eq!(receiver.rows.lock().await.len(), 2);
    task.abort();
}

#[test]
fn trade_projection_excludes_unlisted_columns() {
    let value=project_trade(json!({"buyer_id":Uuid::new_v4(),"seller_id":Uuid::new_v4(),"status":"PAID","is_anonymous":false,"private_future_column":"retained only in storage"}),None,false).unwrap();
    assert!(value.get("private_future_column").is_none());
}

#[test]
fn analytics_configuration_rejects_unbounded_or_credentialed_delivery() {
 let mut config=UmamiConfig{enabled:true,base_url:Some("https://collector.example.test".into()),website_id:Some("site".into()),api_username:None,api_password:None,policy:UmamiPolicy::default(),catalog:Arc::new(Default::default())};
 assert!(AnalyticsSender::new(&config,None,"https://app.example.test",0).is_err());
 config.policy.timeout=Duration::from_secs(11);assert!(AnalyticsSender::new(&config,None,"https://app.example.test",1).is_err());
 config.policy.timeout=Duration::from_secs(1);config.base_url=Some("https://synthetic:unused@collector.example.test".into());assert!(AnalyticsSender::new(&config,None,"https://app.example.test",1).is_err());
}
