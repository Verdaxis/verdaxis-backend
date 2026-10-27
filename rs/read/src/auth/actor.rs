use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CurrentActor {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub role: Option<String>,
    pub email_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLockMode {
    Share,
    Update,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedRequest {
    method: String,
    route_template: String,
    support_context_present: bool,
}

impl AuthenticatedRequest {
    pub fn new(method: &str, route_template: &str, support_context_present: bool) -> Self {
        Self {
            method: method.to_owned(),
            route_template: route_template.to_owned(),
            support_context_present,
        }
    }
    pub(crate) fn route_template(&self) -> &str {
        &self.route_template
    }

    pub fn lock_mode(&self) -> UserLockMode {
        if self.method == "GET"
            && !self.support_context_present
            && matches!(
                self.route_template.as_str(),
                "/api/notifications"
                    | "/api/notifications/unread-count"
                    | "/api/users/me/preferences"
                    | "/api/watchlists"
                    | "/api/watchlists/{watchlist_id}"
                    | "/api/watchlists/{watchlist_id}/events"
            )
        {
            UserLockMode::Share
        } else {
            UserLockMode::Update
        }
    }
}
