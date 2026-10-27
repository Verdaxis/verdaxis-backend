//! The bounded, evicting client store behind the AD-01 price limits: a flood
//! of distinct addresses cannot grow the store past its bound. Retained spent
//! buckets keep their refusal, and IPv6 clients share one /64
//! bucket instead of one bucket per full address.
use std::net::Ipv4Addr;

use verdaxis_read::runtime::http::ClientBuckets;

const LIMIT: u32 = 60;
const PRICES: &str = "/api/prices";

#[test]
fn a_flood_of_distinct_addresses_does_not_grow_the_store_past_its_bound() {
    let mut store = ClientBuckets::default();
    for index in 0..50_100_u32 {
        let client = Ipv4Addr::from(index).to_string();
        assert!(store.accept(&client, PRICES, LIMIT, 1_000), "{client}");
    }
    assert_eq!(store.entries(), 50_000);
    assert_eq!(store.evictions(), 100);
}

#[test]
fn a_client_over_its_limit_keeps_its_refusal_through_a_later_flood() {
    let mut store = ClientBuckets::default();
    for index in 0..50_000_u32 {
        assert!(store.accept(&Ipv4Addr::from(index).to_string(), PRICES, LIMIT, 1_000));
    }
    assert_eq!(store.evictions(), 0);
    // The late client's arrival evicts exactly one entry, the oldest
    // accounting, and never itself: its own entry does not exist yet.
    for _ in 0..LIMIT {
        assert!(store.accept("203.0.113.7", PRICES, LIMIT, 2_000));
    }
    assert!(!store.accept("203.0.113.7", PRICES, LIMIT, 2_000));
    assert_eq!(store.evictions(), 1);
    // Ten later new addresses displace the next ten oldest accountings. The
    // spent client started after the flood, so it expires last and keeps its
    // refusal.
    for index in 50_000..50_010_u32 {
        assert!(store.accept(&Ipv4Addr::from(index).to_string(), PRICES, LIMIT, 3_000));
    }
    assert_eq!(store.evictions(), 11);
    assert!(!store.accept("203.0.113.7", PRICES, LIMIT, 3_000));
    // Its window, begun at its first admitted request, still ends on time.
    assert!(store.accept("203.0.113.7", PRICES, LIMIT, 62_000));
}

#[test]
fn expiration_cleanup_is_bounded_and_makes_progress() {
    let mut store = ClientBuckets::default();
    for index in 0..50_000_u32 {
        assert!(store.accept(&Ipv4Addr::from(index).to_string(), PRICES, LIMIT, 1_000));
    }
    // This identity sorts after the expired entries, so resetting it must not
    // depend on reaching it through background cleanup. The 32-entry batch
    // removes other entries; removing and replacing the caller is size-neutral.
    let client = Ipv4Addr::from(49_999_u32).to_string();
    assert!(store.accept(&client, PRICES, LIMIT, 61_000));
    assert_eq!(store.entries(), 50_000 - 32);
    for _ in 0..50_000_usize.div_ceil(32) {
        let before = store.entries();
        // Repeated refusals must also make cleanup progress without resetting
        // the spent bucket or exceeding the per-request cleanup budget.
        assert!(!store.accept(&client, PRICES, 1, 61_000));
        assert_eq!(store.entries(), before.saturating_sub(32).max(1));
    }
    assert_eq!(store.entries(), 1);
    assert!(!store.accept(&client, PRICES, 1, 61_000));
    assert_eq!(store.evictions(), 0);
}

#[test]
fn a_new_client_removes_only_one_cleanup_batch_after_all_buckets_expire() {
    let mut store = ClientBuckets::default();
    for index in 0..50_000_u32 {
        assert!(store.accept(&Ipv4Addr::from(index).to_string(), PRICES, LIMIT, 1_000));
    }
    assert!(store.accept("203.0.113.7", PRICES, LIMIT, 61_000));
    assert_eq!(store.entries(), 50_000 - 32 + 1);
    assert_eq!(store.evictions(), 0);
}

