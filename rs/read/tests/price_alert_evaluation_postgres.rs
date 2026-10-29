//! Price-alert evaluation against a disposable PostgreSQL database.
//!
//! Each case builds its own schema holding the tables the evaluator touches,
//! with the column types of the deployed tables, and removes it at the end.
//! `RUST_TEST_MIGRATOR_DATABASE_URL` must name a numeric-loopback `_test`
//! database. The evaluator runs on a pool whose sessions default to read-only,
//! like the service pools, so the explicit write transaction is exercised too.

use std::{str::FromStr, sync::Arc};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tokio::sync::Barrier;
use uuid::Uuid;
use verdaxis_read::streams::{
    bus::StoredEvent,
    price_alerts::{TRIGGERED_EVENT_TYPE, evaluate_trade, executed_trade_id},
};

const SCHEMA_SQL: &str = "
CREATE TABLE users(
    id uuid PRIMARY KEY, organization_id uuid, status varchar(20) NOT NULL);
CREATE TABLE trades(
    id uuid PRIMARY KEY, product_id uuid, delivery_point_id uuid,
    product_name varchar(120), delivery_point_name varchar(120),
    price_per_mt_usd numeric NOT NULL, status varchar(30) NOT NULL,
    confirmed_at timestamptz, market_snapshot_version smallint,
    buyer_provenance varchar(10) NOT NULL, seller_provenance varchar(10) NOT NULL);
CREATE TABLE price_alerts(
    id uuid PRIMARY KEY, org_id uuid NOT NULL, product_id uuid NOT NULL,
    delivery_point_id uuid, direction varchar NOT NULL,
    threshold_usd numeric(10,2) NOT NULL, is_active boolean NOT NULL DEFAULT true,
    triggered_at timestamptz, created_at timestamptz DEFAULT now());
CREATE TABLE notifications(
    id uuid PRIMARY KEY, recipient_id uuid NOT NULL REFERENCES users(id),
    type varchar(30) NOT NULL, title varchar NOT NULL, message text NOT NULL,
    data json, is_read boolean NOT NULL, created_at timestamptz NOT NULL);
CREATE TABLE market_event_outbox(
    id uuid PRIMARY KEY, event_type varchar(80) NOT NULL,
    aggregate_type varchar(64) NOT NULL, aggregate_id varchar(100) NOT NULL,
    participant_org_ids json NOT NULL, payload json NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), dispatched_at timestamptz,
    stream_seq bigint, delivery_attempts integer NOT NULL DEFAULT 0, last_error text)";

fn now() -> DateTime<Utc> {
    "2026-10-28T09:00:00Z".parse().unwrap()
}

struct Fixture {
    admin: PgPool,
    runtime: PgPool,
    schema: String,
    product: Uuid,
    point: Uuid,
    org: Uuid,
}

