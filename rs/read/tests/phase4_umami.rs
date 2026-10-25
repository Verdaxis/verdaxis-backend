//! Task 16 suite entry: the adapter and event-policy modules are included
//! below their own test directory.
//!
//! Every provider destination in this suite is a numeric-loopback HTTP stub.
//! The suite never contacts a real Umami host and never reads or prints a real
//! credential; the test credential is obviously fake.
//!
//! The adapter compiles against the accepted Task 15 source modules.
//! No query, policy or provider-envelope test double is used.

pub use verdaxis_read::*;

// Production modules sit at the test-crate root, where `#[path]` resolves
// relative to this file (the same form the other Phase 4 provider suites use).
// The `analytics` and `providers` modules re-export them, so `crate::analytics
// ::events`, `crate::analytics::behavioral` and `crate::providers::umami`
// resolve exactly as they will after integration.
#[path = "../src/analytics/events.rs"]
pub mod events;

#[path = "../src/analytics/behavioral.rs"]
pub mod behavioral;

#[path = "../src/providers/umami.rs"]
pub mod umami;

#[path = "../src/analytics/types.rs"]
pub mod types;
#[path = "../src/analytics/query.rs"]
pub mod query;
#[path = "../src/analytics/policy.rs"]
pub mod policy;

pub mod analytics {
    pub use crate::{behavioral, events, policy, query, types};
}

pub mod providers {
    pub use crate::umami;
}

// ---------------------------------------------------------------------------
// Cases.
// ---------------------------------------------------------------------------

mod cases {
    use super::*;

    use std::{
        collections::BTreeMap,
        future::Future,
        net::SocketAddr,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use axum::{
        Router,
        body::{Body, Bytes},
        response::{IntoResponse, Response},
    };
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use serde_json::{Value, json};
    use zeroize::Zeroizing;

    use super::analytics::{
        types::{BehavioralAvailability, BehavioralWindow},
        events::{
            AllowedFrontendEvent, AllowedProperty, AllowedPropertySet, AllowedPropertyValue,
            AllowedServerEvent, DomainCatalog, EntryPage, ReferrerCategory, RegisteredEvent,
            ServerEvent,
        },
        query::AnalyticsQuery,
        types::{AnalyticsActivity, AnalyticsAudience},
        types::{AnalyticsDiagnostic, AnalyticsSourceStatus, ComponentStatus},
    };
    use super::providers::umami::{UmamiClient, UmamiConfig, UmamiPolicy};
    use verdaxis_read::auth::limits::{BusinessLimitKeys, SharedLimits};
    use verdaxis_read::market::terms::ISO_3166_1_ALPHA_2;

    const WEBSITE: &str = "website-1";
    const USERNAME: &str = "api-user";
    const FAKE_PASSWORD: &str = "not-a-real-umami-password";

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("test timestamp")
    }

    fn catalog() -> Arc<DomainCatalog> {
        Arc::new(DomainCatalog::new(
            ["B30", "BIO_METHANOL"],
            ["Singapore", "Rotterdam"],
            ISO_3166_1_ALPHA_2.iter().copied(),
        ))
    }

    fn query() -> AnalyticsQuery {
        AnalyticsQuery {
            start: at("2026-06-01T00:00:00Z"),
            end: at("2026-07-01T00:00:00Z"),
            previous: None,
            compare: false,
            audience: AnalyticsAudience::All,
            activity: AnalyticsActivity::Live,
            product_id: None,
            delivery_point_id: None,
            availability_window: None,
        }
    }

    fn config(address: SocketAddr) -> UmamiConfig {
        UmamiConfig {
            enabled: true,
            base_url: Some(format!("http://{address}")),
            website_id: Some(WEBSITE.into()),
            api_username: Some(USERNAME.into()),
            api_password: Some(Zeroizing::new(FAKE_PASSWORD.into())),
            policy: UmamiPolicy::default(),
            catalog: catalog(),
        }
    }

    fn client(address: SocketAddr) -> UmamiClient {
        UmamiClient::new(config(address)).expect("loopback client")
    }

    fn destination_set() -> AllowedPropertySet {
        AllowedPropertySet::new([(
            RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
            AllowedProperty::Destination,
        )])
        .expect("registered pair")
    }

    // ---------------------------------------------------------------------------
    // Stub HTTP server. The only address any test client receives is loopback.
    // ---------------------------------------------------------------------------

    #[derive(Clone, Debug)]
    struct RecordedRequest {
        method: String,
        path: String,
        query: Vec<(String, String)>,
        headers: Vec<(String, String)>,
    }

    impl RecordedRequest {
        fn param(&self, name: &str) -> Option<&str> {
            self.query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }
    }

    struct Reply {
        status: u16,
        body: Vec<u8>,
        stream: Option<Vec<Vec<u8>>>,
        headers: Vec<(String, String)>,
    }

    impl Reply {
        fn json(status: u16, value: Value) -> Self {
            Self {
                status,
                body: serde_json::to_vec(&value).expect("json reply"),
                stream: None,
                headers: vec![("content-type".into(), "application/json".into())],
            }
        }

        fn body(status: u16, body: impl Into<Vec<u8>>) -> Self {
            Self {
                status,
                body: body.into(),
                stream: None,
                headers: Vec::new(),
            }
        }

        fn stream(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                status: 200,
                body: Vec::new(),
                stream: Some(chunks),
                headers: vec![("content-type".into(), "application/json".into())],
            }
        }

