//! FastAPI-shaped request validation errors shared by the watchlist routes.
//!
//! FastAPI reports path and body validation failures as one 422 list. Its errors
//! carry no documentation `url`, unlike errors raised by a route's own model.
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiError, ApiErrorKind};

pub fn unprocessable(errors: Vec<Value>) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: Value::Array(errors),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    }
}

/// Reword the `uuid` crate's diagnostic into Pydantic's: character positions
/// count from one, and a wrong length names the simple format.
fn pydantic_uuid_cause(cause: String) -> String {
    if cause.starts_with("invalid character:") {
        return cause
            .rsplit_once(" at ")
            .and_then(|(prefix, position)| {
                position
                    .parse::<usize>()
                    .ok()
                    .map(|position| format!("{prefix} at {}", position + 1))
            })
            .unwrap_or(cause);
    }
    if let Some(length) = cause.strip_prefix("invalid length: found ") {
        return format!("invalid length: expected length 32 for simple format, found {length}");
    }
    cause
}

/// A path UUID. The error item is ready to join other validation errors.
pub fn path_uuid(name: &str, raw: &str) -> Result<Uuid, Value> {
    Uuid::parse_str(raw).map_err(|cause| {
        let cause = pydantic_uuid_cause(cause.to_string());
        json!({"type":"uuid_parsing","loc":["path",name],
            "msg":format!("Input should be a valid UUID, {cause}"),
            "input":raw,"ctx":{"error":cause}})
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uuid_error_matches_the_pydantic_shape() {
        let error = path_uuid("watchlist_id", "01234567-89ab-cdef-0123-456789abcdeg").unwrap_err();
        assert_eq!(error["type"], "uuid_parsing");
        assert_eq!(error["loc"], json!(["path", "watchlist_id"]));
        assert_eq!(error["input"], "01234567-89ab-cdef-0123-456789abcdeg");
        let cause = error["ctx"]["error"].as_str().unwrap();
        assert!(cause.ends_with("found `g` at 36"), "{cause}");
        assert_eq!(
            error["msg"],
            format!("Input should be a valid UUID, {cause}")
        );
        assert!(error.get("url").is_none());
    }

    #[test]
    fn character_positions_count_from_one() {
        assert_eq!(
            pydantic_uuid_cause("invalid character: found `g` at 35".into()),
            "invalid character: found `g` at 36"
        );
    }

    #[test]
    fn a_wrong_length_names_the_simple_format() {
        assert_eq!(
            pydantic_uuid_cause("invalid length: found 5".into()),
            "invalid length: expected length 32 for simple format, found 5"
        );
    }

    #[test]
    fn other_diagnostics_pass_through() {
        assert_eq!(pydantic_uuid_cause("other".into()), "other");
    }

    #[test]
    fn a_valid_uuid_parses() {
        let id = path_uuid("watchlist_id", "70000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(id, Uuid::from_u128(0x70000000_0000_4000_8000_000000000001));
    }
}
