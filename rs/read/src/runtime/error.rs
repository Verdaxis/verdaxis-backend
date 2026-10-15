use crate::{ApiError, Json};
use axum::{
    http::{StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
};

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            axum::Json(serde_json::json!({"detail": self.detail})),
        )
            .into_response();
        if let Some(seconds) = self.retry_after {
            response.headers_mut().insert(
                RETRY_AFTER,
                seconds.to_string().parse().expect("integer header"),
            );
        }
        response
    }
}

pub fn busy() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        detail: Json::String("Public market data is temporarily busy; retry shortly.".into()),
        retry_after: Some(1),
    }
}

pub fn database_error(path: &str, method: &str, sqlstate: Option<&str>) -> ApiError {
    let market = [
        "/api/orderbook",
        "/api/trades",
        "/api/prices",
        "/api/price-discovery",
        "/api/matchmaking",
        "/api/curves/forward",
    ]
    .iter()
    .any(|root| {
        path == *root
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    });
    let retry = matches!(sqlstate, Some("55P03" | "40P01" | "40001"))
        || (matches!(method, "GET" | "HEAD") && sqlstate == Some("57014"));
    if market && retry {
        ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail: Json::String("Market is temporarily busy; retry shortly.".into()),
            retry_after: Some(1),
        }
    } else {
        ApiError::internal()
    }
}
