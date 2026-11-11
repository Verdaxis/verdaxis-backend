//! The serving router mounts every served operation.
//!
//! The router is built over a database pool that never connects, so these
//! cases test routing and admission only. A mounted authenticated operation
//! answers 401 without a bearer; an unmounted path or method would answer 404
//! or 405.

use std::time::Duration;

use axum::{body::Body, http::Request};
use tower::ServiceExt;
use verdaxis_read::{AppState, Config, Phase2Options, runtime::artifact::ALLOWED_REVISIONS};

const ID: &str = "11111111-1111-4111-8111-111111111111";

/// What a mounted operation answers to a request without credentials.
#[derive(Clone, Copy)]
enum Expect {
    /// The operation demands a bearer.
    Bearer,
    /// The operation is mounted; its answer depends on data or input.
    Mounted,
}

/// Every served REST operation, with `{id}` for each path parameter.
const OPERATIONS: &[(&str, &str, Expect)] = &[
    // Notifications, preferences and feedback.
    ("GET", "/api/notifications", Expect::Bearer),
    ("GET", "/api/notifications/unread-count", Expect::Bearer),
    ("PATCH", "/api/notifications/read-all", Expect::Bearer),
    ("PATCH", "/api/notifications/{id}/read", Expect::Bearer),
    ("GET", "/api/users/me/preferences", Expect::Bearer),
    ("PUT", "/api/users/me/preferences/market", Expect::Bearer),
    ("POST", "/api/feedback", Expect::Bearer),
    ("GET", "/api/admin/feedback", Expect::Bearer),
    // Watchlists and alerts.
    ("GET", "/api/watchlists", Expect::Bearer),
    ("POST", "/api/watchlists", Expect::Bearer),
    ("POST", "/api/watchlists/me", Expect::Bearer),
    ("GET", "/api/watchlists/{id}", Expect::Bearer),
    ("DELETE", "/api/watchlists/{id}", Expect::Bearer),
    ("GET", "/api/watchlists/{id}/events", Expect::Bearer),
    ("PATCH", "/api/watchlists/{id}/events/{id}", Expect::Bearer),
    ("POST", "/api/watchlists/{id}/targets", Expect::Bearer),
    (
        "DELETE",
        "/api/watchlists/{id}/targets/{id}",
        Expect::Bearer,
    ),
    ("GET", "/api/alerts", Expect::Bearer),
    ("POST", "/api/alerts", Expect::Bearer),
    ("DELETE", "/api/alerts/{id}", Expect::Bearer),
    // Referrals. Resolving a code is public.
    ("GET", "/api/referrals/my-referrals", Expect::Bearer),
    ("GET", "/api/referrals/leaderboard", Expect::Bearer),
    ("POST", "/api/referrals/my-code", Expect::Bearer),
    ("POST", "/api/referrals/invite", Expect::Bearer),
    ("GET", "/api/referrals/resolve/CODE123", Expect::Mounted),
    // Accounts, admission and KYC.
    ("GET", "/api/auth/me", Expect::Mounted),
    ("PUT", "/api/auth/me", Expect::Mounted),
    ("POST", "/api/auth/survey", Expect::Mounted),
    (
        "GET",
        "/api/auth/admin/invitations/organizations",
        Expect::Mounted,
    ),
    ("POST", "/api/auth/admin/invitations", Expect::Mounted),
    ("POST", "/api/auth/invitations/resolve", Expect::Mounted),
    ("PUT", "/api/auth/approve/{id}", Expect::Mounted),
    ("PUT", "/api/auth/reject/{id}", Expect::Mounted),
    (
        "PUT",
        "/api/auth/organization/{id}/approve",
        Expect::Mounted,
    ),
    ("PUT", "/api/auth/organization/{id}/reject", Expect::Mounted),
    ("GET", "/api/auth/admin/review-queue", Expect::Mounted),
    ("GET", "/api/auth/admin/review-queue/{id}", Expect::Mounted),
    ("GET", "/api/auth/organization-joins", Expect::Mounted),
    (
        "PUT",
        "/api/auth/organization-joins/{id}/approve",
        Expect::Mounted,
    ),
    (
        "PUT",
        "/api/auth/organization-joins/{id}/reject",
        Expect::Mounted,
    ),
    ("POST", "/api/kyc/submit", Expect::Mounted),
    ("GET", "/api/kyc/status", Expect::Mounted),
    ("GET", "/api/kyc/admin/{id}/documents", Expect::Mounted),
    ("GET", "/api/kyc/admin/documents/{id}", Expect::Mounted),
    ("PUT", "/api/kyc/admin/{id}/approve", Expect::Mounted),
    ("PUT", "/api/kyc/admin/{id}/reject", Expect::Mounted),
    // Operator reads, vessels and subscriptions.
    (
        "PUT",
        "/api/admin/analytics/users/{id}/reject",
        Expect::Mounted,
    ),
    ("GET", "/api/admin/onboarding-attention", Expect::Mounted),
    ("GET", "/api/admin/audit-logs", Expect::Mounted),
    ("GET", "/api/vessels", Expect::Mounted),
    ("GET", "/api/vessels/{id}", Expect::Mounted),
    ("GET", "/api/subscriptions/fees", Expect::Mounted),
    ("GET", "/api/subscriptions/me", Expect::Mounted),
    ("GET", "/api/admin/subscriptions", Expect::Mounted),
    ("GET", "/api/admin/subscriptions/{id}", Expect::Mounted),
    ("PUT", "/api/admin/subscriptions/{id}", Expect::Mounted),
    // Operator analytics.
    ("GET", "/api/admin/analytics/product-usage", Expect::Mounted),
    ("GET", "/api/admin/analytics/overview", Expect::Mounted),
    ("GET", "/api/admin/analytics/daily", Expect::Mounted),
    ("GET", "/api/admin/analytics/users", Expect::Mounted),
    (
        "GET",
        "/api/admin/analytics/product-analytics/overview",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/acquisition",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/activation",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/engagement",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/marketplace",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/retention",
        Expect::Mounted,
    ),
    (
        "GET",
        "/api/admin/analytics/product-analytics/reliability",
        Expect::Mounted,
    ),
];

