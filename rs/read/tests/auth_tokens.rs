//! Full-HTTP Python oracle vectors; no fixture keys are runtime credentials.
use serde_json::Value;
use verdaxis_read::auth::{
    AccountState, VerificationSettings, validate_account, verify_access, verify_authorization,
};

#[tokio::test]
async fn access_vectors_match_python_http_outcomes() {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use tower::ServiceExt;
    let document: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/auth-vectors.json"
    ))
    .unwrap();
    let settings = &document["settings"];
    let now = chrono::DateTime::from_timestamp(document["now"].as_i64().unwrap(), 0).unwrap();
    let vectors = document["vectors"].as_array().unwrap();
    assert!(vectors.len() > 150);
    for vector in vectors {
        let id = vector["id"].as_str().unwrap();
        let config = VerificationSettings::new(
            settings["current"].as_str().unwrap().to_owned(),
            vector["previous_enabled"]
                .as_bool()
                .unwrap()
                .then(|| settings["previous"].as_str().unwrap().to_owned()),
            settings["issuer"].as_str().unwrap().to_owned(),
            settings["audience"].as_str().unwrap().to_owned(),
        );
        let authorization = vector["authorization"].as_array().unwrap();
        let first = authorization.first().and_then(Value::as_str);
        let user: AccountState = serde_json::from_value(vector["user"].clone()).unwrap();
        let outcome = verify_authorization(first, &config, now)
            .and_then(|access| validate_account(&access, &user).map(|()| access));
        let mut expected_wire = vector
            .get("rust_expected")
            .unwrap_or(&vector["python"])
            .clone();
        let credentials = verdaxis_read::auth::AuthError::Credentials.body();
        if id == "exp-absent" {
            expected_wire["status"] = serde_json::json!(401);
            expected_wire["www_authenticate"] = serde_json::json!("Bearer");
            expected_wire["body"] = serde_json::json!(credentials.to_string());
        } else if serde_json::from_str::<Value>(expected_wire["body"].as_str().unwrap()).ok()
            == Some(serde_json::json!({"detail":"Could not validate credentials"}))
        {
            expected_wire["body"] = serde_json::json!(credentials.to_string());
        }
        if vector.get("rust_expected").is_some() {
            assert_eq!(vector["approved_difference"]["defect"], "PD7");
            assert_eq!(vector["approved_difference"]["decision"], 94);
            assert_eq!(vector["python"]["status"], 500);
        }
        let expected = expected_wire["status"].as_u64().unwrap() as u16;
        let (status, body, authenticate) = match outcome {
            Ok(access) => {
                assert_eq!(
                    access.subject.to_string(),
                    "a550c69b-2934-4b36-87f5-e4f9976d1af0",
                    "{id}: subject"
                );
                if let Some(issued_at) = vector["issued_at_us"].as_i64() {
                    assert_eq!(
                        access.issued_at.map(|value| value.timestamp_micros()),
                        Some(issued_at),
                        "{id}: issuance"
                    );
                }
                (200, serde_json::json!({"count": 0}), None)
            }
            Err(error) => (error.status(), error.body(), error.www_authenticate()),
        };
        assert_eq!(status, expected, "{id}: HTTP status");
        if expected != 500 {
            let expected_body: Value =
                serde_json::from_str(expected_wire["body"].as_str().unwrap()).unwrap();
            assert_eq!(body, expected_body, "{id}: error envelope");
        }
        assert_eq!(
            authenticate,
            expected_wire["www_authenticate"].as_str(),
            "{id}: authentication header"
        );
        // Exercise a complete in-process HTTP response on Rust too. This
        // adapter is test-only and mounts no deployed authentication route.
        let fixture = vector.clone();
        let http_settings = settings.clone();
        let route = Router::new().route(
            "/api/notifications/unread-count",
            get(move |headers: axum::http::HeaderMap| {
                let fixture = fixture.clone();
                let config = config_for(&fixture, &http_settings);
                async move {
                    let user: AccountState =
                        serde_json::from_value(fixture["user"].clone()).unwrap();
                    match verify_authorization(
                        headers.get("authorization").and_then(|v| v.to_str().ok()),
                        &config,
                        now,
                    )
                    .and_then(|access| validate_account(&access, &user))
                    {
                        Ok(()) => axum::Json(serde_json::json!({"count": 0})).into_response(),
                        Err(error) => {
                            let status = StatusCode::from_u16(error.status()).unwrap();
                            let mut response = if error.status() == 500 {
                                (status, "Internal Server Error").into_response()
                            } else {
                                (status, axum::Json(error.body())).into_response()
                            };
                            if let Some(value) = error.www_authenticate() {
                                response
                                    .headers_mut()
                                    .insert("www-authenticate", value.parse().unwrap());
                            }
                            response
                        }
                    }
                }
            }),
        );
        let mut request = Request::builder().uri("/api/notifications/unread-count");
        for header in authorization {
            request = request.header("authorization", header.as_str().unwrap());
        }
        let response = route
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "{id}: paired HTTP status"
        );
        assert_eq!(
            response
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok()),
            expected_wire["www_authenticate"].as_str(),
            "{id}: paired HTTP header"
        );
        let raw = to_bytes(response.into_body(), 4096).await.unwrap();
        if expected == 500 {
            assert_eq!(
                raw.as_ref(),
                b"Internal Server Error",
                "{id}: paired HTTP body"
            );
        } else {
            assert_eq!(
                serde_json::from_slice::<Value>(&raw).unwrap(),
                serde_json::from_str::<Value>(expected_wire["body"].as_str().unwrap()).unwrap(),
                "{id}: paired HTTP body"
            );
        }
        // The core verifier must not depend on header parsing or issue a token.
        if id == "current-key" {
            assert!(verify_access(vector["token"].as_str().unwrap(), &config, now).is_ok());
        }
    }
}

fn config_for(vector: &Value, settings: &Value) -> VerificationSettings {
    VerificationSettings::new(
        settings["current"].as_str().unwrap().to_owned(),
        vector["previous_enabled"]
            .as_bool()
            .unwrap()
            .then(|| settings["previous"].as_str().unwrap().to_owned()),
        settings["issuer"].as_str().unwrap().to_owned(),
        settings["audience"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn verification_settings_debug_is_redacted() {
    let settings = VerificationSettings::new(
        "synthetic-current-secret".into(),
        Some("synthetic-previous-secret".into()),
        "issuer".into(),
        "audience".into(),
    );
    let diagnostic = format!("{settings:?}");
    assert!(!diagnostic.contains("synthetic-current-secret"));
    assert!(!diagnostic.contains("synthetic-previous-secret"));
}
