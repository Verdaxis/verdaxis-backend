use serde_json::{Value, json};
use verdaxis_read::crud::preferences::validate_value;

#[test]
fn preference_http_oracle_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/preferences.json"
    ))
    .unwrap();
    for case in fixture["vectors"].as_array().unwrap() {
        let name = case["id"].as_str().unwrap();
        // Syntax and required-body validation belong to the HTTP adapter.
        if name.starts_with("syntax-") || name == "empty-namespace" || case["value"].is_null() {
            continue;
        }
        let value = case["value"].clone();
        match validate_value(case["namespace"].as_str().unwrap(), value.clone()) {
            Ok(original) => {
                assert_eq!(case["python_status"], 200, "{name}");
                assert_eq!(original, value, "{name}: original payload changed");
            }
            Err(error) => {
                assert_eq!(
                    error.status.as_u16(),
                    case["python_status"].as_u64().unwrap() as u16,
                    "{name}"
                );
                assert_eq!(
                    json!({"detail": error.detail}),
                    case["python_detail"],
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn every_required_field_is_strict_and_present() {
    let fields = [
        "email_trade_updates",
        "email_market_alerts",
        "email_compliance_digest",
        "email_system_announcements",
        "inapp_trade_updates",
        "inapp_market_alerts",
        "inapp_order_matches",
    ];
    let valid: Value = fields
        .iter()
        .map(|key| ((*key).to_owned(), json!(true)))
        .collect();
    for field in fields {
        for invalid in [json!(0), json!(1), json!("true"), json!(null)] {
            let mut body = valid.clone();
            body[field] = invalid;
            assert_eq!(
                validate_value("notifications", body).unwrap_err().status,
                422
            );
        }
        let mut body = valid.clone();
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            validate_value("notifications", body).unwrap_err().status,
            422
        );
    }
    for field in ["products", "portIds"] {
        let mut body = json!({"products": [], "portIds": []});
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            validate_value("market_watch", body).unwrap_err().status,
            422
        );
    }
}

mod dashboard_namespace {
    use serde_json::{Value, json};
    use verdaxis_read::crud::preferences::{DASHBOARD_WIDGET_TYPES, validate_value};

    fn widget(id: &str, kind: &str, x: i64, y: i64, w: i64, h: i64) -> Value {
        json!({"id": id, "type": kind, "x": x, "y": y, "w": w, "h": h})
    }

    fn layout(widgets: Vec<Value>) -> Value {
        json!({"version": 1, "widgets": widgets})
    }

    fn refusal(value: Value) -> Vec<Value> {
        let error = validate_value("dashboard", value).expect_err("must be refused");
        assert_eq!(error.status, 422);
        error.detail.as_array().expect("diagnostic list").clone()
    }

    fn only_error(value: Value) -> Value {
        let mut errors = refusal(value);
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors.remove(0)
    }

    #[test]
    fn accepts_a_valid_layout_unchanged() {
        let value = layout(vec![
            widget("quick_actions", "quick_actions", 0, 0, 12, 2),
            widget("order-book_2", "order_book", 0, 2, 6, 10),
            widget("trade_tape", "trade_tape", 6, 2, 6, 10),
        ]);
        assert_eq!(validate_value("dashboard", value.clone()).unwrap(), value);
    }

    #[test]
    fn accepts_an_empty_layout_and_every_boundary() {
        assert!(validate_value("dashboard", layout(vec![])).is_ok());
        let edges = layout(vec![
            widget("a", "my_trades", 0, 0, 1, 1),
            widget("b", "my_trades", 11, 199, 1, 12),
            widget(&"x".repeat(64), "forward_curve", 0, 1, 12, 12),
        ]);
        assert!(validate_value("dashboard", edges).is_ok());
    }

    #[test]
    fn accepts_every_declared_widget_type_and_no_other() {
        for kind in DASHBOARD_WIDGET_TYPES {
            let value = layout(vec![widget("w", kind, 0, 0, 1, 1)]);
            assert!(validate_value("dashboard", value).is_ok(), "{kind}");
        }
        let error = only_error(layout(vec![widget("w", "news", 0, 0, 1, 1)]));
        assert_eq!(error["type"], "literal_error");
        assert_eq!(error["loc"], json!(["widgets", 0, "type"]));
    }

    #[test]
    fn accepts_at_most_24_widgets() {
        let build = |count: usize| {
            layout(
                (0..count)
                    .map(|i| widget(&format!("w{i}"), "my_trades", 0, 0, 1, 1))
                    .collect(),
            )
        };
        assert!(validate_value("dashboard", build(24)).is_ok());
        let error = only_error(build(25));
        assert_eq!(error["type"], "too_long");
        assert_eq!(error["loc"], json!(["widgets"]));
        assert_eq!(error["ctx"]["max_length"], 24);
    }

    #[test]
    fn refuses_a_wrong_or_missing_version() {
        for version in [
            json!(2),
            json!(0),
            json!("1"),
            json!(1.0),
            json!(null),
            json!(true),
        ] {
            let error = only_error(json!({"version": version, "widgets": []}));
            assert_eq!(error["type"], "literal_error", "{version}");
            assert_eq!(error["loc"], json!(["version"]));
        }
        let error = only_error(json!({"widgets": []}));
        assert_eq!(error["type"], "missing");
        assert_eq!(error["loc"], json!(["version"]));
    }

    #[test]
    fn refuses_missing_widgets_and_non_lists() {
        let error = only_error(json!({"version": 1}));
        assert_eq!(error["type"], "missing");
        assert_eq!(error["loc"], json!(["widgets"]));
        for widgets in [json!({}), json!("x"), json!(null), json!(3)] {
            let error = only_error(json!({"version": 1, "widgets": widgets}));
            assert_eq!(error["type"], "list_type");
        }
    }

    #[test]
    fn refuses_non_object_bodies() {
        for body in [json!([]), json!("x"), json!(3), json!(true)] {
            let error = only_error(body);
            assert_eq!(error["type"], "model_type");
            assert_eq!(error["ctx"]["class_name"], "DashboardPreferences");
        }
    }

    #[test]
    fn refuses_unknown_keys_at_both_levels() {
        let mut value = layout(vec![]);
        value["theme"] = json!("dark");
        let error = only_error(value);
        assert_eq!(error["type"], "extra_forbidden");
        assert_eq!(error["loc"], json!(["theme"]));

        let mut extra = widget("w", "my_trades", 0, 0, 1, 1);
        extra["color"] = json!("red");
        let error = only_error(layout(vec![extra]));
        assert_eq!(error["type"], "extra_forbidden");
        assert_eq!(error["loc"], json!(["widgets", 0, "color"]));
    }

    #[test]
    fn refuses_a_widget_that_is_not_an_object_or_lacks_a_field() {
        let error = only_error(layout(vec![json!("w")]));
        assert_eq!(error["type"], "model_type");
        assert_eq!(error["loc"], json!(["widgets", 0]));
        for field in ["id", "type", "x", "y", "w", "h"] {
            let mut item = widget("w", "my_trades", 0, 0, 1, 1);
            item.as_object_mut().unwrap().remove(field);
            let error = only_error(layout(vec![item]));
            assert_eq!(error["type"], "missing", "{field}");
            assert_eq!(error["loc"], json!(["widgets", 0, field]));
        }
    }

    #[test]
    fn refuses_invalid_ids() {
        for (id, kind) in [
            (json!(""), "string_too_short"),
            (json!("x".repeat(65)), "string_too_long"),
            (json!("has space"), "string_pattern_mismatch"),
            (json!("dot.dot"), "string_pattern_mismatch"),
            (json!("caf\u{e9}"), "string_pattern_mismatch"),
            (json!(7), "string_type"),
            (json!(null), "string_type"),
        ] {
            let mut item = widget("w", "my_trades", 0, 0, 1, 1);
            item["id"] = id.clone();
            let error = only_error(layout(vec![item]));
            assert_eq!(error["type"], kind, "{id}");
            assert_eq!(error["loc"], json!(["widgets", 0, "id"]));
        }
    }

    #[test]
    fn refuses_integers_outside_their_range_or_of_the_wrong_kind() {
        for (field, value, kind) in [
            ("x", json!(-1), "greater_than_equal"),
            ("x", json!(12), "less_than_equal"),
            ("y", json!(-1), "greater_than_equal"),
            ("y", json!(200), "less_than_equal"),
            ("w", json!(0), "greater_than_equal"),
            ("w", json!(13), "less_than_equal"),
            ("h", json!(0), "greater_than_equal"),
            ("h", json!(13), "less_than_equal"),
            ("h", json!(u64::MAX), "less_than_equal"),
            ("x", json!(1.5), "int_type"),
            ("y", json!(2.0), "int_type"),
            ("w", json!("3"), "int_type"),
            ("h", json!(true), "int_type"),
            ("x", json!(null), "int_type"),
        ] {
            let mut item = widget("w", "my_trades", 0, 0, 1, 1);
            item[field] = value.clone();
            let error = only_error(layout(vec![item]));
            assert_eq!(error["type"], kind, "{field}={value}");
            assert_eq!(error["loc"], json!(["widgets", 0, field]));
        }
    }

    #[test]
    fn refuses_a_widget_that_runs_past_column_twelve() {
        assert!(
            validate_value(
                "dashboard",
                layout(vec![widget("w", "my_trades", 6, 0, 6, 1)])
            )
            .is_ok()
        );
        let error = only_error(layout(vec![widget("w", "my_trades", 6, 0, 7, 1)]));
        assert_eq!(error["type"], "value_error");
        assert_eq!(error["loc"], json!(["widgets", 0]));
        let error = only_error(layout(vec![widget("w", "my_trades", 11, 0, 2, 1)]));
        assert_eq!(error["type"], "value_error");
    }

    #[test]
    fn refuses_duplicate_ids() {
        let error = only_error(layout(vec![
            widget("same", "my_trades", 0, 0, 6, 1),
            widget("same", "order_book", 6, 0, 6, 1),
        ]));
        assert_eq!(error["type"], "value_error");
        assert_eq!(error["loc"], json!(["widgets"]));
        // The same widget type under two different ids is allowed.
        assert!(
            validate_value(
                "dashboard",
                layout(vec![
                    widget("one", "my_trades", 0, 0, 6, 1),
                    widget("two", "my_trades", 6, 0, 6, 1),
                ]),
            )
            .is_ok()
        );
    }

    #[test]
    fn reports_every_invalid_widget_together() {
        let errors = refusal(layout(vec![
            widget("", "my_trades", 0, 0, 1, 1),
            widget("ok", "bogus", 0, 0, 1, 1),
        ]));
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0]["loc"], json!(["widgets", 0, "id"]));
        assert_eq!(errors[1]["loc"], json!(["widgets", 1, "type"]));
    }

    #[test]
    fn oversize_layouts_are_refused_before_the_schema() {
        // 8 KiB cap applies to the compact JSON, ahead of any field checks.
        let value = json!({"version": 1, "widgets": [], "padding": "p".repeat(9000)});
        let error = validate_value("dashboard", value).unwrap_err();
        assert_eq!(error.status, 413);
    }

    #[test]
    fn namespaces_outside_the_set_stay_unknown() {
        for namespace in ["Dashboard", "dashboards", "home", "legacy", ""] {
            let error = validate_value(namespace, layout(vec![])).unwrap_err();
            assert_eq!(error.status, 404, "{namespace}");
            assert_eq!(error.detail, json!("Unknown preference namespace"));
        }
    }
}