/// The serving router over a pool that refuses every connection quickly.
fn app() -> axum::Router {
    verdaxis_read::router(app_state(false))
}

fn app_state(agent_enabled: bool) -> AppState {
    let mut config = Config {
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: ALLOWED_REVISIONS[0].into(),
        database_url: "postgres://unused:unused@127.0.0.1:1/route_mounts_test".into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 52,
        configured_max_connections: 100,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        kyc_execution_gate_enabled: true,
        phase2: Phase2Options {
            jwt_secret: Some("composition-test-signing-authority-000000000".into()),
            jwt_issuer: Some("composition-api".into()),
            jwt_audience: Some("composition-web".into()),
            frontend_url: Some("https://staging.verdaxis.exchange".into()),
            email_from: Some("noreply@example.test".into()),
            resend_api_key: Some("disposable-mail-key".into()),
            ..Default::default()
        },
    };
    let lazy_pool = |connections| {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(connections)
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy(&config.database_url)
            .unwrap()
    };
    let pools = verdaxis_read::runtime::db::Pools {
        read: lazy_pool(4),
        health: lazy_pool(1),
    };
    let policy =
        serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json")).unwrap();
    config.phase2.agent.enabled = agent_enabled;
    AppState::new(config, pools, policy).unwrap()
}

/// Each child owns its environment; no test changes process-wide credentials.
#[tokio::test]
async fn agent_mounting_respects_startup_gates() {
    const CASE: &str = "VERDAXIS_AGENT_MOUNT_CASE";
    let Ok(case) = std::env::var(CASE) else {
        for case in ["disabled", "missing", "rejected", "ready"] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "route_mounts::agent_mounting_respects_startup_gates",
                    "--nocapture",
                ])
                .env(CASE, case);
            for key in [
                "OPENCODE_API_KEY",
                "OPENCODE_API_KEY_2",
                "OPENCODE_API_KEY_3",
            ] {
                command.env_remove(key);
            }
            match case {
                "disabled" | "ready" => {
                    command.env(
                        "OPENCODE_API_KEY",
                        format!("mount-fixture-{}", std::process::id()),
                    );
                }
                "rejected" => {
                    command.env("OPENCODE_API_KEY", "not-ascii-\u{e9}");
                }
                _ => {}
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "child case did not run"
            );
        }
        return;
    };
    let state = app_state(case != "disabled");
    assert_eq!(
        verdaxis_read::runtime::agent::mounts(&state),
        case == "ready"
    );
    let app = verdaxis_read::router(state);
    for (method, path) in [
        ("POST", "/api/agent/conversations".to_owned()),
        ("GET", format!("/api/agent/conversations/{ID}")),
        ("POST", format!("/api/agent/conversations/{ID}/messages")),
        ("GET", "/api/auth/admin/agent/audit".to_owned()),
        ("GET", "/api/auth/admin/agent/audit/verify".to_owned()),
        ("GET", "/api/auth/admin/agent/controls".to_owned()),
        ("PUT", "/api/auth/admin/agent/controls".to_owned()),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if case == "ready" { 401 } else { 404 },
            "{method} {path}: {case}"
        );
    }
}

#[tokio::test]
async fn every_served_operation_is_mounted() {
    let app = app();
    let mut seen = std::collections::HashSet::new();
    for (method, template, expect) in OPERATIONS {
        assert!(
            seen.insert((method, template)),
            "duplicate {method} {template}"
        );
        let path = template.replace("{id}", ID);
        let body = if matches!(*method, "POST" | "PUT" | "PATCH") {
            "{}"
        } else {
            ""
        };
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(*method)
                    .uri(&path)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        match expect {
            Expect::Bearer => assert_eq!(status, 401, "{method} {path} must demand a bearer"),
            Expect::Mounted => assert!(
                !matches!(status, 404 | 405),
                "{method} {path} must be mounted, got {status}"
            ),
        }
    }
}

#[tokio::test]
async fn credential_operations_reach_their_admission_boundary() {
    let app = app();
    for (method, path, expected) in [
        ("POST", "/api/auth/login", 403),
        ("POST", "/api/auth/refresh", 403),
        ("POST", "/api/auth/logout", 403),
        ("PUT", "/api/auth/me/password", 403),
        ("POST", "/api/auth/invitations/accept", 403),
        ("GET", "/api/auth/stream-token", 401),
        ("POST", "/api/auth/forgot-password", 422),
        ("POST", "/api/auth/reset-password", 422),
        ("POST", "/api/auth/verify-email", 422),
        ("POST", "/api/auth/resend-verification-email", 422),
        ("POST", "/api/auth/register", 422),
        ("POST", "/api/auth/register-with-org", 422),
    ] {
        for suffix in ["", "/"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(format!("{path}{suffix}"))
                        .header("origin", "https://untrusted.example.test")
                        .header("cookie", "refresh_token=invalid")
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status().as_u16(),
                expected,
                "{method} {path}{suffix}"
            );
        }
    }
    for (path, expected) in [
        ("/api/auth/me", 405),
        ("/api/auth/invitations/resolve", 422),
        ("/api/auth/survey", 401),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
}

#[tokio::test]
async fn ingress_and_credential_handlers_share_one_preauth_charge() {
    let app = app();
    for index in 0..16 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/forgot-password")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if index < 15 { 422 } else { 429 }
        );
    }
}
