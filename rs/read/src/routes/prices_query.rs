//! FastAPI parameter validation runs before handler-level filter validation.
use super::{date_query, fields, integer, uuid};
use crate::{ApiError, Json};
use axum::http::StatusCode;
use serde_json::json;

#[derive(Clone, Copy)]
pub enum Surface {
    Summary,
    Reference,
    Export,
    Tape,
}

pub fn validate(raw: &str, surface: Surface) -> Result<(), ApiError> {
    let values = fields(raw);
    let mut errors = Vec::new();
    let mut collect = |result: Result<(), ApiError>| {
        if let Err(error) = result
            && let Some(entries) = error.detail.as_array()
        {
            errors.extend(entries.iter().cloned());
        }
    };
    if !matches!(surface, Surface::Tape) {
        collect(uuid(raw, "product_id").map(|_| ()));
    }
    collect(uuid(raw, "delivery_point_id").map(|_| ()));
    match surface {
        Surface::Summary => collect(integer(raw, "hours", 1, 168, 24).map(|_| ())),
        Surface::Reference => {
            for name in ["date_from", "date_to", "from", "to"] {
                collect(date_query(&values, name).map(|_| ()));
            }
            if let Some(value) = values.get("visibility")
                && !["internal", "external"].contains(&value.as_str())
            {
                collect(Err(ApiError {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    detail: json!([{"type":"literal_error","loc":["query","visibility"],"msg":"Input should be 'internal' or 'external'","input":value,"ctx":{"expected":"'internal' or 'external'"}}]),
                    retry_after: None,
                }));
            }
        }
        Surface::Export => {
            for name in ["from_date", "to_date"] {
                collect(date_query(&values, name).map(|_| ()));
            }
        }
        Surface::Tape => {
            if let Some(value) = values.get("demo_only")
                && ![
                    "true", "false", "1", "0", "on", "off", "yes", "no", "t", "f", "y", "n",
                ]
                .contains(&value.to_ascii_lowercase().as_str())
            {
                collect(Err(ApiError {
                    status: StatusCode::UNPROCESSABLE_ENTITY,
                    detail: json!([{"type":"bool_parsing","loc":["query","demo_only"],"msg":"Input should be a valid boolean, unable to interpret input","input":value}]),
                    retry_after: None,
                }));
            }
            collect(integer(raw, "skip", 0, i64::MAX, 0).map(|_| ()));
            collect(integer(raw, "limit", 1, 200, 50).map(|_| ()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail: Json::Array(errors),
            retry_after: None,
        })
    }
}
