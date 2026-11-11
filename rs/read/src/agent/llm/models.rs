//! The Phase A model allow-list and shared output budget.
use super::{ErrorCode, Message};
use serde_json::{Value, json};

pub struct ModelSettings {
    pub id: &'static str,
    /// Hidden reasoning shares the output budget with the visible answer.
    pub reasoning_headroom: u32,
    pub output_limit: u32,
}

pub const MODELS: [ModelSettings; 1] = [ModelSettings {
    id: "deepseek-v4.1-flash",
    reasoning_headroom: 4096,
    output_limit: 32768,
}];

pub fn settings(id: &str) -> Result<&'static ModelSettings, ErrorCode> {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .ok_or(ErrorCode::ModelNotAllowed)
}

pub(super) fn payload(
    model: &str,
    tokens: u32,
    thinking: bool,
    messages: &[Message],
) -> Result<Value, ErrorCode> {
    let settings = settings(model)?;
    if tokens == 0
        || messages.is_empty()
        || !messages
            .iter()
            .any(|message| message.content.to_ascii_lowercase().contains("json"))
    {
        return Err(ErrorCode::InvalidRequest);
    }
    let headroom = if thinking {
        settings.reasoning_headroom
    } else {
        0
    };
    let budget = tokens
        .checked_add(headroom)
        .filter(|budget| *budget <= settings.output_limit)
        .ok_or(ErrorCode::InvalidRequest)?;
    Ok(json!({
        "model": settings.id, "max_tokens": budget, "stream": false,
        "thinking": {"type": if thinking { "enabled" } else { "disabled" }},
        "temperature": 0, "response_format": {"type": "json_object"}, "messages": messages
    }))
}