        fn redirect(location: &str) -> Self {
            Self {
                status: 302,
                body: Vec::new(),
                stream: None,
                headers: vec![("location".into(), location.into())],
            }
        }
    }

    type Handler =
        Arc<dyn Fn(RecordedRequest) -> Pin<Box<dyn Future<Output = Reply> + Send>> + Send + Sync>;

    #[derive(Clone)]
    struct Stub {
        handler: Handler,
        seen: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    impl Stub {
        fn new<F, Fut>(handler: F) -> Self
        where
            F: Fn(RecordedRequest) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = Reply> + Send + 'static,
        {
            Self {
                handler: Arc::new(move |request| Box::pin(handler(request))),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn seen(&self) -> Vec<RecordedRequest> {
            self.seen.lock().expect("stub log").clone()
        }

        fn count(&self, predicate: impl Fn(&RecordedRequest) -> bool) -> usize {
            self.seen()
                .iter()
                .filter(|request| predicate(request))
                .count()
        }

        async fn respond(&self, request: axum::extract::Request) -> Response {
            let (parts, _body) = request.into_parts();
            let recorded = RecordedRequest {
                method: parts.method.to_string(),
                path: parts.uri.path().to_string(),
                query: parts
                    .uri
                    .query()
                    .map(|query| {
                        url::form_urlencoded::parse(query.as_bytes())
                            .into_owned()
                            .collect()
                    })
                    .unwrap_or_default(),
                headers: parts
                    .headers
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.as_str().to_ascii_lowercase(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect(),
            };
            self.seen.lock().expect("stub log").push(recorded.clone());
            let reply = (self.handler)(recorded).await;
            let mut builder = Response::builder().status(reply.status);
            for (key, value) in reply.headers {
                builder = builder.header(key, value);
            }
            match reply.stream {
                Some(chunks) => builder
                    .body(Body::from_stream(futures_util::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok::<_, std::io::Error>(Bytes::from(chunk))),
                    )))
                    .expect("streamed reply"),
                None => builder.body(Body::from(reply.body)).expect("reply"),
            }
        }
    }

    async fn serve(stub: Stub) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let app = Router::new().fallback(move |request: axum::extract::Request| {
            let stub = stub.clone();
            async move { stub.respond(request).await.into_response() }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback listener");
        let address = listener.local_addr().expect("bound address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("stub server");
        });
        (address, server)
    }

    /// The healthy upstream reply used unless a test overrides the request.
    fn healthy_reply(request: &RecordedRequest) -> Option<Reply> {
        let path = request.path.as_str();
        if path == "/api/auth/login" {
            return Some(Reply::json(200, json!({"token": "token-1"})));
        }
        if path == format!("/api/websites/{WEBSITE}/stats") {
            return Some(Reply::json(
                200,
                json!({"visitors": 12, "visits": 8, "pageviews": 30, "totaltime": 160}),
            ));
        }
        if path == format!("/api/websites/{WEBSITE}/pageviews") {
            return Some(Reply::json(
                200,
                json!({"pageviews": [], "sessions": [
                    {"x": "2026-06-10 00:00:00", "y": 4},
                    {"x": "2026-06-11", "y": 1},
                    {"x": "2026-05-31", "y": 9}
                ]}),
            ));
        }
        if path == format!("/api/websites/{WEBSITE}/events/series") {
            return Some(Reply::json(
                200,
                json!([
                    {"x": "signup_started", "t": "2026-06-10 00:00:00", "y": 4},
                    {"x": "signup_started", "t": "2026-06-10 00:00:00", "y": 2},
                    {"x": "unknown_event", "t": "2026-06-10 00:00:00", "y": 99},
                    {"x": "platform_navigation", "t": "2026-06-11", "y": 1}
                ]),
            ));
        }
        if path == format!("/api/websites/{WEBSITE}/metrics") {
            return Some(match request.param("type") {
                Some("entry") => Reply::json(
                    200,
                    json!([
                        {"x": "/marketplace?tab=1", "y": 7},
                        {"x": "/", "y": 1}
                    ]),
                ),
                Some("referrer") => Reply::json(
                    200,
                    json!([
                        {"x": "https://www.google.com/search?q=x", "y": 5},
                        {"x": "https://facebook.com/post", "y": 2}
                    ]),
                ),
                _ => Reply::json(500, json!({})),
            });
        }
        if path == format!("/api/websites/{WEBSITE}/event-data/events") {
            return Some(Reply::json(
                200,
                json!([
                    {"eventName": "platform_navigation", "propertyName": "destination",
                     "dataType": 1, "propertyValue": "marketplace", "total": 7},
                    {"eventName": "platform_navigation", "propertyName": "destination",
                     "dataType": 1, "propertyValue": "map", "total": 1},
                    {"eventName": "platform_navigation", "propertyName": "destination",
                     "dataType": 1, "propertyValue": "not-a-destination", "total": 9},
                    {"eventName": "platform_navigation", "propertyName": "email",
                     "dataType": 1, "propertyValue": "someone@example.com", "total": 4}
                ]),
            ));
        }
        None
    }

    fn stub_with<F>(overrides: F) -> Stub
    where
        F: Fn(&RecordedRequest) -> Option<Reply> + Send + Sync + 'static,
    {
        Stub::new(move |request| {
            std::future::ready(
                overrides(&request)
                    .or_else(|| healthy_reply(&request))
                    .unwrap_or_else(|| Reply::body(404, Vec::new())),
            )
        })
    }

    fn healthy() -> Stub {
        stub_with(|_| None)
    }

    fn is_auth(request: &RecordedRequest) -> bool {
        request.path == "/api/auth/login"
    }

    fn is_stats(request: &RecordedRequest) -> bool {
        request.path.ends_with("/stats")
    }

    #[tokio::test]
    async fn p4_t16_c73() {
        let properties: policy::AllowedPropertySet = destination_set();
        let (address, server) = serve(healthy()).await;
        let window: types::BehavioralWindow = client(address).window(&query(), properties).await;
        server.abort();
        assert!(window.validate().is_ok());
        let _: &types::CoreMetrics = window.core.data().expect("core");
        let _: &[types::AllowedEventAggregate] = window.event_totals.data().unwrap();
        assert!(window.core.is_coherent());
    }

    // ---------------------------------------------------------------------------
    // c01: the caller's property request is a closed, bounded set.
    // ---------------------------------------------------------------------------

    /// `analytics_rejects_unknown_property_values`.
    #[test]
    fn p4_t16_c01() {
        let valid = AllowedPropertySet::new([
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
                AllowedProperty::Destination,
            ),
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
                AllowedProperty::Destination,
            ),
            (
                RegisteredEvent::Server(AllowedServerEvent::OrderCreated),
                AllowedProperty::CanonicalProduct,
            ),
        ])
        .expect("registered pairs");
        assert_eq!(valid.pairs().len(), 2, "duplicate pairs collapse");
        assert_eq!(valid.events().len(), 2);

        // A property that exists, but not for this event.
        let refused = AllowedPropertySet::new([(
            RegisteredEvent::Server(AllowedServerEvent::OrderCreated),
            AllowedProperty::Cta,
        )])
        .map_err(AllowedPropertySet::refusal)
        .expect_err("cta is not an order_created property");
        assert_eq!(refused.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused.detail["code"], "ANALYTICS_PROPERTY_INVALID");

        // More than twelve distinct events never reaches the provider.
        let thirteen: Vec<_> = RegisteredEvent::all()
            .iter()
            .take(13)
            .map(|event| (*event, policy::allowed_properties(*event)[0]))
            .collect();
        let refused = AllowedPropertySet::new(thirteen).map_err(AllowedPropertySet::refusal).expect_err("13 events");
        assert_eq!(refused.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refused.detail["code"], "ANALYTICS_PROPERTY_INVALID");

        // Twelve distinct events are accepted.
        let twelve: Vec<_> = RegisteredEvent::all()
            .iter()
            .take(12)
            .map(|event| (*event, policy::allowed_properties(*event)[0]))
            .collect();
        assert!(AllowedPropertySet::new(twelve).is_ok());

        // Unknown free-text identifiers and commercial names are not properties.
        for name in [
            "email",
            "name",
            "organization_name",
            "order_id",
            "trade_id",
            "counterparty",
            "price",
            "quantity",
            "total_value",
            "user_agent",
            "hostname",
            "ip_address",
        ] {
            assert!(
                AllowedProperty::from_name(name).is_none(),
                "{name} must not be a property"
            );
        }
        assert_eq!(
            AllowedProperty::from_name("canonical_product"),
            Some(AllowedProperty::CanonicalProduct)
        );

        // Closed value domains: unknown and wrong-case values are dropped.
        let event = RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation);
        let catalog = catalog();
        assert!(
            AllowedPropertyValue::parse(event, AllowedProperty::Destination, "home", &catalog)
                .is_some()
        );
        assert!(
            AllowedPropertyValue::parse(event, AllowedProperty::Destination, "HOME", &catalog)
                .is_none()
        );
        assert!(
            AllowedPropertyValue::parse(
                event,
                AllowedProperty::Destination,
                "somewhere-private",
                &catalog
            )
            .is_none()
        );
    }

    // ---------------------------------------------------------------------------
    // c02: identifiers and commercial values never enter an event or a result.
    // ---------------------------------------------------------------------------

    /// `event_policy_filters_identifiers_and_commercial_values`.
    #[tokio::test]
    async fn p4_t16_c02() {
        let order = RegisteredEvent::Server(AllowedServerEvent::OrderCreated);
        let names: Vec<_> = policy::allowed_properties(order)
            .iter()
            .map(|property| property.name())
            .collect();
        assert_eq!(
            names,
            [
                "role",
                "side",
                "canonical_product",
                "delivery_point",
                "availability_window"
            ]
        );
        let event = AllowedServerEvent::OrderCreated;
        let catalog = catalog();
        let field = |property, raw| {
            AllowedPropertyValue::parse(RegisteredEvent::Server(event), property, raw, &catalog)
                .expect("registered value")
        };
        let server_event = ServerEvent::new(
            event,
            [
                (AllowedProperty::Role, field(AllowedProperty::Role, "BUYER")),
                (AllowedProperty::Side, field(AllowedProperty::Side, "BID")),
                (
                    AllowedProperty::CanonicalProduct,
                    field(AllowedProperty::CanonicalProduct, "B30"),
                ),
                (
                    AllowedProperty::DeliveryPoint,
                    field(AllowedProperty::DeliveryPoint, "Singapore"),
                ),
                (
                    AllowedProperty::AvailabilityWindow,
                    field(AllowedProperty::AvailabilityWindow, "2026-Q3"),
                ),
            ],
        )
        .expect("order event");
        let data: BTreeMap<&'static str, String> = server_event.data().into_iter().collect();
        assert_eq!(data.len(), 5);
        assert!(!data.contains_key("email"));
        assert!(!data.contains_key("order_id"));
        assert!(!data.contains_key("price"));
        assert!(!data.contains_key("quantity"));

        let refused = ServerEvent::new(
            event,
            [(
                AllowedProperty::Cta,
                AllowedPropertyValue::parse(
                    RegisteredEvent::Frontend(AllowedFrontendEvent::LandingCtaClicked),
                    AllowedProperty::Cta,
                    "register",
                    &catalog,
                )
                .expect("valid cta value"),
            )],
        )
        .expect_err("cta is not an order event property");
        assert_eq!(refused.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);

        // An upstream row that names an unrequested identifier property or an
        // unknown value is dropped; it is not reflected or fabricated as zero.
        let (address, server) = serve(healthy()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.availability, BehavioralAvailability::Available);
        let results = &window.property_results[0].result;
        let rows = results.data().expect("typed rows");
        assert_eq!(
            rows.len(),
            2,
            "unknown value dropped; email is not a property"
        );
        assert_eq!(rows[0].value.as_str(), "marketplace");
        assert_eq!(rows[0].count.count, Some(7));
        assert_eq!(rows[1].value.as_str(), "map");
        assert!(rows[1].count.suppressed, "a single visitor is suppressed");
        assert_eq!(rows[1].count.count, None);
        let serialized = serde_json::to_string(&window).expect("serializable window");
        for forbidden in ["someone@example.com", "email", "not-a-destination"] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
    }

    // ---------------------------------------------------------------------------
    // c03: suppression is privacy, never a fabricated zero.
    // ---------------------------------------------------------------------------

    /// `analytics_behavioral_small_cells_are_suppressed`.
    #[tokio::test]
    async fn p4_t16_c03() {
        let stub = stub_with(|request| {
            if request.path.ends_with("/events/series") {
                return Some(Reply::json(
                    200,
                    json!([
                        {"x": "signup_started", "t": "2026-06-10", "y": 1},
                        {"x": "platform_navigation", "t": "2026-06-10", "y": 2},
                        {"x": "listing_opened", "t": "2026-06-10", "y": 3},
                        {"x": "tutorial_started", "t": "2026-06-10", "y": 0}
                    ]),
                ));
            }
            if request.path.ends_with("/pageviews") {
                return Some(Reply::json(
                    200,
                    json!({"pageviews": [], "sessions": [
                        {"x": "2026-06-10", "y": 1},
                        {"x": "2026-06-11", "y": 2},
                        {"x": "2026-06-12", "y": 3},
                        {"x": "2026-06-13", "y": 0}
                    ]}),
                ));
            }
            if request.path.ends_with("/event-data/events") {
                return Some(Reply::json(
                    200,
                    json!([
                        {"propertyName": "destination", "propertyValue": "home", "total": 1},
                        {"propertyName": "destination", "propertyValue": "map", "total": 2},
                        {"propertyName": "destination", "propertyValue": "trades", "total": 3}
                    ]),
                ));
            }
            None
        });
        let (address, server) = serve(stub).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();

        let totals = window.event_totals.into_data().expect("event totals");
        let by_name: BTreeMap<_, _> = totals
            .iter()
            .map(|row| (row.event.as_str(), row.count.clone()))
            .collect();
        assert_eq!(by_name["signup_started"].count, None);
        assert!(by_name["signup_started"].suppressed);
        assert!(by_name["platform_navigation"].suppressed);
        assert_eq!(by_name["listing_opened"].count, Some(3));
        assert!(!by_name["listing_opened"].suppressed);
        assert_eq!(by_name["tutorial_started"].count, Some(0));
        assert!(!by_name["tutorial_started"].suppressed, "zero is genuine");

        let daily = window.daily_visitors.into_data().expect("daily visitors");
        assert_eq!(daily.len(), 4);
        assert!(daily[0].visitors.suppressed);
        assert!(daily[1].visitors.suppressed);
        assert_eq!(daily[2].visitors.count, Some(3));
        assert_eq!(daily[3].visitors.count, Some(0));

        let rows = window.property_results[0]
            .result
            .data()
            .expect("rows");
        assert_eq!(rows[0].value.as_str(), "trades");
        assert_eq!(rows[0].count.count, Some(3));
        assert!(rows[1].count.suppressed);
        assert!(rows[2].count.suppressed);
    }

    // ---------------------------------------------------------------------------
    // c04: every failure is a typed unavailable window with no fabricated data.
    // ---------------------------------------------------------------------------

    /// `umami_failure_is_typed_unavailable`.
    #[tokio::test]
    async fn p4_t16_c04() {
        let cases: Vec<(UmamiConfig, AnalyticsDiagnostic)> = vec![
            (
                UmamiConfig {
                    enabled: false,
                    ..config("127.0.0.1:9".parse().unwrap())
                },
                AnalyticsDiagnostic::Disabled,
            ),
            (
                UmamiConfig {
                    api_password: None,
                    ..config("127.0.0.1:9".parse().unwrap())
                },
                AnalyticsDiagnostic::Configuration,
            ),
            (
                UmamiConfig {
                    website_id: None,
                    ..config("127.0.0.1:9".parse().unwrap())
                },
                AnalyticsDiagnostic::Configuration,
            ),
            (
                UmamiConfig {
                    base_url: None,
                    ..config("127.0.0.1:9".parse().unwrap())
                },
                AnalyticsDiagnostic::Configuration,
            ),
        ];
        for (config, expected) in cases {
            let client = UmamiClient::new(config).expect("no network configuration is valid");
            let window = client.window(&query(), destination_set()).await;
            assert_eq!(window.availability, BehavioralAvailability::Unavailable);
            assert_eq!(window.diagnostic, Some(expected));
            for component in [
                serde_json::to_value(&window.core).unwrap(),
                serde_json::to_value(&window.event_totals).unwrap(),
                serde_json::to_value(&window.daily_visitors).unwrap(),
                serde_json::to_value(&window.event_series).unwrap(),
                serde_json::to_value(&window.top_entries).unwrap(),
                serde_json::to_value(&window.top_referrers).unwrap(),
            ] {
                assert_eq!(component["status"], "unavailable");
                assert_eq!(component["data"], Value::Null);
            }
            assert_eq!(window.property_results.len(), 1);
            assert_eq!(
                window.property_results[0].result.status(),
                ComponentStatus::Unavailable
            );
            assert!(window.property_results[0].result.data().is_none());
            assert_eq!(window.property_results[0].result.diagnostic(), Some(expected));
            assert_eq!(window.coverage.coverage_start, None);
            assert_eq!(window.coverage.coverage_end, None);
        }

        // Timeout.
        let slow = Stub::new(|_| async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Reply::json(200, json!({}))
        });
        let (address, server) = serve(slow).await;
        let window = UmamiClient::new(UmamiConfig {
            policy: UmamiPolicy {
                timeout: Duration::from_millis(50),
            },
            ..config(address)
        })
        .expect("bounded timeout")
        .window(&query(), destination_set())
        .await;
        server.abort();
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Timeout));

        // Upstream status.
        let failing = stub_with(|request| {
            is_stats(request).then(|| Reply::json(500, json!({"detail": "secret upstream text"})))
        });
        let (address, server) = serve(failing).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Upstream));
        let serialized = serde_json::to_string(&window).unwrap();
        assert!(!serialized.contains("secret upstream text"));

        // Malformed authentication response.
        let malformed =
            stub_with(|request| is_auth(request).then(|| Reply::body(200, b"not-json".to_vec())));
        let (address, server) = serve(malformed).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(
            window.diagnostic,
            Some(AnalyticsDiagnostic::MalformedResponse)
        );
    }

    // ---------------------------------------------------------------------------
    // c05: the one-megabyte body cap covers announced and streamed bodies.
    // ---------------------------------------------------------------------------

    /// `umami_body_and_property_rows_are_bounded`.
    #[tokio::test]
    async fn p4_t16_c05() {
        let announced =
            stub_with(|request| is_stats(request).then(|| Reply::body(200, vec![b'x'; 1_000_001])));
        let (address, server) = serve(announced).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(
            window.diagnostic,
            Some(AnalyticsDiagnostic::MalformedResponse)
        );

        let streamed = stub_with(|request| {
            is_stats(request).then(|| Reply::stream(vec![vec![b'x'; 600_000], vec![b'x'; 600_000]]))
        });
        let (address, server) = serve(streamed).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(
            window.diagnostic,
            Some(AnalyticsDiagnostic::MalformedResponse),
            "the streaming cap must not buffer past one megabyte"
        );

        // Exactly one megabyte is inside the cap.
        let exactly = stub_with(|request| {
            is_stats(request).then(|| {
                let mut body = br#"{"visitors":1,"visits":1,"pageviews":1,"totaltime":1}"#.to_vec();
                body.resize(1_000_000, b' ');
                Reply::body(200, body)
            })
        });
        let (address, server) = serve(exactly).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.availability, BehavioralAvailability::Available);
        assert_eq!(window.core.into_data().expect("core").visitors, Some(1));

        let bad_shape =
            stub_with(|request| is_stats(request).then(|| Reply::json(200, json!([1, 2, 3]))));
        let (address, server) = serve(bad_shape).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(
            window.diagnostic,
            Some(AnalyticsDiagnostic::MalformedResponse)
        );
    }

    // ---------------------------------------------------------------------------
    // c06: every collection is bounded.
    // ---------------------------------------------------------------------------

    /// `umami_body_and_property_rows_are_bounded` (row bounds).
    #[tokio::test]
    async fn p4_t16_c06() {
        let long_query = AnalyticsQuery {
            start: at("2026-01-01T00:00:00Z"),
            end: at("2027-02-05T00:00:00Z"),
            ..query()
        };
        let mut sessions = Vec::new();
        let mut series = Vec::new();
        let mut properties = Vec::new();
        let mut entries = Vec::new();
        for day in 0..400 {
            let date =
                chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(day);
            sessions.push(json!({"x": date.to_string(), "y": 3}));
            series.push(json!({"x": "platform_navigation", "t": date.to_string(), "y": 3}));
            entries.push(json!({"x": format!("/marketplace/{day}"), "y": 3}));
        }
        for row in 0..600 {
            properties.push(json!({
                "propertyName": "destination",
                "propertyValue": if row < 12 { "home".to_string() } else { format!("value-{row}") },
                "total": 3
            }));
        }
        let stub = stub_with(move |request| {
            if request.path.ends_with("/pageviews") {
                return Some(Reply::json(
                    200,
                    json!({"pageviews": [], "sessions": sessions}),
                ));
            }
            if request.path.ends_with("/events/series") {
                return Some(Reply::json(200, Value::Array(series.clone())));
            }
            if request.path.ends_with("/metrics") {
                return Some(Reply::json(200, Value::Array(entries.clone())));
            }
            if request.path.ends_with("/event-data/events") {
                return Some(Reply::json(200, Value::Array(properties.clone())));
            }
            None
        });
        let (address, server) = serve(stub).await;
        let window = client(address).window(&long_query, destination_set()).await;
        server.abort();

        assert_eq!(window.availability, BehavioralAvailability::Available);
        assert!(window.daily_visitors.data().unwrap().len() <= 366);
        let series = window.event_series.data().unwrap();
        assert!(series.len() <= 366, "event series points per event");
        let entries = window.top_entries.data().unwrap();
        assert!(entries.len() <= 20);
        let rows = window.property_results[0].result.data().unwrap();
        assert!(rows.len() <= 500, "at most 500 property rows per event");
        assert_eq!(rows.len(), 1, "twelve valid rows aggregate to one value");
        assert_eq!(rows[0].value.as_str(), "home");
        assert_eq!(rows[0].count.count, Some(36));
    }

    #[tokio::test]
    async fn p4_t16_c74() {
        let events = ["signup_started", "platform_navigation", "listing_opened", "tutorial_started"];
        let mut series = Vec::new();
        for day in 0..365 {
            let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
                + chrono::Duration::days(day);
            for event in events {
                series.push(json!({"x": event, "t": date.to_string(), "y": 3}));
            }
        }
        let stub = stub_with(move |request| {
            request.path.ends_with("/events/series")
                .then(|| Reply::json(200, Value::Array(series.clone())))
        });
        let (address, server) = serve(stub).await;
        let annual = AnalyticsQuery {
            start: at("2026-01-01T00:00:00Z"),
            end: at("2027-01-01T00:00:00Z"),
            ..query()
        };
        let window = client(address).window(&annual, destination_set()).await;
        server.abort();
        let rows = window.event_series.data().expect("annual series");
        assert_eq!(rows.len(), 365 * events.len());
        for event in events {
            assert_eq!(rows.iter().filter(|row| row.event == event).count(), 365);
        }
    }

    // ---------------------------------------------------------------------------
    // c50: no cross-request state of any kind.
    // ---------------------------------------------------------------------------

    /// `umami_request_is_stateless`.
    #[tokio::test]
    async fn p4_t16_c50() {
        let auth_calls = Arc::new(AtomicUsize::new(0));
        let counter = auth_calls.clone();
        let stub = Stub::new(move |request| {
            let counter = counter.clone();
            async move {
                if is_auth(&request) {
                    let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    return Reply::json(200, json!({"token": format!("token-{call}")}));
                }
                healthy_reply(&request).unwrap_or_else(|| Reply::body(404, Vec::new()))
            }
        });
        let (address, server) = serve(stub.clone()).await;
        let client = client(address);
        let first = client.window(&query(), destination_set()).await;
        let second = client.window(&query(), destination_set()).await;
        server.abort();

        assert_eq!(first.availability, BehavioralAvailability::Available);
        assert_eq!(second.availability, BehavioralAvailability::Available);
        assert_eq!(
            auth_calls.load(Ordering::SeqCst),
            2,
            "authenticate per request"
        );
        assert_eq!(
            stub.count(is_stats),
            2,
            "fetch per request; no aggregate cache"
        );
        assert_eq!(
            stub.count(|request| request.path.ends_with("/events/series")),
            2
        );

        let data_calls: Vec<_> = stub
            .seen()
            .into_iter()
            .filter(|request| !is_auth(request))
            .collect();
        let first_token = data_calls[0].header("authorization").unwrap().to_owned();
        let second_token = data_calls
            .iter()
            .rev()
            .map(|request| request.header("authorization").unwrap().to_owned())
            .next()
            .unwrap();
        assert_ne!(
            first_token, second_token,
            "the second request authenticates again instead of reusing a token cache"
        );

        let debug = format!("{client:?}");
        assert!(!debug.contains(FAKE_PASSWORD));
        assert!(!debug.contains("token-1"));
    }

    // ---------------------------------------------------------------------------
    // c51: authentication failure and exactly one forced re-authentication.
    // ---------------------------------------------------------------------------

    /// `umami_request_is_stateless` (single forced reauth).
    #[tokio::test]
    async fn p4_t16_c51() {
        let stats_calls = Arc::new(AtomicUsize::new(0));
        let auth_calls = Arc::new(AtomicUsize::new(0));
        let (stats_counter, auth_counter) = (stats_calls.clone(), auth_calls.clone());
        let stub = Stub::new(move |request| {
            let (stats_counter, auth_counter) = (stats_counter.clone(), auth_counter.clone());
            async move {
                if is_auth(&request) {
                    let call = auth_counter.fetch_add(1, Ordering::SeqCst) + 1;
                    return Reply::json(200, json!({"token": format!("token-{call}")}));
                }
                if is_stats(&request) {
                    let call = stats_counter.fetch_add(1, Ordering::SeqCst) + 1;
                    if call == 1 {
                        return Reply::json(401, json!({"error": "expired"}));
                    }
                }
                healthy_reply(&request).unwrap_or_else(|| Reply::body(404, Vec::new()))
            }
        });
        let (address, server) = serve(stub.clone()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.availability, BehavioralAvailability::Available);
        assert_eq!(auth_calls.load(Ordering::SeqCst), 2, "one reauth");
        assert_eq!(stats_calls.load(Ordering::SeqCst), 2, "one retry");
        let stats: Vec<_> = stub.seen().into_iter().filter(is_stats).collect();
        assert_eq!(stats[0].header("authorization"), Some("Bearer token-1"));
        assert_eq!(stats[1].header("authorization"), Some("Bearer token-2"));

        // A second 401 is authentication, not an unbounded retry loop.
        let always = Stub::new(|request| async move {
            if is_auth(&request) {
                return Reply::json(200, json!({"token": "token"}));
            }
            if is_stats(&request) {
                return Reply::json(401, json!({}));
            }
            healthy_reply(&request).unwrap_or_else(|| Reply::body(404, Vec::new()))
        });
        let (address, server) = serve(always.clone()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Authentication));
        assert_eq!(
            always.count(is_auth),
            2,
            "initial authentication plus one reauth"
        );
        assert_eq!(always.count(is_stats), 2);

        // 403 is authentication and is not retried.
        let forbidden = Stub::new(|request| async move {
            if is_auth(&request) {
                return Reply::json(200, json!({"token": "token"}));
            }
            if is_stats(&request) {
                return Reply::json(403, json!({}));
            }
            healthy_reply(&request).unwrap_or_else(|| Reply::body(404, Vec::new()))
        });
        let (address, server) = serve(forbidden.clone()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Authentication));
        assert_eq!(forbidden.count(is_auth), 1);
        assert_eq!(forbidden.count(is_stats), 1);
    }

    // ---------------------------------------------------------------------------
    // c52: the verified filtered property route, and per-event partial failure.
    // ---------------------------------------------------------------------------

    /// `umami_failure_is_typed_unavailable` (property endpoint isolation).
    #[tokio::test]
    async fn p4_t16_c52() {
        let stub = healthy();
        let (address, server) = serve(stub.clone()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.availability, BehavioralAvailability::Available);

        let property_calls: Vec<_> = stub
            .seen()
            .into_iter()
            .filter(|request| request.path.ends_with("/event-data/events"))
            .collect();
        assert_eq!(property_calls.len(), 1);
        let call = &property_calls[0];
        let start = query().start.timestamp_millis().to_string();
        assert_eq!(call.method, "GET");
        assert_eq!(call.param("event"), Some("platform_navigation"));
        assert_eq!(call.param("startAt"), Some(start.as_str()));
        assert!(call.param("endAt").is_some());
        assert_eq!(
            call.header("authorization"),
            Some("Bearer token-1"),
            "the property route uses the authenticated API, never the database"
        );

        // One failing event degrades to partial and keeps the core totals.
        let failing = stub_with(|request| {
            request
                .path
                .ends_with("/event-data/events")
                .then(|| Reply::json(500, json!({})))
        });
        let (address, server) = serve(failing).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(window.availability, BehavioralAvailability::Partial);
        assert!(window.validate().is_ok());
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Upstream));
        assert_eq!(
            window.core.data().expect("core survives").visitors,
            Some(12)
        );
        assert_eq!(
            window.property_results[0].result.status(),
            ComponentStatus::Unavailable
        );
        assert!(window.property_results[0].result.data().is_none());
    }

    // ---------------------------------------------------------------------------
    // c55: the declared analytics token policy, including N01.
    // ---------------------------------------------------------------------------

    /// `analytics_token_bucket_contract`.
    #[test]
    fn p4_t16_c55() {
        static ROUTES: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!(
                "../../../tests/rust_contract/phase4/routes.json"
            ))
            .expect("committed route inventory")
        });
        let expected = [
            ("P01", "GET", "/api/admin/analytics/product-usage", 30),
            ("P02", "GET", "/api/admin/analytics/overview", 60),
            ("P03", "GET", "/api/admin/analytics/daily", 60),
            ("P04", "GET", "/api/admin/analytics/users", 60),
            (
                "P05",
                "GET",
                "/api/admin/analytics/product-analytics/overview",
                30,
            ),
            (
                "P06",
                "GET",
                "/api/admin/analytics/product-analytics/acquisition",
                30,
            ),
            (
                "P07",
                "GET",
                "/api/admin/analytics/product-analytics/activation",
                30,
            ),
            (
                "P08",
                "GET",
                "/api/admin/analytics/product-analytics/engagement",
                30,
            ),
            (
                "P09",
                "GET",
                "/api/admin/analytics/product-analytics/marketplace",
                30,
            ),
            (
                "P10",
                "GET",
                "/api/admin/analytics/product-analytics/retention",
                30,
            ),
            (
                "P11",
                "GET",
                "/api/admin/analytics/product-analytics/reliability",
                30,
            ),
            (
                "N01",
                "PUT",
                "/api/admin/analytics/users/{user_id}/reject",
                60,
            ),
        ];
        for (id, method, path, count) in expected {
            let route = ROUTES["routes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|route| route["id"] == id)
                .unwrap_or_else(|| panic!("{id} missing"));
            assert_eq!(route["method"], method, "{id}");
            assert_eq!(route["path"], path, "{id}");
            let limits = route["limits"].as_array().expect("limits");
            assert_eq!(limits.len(), 1, "{id} has one token bucket");
            assert_eq!(limits[0]["id"], "limit_token_minute");
            assert_eq!(limits[0]["count"], count, "{id}");
            assert_eq!(limits[0]["window"], "minute");
            assert_eq!(limits[0]["key"], "token");
            assert_eq!(limits[0]["fallback"], "client");

            let limits_guard = SharedLimits::default();
            let token = format!("{id}-token");
            for _ in 0..count {
                limits_guard
                    .check_business(
                        method,
                        path,
                        BusinessLimitKeys {
                            client: "192.0.2.1",
                            user: Some("user"),
                            organization: Some("org"),
                            token: Some(&token),
                        },
                        100,
                    )
                    .unwrap();
            }
            let refused = limits_guard
                .check_business(
                    method,
                    path,
                    BusinessLimitKeys {
                        client: "192.0.2.1",
                        user: Some("user"),
                        organization: Some("org"),
                        token: Some(&token),
                    },
                    100,
                )
                .unwrap_err();
            assert_eq!(refused.status.as_u16(), 429, "{id}");
            assert_eq!(refused.retry_after, Some(60), "{id}");
            assert_eq!(refused.detail["code"], "RATE_LIMITED", "{id}");
        }
    }

    // ---------------------------------------------------------------------------
    // c56: token and client identities are separate keys.
    // ---------------------------------------------------------------------------

    /// `analytics_token_bucket_contract` (identity separation).
    #[test]
    fn p4_t16_c56() {
        let limits = SharedLimits::default();
        let keys = |token: Option<&'static str>, client: &'static str| BusinessLimitKeys {
            client,
            user: Some("user"),
            organization: Some("org"),
            token,
        };
        let path = "/api/admin/analytics/product-analytics/overview";
        for _ in 0..30 {
            limits
                .check_business("GET", path, keys(Some("token-a"), "192.0.2.1"), 100)
                .unwrap();
        }
        assert!(
            limits
                .check_business("GET", path, keys(Some("token-a"), "192.0.2.1"), 100)
                .is_err()
        );
        // A different token has its own bucket.
        for _ in 0..30 {
            limits
                .check_business("GET", path, keys(Some("token-b"), "192.0.2.1"), 100)
                .unwrap();
        }
        // The client fallback is used only when no token identity exists.
        for _ in 0..30 {
            limits
                .check_business("GET", path, keys(None, "198.51.100.7"), 100)
                .unwrap();
        }
        assert!(
            limits
                .check_business("GET", path, keys(None, "198.51.100.7"), 100)
                .is_err()
        );
        // The window resets after a minute.
        assert!(
            limits
                .check_business("GET", path, keys(Some("token-a"), "192.0.2.1"), 160)
                .is_ok()
        );
        // A route without a business owner spends no quota and is refused.
        let refused = limits
            .check_business("DELETE", path, keys(Some("token-a"), "192.0.2.1"), 100)
            .unwrap_err();
        assert_eq!(refused.status.as_u16(), 404);
        assert_eq!(refused.detail["code"], "ROUTE_NOT_OWNED");
    }

    // ---------------------------------------------------------------------------
    // c57: core metrics and the average-session rule.
    // ---------------------------------------------------------------------------

    /// `umami_core_metrics_are_typed`.
    #[tokio::test]
    async fn p4_t16_c57() {
        let (address, server) = serve(healthy()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        let core = window.core.into_data().expect("core");
        assert_eq!(core.visitors, Some(12));
        assert_eq!(core.visits, Some(8));
        assert_eq!(core.pageviews, Some(30));
        assert_eq!(core.total_time_seconds, Some(160));
        assert_eq!(
            core.average_session_duration_seconds,
            Some(Decimal::new(200, 1))
        );

        let no_visits = stub_with(|request| {
            is_stats(request).then(|| {
                Reply::json(
                    200,
                    json!({"visitors": 0, "visits": 0, "pageviews": 0, "totaltime": 10}),
                )
            })
        });
        let (address, server) = serve(no_visits).await;
        let core = client(address)
            .window(&query(), destination_set())
            .await
            .core
            .into_data()
            .expect("core");
        server.abort();
        assert_eq!(core.average_session_duration_seconds, None);
        assert_eq!(core.visitors, Some(0), "zero is genuine");

        let rounding = stub_with(|request| {
            is_stats(request).then(|| {
                Reply::json(
                    200,
                    json!({"visitors": 1.9, "visits": 3, "pageviews": 4, "totaltime": 7}),
                )
            })
        });
        let (address, server) = serve(rounding).await;
        let core = client(address)
            .window(&query(), destination_set())
            .await
            .core
            .into_data()
            .expect("core");
        server.abort();
        assert_eq!(core.visitors, Some(1), "float truncates toward zero");
        assert_eq!(core.pageviews, Some(4));
        assert_eq!(
            core.average_session_duration_seconds,
            Some(Decimal::new(23, 1)),
            "7/3 rounds to one decimal, half-even"
        );

        for broken in [
            json!({"visitors": true, "visits": 1, "pageviews": 1, "totaltime": 1}),
            json!({"visitors": "1", "visits": 1, "pageviews": 1, "totaltime": 1}),
            json!({"visitors": 1, "visits": 1, "pageviews": 1}),
        ] {
            let stub = stub_with(move |request| {
                is_stats(request).then(|| Reply::json(200, broken.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse)
            );
            assert!(window.core.data().is_none());
        }

        // A negative count is impossible data, never a genuine zero.
        let negative = stub_with(|request| {
            is_stats(request).then(|| {
                Reply::json(
                    200,
                    json!({"visitors": 1, "visits": 1, "pageviews": -4, "totaltime": 1}),
                )
            })
        });
        let (address, server) = serve(negative).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        assert_eq!(
            window.diagnostic,
            Some(AnalyticsDiagnostic::MalformedResponse)
        );
    }

    // ---------------------------------------------------------------------------
    // c58: daily visitor rows are typed, half-open and deduplicated.
    // ---------------------------------------------------------------------------

    /// `umami_daily_rows_are_bounded_by_the_window`.
    #[tokio::test]
    async fn p4_t16_c58() {
        let stub = stub_with(|request| {
            request.path.ends_with("/pageviews").then(|| {
                Reply::json(
                    200,
                    json!({"pageviews": [], "sessions": [
                        {"x": "2026-05-31", "y": 9},
                        {"x": "2026-06-01", "y": 4},
                        {"x": "2026-06-01", "y": 5},
                        {"x": "2026-06-30", "y": 6},
                        {"x": "2026-07-01", "y": 7},
                        {"x": "2026-07-02", "y": 8}
                    ]}),
                )
            })
        });
        let (address, server) = serve(stub).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        let rows = window.daily_visitors.into_data().expect("daily visitors");
        assert_eq!(rows.len(), 2, "only [start, end)");
        assert_eq!(rows[0].visitors.count, Some(9), "duplicates sum");
        assert_eq!(rows[1].visitors.count, Some(6));

        for broken in [
            json!({"sessions": "no"}),
            json!({}),
            json!({"sessions": [{"y": 1}]}),
            json!({"sessions": [{"x": "2026-06-01", "y": "1"}]}),
            json!({"sessions": [{"x": "not-a-date", "y": 1}]}),
        ] {
            let stub = stub_with(move |request| {
                request
                    .path
                    .ends_with("/pageviews")
                    .then(|| Reply::json(200, broken.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse)
            );
        }
    }

    // ---------------------------------------------------------------------------
    // c59: event series are allowlisted, dated and bounded.
    // ---------------------------------------------------------------------------

    /// `umami_event_series_is_allowlisted`.
    #[tokio::test]
    async fn p4_t16_c59() {
        let stub = stub_with(|request| {
            request.path.ends_with("/events/series").then(|| {
                Reply::json(
                    200,
                    json!([
                        {"x": "signup_started", "t": "2026-06-10", "y": 4},
                        {"x": "signup_started", "t": "2026-06-10", "y": 2},
                        {"x": "signup_started", "t": "2026-05-01", "y": 5},
                        {"x": "unknown_event", "t": "2026-06-10", "y": 99}
                    ]),
                )
            })
        });
        let (address, server) = serve(stub).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        let series = window.event_series.into_data().expect("series");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].date.to_string(), "2026-06-10");
        assert_eq!(series[0].count.count, Some(6), "duplicates sum");
        let totals = window.event_totals.into_data().expect("totals");
        let signup = totals
            .iter()
            .find(|row| row.event.as_str() == "signup_started")
            .expect("signup total");
        assert_eq!(
            signup.count.count,
            Some(11),
            "totals include out-of-window rows"
        );
        assert!(
            !totals.iter().any(|row| row.event.as_str() == "unknown_event"),
            "unknown events are discarded from totals"
        );

        for broken in [
            json!({"x": "signup_started"}),
            json!([{"x": 7, "t": "2026-06-10", "y": 1}]),
            json!([{"x": "signup_started", "t": 5, "y": 1}]),
            json!([{"x": "signup_started", "t": "2026-06-10", "y": true}]),
            json!([{"x": "signup_started", "t": "not-a-date", "y": 1}]),
        ] {
            let stub = stub_with(move |request| {
                request
                    .path
                    .ends_with("/events/series")
                    .then(|| Reply::json(200, broken.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse)
            );
        }
    }

    // ---------------------------------------------------------------------------
    // c60: entry and referrer rankings are normalized closed domains.
    // ---------------------------------------------------------------------------

    /// `umami_rankings_normalize_to_closed_domains`.
    #[tokio::test]
    async fn p4_t16_c60() {
        assert_eq!(
            EntryPage::from_raw_path("/marketplace?tab=1#x"),
            EntryPage::Platform
        );
        assert_eq!(EntryPage::from_raw_path("/"), EntryPage::Landing);
        assert_eq!(EntryPage::from_raw_path("/login?next=/x"), EntryPage::Login);
        assert_eq!(
            EntryPage::from_raw_path("/private/buyer-123"),
            EntryPage::Other
        );
        assert_eq!(
            EntryPage::from_raw_path("https://example.com/signup"),
            EntryPage::Signup
        );

        assert_eq!(ReferrerCategory::from_raw(""), ReferrerCategory::Direct);
        assert_eq!(
            ReferrerCategory::from_raw("https://www.google.com/search?q=secret"),
            ReferrerCategory::Search
        );
        assert_eq!(
            ReferrerCategory::from_raw("bing.com"),
            ReferrerCategory::Search
        );
        assert_eq!(
            ReferrerCategory::from_raw("https://facebook.com/page"),
            ReferrerCategory::Social
        );
        assert_eq!(
            ReferrerCategory::from_raw("https://example.com/?ref=secret"),
            ReferrerCategory::Other
        );

        let stub = stub_with(|request| {
            if request.path.ends_with("/metrics") && request.param("type") == Some("entry") {
                return Some(Reply::json(
                    200,
                    json!([
                        {"x": "/marketplace?tab=1", "y": 3},
                        {"x": "/", "y": 1},
                        {"x": "/private/buyer-123", "y": 2}
                    ]),
                ));
            }
            if request.path.ends_with("/metrics") && request.param("type") == Some("referrer") {
                return Some(Reply::json(
                    200,
                    json!([
                        {"x": "https://www.google.com/search?q=secret", "y": 4},
                        {"x": "https://example.com/private", "y": 3}
                    ]),
                ));
            }
            None
        });
        let (address, server) = serve(stub).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();

        let entries = window.top_entries.data().cloned().expect("entries");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry, EntryPage::Platform);
        assert_eq!(entries[0].rank, 1);
        assert_eq!(entries[0].count.count, Some(3));
        assert_eq!(entries[1].entry, EntryPage::Other);
        assert!(entries[1].count.suppressed);
        assert_eq!(entries[2].entry, EntryPage::Landing);
        assert!(entries[2].count.suppressed);

        let referrers = window.top_referrers.data().cloned().expect("referrers");
        assert_eq!(referrers[0].referrer, ReferrerCategory::Search);
        assert_eq!(referrers[1].referrer, ReferrerCategory::Other);

        let serialized = serde_json::to_string(&window).unwrap();
        for forbidden in [
            "/marketplace?tab=1",
            "/private/buyer-123",
            "google.com",
            "example.com",
            "secret",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "raw ranking leaked {forbidden}"
            );
        }
    }

    #[test]
    fn p4_t16_c75() {
        let beyond_bound = format!("{}signup", "/".repeat(500));
        assert_eq!(EntryPage::from_raw_path(&beyond_bound), EntryPage::Landing);
        let long_path = format!("{}signup", "/".repeat(900_000));
        assert_eq!(EntryPage::from_raw_path(&long_path), EntryPage::Landing);
        assert_eq!(EntryPage::from_raw_path("///signup//next"), EntryPage::Signup);
        let unicode_prefix = format!("https://{}/signup", "é".repeat(492));
        assert_eq!(EntryPage::from_raw_path(&unicode_prefix), EntryPage::Landing);
    }

    // ---------------------------------------------------------------------------
    // c61: each core payload shape drift is a typed unavailable window.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c61() {
        for broken in [
            json!({"visitors": 1, "visits": 1, "pageviews": 1}),
            json!({"visitors": 1, "visits": 1, "pageviews": 1, "totaltime": null}),
            json!({"visitors": 1, "visits": 1, "pageviews": 1, "totaltime": {}}),
        ] {
            let stub = stub_with(move |request| {
                is_stats(request).then(|| Reply::json(200, broken.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(window.availability, BehavioralAvailability::Unavailable);
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse)
            );
        }
    }

    // ---------------------------------------------------------------------------
    // c62: the events series must be an array of typed rows.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c62() {
        for broken in [
            json!({}),
            json!([1]),
            json!([{"x": "signup_started", "t": "2026-06-10"}]),
            json!([{"t": "2026-06-10", "y": 1}]),
            json!([{"x": "signup_started", "t": "2026-06-10", "y": "1"}]),
        ] {
            let stub = stub_with(move |request| {
                request
                    .path
                    .ends_with("/events/series")
                    .then(|| Reply::json(200, broken.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse)
            );
        }
    }

    // ---------------------------------------------------------------------------
    // c63: the pageviews payload must carry a sessions array.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c63() {
        for broken in [
            json!([]),
            json!({"sessions": [1]}),
            json!({"sessions": [{"x": 1, "y": 1}]}),
            json!({"sessions": [{"x": "2026-06-10", "y": -1}]}),
        ] {
            let body = broken.clone();
            let stub = stub_with(move |request| {
                request
                    .path
                    .ends_with("/pageviews")
                    .then(|| Reply::json(200, body.clone()))
            });
            let (address, server) = serve(stub).await;
            let window = client(address).window(&query(), destination_set()).await;
            server.abort();
            assert_eq!(
                window.diagnostic,
                Some(AnalyticsDiagnostic::MalformedResponse),
                "{broken}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // c66: multiple requested events isolate their own failure.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c66() {
        let properties = AllowedPropertySet::new([
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::LoginFailed),
                AllowedProperty::ReasonCategory,
            ),
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
                AllowedProperty::Destination,
            ),
        ])
        .expect("pairs");
        let stub = stub_with(|request| {
            if request.path.ends_with("/event-data/events") {
                if request.param("event") == Some("login_failed") {
                    return Some(Reply::json(500, json!({})));
                }
                return Some(Reply::json(
                    200,
                    json!([
                        {"propertyName": "destination", "propertyValue": "home", "total": 5}
                    ]),
                ));
            }
            None
        });
        let (address, server) = serve(stub.clone()).await;
        let window = client(address).window(&query(), properties).await;
        server.abort();

        assert_eq!(window.availability, BehavioralAvailability::Partial);
        assert!(window.validate().is_ok());
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Upstream));
        assert_eq!(
            stub.count(|request| request.path.ends_with("/event-data/events")),
            2,
            "one fetch per distinct event"
        );
        assert_eq!(window.property_results.len(), 2);
        assert_eq!(
            window.property_results[0].property,
            AllowedProperty::ReasonCategory.name()
        );
        assert!(window.property_results[0].result.data().is_none());
        assert_eq!(
            window.property_results[1].result.data().unwrap()[0]
                .value
                .as_str(),
            "home"
        );
    }

    // ---------------------------------------------------------------------------
    // c67: every requested pair has exactly one result, including empty.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c67() {
        let properties = AllowedPropertySet::new([
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
                AllowedProperty::Destination,
            ),
            (
                RegisteredEvent::Frontend(AllowedFrontendEvent::PlatformNavigation),
                AllowedProperty::ViewMode,
            ),
        ])
        .expect("pairs");
        let stub = stub_with(|request| {
            request.path.ends_with("/event-data/events").then(|| {
                Reply::json(
                    200,
                    json!([
                        {"propertyName": "destination", "propertyValue": "home", "total": 5}
                    ]),
                )
            })
        });
        let (address, server) = serve(stub.clone()).await;
        let window = client(address).window(&query(), properties).await;
        server.abort();
        assert_eq!(window.property_results.len(), 2);
        assert_eq!(
            window.property_results[0].result.status(),
            ComponentStatus::Available
        );
        assert_eq!(
            window.property_results[1].result.status(),
            ComponentStatus::Available
        );
        assert_eq!(
            window.property_results[1]
                .result
                .data()
                .expect("genuine empty"),
            &Vec::new(),
            "an empty result is available empty, not unavailable"
        );
        assert_eq!(
            stub.count(|request| request.path.ends_with("/event-data/events")),
            1
        );
    }

    // ---------------------------------------------------------------------------
    // c68: disabled and misconfigured clients make no HTTP request at all.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c68() {
        let stub = healthy();
        let (address, server) = serve(stub.clone()).await;
        for config in [
            UmamiConfig {
                enabled: false,
                ..config(address)
            },
            UmamiConfig {
                website_id: None,
                ..config(address)
            },
            UmamiConfig {
                api_username: None,
                ..config(address)
            },
            UmamiConfig {
                api_password: None,
                ..config(address)
            },
        ] {
            let window = UmamiClient::new(config)
                .expect("valid disabled configuration")
                .window(&query(), destination_set())
                .await;
            assert_eq!(window.availability, BehavioralAvailability::Unavailable);
        }
        server.abort();
        assert_eq!(
            stub.seen().len(),
            0,
            "configuration failure never calls out"
        );
    }

    // ---------------------------------------------------------------------------
    // c69: no credential or token enters a response or a log-visible value.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c69() {
        let (address, server) = serve(healthy()).await;
        let client = client(address);
        let window = client.window(&query(), destination_set()).await;
        server.abort();
        let serialized = serde_json::to_string(&window).unwrap();
        for secret in [FAKE_PASSWORD, "token-1", USERNAME] {
            assert!(!serialized.contains(secret), "window leaked {secret}");
        }
        assert!(!format!("{client:?}").contains(FAKE_PASSWORD));
    }

    #[test]
    fn p4_t16_c76() {
        let config = UmamiConfig {
            base_url: Some("https://fake-url-user:fake-url-password@127.0.0.1".into()),
            ..config("127.0.0.1:9".parse().unwrap())
        };
        let debug = format!("{config:?}");
        for hidden in [USERNAME, FAKE_PASSWORD, "fake-url-user", "fake-url-password", "127.0.0.1"] {
            assert!(!debug.contains(hidden));
        }
        assert_eq!(ReferrerCategory::from_raw("//google.com/search?q=fake"), ReferrerCategory::Search);
    }

    #[tokio::test]
    async fn p4_t16_c77() {
        let stub = stub_with(|request| {
            let rows = json!([
                {"x": "2026-06-01 11:59:59", "y": 3},
                {"x": "2026-06-01 12:00:00", "y": 4},
                {"x": "2026-06-02T13:00:00+01:00", "y": 5},
                {"x": "2026-06-02 12:00:01", "y": 6},
                {"x": "2026-06-02 11:59:59", "y": 7}
            ]);
            if request.path.ends_with("/pageviews") {
                return Some(Reply::json(200, json!({"sessions": rows})));
            }
            if request.path.ends_with("/events/series") {
                let events: Vec<_> = rows.as_array().unwrap().iter().map(|row|
                    json!({"x": "signup_started", "t": row["x"], "y": row["y"]})
                ).collect();
                return Some(Reply::json(200, json!(events)));
            }
            None
        });
        let (address, server) = serve(stub).await;
        let period = AnalyticsQuery {
            start: at("2026-06-01T12:00:00Z"),
            end: at("2026-06-02T12:00:00Z"),
            ..query()
        };
        let window = client(address).window(&period, destination_set()).await;
        server.abort();
        let daily = window.daily_visitors.data().unwrap();
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[0].visitors.count, Some(4));
        assert_eq!(daily[1].visitors.count, Some(7));
        let events = window.event_series.data().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].count.count, Some(4));
        assert_eq!(events[1].count.count, Some(7));
    }

    // ---------------------------------------------------------------------------
    // c70: redirects are not followed, so a credential cannot reach a new host.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c70() {
        let target = healthy();
        let (target_address, target_server) = serve(target.clone()).await;
        let redirecting = stub_with(move |request| {
            is_stats(request).then(|| Reply::redirect(&format!("http://{target_address}/stolen")))
        });
        let (address, server) = serve(redirecting).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        target_server.abort();
        assert_eq!(window.diagnostic, Some(AnalyticsDiagnostic::Upstream));
        assert_eq!(target.seen().len(), 0, "no redirect is followed");
    }

    // ---------------------------------------------------------------------------
    // c71: construction refuses an unbounded or credential-bearing endpoint.
    // ---------------------------------------------------------------------------

    #[test]
    fn p4_t16_c71() {
        let good = config("127.0.0.1:9".parse().unwrap());
        assert!(UmamiClient::new(good.clone()).is_ok());
        assert!(
            UmamiClient::new(UmamiConfig {
                policy: UmamiPolicy {
                    timeout: Duration::ZERO
                },
                ..good.clone()
            })
            .is_err()
        );
        assert!(
            UmamiClient::new(UmamiConfig {
                policy: UmamiPolicy {
                    timeout: Duration::from_millis(10_001)
                },
                ..good.clone()
            })
            .is_err()
        );
        assert!(
            UmamiClient::new(UmamiConfig {
                policy: UmamiPolicy {
                    timeout: Duration::from_secs(10)
                },
                ..good.clone()
            })
            .is_ok(),
            "ten seconds is the inclusive maximum"
        );
        for base in [
            Some("ftp://analytics.example.com".to_string()),
            Some("analytics.example.com".to_string()),
            Some("https://user:pass@analytics.example.com".to_string()),
            Some("   ".to_string()),
        ] {
            let result = UmamiClient::new(UmamiConfig {
                base_url: base.clone(),
                ..good.clone()
            });
            if base.as_deref() == Some("   ") {
                assert!(result.is_ok(), "blank base means unconfigured");
            } else {
                assert!(result.is_err(), "{base:?} must be refused");
            }
        }
        // A disabled client still validates a supplied base URL, as settings do.
        assert!(
            UmamiClient::new(UmamiConfig {
                enabled: false,
                base_url: Some("ftp://analytics.example.com".into()),
                ..good
            })
            .is_err()
        );
    }

    // ---------------------------------------------------------------------------
    // c72: domains are the pinned catalog, countries and canonical windows.
    // ---------------------------------------------------------------------------

    #[test]
    fn p4_t16_c72() {
        let event = RegisteredEvent::Server(AllowedServerEvent::OrderCreated);
        let catalog = catalog();
        assert!(
            AllowedPropertyValue::parse(event, AllowedProperty::CanonicalProduct, "B30", &catalog)
                .is_some()
        );
        assert!(
            AllowedPropertyValue::parse(
                event,
                AllowedProperty::CanonicalProduct,
                "UCOME_B100",
                &catalog
            )
            .is_none(),
            "a product outside the pinned catalog is not a value"
        );
        assert!(
            AllowedPropertyValue::parse(
                event,
                AllowedProperty::DeliveryPoint,
                "Singapore",
                &catalog
            )
            .is_some()
        );
        assert!(
            AllowedPropertyValue::parse(
                event,
                AllowedProperty::DeliveryPoint,
                "Atlantis",
                &catalog
            )
            .is_none()
        );
        let signup = RegisteredEvent::Frontend(AllowedFrontendEvent::SignupOrganizationSubmitted);
        assert!(
            AllowedPropertyValue::parse(signup, AllowedProperty::Country, "NL", &catalog).is_some()
        );
        assert!(
            AllowedPropertyValue::parse(signup, AllowedProperty::Country, "nl", &catalog).is_none()
        );
        assert!(
            AllowedPropertyValue::parse(signup, AllowedProperty::Country, "XK", &catalog).is_none(),
            "user-assigned codes are not assigned countries"
        );
        for window in ["SPOT", "2026-01", "2026-Q4", "2026-CAL"] {
            assert!(
                AllowedPropertyValue::parse(
                    event,
                    AllowedProperty::AvailabilityWindow,
                    window,
                    &catalog
                )
                .is_some(),
                "{window}"
            );
        }
        for window in ["spot", "2026-00", "2026-13", "2026-Q5", "0000-01", "2026"] {
            assert!(
                AllowedPropertyValue::parse(
                    event,
                    AllowedProperty::AvailabilityWindow,
                    window,
                    &catalog
                )
                .is_none(),
                "{window}"
            );
        }

        let policy: MarketPolicy = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../deploy/rust/market-policy.json"
            ))
            .expect("policy artifact"),
        )
        .expect("policy");
        let from_policy = DomainCatalog::from_policy(&policy, ISO_3166_1_ALPHA_2.iter().copied());
        assert!(from_policy.is_product("B30"));
        assert!(from_policy.is_delivery_point("Singapore"));
        assert!(from_policy.is_country("NL"));
        assert!(!from_policy.is_country("XX"));
    }

    // ---------------------------------------------------------------------------
    // c73: only typed aggregates leave the adapter; no raw upstream document.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn p4_t16_c73() {
        let (address, server) = serve(healthy()).await;
        let window = client(address).window(&query(), destination_set()).await;
        server.abort();
        let serialized = serde_json::to_string(&window).unwrap();
        for raw_key in [
            "\"propertyName\"",
            "\"propertyValue\"",
            "\"eventName\"",
            "\"dataType\"",
            "\"x\":",
            "\"y\":",
            "\"t\":",
            "\"sessions\"",
            "\"totaltime\"",
        ] {
            assert!(
                !serialized.contains(raw_key),
                "raw upstream shape {raw_key} must not pass through"
            );
        }
        let value = serde_json::to_value(&window).unwrap();
        assert!(value["coverage"]["status"].is_string());
        assert!(value["core"]["data"]["average_session_duration_seconds"].is_string());
        assert_eq!(window.coverage.status, AnalyticsSourceStatus::Available);
        assert_eq!(window.coverage.coverage_start, Some(query().start));
        assert_eq!(window.coverage.coverage_end, Some(query().end));
        assert!(window.coverage.observed_at.is_some());
    }

    // ---------------------------------------------------------------------------
    // Contract-shape compile checks: verify the frozen CF4_03 declarations.
    // ---------------------------------------------------------------------------

    #[test]
    fn p4_t16_c74() {
        fn suppress_signature(
            count: Option<u64>,
            entities: Option<u64>,
        ) -> analytics::types::AggregateCell {
            analytics::policy::suppress(count, entities)
        }
        let _ = suppress_signature;
        fn window_signature<'a>(
            client: &'a UmamiClient,
            query: &'a AnalyticsQuery,
            properties: AllowedPropertySet,
        ) -> Pin<Box<dyn Future<Output = BehavioralWindow> + Send + 'a>> {
            Box::pin(client.window(query, properties))
        }
        let _ = window_signature;
    }
}
