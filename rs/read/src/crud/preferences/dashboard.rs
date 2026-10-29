//! Validation for the `dashboard` preference namespace (home-page layout).
//!
//! Python has no such namespace, so the schema is native to Rust:
//!
//! ```json
//! {"version": 1,
//!  "widgets": [{"id": "needs_attention", "type": "needs_attention",
//!               "x": 0, "y": 4, "w": 12, "h": 3}]}
//! ```
//!
//! Unknown keys are refused. Diagnostics use the same pydantic-style entries
//! as the other namespaces, so the client reads one error shape.

use serde_json::{Map, Value, json};

use super::diagnostic;

/// Widget type ids the home page can render. The frontend keeps the same list
/// in `src/components/dashboard/dashboardLayout.ts`.
pub const WIDGET_TYPES: &[&str] = &[
    "quick_actions",
    "trade_summary",
    "needs_attention",
    "market_radar",
    "supplier_demand",
    "forward_curve",
    "order_book",
    "trade_tape",
    "my_trades",
];

const LAYOUT_VERSION: i64 = 1;
const MAX_WIDGETS: usize = 24;
const MAX_ID_LENGTH: usize = 64;
const GRID_COLUMNS: i64 = 12;
const MAX_X: i64 = GRID_COLUMNS - 1;
const MAX_Y: i64 = 199;
const MAX_SPAN: i64 = 12;
const ID_PATTERN: &str = "^[A-Za-z0-9_-]+$";
const CLASS_LAYOUT: &str = "DashboardPreferences";
const CLASS_WIDGET: &str = "DashboardWidget";
const WIDGET_FIELDS: &[&str] = &["id", "type", "x", "y", "w", "h"];

/// Returns every schema violation found in `value`, or an empty list.
pub(super) fn validate(value: &Value) -> Vec<Value> {
    let Some(object) = value.as_object() else {
        return vec![with_context(
            diagnostic(
                "model_type",
                json!([]),
                &format!("Input should be a valid dictionary or instance of {CLASS_LAYOUT}"),
                value,
            ),
            json!({"class_name": CLASS_LAYOUT}),
        )];
    };
    let mut errors = vec![];
    check_version(object, value, &mut errors);
    check_widgets(object, value, &mut errors);
    for (key, input) in object {
        if key != "version" && key != "widgets" {
            errors.push(extra_forbidden(json!([key]), input));
        }
    }
    errors
}

fn with_context(mut error: Value, context: Value) -> Value {
    error["ctx"] = context;
    error
}

fn extra_forbidden(loc: Value, input: &Value) -> Value {
    diagnostic(
        "extra_forbidden",
        loc,
        "Extra inputs are not permitted",
        input,
    )
}

fn missing(loc: Value, owner: &Value) -> Value {
    diagnostic("missing", loc, "Field required", owner)
}

fn check_version(object: &Map<String, Value>, owner: &Value, errors: &mut Vec<Value>) {
    match object.get("version") {
        None => errors.push(missing(json!(["version"]), owner)),
        Some(input) if input.as_i64() == Some(LAYOUT_VERSION) => {}
        Some(input) => errors.push(with_context(
            diagnostic(
                "literal_error",
                json!(["version"]),
                &format!("Input should be {LAYOUT_VERSION}"),
                input,
            ),
            json!({"expected": LAYOUT_VERSION.to_string()}),
        )),
    }
}

fn check_widgets(object: &Map<String, Value>, owner: &Value, errors: &mut Vec<Value>) {
    let Some(input) = object.get("widgets") else {
        errors.push(missing(json!(["widgets"]), owner));
        return;
    };
    let Some(items) = input.as_array() else {
        errors.push(diagnostic(
            "list_type",
            json!(["widgets"]),
            "Input should be a valid list",
            input,
        ));
        return;
    };
    if items.len() > MAX_WIDGETS {
        errors.push(with_context(
            diagnostic(
                "too_long",
                json!(["widgets"]),
                &format!(
                    "List should have at most {MAX_WIDGETS} items after validation, not {}",
                    items.len()
                ),
                input,
            ),
            json!({"field_type":"List","max_length":MAX_WIDGETS,"actual_length":items.len()}),
        ));
        return;
    }
    let mut ids_seen = vec![];
    for (index, item) in items.iter().enumerate() {
        let ok = check_widget(index, item, errors);
        if ok {
            if let Some(id) = item["id"].as_str() {
                ids_seen.push(id);
            }
            if let Some(problem) = column_overflow(item) {
                errors.push(with_context(
                    diagnostic(
                        "value_error",
                        json!(["widgets", index]),
                        &format!("Value error, {problem}"),
                        item,
                    ),
                    json!({"error": problem}),
                ));
            }
        }
    }
    if has_duplicates(&ids_seen) {
        let problem = "widget ids must be unique";
        errors.push(with_context(
            diagnostic(
                "value_error",
                json!(["widgets"]),
                &format!("Value error, {problem}"),
                input,
            ),
            json!({"error": problem}),
        ));
    }
}