struct Print {
    price: &'static str,
    buyer: &'static str,
    seller: &'static str,
    status: &'static str,
    snapshot: Option<i16>,
    product: Option<Uuid>,
    point: Option<Uuid>,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().ends_with("_test"));
        assert!(!matches!(parsed.port(), Some(5432 | 8000 | 8001)));

        let schema = format!("price_alert_{}", Uuid::new_v4().simple());
        let pool = |read_only: bool| {
            let schema = schema.clone();
            let mut options = PgConnectOptions::from_str(&url).unwrap();
            if read_only {
                options = options.options([("default_transaction_read_only", "on")]);
            }
            PgPoolOptions::new()
                .max_connections(6)
                .after_connect(move |connection, _| {
                    let schema = schema.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .connect_lazy_with(options)
        };
        let setup = PgPoolOptions::new().connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&setup)
            .await
            .unwrap();
        setup.close().await;
        let admin = pool(false);
        sqlx::raw_sql(SCHEMA_SQL).execute(&admin).await.unwrap();
        let fixture = Self {
            admin,
            runtime: pool(true),
            schema,
            product: Uuid::new_v4(),
            point: Uuid::new_v4(),
            org: Uuid::new_v4(),
        };
        fixture.member(fixture.org, "APPROVED").await;
        fixture.member(fixture.org, "APPROVED").await;
        fixture.member(fixture.org, "PENDING").await;
        fixture
    }

    async fn finish(self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .unwrap();
        self.admin.close().await;
        self.runtime.close().await;
    }

    async fn member(&self, org: Uuid, status: &str) {
        sqlx::query("INSERT INTO users(id, organization_id, status) VALUES($1, $2, $3)")
            .bind(Uuid::new_v4())
            .bind(org)
            .bind(status)
            .execute(&self.admin)
            .await
            .unwrap();
    }

    fn real(&self, price: &'static str) -> Print {
        Print {
            price,
            buyer: "REAL",
            seller: "REAL",
            status: "CONFIRMED",
            snapshot: Some(1),
            product: Some(self.product),
            point: Some(self.point),
        }
    }

    async fn trade(&self, print: Print) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO trades(id, product_id, delivery_point_id, product_name, \
             delivery_point_name, price_per_mt_usd, status, confirmed_at, \
             market_snapshot_version, buyer_provenance, seller_provenance) \
             VALUES($1, $2, $3, 'Bio-Methanol', 'Singapore', $4::text::numeric, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(print.product)
        .bind(print.point)
        .bind(print.price)
        .bind(print.status)
        .bind(now())
        .bind(print.snapshot)
        .bind(print.buyer)
        .bind(print.seller)
        .execute(&self.admin)
        .await
        .unwrap();
        id
    }

    async fn alert(&self, direction: &str, threshold: &str, point: Option<Uuid>) -> Uuid {
        self.alert_for(self.org, self.product, direction, threshold, point)
            .await
    }

    async fn alert_for(
        &self,
        org: Uuid,
        product: Uuid,
        direction: &str,
        threshold: &str,
        point: Option<Uuid>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO price_alerts(id, org_id, product_id, delivery_point_id, direction, \
             threshold_usd) VALUES($1, $2, $3, $4, $5, $6::text::numeric)",
        )
        .bind(id)
        .bind(org)
        .bind(product)
        .bind(point)
        .bind(direction)
        .bind(threshold)
        .execute(&self.admin)
        .await
        .unwrap();
        id
    }

    async fn is_active(&self, alert: Uuid) -> bool {
        sqlx::query_scalar("SELECT is_active FROM price_alerts WHERE id = $1")
            .bind(alert)
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn an_above_alert_fires_once_and_is_deactivated() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "700.00", Some(fixture.point)).await;
    let trade = fixture.trade(fixture.real("750.5")).await;

    let fired = evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].alert_id, alert);
    assert!(!fixture.is_active(alert).await);
    let triggered_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT triggered_at FROM price_alerts WHERE id = $1")
            .bind(alert)
            .fetch_one(&fixture.admin)
            .await
            .unwrap();
    assert_eq!(triggered_at, Some(now()));

    // Evaluating the same print again changes nothing.
    let again = evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(fixture.count("market_event_outbox").await, 1);
    assert_eq!(fixture.count("notifications").await, 2);
    fixture.finish().await;
}

