use verdaxis_read::auth::limits::{BusinessLimitKeys, SharedLimits};

#[test]
fn p4_t2_c50() {
    let limits = SharedLimits::default();
    let keys = BusinessLimitKeys {
        client: "client",
        user: Some("user"),
        organization: Some("org"),
        token: Some("token"),
    };
    for _ in 0..10 {
        limits
            .check_business("PUT", "/api/auth/me", keys, 100)
            .unwrap();
    }
    let refused = limits
        .check_business("PUT", "/api/auth/me", keys, 100)
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 429);
    assert_eq!(refused.retry_after, Some(60));
    for _ in 0..120 {
        limits
            .check_business("GET", "/api/auth/me", keys, 100)
            .unwrap();
    }
    assert!(
        limits
            .check_business("GET", "/api/auth/me", keys, 100)
            .is_err()
    );
    assert!(
        limits
            .check_business("PUT", "/api/auth/me", keys, 160)
            .is_ok()
    );
}

#[test]
fn business_quota_capacity_refuses_without_spending_existing_quota() {
    let limits = SharedLimits::default();
    let keys = BusinessLimitKeys {
        client: "client",
        user: Some("existing"),
        organization: Some("org"),
        token: None,
    };
    limits
        .check_business("PUT", "/api/auth/me", keys, 100)
        .unwrap();
    for index in 0..49_999 {
        let user = format!("user-{index}");
        limits
            .check_business(
                "PUT",
                "/api/auth/me",
                BusinessLimitKeys {
                    client: &user,
                    ..keys
                },
                100,
            )
            .unwrap();
    }
    let error = limits
        .check_business(
            "PUT",
            "/api/auth/me",
            BusinessLimitKeys {
                client: "new",
                ..keys
            },
            100,
        )
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 503);
    for _ in 0..9 {
        limits
            .check_business("PUT", "/api/auth/me", keys, 100)
            .unwrap();
    }
    assert_eq!(
        limits
            .check_business("PUT", "/api/auth/me", keys, 100)
            .unwrap_err()
            .status
            .as_u16(),
        429
    );
    limits
        .check_business(
            "PUT",
            "/api/auth/me",
            BusinessLimitKeys {
                client: "fresh",
                ..keys
            },
            160,
        )
        .expect("expired accounting must release capacity for a fresh identity");
}

#[test]
fn p4_t2_c51() {
    let limits=SharedLimits::default();
    let keys=BusinessLimitKeys { client:"client",user:Some("user"),organization:Some("org"),token:None };
    let error=limits.check_business("POST","/api/ai/chat",keys,100).unwrap_err();
    assert_eq!(error.status.as_u16(),404);
}
