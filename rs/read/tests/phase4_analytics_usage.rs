//! P01 `GET /api/admin/analytics/product-usage`.
//!
//! The authoritative counts come from PostgreSQL; the behavioral aggregate is
//! optional. An unavailable provider keeps the authoritative counts and still
//! answers 200, as the Python route does. The provider seam is a test double
//! built from the accepted CF4_03 declarations; it never contacts Umami.

pub use verdaxis_read::*;

#[path = "../src/analytics/usage.rs"]
mod usage;

#[allow(dead_code)]
#[path = "../src/analytics/policy.rs"]
mod policy;
#[allow(dead_code)]
#[path = "../src/analytics/types.rs"]
mod types;

use types::{
    AggregateCell, AllowedEventAggregate, AnalyticsDiagnostic, AnalyticsSourceCoverage,
    BehavioralAvailability, BehavioralWindow, CoreMetrics, DailyEventRow, DailyVisitorRow,
    EntryPage, EntryRankingRow, ProviderComponent, ReferrerCategory, ReferrerRankingRow,
};

use axum::http::StatusCode;
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use usage::{AuthoritativeUsage, UsageDays};
use uuid::Uuid;
use verdaxis_read::runtime::db::Pools;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap()
}

fn cell(count: u64) -> AggregateCell {
    policy::suppress(Some(count), None)
}

fn available() -> BehavioralWindow {
    BehavioralWindow {
        start: now() - Duration::days(30),
        end: now(),
        availability: BehavioralAvailability::Available,
        diagnostic: None,
        observed_at: now(),
        coverage: AnalyticsSourceCoverage::available(),
        core: ProviderComponent::available(CoreMetrics {
            visitors: Some(10),
            visits: Some(5),
            pageviews: Some(20),
            total_time_seconds: Some(100),
            average_session_duration_seconds: Some(rust_decimal::Decimal::from(20)),
        }),
        event_totals: ProviderComponent::available(vec![
            AllowedEventAggregate {
                event: "signup_started".into(),
                count: cell(4),
            },
            AllowedEventAggregate {
                event: "order_created".into(),
                count: cell(3),
            },
            AllowedEventAggregate {
                event: "not_a_registered_event".into(),
                count: cell(9),
            },
        ]),
        daily_visitors: ProviderComponent::available(vec![DailyVisitorRow {
            date: now().date_naive(),
            visitors: cell(3),
        }]),
        event_series: ProviderComponent::available(vec![DailyEventRow {
            date: now().date_naive(),
            event: "signup_started".into(),
            count: cell(2),
        }]),
        top_entries: ProviderComponent::available(vec![EntryRankingRow {
            entry: EntryPage::Signup,
            count: cell(4),
            rank: 1,
        }]),
        top_referrers: ProviderComponent::available(vec![ReferrerRankingRow {
            referrer: ReferrerCategory::Search,
            count: cell(2),
            rank: 1,
        }]),
        property_results: Vec::new(),
    }
}

fn unavailable(diagnostic: AnalyticsDiagnostic) -> BehavioralWindow {
    BehavioralWindow::unavailable(now() - Duration::days(30), now(), now(), diagnostic)
}

fn authoritative(registrations: u64, logins: u64, organizations: u64) -> AuthoritativeUsage {
    AuthoritativeUsage {
        registrations,
        users_logging_in: logins,
        order_placing_organizations: organizations,
    }
}

