use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub fn known_get_refusal(method: &str, path: &str) -> Option<Response> {
    if method == "GET"
        && matches!(
            path.trim_end_matches('/'),
            "/api/referrals/my-code" | "/api/watchlists/me"
        )
    {
        Some(
            (
                StatusCode::METHOD_NOT_ALLOWED,
                Json(serde_json::json!({"detail":{
            "code":"STATE_CHANGING_GET_NOT_ALLOWED",
            "message":"State-changing operations must use their declared mutation method"}})),
            )
                .into_response(),
        )
    } else {
        None
    }
}

pub fn support_present(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("x-verdaxis-market-support-context")
        .is_some_and(|value| !value.is_empty())
}
