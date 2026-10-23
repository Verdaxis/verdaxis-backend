//! Task 19: no test uses an external mail provider.
use serde_json::json;
use verdaxis_read::crud::referrals::invite::{MailClient, normalize_email, render_invite};

#[test]
fn invite_addresses_preserve_local_case_and_normalize_domains() {
    assert_eq!(
        normalize_email(&json!({"email":" User@EXAMPLE.COM "})).unwrap(),
        "User@example.com"
    );
    assert_eq!(
        normalize_email(&json!({"email":"Ada <user@例子.公司>"})).unwrap(),
        "user@例子.公司"
    );
    assert!(normalize_email(&json!({"email":"not-email"})).is_err());
    assert!(normalize_email(&json!({"email":null})).is_err());
    assert!(normalize_email(&json!({})).is_err());
}

/// Every Python-captured vector, compared in full: the normalized address, or
/// the complete pydantic error list. All differences are reported together.
#[test]
fn emailstr_oracle_vectors_keep_normalized_values_and_diagnostics() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/referral-email-vectors.json"
    ))
    .unwrap();
    let vectors = vectors.as_array().unwrap();
    assert!(
        vectors.len() >= 500,
        "the corpus must cover the grammar broadly"
    );
    let mut differences = Vec::new();
    for vector in vectors {
        let result = normalize_email(&json!({"email": vector["input"]}));
        let matches = if let Some(expected) = vector.get("email") {
            matches!(&result, Ok(actual) if actual == expected.as_str().unwrap())
        } else {
            let mut expected = vector["errors"].clone();
            for error in expected.as_array_mut().unwrap() {
                error["loc"] = json!(["body", "email"]);
            }
            matches!(&result, Err(actual) if actual.detail == expected)
        };
        if !matches {
            let actual = match result {
                Ok(address) => format!("accepted {address:?}"),
                Err(error) => error.detail["reason"]
                    .as_str()
                    .map_or_else(|| error.detail.to_string(), str::to_owned),
            };
            differences.push(format!(
                "input {} expected {} actual {}",
                vector["input"],
                vector.get("email").map_or_else(
                    || vector["errors"][0]["ctx"]["reason"].to_string(),
                    |e| e.to_string()
                ),
                actual
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} of {} vectors differ; first {:#?}",
        differences.len(),
        vectors.len(),
        &differences[..differences.len().min(60)]
    );
}

/// Replays a Python-generated sweep of code points in address positions (see
/// `scripts/rust_contract/referral_email_sweep.py`). Run it only against a
/// freshly generated file:
/// `EMAILSTR_SWEEP_FILE=... cargo test --test crud_referral_invite -- --ignored`.
#[test]
#[ignore = "needs a sweep file written by the Python oracle"]
fn emailstr_unicode_sweep_matches_python() {
    use std::io::BufRead;
    let path = std::env::var("EMAILSTR_SWEEP_FILE").expect("EMAILSTR_SWEEP_FILE");
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let (mut total, mut differences) = (0_usize, Vec::new());
    for line in file.lines() {
        let line = line.unwrap();
        let mut fields = line.splitn(4, '\t');
        let (position, status, input, expected) = (
            fields.next().unwrap(),
            fields.next().unwrap(),
            serde_json::from_str::<String>(fields.next().unwrap()).unwrap(),
            serde_json::from_str::<String>(fields.next().unwrap()).unwrap(),
        );
        total += 1;
        let actual = match normalize_email(&json!({"email": input})) {
            Ok(address) => ("O", address),
            Err(error) => (
                "E",
                error.detail[0]["ctx"]["reason"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
        };
        if actual != (status, expected.clone()) && differences.len() < 80 {
            differences.push(format!(
                "{position} input {input:?} expected {status} {expected:?} actual {actual:?}"
            ));
        } else if actual != (status, expected) {
            differences.push(String::new());
        }
    }
    assert!(total > 100_000, "the sweep file is too small: {total}");
    assert!(
        differences.is_empty(),
        "{} of {total} sweep lines differ; first {:#?}",
        differences.len(),
        differences
            .iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
    );
}

/// Approved difference (decision 94): Python answers a lone surrogate escape
/// with HTTP 500; Rust must refuse it as invalid JSON, never with a 5xx.
#[test]
fn lone_surrogate_escapes_are_refused_without_a_server_error() {
    let record: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/referral-email-approved-differences.json"
    ))
    .unwrap();
    for vector in record["vectors"].as_array().unwrap() {
        let error = verdaxis_read::auth::request::parse_json_body(
            vector["raw"].as_str().unwrap().as_bytes(),
        )
        .expect_err("a lone surrogate is not representable");
        assert_eq!(error.status.as_u16(), 422, "{}", vector["id"]);
        assert_eq!(error.detail[0]["type"], "json_invalid", "{}", vector["id"]);
    }
}

#[test]
fn invitation_escapes_content_and_trims_only_its_link() {
    let payload = render_invite(
        "sender@example.com",
        "to@example.com",
        "<Ada & \"Co\">",
        "VDX-A/B123",
        "https://app.example///",
    );
    assert_eq!(payload["subject"], "<Ada & \"Co\"> invited you to Verdaxis");
    assert_eq!(payload["from"], "sender@example.com");
    assert_eq!(payload["reply_to"], "admin@verdaxis.exchange");
    assert_eq!(payload["to"], json!(["to@example.com"]));
    let html = payload["html"].as_str().unwrap();
    assert!(html.contains("&lt;Ada &amp; &quot;Co&quot;&gt;"));
    assert!(!html.contains("<Ada"));
    assert!(html.contains("https://app.example/invite/VDX-A%2FB123"));
}

#[tokio::test]
async fn provider_accepts_only_200_and_201_without_sending_real_mail() {
    use axum::{Router, http::StatusCode, routing::post};
    for status in [200, 201, 202, 400, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/emails",
            post(move || async move { (StatusCode::from_u16(status).unwrap(), "not-json") }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            MailClient::loopback(format!("http://{address}/emails"), "test-only-key".into())
                .unwrap();
        let payload = render_invite(
            "sender@example.com",
            "to@example.com",
            "Ada",
            "VDX-ABC123",
            "https://app.example",
        );
        assert_eq!(client.send(&payload).await, matches!(status, 200 | 201));
        server.abort();
        let _ = server.await;
    }
    assert!(MailClient::loopback("https://api.resend.com/emails".into(), "test".into()).is_err());
    assert!(!MailClient::disabled().send(&json!({})).await);
}