/// c01: an unavailable provider keeps the authoritative counts and every
/// unavailable behavioral value stays the accepted null contract. The funnel
/// derives no conversion from a zero previous stage.
#[test]
fn p4_t17a_c01() {
    let response = usage::compose(
        UsageDays::Seven,
        now() - Duration::days(7),
        now(),
        authoritative(2, 2, 1),
        &unavailable(AnalyticsDiagnostic::Timeout),
    );
    let body = serde_json::to_value(&response).unwrap();

    assert_eq!(body["days"], 7);
    assert_eq!(body["behavioral_status"], "unavailable");
    assert_eq!(body["diagnostic"], "timeout");
    assert_eq!(body["observed_at"], "2026-10-24T12:00:00Z");
    assert_eq!(body["authoritative"]["registrations"], 2);
    assert_eq!(body["authoritative"]["users_logging_in"], 2);
    assert_eq!(body["authoritative"]["order_placing_organizations"], 1);

    assert_eq!(body["behavioral"]["visitors"], Value::Null);
    assert_eq!(body["behavioral"]["visits"], Value::Null);
    assert_eq!(
        body["behavioral"]["average_session_duration_seconds"],
        Value::Null
    );
    assert_eq!(body["behavioral"]["event_totals"], Value::Null);
    assert_eq!(body["behavioral"]["daily_visitors"], Value::Null);
    assert_eq!(body["behavioral"]["event_series"], Value::Null);
    assert_eq!(body["behavioral"]["top_entries"], Value::Null);
    assert_eq!(body["behavioral"]["top_referrers"], Value::Null);

    let funnel = body["funnel"].as_array().unwrap();
    assert_eq!(
        funnel
            .iter()
            .map(|stage| stage["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "visitors",
            "signup_started",
            "registrations",
            "users_logging_in",
            "order_placing_organizations"
        ]
    );
    assert_eq!(funnel[0]["conversion_from_previous_pct"], Value::Null);
    assert_eq!(funnel[1]["conversion_from_previous_pct"], Value::Null);
    assert_eq!(funnel[2]["conversion_from_previous_pct"], Value::Null);
    assert_eq!(funnel[3]["conversion_from_previous_pct"], 100.0);
    assert_eq!(funnel[4]["conversion_from_previous_pct"], 50.0);
    assert_eq!(
        funnel
            .iter()
            .map(|stage| stage["value"].clone())
            .collect::<Vec<_>>(),
        vec![Value::Null, Value::Null, json!(2), json!(2), json!(1)]
    );

    let text = body.to_string().to_lowercase();
    for forbidden in [
        "example.test",
        "@",
        "organization_name",
        "price",
        "quantity",
        "order_id",
        "trade_id",
        "session_id",
        "distinct_id",
    ] {
        assert!(!text.contains(forbidden), "exposed {forbidden}");
    }
}

/// c03: the available aggregate exposes only declared events and bounded
/// collections, calculates the session average and derives real conversions.
#[test]
fn p4_t17a_c03() {
    let response = usage::compose(
        UsageDays::Thirty,
        now() - Duration::days(30),
        now(),
        authoritative(2, 2, 1),
        &available(),
    );
    let body = serde_json::to_value(&response).unwrap();
    let behavioral = &body["behavioral"];

    assert_eq!(body["behavioral_status"], "available");
    assert_eq!(body["diagnostic"], Value::Null);
    assert_eq!(behavioral["visitors"], 10);
    assert_eq!(behavioral["visits"], 5);
    assert_eq!(behavioral["pageviews"], 20);
    assert_eq!(behavioral["total_time_seconds"], 100);
    assert_eq!(behavioral["average_session_duration_seconds"], 20.0);
    assert_eq!(
        behavioral["event_totals"],
        json!({"order_created": 3, "signup_started": 4})
    );
    assert_eq!(
        behavioral["daily_visitors"],
        json!([{"date": "2026-10-24", "value": 3}])
    );
    assert_eq!(
        behavioral["event_series"],
        json!([{"date": "2026-10-24", "event": "signup_started", "value": null}])
    );
    assert_eq!(
        behavioral["top_entries"],
        json!([{"name": "signup", "value": 4}])
    );
    assert_eq!(
        behavioral["top_referrers"],
        json!([{"name": "search", "value": null}])
    );

    let funnel = body["funnel"].as_array().unwrap();
    assert_eq!(funnel[1]["conversion_from_previous_pct"], 40.0);
    assert_eq!(funnel[2]["conversion_from_previous_pct"], 50.0);
    assert_eq!(funnel[3]["conversion_from_previous_pct"], 100.0);
    assert_eq!(funnel[4]["conversion_from_previous_pct"], 50.0);

    // The legacy field name and the identifier/price vocabulary never appear.
    for forbidden in [
        "average_duration_seconds",
        "sessions",
        "distinct_ids",
        "email",
        "organization_names",
        "prices",
        "quantities",
        "not_a_registered_event",
    ] {
        assert!(!body.to_string().contains(forbidden), "exposed {forbidden}");
    }

    let mut partial = available();
    partial.availability = BehavioralAvailability::Partial;
    partial.core = ProviderComponent::not_requested();
    partial.event_totals = ProviderComponent::available(vec![AllowedEventAggregate {
        event: "signup_started".into(),
        count: cell(2),
    }]);
    let body = serde_json::to_value(usage::compose(
        UsageDays::Thirty,
        now() - Duration::days(30),
        now(),
        authoritative(2, 2, 1),
        &partial,
    ))
    .unwrap();
    assert_eq!(body["behavioral"]["visitors"], Value::Null);
    assert_eq!(
        body["behavioral"]["event_totals"]["signup_started"],
        Value::Null
    );
    assert_eq!(body["funnel"][1]["value"], Value::Null);
    assert_eq!(
        body["funnel"][2]["conversion_from_previous_pct"],
        Value::Null
    );

    // Response bounds remain independent of the optional adapter.
    let mut wide = available();
    wide.daily_visitors = ProviderComponent::available(vec![
        DailyVisitorRow {
            date: now().date_naive(),
            visitors: cell(3),
        };
        400
    ]);
    wide.event_series = ProviderComponent::available(vec![
        DailyEventRow {
            date: now().date_naive(),
            event: "signup_started".into(),
            count: cell(3),
        };
        2600
    ]);
    wide.top_entries = ProviderComponent::available(vec![
        EntryRankingRow {
            entry: EntryPage::Other,
            count: cell(3),
            rank: 1,
        };
        30
    ]);
    let bounded = serde_json::to_value(usage::compose(
        UsageDays::Ninety,
        now() - Duration::days(90),
        now(),
        authoritative(1, 1, 1),
        &wide,
    ))
    .unwrap();
    assert_eq!(
        bounded["behavioral"]["daily_visitors"]
            .as_array()
            .unwrap()
            .len(),
        92
    );
    assert_eq!(
        bounded["behavioral"]["event_series"]
            .as_array()
            .unwrap()
            .len(),
        2500
    );
    assert_eq!(
        bounded["behavioral"]["top_entries"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
}

/// c04: only 7, 30 and 90 select a period; the default is 30.
#[test]
fn p4_t17a_c04() {
    assert_eq!(UsageDays::parse(None).unwrap(), UsageDays::Thirty);
    assert_eq!(UsageDays::parse(Some("7")).unwrap(), UsageDays::Seven);
    assert_eq!(UsageDays::parse(Some("30")).unwrap(), UsageDays::Thirty);
    assert_eq!(UsageDays::parse(Some("90")).unwrap(), UsageDays::Ninety);
    for refused in ["1", "8", "31", "365", "", "seven", "7.0", "-7"] {
        let error = UsageDays::parse(Some(refused)).unwrap_err();
        assert_eq!(
            error.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{refused:?}"
        );
        assert_eq!(error.detail["code"], "ANALYTICS_QUERY_INVALID");
    }
    assert_eq!(UsageDays::Seven.value(), 7);
    assert_eq!(UsageDays::Thirty.value(), 30);
    assert_eq!(UsageDays::Ninety.value(), 90);
}

/// c08: the declared 30/minute token quota is bounded, keyed by token and
/// recovers when its window moves. Administrator authority never charges it.
#[test]
fn p4_t17a_c08() {
    let limits = auth::limits::SharedLimits::default();
    let actor = |role: &str| auth::CurrentActor {
        id: uuid::Uuid::new_v4(),
        organization_id: None,
        role: Some(role.into()),
        email_verified: true,
    };
    let admin = actor("ADMIN");
    let buyer = actor("BUYER");
    let refused = usage::require_admin(&buyer).unwrap_err();
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.detail["code"], "ADMIN_REQUIRED");
    usage::require_admin(&admin).unwrap();

    assert_ne!(
        usage::token_identity(Some("Bearer a")),
        usage::token_identity(Some("Bearer b")),
        "a token rotation starts a fresh bucket"
    );
    for respelled in ["bearer a", "BEARER a", "Bearer  a", "Bearer   a"] {
        assert_eq!(
            usage::token_identity(Some(respelled)),
            usage::token_identity(Some("Bearer a")),
            "{respelled:?} must not open a second bucket"
        );
    }
    assert_eq!(usage::token_identity(None), usage::token_identity(Some("")));
    for _ in 0..30 {
        usage::check_quota(&limits, &admin, Some("Bearer a"), 1_000).unwrap();
    }
    let refused = usage::check_quota(&limits, &admin, Some("Bearer a"), 1_000).unwrap_err();
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.retry_after, Some(60));
    assert_eq!(refused.detail["code"], "RATE_LIMITED");
    usage::check_quota(&limits, &admin, Some("Bearer b"), 1_000).unwrap();
    usage::check_quota(&limits, &admin, Some("Bearer a"), 1_060).unwrap();
}

/// c09: the module's declared seams are present and every diagnostic value
/// keeps its wire spelling; a window is half-open and always ordered.
#[tokio::test]
async fn p4_t17a_c09() {
    use usage::BehavioralUsageSource as _;

    // Interface check: the authoritative count query keeps its borrowed
    // connection and checked window, and starts no transaction of its own.
    fn counts_interface<'a>(
        connection: &'a mut sqlx::PgConnection,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AuthoritativeUsage, ApiError>> + Send + 'a>,
    > {
        Box::pin(usage::authoritative_counts(connection, start, end))
    }
    let _ = counts_interface;

    let window = usage::UsageWindow::days(UsageDays::Ninety, now());
    assert_eq!(window.end, now());
    assert_eq!(window.start, now() - Duration::days(90));

    struct Offline;
    impl usage::BehavioralUsageSource for Offline {
        fn window(&self, window: usage::UsageWindow) -> usage::BehavioralFuture<'_> {
            Box::pin(async move {
                assert_eq!(window.end - window.start, Duration::days(7));
                BehavioralWindow::unavailable(
                    window.start,
                    window.end,
                    window.end,
                    AnalyticsDiagnostic::Upstream,
                )
            })
        }
    }
    let observed = Offline
        .window(usage::UsageWindow::days(UsageDays::Seven, now()))
        .await;
    let body = serde_json::to_value(usage::compose(
        UsageDays::Seven,
        now() - Duration::days(7),
        now(),
        authoritative(0, 0, 0),
        &observed,
    ))
    .unwrap();
    assert_eq!(body["behavioral_status"], "unavailable");
    assert_eq!(body["diagnostic"], "upstream");

    for (diagnostic, wire) in [
        (AnalyticsDiagnostic::Disabled, "disabled"),
        (AnalyticsDiagnostic::Configuration, "configuration"),
        (AnalyticsDiagnostic::Authentication, "authentication"),
        (AnalyticsDiagnostic::Timeout, "timeout"),
        (AnalyticsDiagnostic::Upstream, "upstream"),
        (AnalyticsDiagnostic::MalformedResponse, "malformed_response"),
        (
            AnalyticsDiagnostic::InsufficientCoverage,
            "insufficient_coverage",
        ),
    ] {
        assert_eq!(serde_json::to_value(diagnostic).unwrap(), json!(wire));
        let mut partial = available();
        partial.availability = BehavioralAvailability::Partial;
        partial.event_series = ProviderComponent::unavailable(diagnostic);
        partial.diagnostic = Some(diagnostic);
        let body = serde_json::to_value(usage::compose(
            UsageDays::Thirty,
            now() - Duration::days(30),
            now(),
            authoritative(1, 1, 1),
            &partial,
        ))
        .unwrap();
        assert_eq!(body["behavioral_status"], "partial");
        assert_eq!(body["diagnostic"], json!(wire));
        assert_eq!(
            body["behavioral"]["visitors"], 10,
            "a partial window keeps the components it did resolve"
        );
    }

    // The canonical demo identities are the ones Python excludes.
    assert_eq!(usage::DEMO_MARKET_ORGANIZATION_IDS.len(), 14);
    assert!(
        usage::DEMO_MARKET_ORGANIZATION_IDS
            .contains(&Uuid::parse_str("7cc77115-0a9f-4ec4-8c74-05aa10050111").unwrap())
    );
}

