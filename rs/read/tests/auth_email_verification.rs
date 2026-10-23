//! E3/E4 component checks.
pub use verdaxis_read::{ApiError, ApiErrorKind, auth, crud, runtime};
#[path = "../src/auth/email.rs"]
pub mod email;
#[path = "../src/auth/email_verification.rs"]
pub mod email_verification;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use crud::referrals::invite::MailClient;
use email::{AuthMail, MailDispatcher};
use email_verification::{EmailSettings, router};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[test]
fn e3_template_encodes_values_once() {
    let mail = AuthMail::Verification {
        to: "recipient@example.com".into(),
        name: "<Name> {{TOKEN}}".into(),
        token: "a&b".into(),
        pending: None,
    };
    let payload = email::render(&mail, "sender@example.com", "https://app.example.com");
    let html = payload["html"].as_str().unwrap();
    assert!(html.contains("&lt;Name&gt; {{TOKEN}}"));
    assert!(html.contains("token=a%26b"));
    assert!(!format!("{mail:?}").contains("recipient@example.com"));
}

#[test]
fn e4_canary_requires_admission_and_address() {
    assert!(email::suppress_verification(
        true,
        "canary+123@one.canary.verdaxis.exchange"
    ));
    assert!(!email::suppress_verification(
        false,
        "canary+123@one.canary.verdaxis.exchange"
    ));
    assert!(!email::suppress_verification(
        true,
        "other@one.canary.verdaxis.exchange"
    ));
    assert!(!email::suppress_verification(
        true,
        "canary+123@elsewhere.example.com"
    ));
}

#[tokio::test]
async fn e4_delivery_has_three_attempts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                post(move || {
                    let calls = observed.clone();
                    async move {
                        if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                            StatusCode::BAD_GATEWAY
                        } else {
                            StatusCode::OK
                        }
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let dispatcher = MailDispatcher::new(
        MailClient::loopback(format!("http://{address}/"), "synthetic".into()).unwrap(),
    );
    dispatcher.dispatch(AuthMail::Verification {
        to: "recipient@example.com".into(),
        name: "Name".into(),
        token: "synthetic".into(),
        pending: None,
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), dispatcher.drain())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn e3_e4_validation_and_limits_precede_storage() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let app = router(EmailSettings::new(
        pool,
        MailDispatcher::new(MailClient::disabled()),
    ));
    let invalid = app
        .clone()
        .oneshot(
            Request::post("/api/auth/resend-verification-email")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"invalid"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    for _ in 0..10 {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/auth/verify-email?token=")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["detail"]["code"],
            "EMAIL_TOKEN_INVALID"
        );
    }
    let limited = app
        .oneshot(
            Request::post("/api/auth/verify-email?token=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn e3_e4_rejections_use_auth_envelopes() {
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://unused:unused@127.0.0.1:1/unused").unwrap();
    let app = router(EmailSettings::new(pool, MailDispatcher::new(MailClient::disabled())));
    for (path, body) in [("/api/auth/verify-email", ""), ("/api/auth/resend-verification-email", "{"), ("/api/auth/resend-verification-email", r#"{"email":42}"#)] {
        let response = app.clone().oneshot(Request::post(path).header("content-type","application/json").body(Body::from(body)).unwrap()).await.unwrap();
        assert!(response.status().is_client_error());
        let bytes=to_bytes(response.into_body(),4096).await.unwrap();
        let value:serde_json::Value=serde_json::from_slice(&bytes).expect("JSON envelope");
        assert!(value["detail"]["code"].is_string());
        assert!(value["detail"]["message"].is_string());
    }
}

#[tokio::test]
async fn e4_dispatch_capacity_and_failure_record() {
    #[derive(Clone)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes:&[u8])->std::io::Result<usize>{self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())}
        fn flush(&mut self)->std::io::Result<()> {Ok(())}
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer=Self;
        fn make_writer(&'a self)->Self::Writer {self.clone()}
    }
    let output=Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber=tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(Capture(output.clone())).finish();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let release=Arc::new(tokio::sync::Semaphore::new(0));
    let observed=release.clone();
    let calls=Arc::new(AtomicUsize::new(0));
    let counted=calls.clone();
    let server=tokio::spawn(async move {axum::serve(listener,Router::new().route("/",post(move || {
        let release=observed.clone();let calls=counted.clone();async move {
            calls.fetch_add(1,Ordering::SeqCst);
            release.acquire().await.unwrap().forget();
            StatusCode::BAD_GATEWAY
        }
    }))).await.unwrap();});
    let dispatcher=MailDispatcher::new(MailClient::loopback(format!("http://{address}/"),"synthetic".into()).unwrap());
    {
        let _guard=tracing::subscriber::set_default(subscriber);
        for _ in 0..33 {dispatcher.dispatch(AuthMail::Verification{to:"record@example.com".into(),name:"Record".into(),token:"record-token".into(),pending:None});}
    }
    tokio::time::timeout(std::time::Duration::from_secs(5),async {while calls.load(Ordering::SeqCst)<32 {tokio::task::yield_now().await;}}).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),32);
    release.add_permits(96);
    tokio::time::timeout(std::time::Duration::from_secs(20),dispatcher.drain()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),96);
    let logs=String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.matches("auth_mail_capacity").count(),1);
    assert_eq!(logs.matches("auth_mail_delivery_failed").count(),32);
    assert!(logs.contains(&auth::token_bytes::token_digest("record@example.com")[..12]));
    assert!(!logs.contains("record@example.com"));
    assert!(!logs.contains("record-token"));
    server.abort();let _=server.await;
}
