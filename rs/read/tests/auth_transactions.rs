use verdaxis_read::auth::{AuthenticatedRequest, UserLockMode};

#[test]
fn only_audited_get_templates_share_the_user_lock() {
    for path in [
        "/api/notifications",
        "/api/notifications/unread-count",
        "/api/users/me/preferences",
        "/api/watchlists",
        "/api/watchlists/{watchlist_id}",
        "/api/watchlists/{watchlist_id}/events",
    ] {
        assert_eq!(
            AuthenticatedRequest::new("GET", path, false).lock_mode(),
            UserLockMode::Share
        );
        assert_eq!(
            AuthenticatedRequest::new("GET", path, true).lock_mode(),
            UserLockMode::Update
        );
        assert_eq!(
            AuthenticatedRequest::new("POST", path, false).lock_mode(),
            UserLockMode::Update
        );
    }
    for path in [
        "/api/alerts",
        "/api/referrals/my-referrals",
        "/api/admin/feedback",
        "/api/future",
    ] {
        assert_eq!(
            AuthenticatedRequest::new("GET", path, false).lock_mode(),
            UserLockMode::Update
        );
    }
}