#[tokio::test]
async fn a_below_alert_fires_and_a_price_equal_to_the_threshold_does_not() {
    let fixture = Fixture::new().await;
    let below = fixture.alert("below", "700.00", Some(fixture.point)).await;
    let equal = fixture.trade(fixture.real("700")).await;
    assert!(
        evaluate_trade(&fixture.runtime, equal, now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fixture.is_active(below).await);

    let lower = fixture.trade(fixture.real("699.99")).await;
    let fired = evaluate_trade(&fixture.runtime, lower, now())
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert!(!fixture.is_active(below).await);
    fixture.finish().await;
}

#[tokio::test]
async fn prints_that_do_not_qualify_leave_alerts_untouched() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "100", Some(fixture.point)).await;
    let other_product = Uuid::new_v4();
    let other_point = Uuid::new_v4();
    let prints = [
        Print {
            product: Some(other_product),
            ..fixture.real("500")
        },
        Print {
            point: Some(other_point),
            ..fixture.real("500")
        },
        Print {
            buyer: "DEMO",
            seller: "DEMO",
            ..fixture.real("500")
        },
        Print {
            buyer: "TEST",
            seller: "TEST",
            ..fixture.real("500")
        },
        Print {
            buyer: "CANARY",
            seller: "REAL",
            ..fixture.real("500")
        },
        Print {
            buyer: "REAL",
            seller: "UNKNOWN",
            ..fixture.real("500")
        },
        Print {
            status: "PENDING_CONFIRMATION",
            ..fixture.real("500")
        },
        Print {
            status: "DECLINED",
            ..fixture.real("500")
        },
        Print {
            snapshot: None,
            ..fixture.real("500")
        },
        Print {
            point: None,
            ..fixture.real("500")
        },
    ];
    for print in prints {
        let trade = fixture.trade(print).await;
        let fired = evaluate_trade(&fixture.runtime, trade, now())
            .await
            .unwrap();
        assert!(fired.is_empty());
    }
    // A trade id that does not exist is also a quiet no-op.
    assert!(
        evaluate_trade(&fixture.runtime, Uuid::new_v4(), now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fixture.is_active(alert).await);
    assert_eq!(fixture.count("notifications").await, 0);
    assert_eq!(fixture.count("market_event_outbox").await, 0);
    fixture.finish().await;
}

#[tokio::test]
async fn delivered_and_paid_trades_are_formal_prints_too() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "100", Some(fixture.point)).await;
    let trade = fixture
        .trade(Print {
            status: "PAID",
            ..fixture.real("500")
        })
        .await;
    let fired = evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert!(!fixture.is_active(alert).await);
    fixture.finish().await;
}

#[tokio::test]
async fn an_any_point_alert_fires_for_any_point_of_its_product() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "100", None).await;
    let elsewhere = Uuid::new_v4();
    let trade = fixture
        .trade(Print {
            point: Some(elsewhere),
            ..fixture.real("500")
        })
        .await;
    let fired = evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert!(!fixture.is_active(alert).await);
    let payload: Value = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT payload::text FROM market_event_outbox")
            .fetch_one(&fixture.admin)
            .await
            .unwrap(),
    )
    .unwrap();
    // The event names where the price executed.
    assert_eq!(payload["delivery_point_id"], elsewhere.to_string());
    fixture.finish().await;
}

#[tokio::test]
async fn inactive_alerts_other_products_and_other_organizations_are_independent() {
    let fixture = Fixture::new().await;
    let inactive = fixture.alert("above", "100", None).await;
    sqlx::query("UPDATE price_alerts SET is_active = false WHERE id = $1")
        .bind(inactive)
        .execute(&fixture.admin)
        .await
        .unwrap();
    let other_org = Uuid::new_v4();
    fixture.member(other_org, "APPROVED").await;
    let theirs = fixture
        .alert_for(other_org, fixture.product, "above", "100", None)
        .await;
    let unrelated = fixture
        .alert_for(fixture.org, Uuid::new_v4(), "above", "100", None)
        .await;
    let not_crossed = fixture.alert("above", "900", None).await;

    let trade = fixture.trade(fixture.real("500")).await;
    let fired = evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].alert_id, theirs);
    assert!(fixture.is_active(unrelated).await);
    assert!(fixture.is_active(not_crossed).await);
    assert!(!fixture.is_active(inactive).await);
    // Only the alert owner's organization is notified and sees the event.
    let recipients: Vec<Uuid> = sqlx::query_scalar(
        "SELECT u.organization_id FROM notifications n JOIN users u ON u.id = n.recipient_id",
    )
    .fetch_all(&fixture.admin)
    .await
    .unwrap();
    assert_eq!(recipients, vec![other_org]);
    fixture.finish().await;
}