/// c10: the mounted router refuses an anonymous request before any database
/// work and answers a non-GET method as not allowed.
#[tokio::test]
async fn p4_t17a_c10() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let url = "postgres://offline_app:offline_password@127.0.0.1:1/offline_test";
    let read = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(url)
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(url)
        .unwrap();
    let state = AppState::new(
        Config {
            kyc_execution_gate_enabled: true,
            environment: "test".into(),
            release_sha: compiled_sha().into(),
            migration_revision: "kycg_20261014_staging".into(),
            database_url: url.into(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 20,
            configured_max_connections: 100,
        },
        Pools { read, health },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let app = usage::routes(usage::UsageAuthority::new(
        auth::VerificationSettings::new(
            "usage-component-test-secret-at-least-32-characters".into(),
            None,
            "usage-test".into(),
            "usage-test".into(),
        ),
        auth::limits::SharedLimits::default(),
        None,
    ))
    .with_state(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(usage::PATH)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["detail"]["code"], "INVALID_CREDENTIALS");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(usage::PATH)
                .header("authorization", "Bearer not-a-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
    };
    use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
    use tower::ServiceExt;
    use usage::{BehavioralFuture, BehavioralUsageSource, UsageWindow};
    use uuid::Uuid;
    use verdaxis_read::{
        auth::VerificationSettings, auth::limits::SharedLimits, runtime::db::Pools,
    };

    const SECRET: &str = "usage-component-test-secret-at-least-32-characters";
    const PATH: &str = usage::PATH;

    fn verification() -> VerificationSettings {
        VerificationSettings::new(
            SECRET.into(),
            None,
            "usage-test".into(),
            "usage-test".into(),
        )
    }

    /// The accepted CF4_03 behavioral input shape, served from a fixed value.
    struct Stub {
        calls: AtomicUsize,
        windows: Mutex<Vec<UsageWindow>>,
        window: BehavioralWindow,
    }

    impl Stub {
        fn new(window: BehavioralWindow) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                windows: Mutex::new(Vec::new()),
                window,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn windows(&self) -> Vec<UsageWindow> {
            self.windows.lock().unwrap().clone()
        }
    }

    impl BehavioralUsageSource for Stub {
        fn window(&self, window: UsageWindow) -> BehavioralFuture<'_> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.windows.lock().unwrap().push(window);
            let reply = self.window.clone();
            Box::pin(async move { reply })
        }
    }

    struct Fixture {
        admin: PgConnection,
        pool: sqlx::PgPool,
        runtime: String,
        schema: String,
        users: [Uuid; 7],
    }

    impl Fixture {
        async fn new() -> Self {
            let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL");
            let runtime =
                std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
            let m = url::Url::parse(&migrator).unwrap();
            let r = url::Url::parse(&runtime).unwrap();
            assert_eq!(m.host_str(), Some("127.0.0.1"));
            assert_eq!(r.host_str(), m.host_str());
            assert_eq!(r.port(), m.port());
            assert_eq!(r.path(), m.path());
            assert!(r.path().ends_with("_test"));
            assert_eq!(r.username(), "rust_app");
            assert_ne!(m.username(), r.username());
            let mut admin = PgConnection::connect(&migrator).await.unwrap();
            let schema = format!("p4_usage_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema},public"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(
                "CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid,role text,status text,\
                 email_verified boolean NOT NULL DEFAULT true,\
                 must_change_password boolean NOT NULL DEFAULT false,\
                 password_changed_at timestamptz,authentication_revoked_at timestamptz,\
                 created_at timestamptz NOT NULL DEFAULT now(),last_login timestamptz); \
                 CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid,\
                 created_at timestamptz NOT NULL)",
            )
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
                 GRANT SELECT,UPDATE ON {schema}.users TO rust_app; \
                 GRANT SELECT ON {schema}.orderbook_orders TO rust_app"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .after_connect({
                    let search = format!("SET search_path TO {schema},public");
                    move |connection, _| {
                        let search = search.clone();
                        Box::pin(async move {
                            sqlx::raw_sql(sqlx::AssertSqlSafe(search))
                                .execute(connection)
                                .await?;
                            Ok(())
                        })
                    }
                })
                .connect(&runtime)
                .await
                .unwrap();
            Self {
                admin,
                pool,
                runtime,
                schema,
                users: std::array::from_fn(|_| Uuid::new_v4()),
            }
        }

        async fn insert_user(
            &mut self,
            index: usize,
            role: &str,
            created_at: DateTime<Utc>,
            last_login: Option<DateTime<Utc>>,
        ) {
            sqlx::query(
                "INSERT INTO users(id,role,status,created_at,last_login) VALUES($1,$2,'APPROVED',$3,$4)",
            )
            .bind(self.users[index])
            .bind(role)
            .bind(created_at)
            .bind(last_login)
            .execute(&mut self.admin)
            .await
            .unwrap();
        }

        async fn set_organization(&mut self, index: usize, organization: Option<Uuid>) {
            sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
                .bind(organization)
                .bind(self.users[index])
                .execute(&mut self.admin)
                .await
                .unwrap();
        }

        async fn insert_order(&mut self, organization: Uuid, created_at: DateTime<Utc>) {
            sqlx::query(include_str!("fixtures/t17a_order_insert.sql"))
                .bind(Uuid::new_v4())
                .bind(organization)
                .bind(created_at)
                .execute(&mut self.admin)
                .await
                .unwrap();
        }

        fn app(&mut self, source: Option<Arc<dyn BehavioralUsageSource>>) -> Router {
            self.app_with_limits(source, SharedLimits::default())
        }

        fn app_with_limits(
            &mut self,
            source: Option<Arc<dyn BehavioralUsageSource>>,
            limits: SharedLimits,
        ) -> Router {
            build_app(self.pool.clone(), &self.runtime, source, limits)
        }

        async fn finish(mut self) {
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
    }

    fn build_app(
        pool: sqlx::PgPool,
        runtime: &str,
        source: Option<Arc<dyn BehavioralUsageSource>>,
        limits: SharedLimits,
    ) -> Router {
        let health = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(runtime)
            .unwrap();
        let state = AppState::new(
            Config {
                kyc_execution_gate_enabled: true,
                environment: "test".into(),
                release_sha: compiled_sha().into(),
                migration_revision: "kycg_20261014_staging".into(),
                database_url: runtime.into(),
                db_session_time_zone: "UTC".into(),
                prepared_budget: None,
                bind: "127.0.0.1:0".parse().unwrap(),
                read_admission_slots: 4,
                pool_max: 5,
                python_budget: 20,
                configured_max_connections: 100,
            },
            Pools { read: pool, health },
            MarketPolicy {
                source_sha: compiled_sha().into(),
                products: vec![],
                points: vec![],
            },
        )
        .unwrap();
        usage::routes(usage::UsageAuthority::new(verification(), limits, source)).with_state(state)
    }

    fn token(id: Uuid) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({"sub":id,"type":"access","iss":"usage-test","aud":"usage-test",
                "jti":Uuid::new_v4(),"iat":Utc::now().timestamp(),
                "exp":(Utc::now()+Duration::hours(1)).timestamp()}),
            &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    async fn send(
        app: &Router,
        path: &str,
        bearer: Option<&str>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut builder = Request::builder().method("GET").uri(path);
        if let Some(bearer) = bearer {
            builder = builder.header("authorization", format!("Bearer {bearer}"));
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}));
        (status, body, retry_after)
    }

    fn quota_keys(identity: &str) -> verdaxis_read::auth::limits::BusinessLimitKeys<'_> {
        verdaxis_read::auth::limits::BusinessLimitKeys {
            client: identity,
            user: Some(identity),
            organization: None,
            token: Some(identity),
        }
    }

    /// c05: the authoritative counts use a half-open UTC window and exclude
    /// demo, admin and non-market users and organizations.
    #[tokio::test]
    async fn p4_t17a_c05() {
        let mut fixture = Fixture::new().await;
        let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
        let end = start + Duration::days(7);
        let before = start - Duration::seconds(1);
        let demo = usage::DEMO_MARKET_ORGANIZATION_IDS[0];
        let market = Uuid::new_v4();
        let other_market = Uuid::new_v4();
        let non_market = Uuid::new_v4();

        fixture.insert_user(0, "ADMIN", before, None).await;
        fixture.insert_user(1, "BUYER", start, None).await;
        fixture.insert_user(2, "BUYER", end, None).await;
        fixture.insert_user(3, "BUYER", before, Some(start)).await;
        fixture.insert_user(4, "SUPPLIER", before, Some(end)).await;
        fixture
            .insert_user(5, "SUPPLIER", before, Some(before))
            .await;
        fixture.insert_user(6, "ADMIN", start, Some(start)).await;
        for (index, organization) in [
            None,
            Some(market),
            Some(demo),
            Some(non_market),
            Some(other_market),
            None,
            Some(non_market),
        ]
        .into_iter()
        .enumerate()
        {
            fixture.set_organization(index, organization).await;
        }
        fixture.insert_order(market, start).await;
        fixture.insert_order(other_market, start).await;
        fixture.insert_order(market, end).await;
        fixture.insert_order(demo, start).await;

        let mut connection = fixture.pool.acquire().await.unwrap();
        let counts = usage::authoritative_counts(&mut connection, start, end)
            .await
            .unwrap();
        assert_eq!(
            counts,
            authoritative(1, 1, 2),
            "only the buyer created at start, the login at start and the two \
             real market organizations ordering at start qualify"
        );
        drop(connection);
        fixture.finish().await;
    }

    /// c02: anonymous, non-admin, invalid-period and quota refusals use the
    /// structured envelope; an unavailable provider still answers 200.
    #[tokio::test]
    async fn p4_t17a_c02() {
        let mut fixture = Fixture::new().await;
        let now = Utc::now();
        fixture
            .insert_user(0, "ADMIN", now - Duration::days(30), None)
            .await;
        fixture
            .insert_user(
                1,
                "BUYER",
                now - Duration::days(2),
                Some(now - Duration::days(1)),
            )
            .await;
        fixture
            .insert_user(
                2,
                "BUYER",
                now - Duration::days(1),
                Some(now - Duration::days(1)),
            )
            .await;
        let market = Uuid::new_v4();
        fixture.set_organization(1, Some(market)).await;
        fixture.insert_order(market, now - Duration::days(1)).await;
        fixture
            .insert_order(
                usage::DEMO_MARKET_ORGANIZATION_IDS[0],
                now - Duration::days(1),
            )
            .await;

        let admin = token(fixture.users[0]);
        let buyer = token(fixture.users[1]);
        let stub = Stub::new(unavailable(AnalyticsDiagnostic::Timeout));
        let app = fixture.app(Some(stub.clone()));

        let (status, body, _) = send(&app, PATH, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["detail"]["code"], "INVALID_CREDENTIALS");

        let (status, body, _) = send(&app, PATH, Some(&buyer)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["detail"]["code"], "ADMIN_REQUIRED");

        let (status, body, _) = send(&app, &format!("{PATH}?days=8"), Some(&admin)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["detail"]["code"], "ANALYTICS_QUERY_INVALID");

        let (status, body, retry_after) =
            send(&app, &format!("{PATH}?days=30"), Some(&admin)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(retry_after, None);
        assert_eq!(body["days"], 30);
        assert_eq!(body["behavioral_status"], "unavailable");
        assert_eq!(body["diagnostic"], "timeout");
        assert_eq!(
            body["authoritative"],
            json!({
                "registrations": 2,
                "users_logging_in": 2,
                "order_placing_organizations": 1,
            })
        );
        let text = body.to_string().to_lowercase();
        for forbidden in [
            "example.test",
            "@",
            "price",
            "quantity",
            "order_id",
            "trade_id",
        ] {
            assert!(!text.contains(forbidden), "exposed {forbidden}");
        }
        assert_eq!(stub.calls(), 1);
        let window = stub.windows()[0];
        assert_eq!(window.end - window.start, Duration::days(30));

        fixture.finish().await;
    }

    /// c06: authority and input validation precede the quota, so every
    /// refusal spends none of the bucket; an exhausted bucket refuses a valid
    /// request with 429 and no provider call.
    #[tokio::test]
    async fn p4_t17a_c06() {
        let mut fixture = Fixture::new().await;
        let now = Utc::now();
        fixture
            .insert_user(0, "ADMIN", now - Duration::days(30), None)
            .await;
        fixture
            .insert_user(1, "BUYER", now - Duration::days(30), None)
            .await;
        let admin = token(fixture.users[0]);
        let buyer = token(fixture.users[1]);
        let stub = Stub::new(unavailable(AnalyticsDiagnostic::Disabled));

        // The route reads its own wall-clock minute. Each attempt fills the
        // bucket at that minute and repeats if the minute rolls over mid-test.
        let mut observed = None;
        for _ in 0..5 {
            let minute = Utc::now().timestamp() / 60;
            let limits = SharedLimits::default();
            let identity = usage::token_identity(Some(&format!("Bearer {admin}")));
            for _ in 0..30 {
                limits
                    .check_business(
                        "GET",
                        PATH,
                        quota_keys(&identity),
                        u64::try_from(minute * 60).unwrap(),
                    )
                    .unwrap();
            }
            let app = fixture.app_with_limits(Some(stub.clone()), limits);

            // Non-admin authority precedes the period, and both precede quota.
            let buyer_refusal = send(&app, &format!("{PATH}?days=8"), Some(&buyer)).await;
            // An invalid period is refused before quota, even with none left.
            let period_refusal = send(&app, &format!("{PATH}?days=8"), Some(&admin)).await;
            // A valid request meets the exhausted bucket.
            let quota_refusal = send(&app, PATH, Some(&admin)).await;
            if Utc::now().timestamp() / 60 != minute {
                continue;
            }
            observed = Some((buyer_refusal, period_refusal, quota_refusal));
            break;
        }
        let ((status, body, _), (status2, body2, _), (status3, body3, retry_after)) =
            observed.expect("a stable wall-clock minute");
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["detail"]["code"], "ADMIN_REQUIRED");
        assert_eq!(status2, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body2["detail"]["code"], "ANALYTICS_QUERY_INVALID");
        assert_eq!(status3, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body3["detail"]["code"], "RATE_LIMITED");
        let seconds: u32 = retry_after
            .as_deref()
            .expect("429 carries Retry-After")
            .parse()
            .unwrap();
        assert!(
            (1..=60).contains(&seconds),
            "bounded Retry-After: {seconds}"
        );
        assert_eq!(
            stub.calls(),
            0,
            "every refusal spends neither quota nor a provider call"
        );

        fixture.finish().await;
    }

    /// c07: each accepted period requests exactly its rolling UTC window.
    #[tokio::test]
    async fn p4_t17a_c07() {
        let mut fixture = Fixture::new().await;
        let now = Utc::now();
        fixture
            .insert_user(0, "ADMIN", now - Duration::days(120), None)
            .await;
        let admin = token(fixture.users[0]);
        let stub = Stub::new(unavailable(AnalyticsDiagnostic::Disabled));
        let app = fixture.app(Some(stub.clone()));

        for (days, duration) in [
            (7, Duration::days(7)),
            (30, Duration::days(30)),
            (90, Duration::days(90)),
        ] {
            let (status, body, _) = send(&app, &format!("{PATH}?days={days}"), Some(&admin)).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["days"], days);
            assert_eq!(body["behavioral_status"], "unavailable");
            let window = *stub.windows().last().unwrap();
            assert_eq!(window.end - window.start, duration);
            let start: DateTime<Utc> = body["period_start"].as_str().unwrap().parse().unwrap();
            let end: DateTime<Utc> = body["period_end"].as_str().unwrap().parse().unwrap();
            assert_eq!(end - start, duration);
            assert_eq!((window.start, window.end), (start, end));
        }
        assert_eq!(stub.calls(), 3);

        // The default period is thirty days.
        let (status, body, _) = send(&app, PATH, Some(&admin)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["days"], 30);
        fixture.finish().await;
    }
}