#[test]
fn ipv6_clients_share_one_prefix_bucket() {
    let mut store = ClientBuckets::default();
    for host in 1..=LIMIT {
        let client = format!("2001:db8:aa::{host:x}");
        assert!(store.accept(&client, PRICES, LIMIT, 1_000), "{client}");
    }
    assert_eq!(store.entries(), 1);
    assert!(!store.accept("2001:db8:aa::ff", PRICES, LIMIT, 1_000));
    // A different /64 keeps its own bucket.
    assert!(store.accept("2001:db8:bb::1", PRICES, LIMIT, 1_000));
    assert_eq!(store.entries(), 2);
}

#[test]
fn ipv4_mapped_ipv6_shares_the_ipv4_bucket() {
    let mut store = ClientBuckets::default();
    for _ in 0..LIMIT {
        assert!(store.accept("192.0.2.9", PRICES, LIMIT, 1_000));
    }
    assert!(!store.accept("::ffff:192.0.2.9", PRICES, LIMIT, 1_000));
    assert!(store.accept("192.0.2.10", PRICES, LIMIT, 1_000));
    assert_eq!(store.entries(), 2);
}

#[test]
fn one_client_keeps_separate_accounting_per_route() {
    let mut store = ClientBuckets::default();
    for _ in 0..30 {
        assert!(store.accept("198.51.100.4", "/api/prices/reference", 30, 1_000));
    }
    assert!(!store.accept("198.51.100.4", "/api/prices/reference", 30, 1_000));
    assert!(store.accept("198.51.100.4", PRICES, LIMIT, 1_000));
    assert!(store.accept("198.51.100.4", "/api/prices/reference/export", 10, 1_000));
    assert_eq!(store.entries(), 3);
}

#[test]
fn the_window_resets_exactly_sixty_seconds_after_its_first_admission() {
    for (route, limit) in [
        (PRICES, 60),
        ("/api/prices/reference", 30),
        ("/api/prices/reference/export", 10),
    ] {
        let mut store = ClientBuckets::default();
        for _ in 0..limit {
            assert!(store.accept("198.51.100.4", route, limit, 1_000));
        }
        // A refused request never extends the window.
        assert!(!store.accept("198.51.100.4", route, limit, 60_999));
        assert!(store.accept("198.51.100.4", route, limit, 61_000));
    }
}

#[tokio::test]
async fn a_small_distinct_client_flood_preserves_a_retained_clients_refusal() {
    use axum::{Router, body::Body, http::Request, routing::get};
    use tower::ServiceExt;
    use verdaxis_read::runtime::http::middleware;

    let app = middleware(
        Router::new().route(PRICES, get(|| async { "prices" })),
        "test",
    );
    let call = |client: &'static str| {
        let app = app.clone();
        async move {
            // One loopback proxy peer, so the trusted final forwarded hop is
            // the client key.
            let mut request = Request::builder()
                .uri(PRICES)
                .header("x-forwarded-for", client)
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                "127.0.0.1:5000".parse::<std::net::SocketAddr>().unwrap(),
            ));
            app.oneshot(request).await.unwrap()
        }
    };
    for _ in 0..LIMIT {
        assert_eq!(call("192.0.2.50").await.status(), 200);
    }
    let refused = call("192.0.2.50").await;
    assert_eq!(refused.status(), 429);
    // AD-01 has no Retry-After header on staging; keep that contract.
    assert!(refused.headers().get("retry-after").is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(refused.into_body(), 1024)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::json!({"detail":"Rate limit exceeded: 60 per 1 minute"})
    );
    // Below capacity, these other clients do not evict the spent bucket.
    for host in 1..=250_u32 {
        let client: &'static str = Box::leak(format!("203.0.113.{host}").into_boxed_str());
        assert_eq!(call(client).await.status(), 200, "{client}");
    }
    assert_eq!(call("192.0.2.50").await.status(), 429);
}
