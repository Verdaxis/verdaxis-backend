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

/// A required string field with no bound.
fn required_string(
    body: &Value,
    object: &serde_json::Map<String, Value>,
    path: &[&str],
) -> Result<String, Value> {
    let name = path[path.len() - 1];
    match object.get(name) {
        None => Err(body_field_error(path, "missing", "Field required", body)),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(other) => Err(body_field_error(
            path,
            "string_type",
            "Input should be a valid string",
            other,
        )),
    }
}

/// A required UUID field as Pydantic validates it from JSON: a string only.
fn required_uuid(
    body: &Value,
    object: &serde_json::Map<String, Value>,
    path: &[&str],
) -> Result<Uuid, Value> {
    let name = path[path.len() - 1];
    match object.get(name) {
        None => Err(body_field_error(path, "missing", "Field required", body)),
        Some(Value::String(text)) => Uuid::parse_str(text).map_err(|cause| {
            let cause = pydantic_uuid_cause(cause.to_string());
            let mut error = body_field_error(
                path,
                "uuid_parsing",
                &format!("Input should be a valid UUID, {cause}"),
                &Value::String(text.clone()),
            );
            error["ctx"] = json!({"error":cause});
            error
        }),
        Some(other) => Err(body_field_error(
            path,
            "uuid_type",
            "UUID input should be a string, bytes or UUID object",
            other,
        )),
    }
}

/// The two target shapes, chosen by `target_type`.
#[derive(Debug, PartialEq)]
pub enum TargetCreate {
    Slice {
        market_product_code: String,
        delivery_point_id: Uuid,
        availability_window_code: String,
    },
    Pin {
        order_id: Uuid,
    },
}

/// Python's `str()` of a JSON value that reached a discriminator lookup.
fn python_text(tag: &Value) -> String {
    match tag {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// `WatchlistTargetCreate`: a discriminated union on `target_type`. Errors for a
/// chosen member carry the tag in their location, as Pydantic reports them.
pub fn validate_target(body: &Value) -> Result<TargetCreate, Vec<Value>> {
    let Some(object) = body.as_object() else {
        return Err(vec![body_shape_error(body)]);
    };
    let Some(tag) = object.get("target_type") else {
        return Err(vec![json!({"type":"union_tag_not_found","loc":["body"],
            "msg":"Unable to extract tag using discriminator 'target_type'","input":body,
            "ctx":{"discriminator":"'target_type'"}})]);
    };
    match tag.as_str() {
        Some("SLICE") => {
            let fields = [
                required_string(body, object, &["SLICE", "market_product_code"]),
                required_uuid(body, object, &["SLICE", "delivery_point_id"])
                    .map(|id| id.to_string()),
                required_string(body, object, &["SLICE", "availability_window_code"]),
            ];
            let errors: Vec<Value> = fields
                .iter()
                .filter_map(|field| field.as_ref().err().cloned())
                .collect();
            if !errors.is_empty() {
                return Err(errors);
            }
            let [code, point, window] = fields.map(|field| field.expect("checked"));
            Ok(TargetCreate::Slice {
                market_product_code: code,
                delivery_point_id: Uuid::parse_str(&point).expect("validated"),
                availability_window_code: window,
            })
        }
        Some("PIN") => required_uuid(body, object, &["PIN", "order_id"])
            .map(|order_id| TargetCreate::Pin { order_id })
            .map_err(|error| vec![error]),
        _ => {
            let tag = python_text(tag);
            Err(vec![json!({"type":"union_tag_invalid","loc":["body"],
                "msg":format!("Input tag '{tag}' found using 'target_type' does not match any of the expected tags: 'SLICE', 'PIN'"),
                "input":body,
                "ctx":{"discriminator":"'target_type'","tag":tag,"expected_tags":"'SLICE', 'PIN'"}})])
        }
    }
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
    fn a_slice_body_is_accepted_and_extra_fields_are_ignored() {
        let point = "70000000-0000-4000-8000-000000000001";
        let body = json!({"target_type":"SLICE","market_product_code":"B30",
            "delivery_point_id":point,"availability_window_code":"SPOT","order_id":5});
        assert_eq!(
            validate_target(&body).unwrap(),
            TargetCreate::Slice {
                market_product_code: "B30".into(),
                delivery_point_id: Uuid::parse_str(point).unwrap(),
                availability_window_code: "SPOT".into(),
            }
        );
    }

    #[test]
    fn a_pin_body_needs_only_an_order() {
        let order = "74000000-0000-4000-8000-000000000001";
        let body = json!({"target_type":"PIN","order_id":order});
        assert_eq!(
            validate_target(&body).unwrap(),
            TargetCreate::Pin {
                order_id: Uuid::parse_str(order).unwrap()
            }
        );
    }

    #[test]
    fn slice_errors_follow_field_order_and_carry_the_tag() {
        let body =
            json!({"target_type":"SLICE","delivery_point_id":"x","availability_window_code":4});
        let errors = validate_target(&body).unwrap_err();
        let located: Vec<_> = errors
            .iter()
            .map(|e| (e["type"].clone(), e["loc"].clone()))
            .collect();
        assert_eq!(
            located,
            vec![
                (
                    json!("missing"),
                    json!(["body", "SLICE", "market_product_code"])
                ),
                (
                    json!("uuid_parsing"),
                    json!(["body", "SLICE", "delivery_point_id"])
                ),
                (
                    json!("string_type"),
                    json!(["body", "SLICE", "availability_window_code"])
                ),
            ]
        );
        assert_eq!(errors[0]["input"], body);
        assert_eq!(errors[1]["input"], "x");
        assert_eq!(errors[2]["input"], 4);
    }

    #[test]
    fn a_uuid_field_rejects_a_non_string() {
        let errors = validate_target(&json!({"target_type":"PIN","order_id":7})).unwrap_err();
        assert_eq!(errors[0]["type"], "uuid_type");
        assert_eq!(
            errors[0]["msg"],
            "UUID input should be a string, bytes or UUID object"
        );
    }

    #[test]
    fn the_discriminator_errors_name_the_tag() {
        let missing = validate_target(&json!({"order_id":"x"})).unwrap_err();
        assert_eq!(missing[0]["type"], "union_tag_not_found");
        assert_eq!(missing[0]["ctx"], json!({"discriminator":"'target_type'"}));
        assert_eq!(missing[0]["loc"], json!(["body"]));
        for (tag, shown) in [
            (json!("slice"), "slice"),
            (json!(5), "5"),
            (json!(true), "True"),
            (Value::Null, "None"),
        ] {
            let invalid = validate_target(&json!({"target_type":tag})).unwrap_err();
            assert_eq!(invalid[0]["type"], "union_tag_invalid");
            assert_eq!(
                invalid[0]["ctx"],
                json!({"discriminator":"'target_type'","tag":shown,"expected_tags":"'SLICE', 'PIN'"})
            );
            assert_eq!(
                invalid[0]["msg"],
                format!(
                    "Input tag '{shown}' found using 'target_type' does not match any of the expected tags: 'SLICE', 'PIN'"
                )
            );
        }
    }

    #[test]
    fn a_target_body_that_is_not_an_object_names_its_shape() {
        assert_eq!(
            validate_target(&Value::Null).unwrap_err()[0]["type"],
            "missing"
        );
        assert_eq!(
            validate_target(&json!("SLICE")).unwrap_err()[0]["type"],
            "model_attributes_type"
        );
    }

    #[test]
    fn a_valid_uuid_parses() {
        let id = path_uuid("watchlist_id", "70000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(id, Uuid::from_u128(0x70000000_0000_4000_8000_000000000001));
    }
}
