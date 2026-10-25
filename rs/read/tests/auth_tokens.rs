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
        let expected_wire = vector
            .get("rust_expected")
            .unwrap_or(&vector["python"])
            .clone();
        // Every recorded difference names its review record; no expectation is
        // derived from Rust's own output here.
        if vector.get("rust_expected").is_some() {
            let approved = &vector["approved_difference"];
            let credential_refusal = |detail: &Value| {
                detail["detail"]["code"] == "AUTH_CREDENTIALS_INVALID"
                    && detail["detail"]
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|m| !m.is_empty())
            };
            if approved.get("defect").is_some() {
                assert_eq!(approved["defect"], "PD7", "{id}: defect record");
                assert_eq!(approved["decision"], 94, "{id}: decision record");
                assert_eq!(vector["python"]["status"], 500, "{id}: PD7 baseline");
            } else if approved["decision"] == "PD3-EXP-1" {
                // Rust requires `exp`; Python accepts this token. The code is
                // pinned and the message wording stays free.
                let detail: Value =
                    serde_json::from_str(expected_wire["body"].as_str().unwrap()).unwrap();
                assert!(credential_refusal(&detail), "{id}: exp requirement record");
            } else {
                assert_eq!(
                    approved["decisions"],
                    serde_json::json!(["PD89-CF4-1", "PD89-CF4-11", "PD3-EXP-1"]),
                    "{id}: refusal record"
                );
                assert_eq!(vector["python"]["status"], 401, "{id}: refusal baseline");
                // Wording is free (PD89-CF4-1): only the reviewed code is
                // pinned by the vector; the message must be a non-empty
                // string in both the vector and the response below.
                let detail: Value =
                    serde_json::from_str(expected_wire["body"].as_str().unwrap()).unwrap();
                assert!(credential_refusal(&detail), "{id}: refusal envelope record");
            }
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
            if expected_body["detail"]["code"] == "AUTH_CREDENTIALS_INVALID" {
                // PD89-CF4-1: the message is free; the code and the rest of
                // the envelope are pinned by the vector.
                assert_eq!(
                    body["detail"]["code"], "AUTH_CREDENTIALS_INVALID",
                    "{id}: code"
                );
                assert!(
                    body["detail"]
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|m| { !m.is_empty() }),
                    "{id}: response message"
                );
            } else {
                assert_eq!(body, expected_body, "{id}: error envelope");
            }
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
            let paired_body: Value = serde_json::from_slice(&raw).unwrap();
            let expected_body: Value =
                serde_json::from_str(expected_wire["body"].as_str().unwrap()).unwrap();
            if expected_body["detail"]["code"] == "AUTH_CREDENTIALS_INVALID" {
                // Same free-wording rule as the direct comparison above.
                assert_eq!(
                    paired_body["detail"]["code"], "AUTH_CREDENTIALS_INVALID",
                    "{id}: paired HTTP code"
                );
                assert!(
                    paired_body["detail"]
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|m| !m.is_empty()),
                    "{id}: paired HTTP message"
                );
            } else {
                assert_eq!(paired_body, expected_body, "{id}: paired HTTP body");
            }
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
