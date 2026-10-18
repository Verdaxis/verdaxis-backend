//! Compatibility dictionary ordering follows the same recorded public schemas
//! as the snapshot, after full legacy declaration validation.
use crate::{ApiError, Json};

pub fn ordered(value: &Json, schema: &str) -> Result<Json, ApiError> {
    if value.is_null() {
        return Ok(Json::Null);
    }
    let data: Json = serde_json::from_str(include_str!("orderbook_query.json"))
        .map_err(|_| ApiError::internal())?;
    let fields = data["public_terms_fields"][schema]
        .as_array()
        .ok_or_else(ApiError::internal)?;
    let mut result = serde_json::Map::new();
    for name in fields {
        let name = name.as_str().ok_or_else(ApiError::internal)?;
        let child = match name {
            "quality_evidence" | "sustainability_evidence" => ordered(&value[name], name)?,
            "results" => Json::Array(
                value[name]
                    .as_array()
                    .ok_or_else(ApiError::internal)?
                    .iter()
                    .map(|item| ordered(item, "quality_result"))
                    .collect::<Result<_, _>>()?,
            ),
            _ => value[name].clone(),
        };
        result.insert(name.into(), child);
    }
    Ok(Json::Object(result))
}