#[tokio::test]
async fn the_notification_and_the_event_carry_the_alert() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "700", Some(fixture.point)).await;
    let trade = fixture.trade(fixture.real("725.5")).await;
    evaluate_trade(&fixture.runtime, trade, now())
        .await
        .unwrap();

    // One notification for each approved member; the pending member gets none.
    let rows = sqlx::query(
        "SELECT type, title, message, data::text AS data, is_read, created_at FROM notifications",
    )
    .fetch_all(&fixture.admin)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row.get::<String, _>("type"), "SYSTEM");
        assert_eq!(row.get::<String, _>("title"), "Price alert triggered");
        assert_eq!(
            row.get::<String, _>("message"),
            "Bio-Methanol at Singapore traded at $725.50/MT, above your $700.00 alert."
        );
        assert!(!row.get::<bool, _>("is_read"));
        assert_eq!(row.get::<DateTime<Utc>, _>("created_at"), now());
        let data: Value = serde_json::from_str(&row.get::<String, _>("data")).unwrap();
        assert_eq!(data["alert_id"], alert.to_string());
        assert_eq!(data["type"], TRIGGERED_EVENT_TYPE);
    }

    let event = sqlx::query(
        "SELECT event_type, aggregate_type, aggregate_id, participant_org_ids::text AS orgs, \
         payload::text AS payload, stream_seq FROM market_event_outbox",
    )
    .fetch_one(&fixture.admin)
    .await
    .unwrap();
    assert_eq!(event.get::<String, _>("event_type"), TRIGGERED_EVENT_TYPE);
    assert_eq!(event.get::<String, _>("aggregate_type"), "price_alert");
    assert_eq!(event.get::<String, _>("aggregate_id"), alert.to_string());
    // Waiting for the sequencer, and private to the alert's organization.
    assert_eq!(event.get::<Option<i64>, _>("stream_seq"), None);
    let orgs: Vec<Uuid> = serde_json::from_str(&event.get::<String, _>("orgs")).unwrap();
    assert_eq!(orgs, vec![fixture.org]);
    let payload: Value = serde_json::from_str(&event.get::<String, _>("payload")).unwrap();
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["alert_id"], alert.to_string());
    assert_eq!(payload["product_id"], fixture.product.to_string());
    assert_eq!(payload["delivery_point_id"], fixture.point.to_string());
    assert_eq!(payload["direction"], "above");
    assert_eq!(payload["threshold_usd"], "700.00");
    assert_eq!(payload["price"], "725.5");

    // The appended event is not an execution: it can never trigger again.
    let stored = StoredEvent::new(1, vec![fixture.org], TRIGGERED_EVENT_TYPE, payload).unwrap();
    assert_eq!(executed_trade_id(&stored), None);
    fixture.finish().await;
}

#[tokio::test]
async fn concurrent_evaluations_fire_an_alert_once() {
    let fixture = Fixture::new().await;
    let alerts = [
        fixture.alert("above", "100", Some(fixture.point)).await,
        fixture.alert("above", "200", None).await,
        fixture.alert("above", "300", Some(fixture.point)).await,
    ];
    let trade = fixture.trade(fixture.real("500")).await;

    let workers = 4;
    let barrier = Arc::new(Barrier::new(workers));
    let tasks: Vec<_> = (0..workers)
        .map(|_| {
            let pool = fixture.runtime.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                evaluate_trade(&pool, trade, now()).await.unwrap().len()
            })
        })
        .collect();
    let mut fired = 0;
    for task in tasks {
        fired += task.await.unwrap();
    }

    assert_eq!(fired, alerts.len());
    for alert in alerts {
        assert!(!fixture.is_active(alert).await);
    }
    // One event per alert, and one notification per approved member and alert.
    assert_eq!(fixture.count("market_event_outbox").await, 3);
    assert_eq!(fixture.count("notifications").await, 6);
    fixture.finish().await;
}

#[tokio::test]
async fn a_failed_write_leaves_the_alert_active() {
    let fixture = Fixture::new().await;
    let alert = fixture.alert("above", "100", Some(fixture.point)).await;
    let trade = fixture.trade(fixture.real("500")).await;
    sqlx::raw_sql("DROP TABLE market_event_outbox")
        .execute(&fixture.admin)
        .await
        .unwrap();

    assert!(
        evaluate_trade(&fixture.runtime, trade, now())
            .await
            .is_err()
    );
    // Nothing was kept: the next attempt can still fire the alert.
    assert!(fixture.is_active(alert).await);
    assert_eq!(fixture.count("notifications").await, 0);
    fixture.finish().await;
}