fn has_duplicates(ids: &[&str]) -> bool {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.windows(2).any(|pair| pair[0] == pair[1])
}

/// The only rule that spans two fields of one widget.
fn column_overflow(widget: &Value) -> Option<&'static str> {
    let x = widget["x"].as_i64()?;
    let w = widget["w"].as_i64()?;
    (x + w > GRID_COLUMNS).then_some("widget must fit within 12 columns (x + w <= 12)")
}

/// Validates one widget; returns true when every field is individually valid.
fn check_widget(index: usize, item: &Value, errors: &mut Vec<Value>) -> bool {
    let Some(object) = item.as_object() else {
        errors.push(with_context(
            diagnostic(
                "model_type",
                json!(["widgets", index]),
                &format!("Input should be a valid dictionary or instance of {CLASS_WIDGET}"),
                item,
            ),
            json!({"class_name": CLASS_WIDGET}),
        ));
        return false;
    };
    let before = errors.len();
    for field in WIDGET_FIELDS {
        let loc = json!(["widgets", index, field]);
        let Some(input) = object.get(*field) else {
            errors.push(missing(loc, item));
            continue;
        };
        match *field {
            "id" => check_id(loc, input, errors),
            "type" => check_type(loc, input, errors),
            "x" => check_integer(loc, input, 0, MAX_X, errors),
            "y" => check_integer(loc, input, 0, MAX_Y, errors),
            _ => check_integer(loc, input, 1, MAX_SPAN, errors),
        }
    }
    for (key, input) in object {
        if !WIDGET_FIELDS.contains(&key.as_str()) {
            errors.push(extra_forbidden(json!(["widgets", index, key]), input));
        }
    }
    errors.len() == before
}

fn check_id(loc: Value, input: &Value, errors: &mut Vec<Value>) {
    let Some(text) = input.as_str() else {
        errors.push(diagnostic(
            "string_type",
            loc,
            "Input should be a valid string",
            input,
        ));
        return;
    };
    let length = text.chars().count();
    if length < 1 {
        errors.push(with_context(
            diagnostic(
                "string_too_short",
                loc,
                "String should have at least 1 character",
                input,
            ),
            json!({"min_length": 1}),
        ));
    } else if length > MAX_ID_LENGTH {
        errors.push(with_context(
            diagnostic(
                "string_too_long",
                loc,
                &format!("String should have at most {MAX_ID_LENGTH} characters"),
                input,
            ),
            json!({"max_length": MAX_ID_LENGTH}),
        ));
    } else if !text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        errors.push(with_context(
            diagnostic(
                "string_pattern_mismatch",
                loc,
                &format!("String should match pattern '{ID_PATTERN}'"),
                input,
            ),
            json!({"pattern": ID_PATTERN}),
        ));
    }
}

fn check_type(loc: Value, input: &Value, errors: &mut Vec<Value>) {
    if input
        .as_str()
        .is_some_and(|text| WIDGET_TYPES.contains(&text))
    {
        return;
    }
    let names: Vec<String> = WIDGET_TYPES
        .iter()
        .map(|name| format!("'{name}'"))
        .collect();
    let expected = format!(
        "{} or {}",
        names[..names.len() - 1].join(", "),
        names[names.len() - 1]
    );
    errors.push(with_context(
        diagnostic(
            "literal_error",
            loc,
            &format!("Input should be {expected}"),
            input,
        ),
        json!({"expected": expected}),
    ));
}

/// Strict integer: JSON booleans, strings and fractional numbers are refused.
fn check_integer(loc: Value, input: &Value, min: i64, max: i64, errors: &mut Vec<Value>) {
    let Some(number) = input.as_number().filter(|n| n.is_i64() || n.is_u64()) else {
        errors.push(diagnostic(
            "int_type",
            loc,
            "Input should be a valid integer",
            input,
        ));
        return;
    };
    // A u64 above i64::MAX is always above every upper bound.
    let number = number.as_i64().unwrap_or(i64::MAX);
    if number < min {
        errors.push(with_context(
            diagnostic(
                "greater_than_equal",
                loc,
                &format!("Input should be greater than or equal to {min}"),
                input,
            ),
            json!({"ge": min}),
        ));
    } else if number > max {
        errors.push(with_context(
            diagnostic(
                "less_than_equal",
                loc,
                &format!("Input should be less than or equal to {max}"),
                input,
            ),
            json!({"le": max}),
        ));
    }
}
