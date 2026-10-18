//! Python-compatible tool argument validation, before database admission.
use super::transport::python_string_repr;
use crate::Json;
use serde_json::json;

use crate::routes::prices::date_parser;

pub fn repr(value: &Json) -> String {
    match value {
        Json::String(text) => python_string_repr(text),
        Json::Null => "None".into(),
        Json::Bool(value) => if *value { "True" } else { "False" }.into(),
        Json::Array(values) => format!(
            "[{}]",
            values.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Json::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_string_repr(key), repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}
fn type_name(value: &Json) -> &'static str {
    match value {
        Json::String(_) => "str",
        Json::Null => "NoneType",
        Json::Bool(_) => "bool",
        Json::Array(_) => "list",
        Json::Object(_) => "dict",
        Json::Number(n) if n.is_i64() || n.is_u64() => "int",
        _ => "float",
    }
}
pub(super) fn diagnostic(path: &str, message: &str, kind: &str, input: &Json) -> String {
    let text = repr(input);
    let chars: Vec<_> = text.chars().collect();
    let text = if chars.len() > 50 {
        format!(
            "{}...{}",
            chars[..25].iter().collect::<String>(),
            chars[chars.len() - 24..].iter().collect::<String>()
        )
    } else {
        text
    };
    format!(
        "{path}\n  {message} [type={kind}, input_value={text}, input_type={}]\n    For further information visit https://errors.pydantic.dev/2.13/v/{kind}",
        type_name(input)
    )
}
fn decimal_digit(character: char) -> Option<u8> {
    static ZEROS: std::sync::LazyLock<Vec<u32>> = std::sync::LazyLock::new(|| {
        let data: Json = serde_json::from_str(include_str!(
            "../../../../deploy/rust/python-decimal-digits.json"
        ))
        .expect("Python Unicode digits");
        data["zero_codepoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect()
    });
    let code = character as u32;
    ZEROS.iter().find_map(|zero| {
        code.checked_sub(*zero)
            .filter(|digit| *digit < 10)
            .map(|digit| digit as u8)
    })
}

fn child(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.into()
    } else {
        format!("{path}.{field}")
    }
}

struct Validator<'a> {
    definitions: &'a Json,
    errors: Vec<Json>,
}
impl Validator<'_> {
    fn error(&mut self, path: &str, message: &str, kind: &str, input: &Json) {
        let location: Vec<Json> = path.split('.').filter(|part| !part.is_empty())
            .map(|part| part.parse::<usize>().map_or_else(|_| json!(part), |index| json!(index)))
            .collect();
        self.errors.push(json!({"type": kind, "loc": location, "msg": message, "input": input}));
    }
    fn value(&mut self, schema: &Json, input: &Json, path: &str) -> Json {
        if let Some(reference) = schema["$ref"].as_str() {
            return self.value(
                &self.definitions[reference.rsplit('/').next().unwrap()],
                input,
                path,
            );
        }
        if let Some(choices) = schema["anyOf"].as_array() {
            if input.is_null() && choices.iter().any(|s| s["type"] == "null") {
                return Json::Null;
            }
            return self.value(&choices[0], input, path);
        }
        if let Some(choices) = schema["enum"].as_array() {
            if !choices.contains(input) {
                let names: Vec<_> = choices.iter().map(repr).collect();
                let expected = if names.len() == 2 {
                    names.join(" or ")
                } else {
                    format!(
                        "{} or {}",
                        names[..names.len() - 1].join(", "),
                        names.last().unwrap()
                    )
                };
                self.error(path, &format!("Input should be {expected}"), "enum", input);
            }
            return input.clone();
        }
        match schema["type"].as_str().unwrap_or("") {
            "object" => self.object(schema, input, path),
            "array" => self.array(schema, input, path),
            "number" => self.decimal(schema, input, path),
            "integer" => {
                let text = input
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| input.to_string());
                let parsed = input
                    .as_bool()
                    .map(i128::from)
                    .or_else(|| crate::runtime::http::python_integer(&text));
                if let Some(value) = parsed {
                    let bounded = json!(value.to_string());
                    self.bounds(schema, &bounded, input, path);
                    json!(value.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
                } else {
                    let (message, kind) = if input.is_string() {
                        (
                            "Input should be a valid integer, unable to parse string as an integer",
                            "int_parsing",
                        )
                    } else if input.is_number() {
                        (
                            "Input should be a valid integer, got a number with a fractional part",
                            "int_from_float",
                        )
                    } else {
                        ("Input should be a valid integer", "int_type")
                    };
                    self.error(path, message, kind, input);
                    input.clone()
                }
            }
            "string" if schema["format"] == "date" => {
                if input.is_string() || input.is_number() {
                    let text = input
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| input.to_string());
                    match date_parser::parse(&text, path) {
                        Ok(date) => json!(date.to_string()),
                        Err(error) => {
                            let e = &error.detail[0];
                            self.error(
                                path,
                                e["msg"].as_str().unwrap(),
                                e["type"].as_str().unwrap(),
                                input,
                            );
                            input.clone()
                        }
                    }
                } else {
                    self.error(path, "Input should be a valid date", "date_type", input);
                    input.clone()
                }
            }
            "string" => {
                if let Some(text) = input.as_str() {
                    if schema["maxLength"]
                        .as_u64()
                        .is_some_and(|max| text.chars().count() > max as usize)
                    {
                        self.error(
                            path,
                            &format!(
                                "String should have at most {} characters",
                                schema["maxLength"]
                            ),
                            "string_too_long",
                            input,
                        );
                    }
                    if schema["pattern"] == r"^\d{7}$"
                        && (text.chars().count() != 7
                            || !text.chars().all(|c| decimal_digit(c).is_some()))
                    {
                        self.error(
                            path,
                            r"String should match pattern '^\d{7}$'",
                            "string_pattern_mismatch",
                            input,
                        );
                    }
                    if path.ends_with(".from_port") || path.ends_with(".to_port") {
                        match super::voyage::canonical_port(text) {
                            Ok(port) => return json!(port),
                            Err(error) => self.error(
                                path,
                                &format!("Value error, {error}"),
                                "value_error",
                                input,
                            ),
                        }
                    }
                } else {
                    self.error(path, "Input should be a valid string", "string_type", input);
                }
                input.clone()
            }
            _ => input.clone(),
        }
    }
    fn decimal(&mut self, schema: &Json, input: &Json, path: &str) -> Json {
        if !input.is_string() && !input.is_number() {
            self.error(
                path,
                "Decimal input should be an integer, float, string or Decimal object",
                "decimal_type",
                input,
            );
            return input.clone();
        }
        let text = input
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| input.to_string());
        let text: String = text
            .trim_matches(crate::market::whitespace::is_python_whitespace)
            .chars()
            .filter(|c| *c != '_')
            .map(|c| {
                decimal_digit(c)
                    .map(|digit| char::from(b'0' + digit))
                    .unwrap_or(c)
            })
            .collect();
        if matches!(
            text.to_lowercase().trim_start_matches(['+', '-']),
            "nan" | "snan" | "inf" | "infinity"
        ) {
            self.error(
                path,
                "Input should be a finite number",
                "finite_number",
                input,
            );
            return input.clone();
        }
        match crate::market::amount::Amount::parse(&text) {
            Ok(value) => {
                let output = json!(value.text());
                self.bounds(schema, &output, input, path);
                output
            }
            Err(_) => {
                self.error(
                    path,
                    "Input should be a valid decimal",
                    "decimal_parsing",
                    input,
                );
                input.clone()
            }
        }
    }
    fn bounds(&mut self, schema: &Json, value: &Json, input: &Json, path: &str) {
        use std::cmp::Ordering;
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let Ok(value) = crate::market::amount::Amount::parse(&text) else {
            return;
        };
        for (key, kind, phrase, fail) in [
            (
                "exclusiveMinimum",
                "greater_than",
                "greater than",
                Ordering::Greater,
            ),
            (
                "minimum",
                "greater_than_equal",
                "greater than or equal to",
                Ordering::Less,
            ),
            (
                "maximum",
                "less_than_equal",
                "less than or equal to",
                Ordering::Greater,
            ),
        ] {
            if let Some(limit) = schema.get(key) {
                let text = limit.to_string();
                let text = text.strip_suffix(".0").unwrap_or(&text);
                let bound = crate::market::amount::Amount::parse(text).unwrap();
                let order = value.compare(&bound);
                if (key == "exclusiveMinimum" && order != fail)
                    || (key != "exclusiveMinimum" && order == fail)
                {
                    self.error(
                        path,
                        &format!("Input should be {phrase} {text}"),
                        kind,
                        input,
                    );
                    return;
                }
            }
        }
    }
    fn array(&mut self, schema: &Json, input: &Json, path: &str) -> Json {
        let Some(values) = input.as_array() else {
            self.error(path, "Input should be a valid tuple", "tuple_type", input);
            return input.clone();
        };
        let mut valid = 0;
        let output: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let before = self.errors.len();
                let result = self.value(&schema["items"], value, &child(path, &index.to_string()));
                valid += usize::from(before == self.errors.len());
                result
            })
            .collect();
        if schema["minItems"]
            .as_u64()
            .is_some_and(|min| valid < min as usize)
        {
            self.error(
                path,
                &format!(
                    "Tuple should have at least {} item after validation, not {valid}",
                    schema["minItems"]
                ),
                "too_short",
                input,
            );
        }
        if schema["maxItems"]
            .as_u64()
            .is_some_and(|max| valid > max as usize)
        {
            self.error(
                path,
                &format!(
                    "Tuple should have at most {} items after validation, not {}",
                    schema["maxItems"],
                    values.len()
                ),
                "too_long",
                input,
            );
        }
        if valid == values.len()
            && schema["maxItems"]
                .as_u64()
                .is_none_or(|max| valid <= max as usize)
            && path.ends_with(".compatible_fuels")
        {
            for (index, value) in values.iter().enumerate() {
                if values[..index].contains(value) {
                    self.error(
                        path,
                        "Value error, compatible_fuels must not repeat a fuel",
                        "value_error",
                        input,
                    );
                    break;
                }
            }
        }
        json!(output)
    }
    fn object(&mut self, schema: &Json, input: &Json, path: &str) -> Json {
        let initial_errors = self.errors.len();
        let Some(fields) = input.as_object() else {
            if schema.get("properties").is_none() {
                self.error(
                    path,
                    "Input should be a valid dictionary",
                    "dict_type",
                    input,
                );
                return input.clone();
            }
            let title = schema["title"].as_str().unwrap_or("dictionary");
            self.error(
                path,
                &format!("Input should be a valid dictionary or instance of {title}"),
                "model_type",
                input,
            );
            return input.clone();
        };
        let mut output = serde_json::Map::new();
        if let Some(properties) = schema["properties"].as_object() {
            for (name, property) in properties {
                let location = child(path, name);
                if let Some(value) = fields.get(name) {
                    output.insert(name.clone(), self.value(property, value, &location));
                } else if schema["required"]
                    .as_array()
                    .is_some_and(|required| required.contains(&json!(name)))
                {
                    self.error(&location, "Field required", "missing", input);
                } else if let Some(default) = property.get("default") {
                    output.insert(name.clone(), default.clone());
                } else if property["type"] == "object" || property.get("$ref").is_some() {
                    output.insert(name.clone(), self.value(property, &json!({}), &location));
                }
            }
            if schema["additionalProperties"] == false {
                for (name, value) in fields {
                    if !properties.contains_key(name) {
                        self.error(
                            &child(path, name),
                            "Extra inputs are not permitted",
                            "extra_forbidden",
                            value,
                        );
                    }
                }
            }
        } else {
            let before = self.errors.len();
            for (name, value) in fields {
                if schema.get("propertyNames").is_some() {
                    self.value(
                        &schema["propertyNames"],
                        &json!(name),
                        &child(&child(path, name), "[key]"),
                    );
                }
                output.insert(
                    name.clone(),
                    self.value(&schema["additionalProperties"], value, &child(path, name)),
                );
            }
            if before == self.errors.len() {
                for (key, value) in &output {
                    let decimal = crate::market::amount::Amount::parse(value.as_str().unwrap()).unwrap();
                    let zero = crate::market::amount::Amount::parse("0").unwrap();
                    let price = path.ends_with(".vlsfo_price_usd_per_mt");
                    let high = crate::market::amount::Amount::parse(if price { "10000" } else { "200" })
                        .unwrap();
                    let low_order = decimal.compare(&zero);
                    if low_order == std::cmp::Ordering::Less
                        || (price && low_order == std::cmp::Ordering::Equal)
                        || decimal.compare(&high) == std::cmp::Ordering::Greater
                    {
                        let message = if price {
                            format!(
                                "Value error, VLSFO price for {key} must be above 0 and at most 10000 USD/t"
                            )
                        } else {
                            format!(
                                "Value error, GHG intensity for {key} must be between 0 and 200 gCO2e/MJ"
                            )
                        };
                        self.error(path, &message, "value_error", input);
                        break;
                    }
                }
            }
            if before == self.errors.len() && path.ends_with(".vlsfo_price_usd_per_mt") {
                let mut canonical = serde_json::Map::new();
                for (port, price) in &output {
                    match super::voyage::canonical_port(port) {
                        Ok(port) => {
                            canonical.insert(port, price.clone());
                        }
                        Err(error) => {
                            self.error(
                                path,
                                &format!("Value error, {error}"),
                                "value_error",
                                input,
                            );
                            break;
                        }
                    }
                }
                output = canonical;
            }
        }
        if self.errors.len() == initial_errors {
            if schema["title"] == "VoyageLegInput" && output["from_port"] == output["to_port"] {
                self.error(
                    path,
                    "Value error, A leg must connect two different ports",
                    "value_error",
                    input,
                );
            }
            if schema["title"] == "VoyagePlanRequest" {
                for pair in output["legs"].as_array().unwrap().windows(2) {
                    if pair[0]["to_port"] != pair[1]["from_port"] {
                        self.error(path, &format!("Value error, Legs must connect: a leg ends at {} but the next leg starts at {}", pair[0]["to_port"].as_str().unwrap(), pair[1]["from_port"].as_str().unwrap()), "value_error", input);
                        break;
                    }
                }
            }
        }
        Json::Object(output)
    }
}

pub fn tool_arguments(tool: &str, arguments: &mut Json) -> Result<(), String> {
    static SCHEMAS: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../../deploy/rust/mcp-tools.json"))
            .expect("tool schemas")
    });
    let Some(schema) = SCHEMAS["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == tool)
    else {
        return Ok(());
    };
    let schema = &schema["inputSchema"];
    let mut validator = Validator {
        definitions: &schema["$defs"],
        errors: Vec::new(),
    };
    let mut parsed = arguments.clone();
    // The Python SDK pre-parses only top-level non-string arguments. JSON
    // scalars remain strings, while objects, arrays and null reach Pydantic.
    if tool == "plan_voyage_fuel"
        && let Some(text) = parsed["request"].as_str()
        && let Ok(value) = serde_json::from_str::<Json>(text)
        && (value.is_object() || value.is_array() || value.is_null())
    {
        parsed["request"] = value;
    }
    let normalized = validator.value(schema, &parsed, "");
    if validator.errors.is_empty() {
        *arguments = normalized;
        Ok(())
    } else {
        Err(format!("Validation failed: {}", Json::Array(validator.errors)))
    }
}
