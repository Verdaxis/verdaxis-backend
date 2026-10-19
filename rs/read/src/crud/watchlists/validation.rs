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

/// JSON syntax is checked before authentication. An empty body is "no body".
pub fn parse_body(raw: &[u8]) -> Result<Value, ApiError> {
    if raw.is_empty() {
        return Ok(Value::Null);
    }
    crate::auth::request::parse_json_body(raw)
}

/// Errors for a body that is not a JSON object. FastAPI reports an absent or
/// null body as missing and every other non-object as a model type error.
fn body_shape_error(body: &Value) -> Value {
    if body.is_null() {
        json!({"type":"missing","loc":["body"],"msg":"Field required","input":body})
    } else {
        json!({"type":"model_attributes_type","loc":["body"],
            "msg":"Input should be a valid dictionary or object to extract fields from",
            "input":body})
    }
}

pub fn body_field_error(path: &[&str], kind: &str, message: &str, input: &Value) -> Value {
    let mut location = vec![json!("body")];
    location.extend(path.iter().map(|part| json!(part)));
    json!({"type":kind,"loc":location,"msg":message,"input":input})
}

/// A required string with Pydantic's length bounds, counted in code points.
fn bounded_string(
    body: &Value,
    object: &serde_json::Map<String, Value>,
    name: &str,
    length: std::ops::RangeInclusive<usize>,
) -> Result<String, Value> {
    let Some(value) = object.get(name) else {
        return Err(body_field_error(&[name], "missing", "Field required", body));
    };
    let Some(text) = value.as_str() else {
        return Err(body_field_error(
            &[name],
            "string_type",
            "Input should be a valid string",
            value,
        ));
    };
    let characters = text.chars().count();
    if characters < *length.start() {
        let mut error = body_field_error(
            &[name],
            "string_too_short",
            "String should have at least 1 character",
            value,
        );
        error["ctx"] = json!({"min_length":length.start()});
        return Err(error);
    }
    if characters > *length.end() {
        let mut error = body_field_error(
            &[name],
            "string_too_long",
            &format!("String should have at most {} characters", length.end()),
            value,
        );
        error["ctx"] = json!({"max_length":length.end()});
        return Err(error);
    }
    Ok(text.to_owned())
}

/// `WatchlistCreateRequest`: one name of 1 to 100 characters. Whitespace counts.
pub fn validate_create(body: &Value) -> Result<String, Vec<Value>> {
    let Some(object) = body.as_object() else {
        return Err(vec![body_shape_error(body)]);
    };
    bounded_string(body, object, "name", 1..=100).map_err(|error| vec![error])
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
    fn a_name_of_one_to_one_hundred_characters_is_accepted() {
        for name in ["a", "   ", &"é".repeat(100)] {
            assert_eq!(validate_create(&json!({"name":name})).unwrap(), name);
        }
    }

    #[test]
    fn extra_fields_are_ignored() {
        let body = json!({"name":"x","kind":"RADAR_DEFAULT"});
        assert_eq!(validate_create(&body).unwrap(), "x");
    }

    #[test]
    fn name_length_errors_carry_the_bound() {
        let short = validate_create(&json!({"name":""})).unwrap_err();
        assert_eq!(short[0]["type"], "string_too_short");
        assert_eq!(short[0]["ctx"], json!({"min_length":1}));
        assert_eq!(short[0]["loc"], json!(["body", "name"]));
        let long = validate_create(&json!({"name":"x".repeat(101)})).unwrap_err();
        assert_eq!(long[0]["type"], "string_too_long");
        assert_eq!(long[0]["msg"], "String should have at most 100 characters");
        assert_eq!(long[0]["ctx"], json!({"max_length":100}));
    }

    #[test]
    fn missing_and_mistyped_names_echo_their_input() {
        let body = json!({"other":1});
        let missing = validate_create(&body).unwrap_err();
        assert_eq!(missing[0]["type"], "missing");
        assert_eq!(missing[0]["input"], body);
        let typed = validate_create(&json!({"name":5})).unwrap_err();
        assert_eq!(typed[0]["type"], "string_type");
        assert_eq!(typed[0]["input"], 5);
    }

    #[test]
    fn a_body_that_is_not_an_object_names_its_shape() {
        let null = validate_create(&Value::Null).unwrap_err();
        assert_eq!(null[0]["type"], "missing");
        assert_eq!(null[0]["loc"], json!(["body"]));
        let array = validate_create(&json!(["name"])).unwrap_err();
        assert_eq!(array[0]["type"], "model_attributes_type");
        assert_eq!(array[0]["input"], json!(["name"]));
    }

    #[test]
    fn an_empty_body_is_null_and_bad_syntax_is_refused() {
        assert_eq!(parse_body(b"").unwrap(), Value::Null);
        assert_eq!(
            parse_body(b"{\"name\":\"a\"}").unwrap(),
            json!({"name":"a"})
        );
        assert_eq!(
            parse_body(b"{").unwrap_err().status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn a_valid_uuid_parses() {
        let id = path_uuid("watchlist_id", "70000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(id, Uuid::from_u128(0x70000000_0000_4000_8000_000000000001));
    }
}
