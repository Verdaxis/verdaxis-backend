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
    assert!(limits
        .check_business("GET", "/api/auth/me", keys, 100)
        .is_err());
    assert!(limits
        .check_business("PUT", "/api/auth/me", keys, 160)
        .is_ok());
}

#[test]
fn p4_t2_c51() {
    let limits = SharedLimits::default();
    let keys = BusinessLimitKeys {
        client: "client",
        user: Some("user"),
        organization: Some("org"),
        token: None,
    };
    for _ in 0..10 {
        limits
            .check_business("POST", "/api/ai/chat", keys, 100)
            .unwrap();
    }
    assert!(limits
        .check_business("POST", "/api/ai/chat", keys, 100)
        .is_err());
    assert!(limits
        .check_business("DELETE", "/api/ai/chat", keys, 100)
        .is_err());
    assert!(limits
        .check_business("POST", "/api/ai/chat/extra", keys, 100)
        .is_err());
}
