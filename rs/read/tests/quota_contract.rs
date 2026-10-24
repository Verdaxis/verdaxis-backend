//! PD2-SEC-1: the rate-limit table must not lock out new clients.
//!
//! Every case here uses only the public authority surface, so this file also
//! runs unchanged against the implementation before the fix and fails on an
//! assertion rather than a compile error.

use verdaxis_read::auth::limits::{Phase2Limits, SharedLimits};

/// The pre-auth store is shared by client address. A flood of distinct
/// addresses must never stop the next legitimate client from being admitted.
#[test]
fn preauth_flood_admits_every_client_including_the_next_new_one() {
    let mut limits = Phase2Limits::default();
    for client in 0..50_100_u32 {
        let address = std::net::Ipv4Addr::from(client).to_string();
        assert!(
            limits.preauth(&address, "/api/admin/users", 100),
            "flood client {client} was refused"
        );
    }
    assert!(
        limits.preauth("203.0.113.99", "/api/admin/users", 100),
        "a new legitimate client was locked out after the flood"
    );
}

/// An address flood must not consume the quotas of verified accounts. These
/// quotas are keyed by user ID in their own store.
#[test]
fn preauth_flood_does_not_consume_user_quota() {
    let mut limits = Phase2Limits::default();
    for client in 0..50_100_u32 {
        let address = std::net::Ipv4Addr::from(client).to_string();
        let _ = limits.preauth(&address, "/api/admin/users", 100);
    }
    for index in 0..5 {
        assert!(
            limits.endpoint("verified-user", "/api/feedback", 100),
            "authenticated request {index} lost its quota to the pre-auth flood"
        );
    }
    assert!(!limits.endpoint("verified-user", "/api/feedback", 100));
    assert!(
        limits.endpoint("another-user", "/api/feedback", 100),
        "one user's quota must not spend another user's"
    );
}

/// A /64 is the smallest unit an ordinary IPv6 customer is assigned, so every
/// address in one /64 shares a single pre-auth bucket.
#[test]
fn ipv6_clients_share_one_prefix_bucket() {
    let mut limits = Phase2Limits::default();
    for address in 0..1_000_u32 {
        let admitted = limits.preauth(
            &format!("2001:db8:1:2::{address:x}"),
            "/api/admin/users",
            100,
        );
        assert_eq!(admitted, address < 300, "address {address}");
    }
    assert!(
        limits.preauth("2001:db8:1:3::1", "/api/admin/users", 100),
        "a different /64 must have its own bucket"
    );
}

/// An IPv4-mapped IPv6 spelling is the same IPv4 client, not a new identity.
#[test]
fn ipv4_mapped_ipv6_shares_the_ipv4_bucket() {
    let mut limits = Phase2Limits::default();
    for _ in 0..300 {
        assert!(limits.preauth("192.0.2.7", "/api/admin/users", 100));
    }
    assert!(
        !limits.preauth("::ffff:192.0.2.7", "/api/admin/users", 100),
        "the mapped spelling opened a second bucket"
    );
}

/// The admin pre-auth budget is separate from the authenticated quotas, and
/// exhausting it must not spend a user's quota.
#[test]
fn admin_preauth_is_a_separate_budget_from_user_quotas() {
    let mut limits = Phase2Limits::default();
    for _ in 0..300 {
        assert!(limits.preauth("198.51.100.4", "/api/admin/users", 100));
    }
    assert!(!limits.preauth("198.51.100.4", "/api/admin/users", 100));
    assert!(limits.endpoint("verified-user", "/api/feedback", 100));
}

/// Paths outside the admin prefix are not charged to the pre-auth bucket.
#[test]
fn non_admin_paths_are_not_charged_to_the_preauth_bucket() {
    let mut limits = Phase2Limits::default();
    for _ in 0..10_000 {
        assert!(limits.preauth("198.51.100.9", "/api/prices", 100));
    }
}

/// The refusal contract is unchanged: machine-readable code and Retry-After.
#[test]
fn endpoint_refusal_keeps_the_rate_limited_code_and_retry_after() {
    let limits = SharedLimits::default();
    for _ in 0..5 {
        assert!(limits
            .check_endpoint("verified-user", "/api/feedback", 100)
            .is_ok());
    }
    let error = limits
        .check_endpoint("verified-user", "/api/feedback", 100)
        .unwrap_err();
    assert_eq!(error.detail["code"], "RATE_LIMITED");
    assert!(error.detail["message"].is_string());
    assert_eq!(error.retry_after, Some(60));
}

/// Every eviction is counted. The counter is a number, so it carries no client
/// address, path, user or other identifying data.
#[test]
fn evictions_are_counted_and_carry_no_client_data() {
    let mut limits = Phase2Limits::default();
    for client in 0..50_100_u32 {
        let address = std::net::Ipv4Addr::from(client).to_string();
        assert!(limits.preauth(&address, "/api/admin/users", 100));
    }
    // Fifty thousand distinct addresses fill the table; the remaining hundred
    // each evict exactly one entry, and every one of them was admitted. No
    // address is refused merely because the table is full.
    assert_eq!(limits.preauth_evictions(), 100);
}

/// A bucket is released when its own window ends, independently of the other
/// store's window.
#[test]
fn each_window_expires_on_its_own_clock() {
    let mut limits = Phase2Limits::default();
    for _ in 0..10 {
        assert!(limits.endpoint("inviter", "/api/referrals/invite", 100));
    }
    assert!(!limits.endpoint("inviter", "/api/referrals/invite", 100));
    // The admin minute window rolls over while the hourly invite stays spent.
    assert!(limits.preauth("192.0.2.2", "/api/admin/users", 100 + 61));
    assert!(limits.endpoint("inviter", "/api/referrals/invite", 100 + 3600));
}
