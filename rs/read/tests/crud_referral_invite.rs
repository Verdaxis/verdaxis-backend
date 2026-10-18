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

#[test]
fn emailstr_oracle_vectors_keep_normalized_values_and_diagnostics() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/referral-email-vectors.json"
    ))
    .unwrap();
    for vector in vectors.as_array().unwrap() {
        let result = normalize_email(&json!({"email": vector["input"]}));
        if let Some(expected) = vector.get("email") {
            assert_eq!(
                result.unwrap(),
                expected.as_str().unwrap(),
                "{}",
                vector["input"]
            );
        } else {
            let mut expected = vector["errors"].clone();
            for error in expected.as_array_mut().unwrap() {
                error["loc"] = json!(["body", "email"]);
            }
            assert_eq!(result.unwrap_err().detail, expected, "{}", vector["input"]);
        }
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
