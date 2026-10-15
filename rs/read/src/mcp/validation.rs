//! Python-compatible tool argument diagnostics. Validation precedes tool admission.
use super::transport::python_string_repr;
use crate::Json;

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
fn diagnostic(path: &str, message: &str, kind: &str, input: &Json) -> String {
    format!(
        "{path}\n  {message} [type={kind}, input_value={}, input_type={}]\n    For further information visit https://errors.pydantic.dev/2.13/v/{kind}",
        repr(input),
        type_name(input)
    )
}

pub fn tool_arguments(tool: &str, arguments: &mut Json) -> Result<(), String> {
    let fields: &[&str] = match tool {
        "get_market_slice" => &["market_product", "delivery_point", "availability_window"],
        "plan_voyage_fuel" => &["request"],
        _ => return Ok(()),
    };
    let mut errors = Vec::new();
    for field in fields {
        if arguments.get(*field).is_none() {
            errors.push(diagnostic(field, "Field required", "missing", arguments));
        }
    }
    if tool == "plan_voyage_fuel" && arguments.get("request").is_some() {
        let request = &arguments["request"];
        let vessel = &request["vessel"];
        if !vessel.is_object() {
            errors.push(diagnostic(
                "request.vessel",
                "Input should be a valid dictionary or instance of VesselInput",
                "model_type",
                vessel,
            ));
        } else {
            for (field, low, high, exclusive) in [
                ("speed_knots", "1", "30", false),
                ("sea_consumption_mt_per_day", "0", "400", true),
                ("berth_consumption_mt_per_day", "0", "100", false),
            ] {
                if let Some(input) = vessel.get(field) {
                    let text = input
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| input.to_string());
                    if let Ok(value) = super::decimal::Decimal::parse(&text) {
                        let low_value = super::decimal::Decimal::parse(low).unwrap();
                        let high_value = super::decimal::Decimal::parse(high).unwrap();
                        let below = value.compare(&low_value) == std::cmp::Ordering::Less;
                        let equal = value.compare(&low_value) == std::cmp::Ordering::Equal;
                        if below || (exclusive && equal) {
                            let (kind, phrase) = if exclusive {
                                ("greater_than", "greater than")
                            } else {
                                ("greater_than_equal", "greater than or equal to")
                            };
                            errors.push(diagnostic(
                                &format!("request.vessel.{field}"),
                                &format!("Input should be {phrase} {low}"),
                                kind,
                                input,
                            ));
                        } else if value.compare(&high_value) == std::cmp::Ordering::Greater {
                            errors.push(diagnostic(
                                &format!("request.vessel.{field}"),
                                &format!("Input should be less than or equal to {high}"),
                                "less_than_equal",
                                input,
                            ));
                        }
                    }
                }
            }
            for (field, value) in vessel.as_object().unwrap() {
                if ![
                    "name",
                    "imo_number",
                    "speed_knots",
                    "sea_consumption_mt_per_day",
                    "berth_consumption_mt_per_day",
                    "compatible_fuels",
                ]
                .contains(&field.as_str())
                {
                    errors.push(diagnostic(
                        &format!("request.vessel.{field}"),
                        "Extra inputs are not permitted",
                        "extra_forbidden",
                        value,
                    ));
                }
            }
        }
        if request["legs"].as_array().is_some_and(Vec::is_empty) {
            errors.push(diagnostic(
                "request.legs",
                "Tuple should have at least 1 item after validation, not 0",
                "too_short",
                &request["legs"],
            ));
        }
    }
    if tool == "plan_voyage_fuel"
        && let Some(legs) = arguments["request"]["legs"].as_array_mut()
    {
        let original = Json::Array(legs.clone());
        let catalog: Json =
            serde_json::from_str(include_str!("../../../../deploy/rust/mcp-catalog.json"))
                .expect("catalog data");
        let ports: Vec<_> = catalog["delivery_points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point["name"].as_str().unwrap())
            .collect();
        let mut invalid_legs = 0;
        for (index, leg) in legs.iter_mut().enumerate() {
            let mut invalid = false;
            for field in ["from_port", "to_port"] {
                if let Some(port) = leg[field].as_str() {
                    if let Some(canonical) = ports
                        .iter()
                        .find(|name| name.to_lowercase() == port.trim().to_lowercase())
                    {
                        leg[field] = Json::String((*canonical).into());
                    } else {
                        errors.push(diagnostic(
                            &format!("request.legs.{index}.{field}"),
                            &format!(
                                "Value error, Unsupported port {}. Supported ports: {}",
                                python_string_repr(port),
                                ports.join(", ")
                            ),
                            "value_error",
                            &leg[field],
                        ));
                        invalid = true;
                    }
                }
            }
            invalid_legs += usize::from(invalid);
        }
        if !legs.is_empty() && invalid_legs == legs.len() {
            errors.push(diagnostic(
                "request.legs",
                "Tuple should have at least 1 item after validation, not 0",
                "too_short",
                &original,
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} validation error{} for {tool}Arguments\n{}",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" },
            errors.join("\n")
        ))
    }
}
