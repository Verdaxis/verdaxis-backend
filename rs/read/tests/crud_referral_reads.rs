//! Task 17: the R02 referral statistics, R03 leaderboard and R05 public
//! resolution route.
//!
//! The projection and code-grammar tests run everywhere, with no database and
//! `SQLX_OFFLINE=true`. The one end-to-end test drives the real axum router
//! against a disposable database when `REFERRAL_INTEROP_ADMIN_URL` is present;
//! the disposable runner supplies it and also sets `REQUIRE_REFERRAL_INTEROP`,
//! so a missing URL fails there instead of silently skipping.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use verdaxis_read::crud::referrals::{
    LeaderboardUser, MyReferralRow, ReferralSettings, ReferrerCount, ResolveRow,
    code_matches_python_format, display_name, leaderboard_entries, referral_stats,
    resolve_response,
};

fn item(
    status: &str,
    organization: Option<&str>,
    role: Option<&str>,
    seconds: i64,
) -> MyReferralRow {
    MyReferralRow {
        organization_name: organization.map(str::to_owned),
        role: role.map(str::to_owned),
        status: status.to_owned(),
        signed_up_at: Utc.timestamp_opt(seconds, 0).unwrap(),
    }
}

#[test]
fn statistics_count_only_verified_and_active() {
    let rows = vec![
        item("ACTIVE", Some("Buyer Co"), Some("BUYER"), 30),
        item("VERIFIED", None, None, 20),
        item("SIGNED_UP", Some("Fuel Co"), Some("SELLER"), 10),
    ];
    let body = referral_stats(&rows);
    assert_eq!(body["total"], 3);
    assert_eq!(body["verified"], 2);
    assert_eq!(body["active"], 1);
    assert_eq!(body["referrals"].as_array().unwrap().len(), 3);
}

#[test]
fn statistics_keep_row_order_and_explicit_nulls() {
    let rows = vec![
        item("ACTIVE", Some("Buyer Co"), Some("BUYER"), 30),
        item("SIGNED_UP", None, None, 20),
    ];
    let items = referral_stats(&rows)["referrals"].clone();
    let items = items.as_array().unwrap();
    // Source order is preserved exactly; this projection never re-sorts.
    assert_eq!(items[0]["status"], "ACTIVE");
    assert_eq!(items[0]["organization_name"], "Buyer Co");
    assert_eq!(items[0]["signed_up_at"], "1970-01-01T00:00:30Z");
    assert_eq!(items[1]["status"], "SIGNED_UP");
    assert_eq!(items[1]["organization_name"], Value::Null);
    assert_eq!(items[1]["role"], Value::Null);
}

#[test]
fn statistics_items_never_expose_email_or_identifiers() {
    let rows = vec![item("ACTIVE", Some("Buyer Co"), Some("BUYER"), 30)];
    let items = referral_stats(&rows)["referrals"].clone();
    let entry = items[0].clone();
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["organization_name", "role", "status", "signed_up_at"],
        "the item contract has exactly the four declared fields"
    );
    let text = entry.to_string();
    assert!(!text.contains("email"));
    assert!(!text.contains("id"));
}

#[test]
fn leaderboard_skips_missing_users_without_advancing_rank() {
    let first = Uuid::from_u128(1);
    let missing = Uuid::from_u128(2);
    let third = Uuid::from_u128(3);
    let counts = vec![
        ReferrerCount {
            referrer_id: first,
            referral_count: 5,
        },
        ReferrerCount {
            referrer_id: missing,
            referral_count: 4,
        },
        ReferrerCount {
            referrer_id: third,
            referral_count: 3,
        },
    ];
    let users = HashMap::from([
        (
            first,
            LeaderboardUser {
                id: first,
                first_name: Some("Ada".into()),
                last_name: Some("Lovelace".into()),
                organization_name: Some("Analytical".into()),
            },
        ),
        (
            third,
            LeaderboardUser {
                id: third,
                first_name: Some("   ".into()),
                last_name: None,
                organization_name: None,
            },
        ),
    ]);
    let body = leaderboard_entries(&counts, &users);
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["rank"], 1);
    assert_eq!(entries[0]["user_name"], "Ada Lovelace");
    assert_eq!(entries[0]["organization_name"], "Analytical");
    assert_eq!(entries[0]["referral_count"], 5);
    // Python skips the row entirely, so rank still reads 2, not 3.
    assert_eq!(entries[1]["rank"], 2);
    assert_eq!(entries[1]["user_name"], "Anonymous");
    assert_eq!(entries[1]["organization_name"], Value::Null);
    assert_eq!(entries[1]["referral_count"], 3);
}

#[test]
fn leaderboard_keeps_ties_as_distinct_ranked_entries() {
    let first = Uuid::from_u128(1);
    let second = Uuid::from_u128(2);
    let counts = vec![
        ReferrerCount {
            referrer_id: first,
            referral_count: 7,
        },
        ReferrerCount {
            referrer_id: second,
            referral_count: 7,
        },
    ];
    let entry = |id: Uuid, name: &str| {
        (
            id,
            LeaderboardUser {
                id,
                first_name: Some(name.into()),
                last_name: None,
                organization_name: None,
            },
        )
    };
    let users = HashMap::from([entry(first, "Ada"), entry(second, "Bob")]);
    let body = leaderboard_entries(&counts, &users);
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    // A tie is not collapsed: both members survive with their own rank.
    assert_ne!(entries[0]["rank"], entries[1]["rank"]);
    assert_eq!(entries[0]["referral_count"], 7);
    assert_eq!(entries[1]["referral_count"], 7);
}

#[test]
fn leaderboard_is_empty_without_referrals() {
    assert_eq!(leaderboard_entries(&[], &HashMap::new()), json!([]));
}

#[test]
fn display_name_trims_like_python_and_falls_back() {
    assert_eq!(display_name(Some("Ada"), Some("Lovelace")), "Ada Lovelace");
    assert_eq!(display_name(Some("Ada"), None), "Ada");
    assert_eq!(display_name(None, Some("Lovelace")), "Lovelace");
    // Both outer spaces are trimmed; the three spaces between the parts stay.
    assert_eq!(
        display_name(Some(" Ada "), Some(" Lovelace ")),
        "Ada   Lovelace"
    );
    assert_eq!(display_name(None, None), "Anonymous");
    assert_eq!(display_name(Some("   "), Some("  ")), "Anonymous");
    // Python `str.isspace()` also treats the four C0 separators as space.
    assert_eq!(display_name(Some("\u{1c}"), None), "Anonymous");
}

#[test]
fn resolution_covers_invalid_unknown_valid_and_admin() {
    // Unknown code: the database returned no row.
    let unknown = resolve_response(None);
    assert_eq!(unknown["valid"], false);
    assert_eq!(unknown["organization_name"], Value::Null);
    assert_eq!(unknown["organization_type"], Value::Null);
    assert_eq!(unknown["referrer_name"], Value::Null);

    let valid = ResolveRow {
        role: Some("BUYER".into()),
        first_name: Some("Ada".into()),
        last_name: Some("Lovelace".into()),
        organization_name: Some("Analytical".into()),
        organization_type: Some("FUEL_BUYER".into()),
    };
    let body = resolve_response(Some(&valid));
    assert_eq!(body["valid"], true);
    assert_eq!(body["organization_name"], "Analytical");
    assert_eq!(body["organization_type"], "FUEL_BUYER");
    assert_eq!(body["referrer_name"], "Ada Lovelace");

    // ADMIN keeps the platform label and discloses no person or organization type.
    let admin = ResolveRow {
        role: Some("ADMIN".into()),
        first_name: Some("Root".into()),
        last_name: None,
        organization_name: Some("Platform".into()),
        organization_type: Some("FUEL_BUYER".into()),
    };
    let body = resolve_response(Some(&admin));
    assert_eq!(body["valid"], true);
    assert_eq!(body["organization_name"], "Verdaxis Exchange");
    assert_eq!(body["organization_type"], Value::Null);
    assert_eq!(body["referrer_name"], Value::Null);

    // A nameless non-admin keeps valid=true and suppresses the fallback name.
    let anonymous = ResolveRow {
        role: Some("BUYER".into()),
        first_name: None,
        last_name: None,
        organization_name: None,
        organization_type: None,
    };
    let body = resolve_response(Some(&anonymous));
    assert_eq!(body["valid"], true);
    assert_eq!(body["referrer_name"], Value::Null);
    assert_eq!(body["organization_name"], Value::Null);
    assert_eq!(body["organization_type"], Value::Null);
}

#[test]
fn code_format_matches_python_end_anchor() {
    assert!(code_matches_python_format("VDX-ABC123"));
    assert!(code_matches_python_format("VDX-123456"));
    // Python's `$` also matches immediately before one trailing newline, which a
    // Rust `$` would not.
    assert!(code_matches_python_format("VDX-ABC123\n"));
    assert!(!code_matches_python_format("VDX-ABC123\n\n"));
    assert!(!code_matches_python_format("\nVDX-ABC123"));
    assert!(!code_matches_python_format("VDX-ABC12"));
    assert!(!code_matches_python_format("VDX-ABC1234"));
    assert!(!code_matches_python_format("VDX-abc123"));
    assert!(!code_matches_python_format("VDX-ABC12-"));
    assert!(!code_matches_python_format("VDX-ABC 23"));
    assert!(!code_matches_python_format("VDX-ABC123\r\n"));
    assert!(!code_matches_python_format(""));
    assert!(!code_matches_python_format("xVDX-ABC123"));
}

// ---------------------------------------------------------------------------
// End-to-end route test against one disposable database.
// ---------------------------------------------------------------------------

const ADMIN_URL: &str = "REFERRAL_INTEROP_ADMIN_URL";
const APP_URL: &str = "REFERRAL_INTEROP_APP_URL";
const TIME_ZONE: &str = "REFERRAL_INTEROP_TIME_ZONE";
const REQUIRE: &str = "REQUIRE_REFERRAL_INTEROP";
const SECRET: &str = "referral-interop-secret-at-least-32-bytes";
const ISSUER: &str = "verdaxis-referral-test-issuer";
const AUDIENCE: &str = "verdaxis-referral-test-audience";

fn access_token(subject: Uuid) -> String {
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": subject.to_string(),
        "iss": ISSUER,
        "aud": AUDIENCE,
        "type": "access",
        "iat": now,
        "exp": now + 3_600,
        "jti": "referral-interop-token",
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign fixture token")
}

async fn insert_organization(admin: &sqlx::PgPool, id: Uuid, name: &str) {
    sqlx::query("INSERT INTO organizations (id,name,domain,type) VALUES ($1,$2,$3,'FUEL_BUYER')")
        .bind(id)
        .bind(name)
        .bind(format!("{id}.phase2.invalid"))
        .execute(admin)
        .await
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_user(
    admin: &sqlx::PgPool,
    id: Uuid,
    role: &str,
    organization: Option<Uuid>,
    verified: bool,
    referral_code: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO users (id,email,password_hash,first_name,last_name,role,status,organization_id,email_verified,must_change_password,referral_code) \
         VALUES ($1,$2,'not-a-login',$3,$4,$5,'APPROVED',$6,$7,false,$8)",
    )
    .bind(id)
    .bind(format!("{id}@phase2.invalid"))
    .bind(first_name)
    .bind(last_name)
    .bind(role)
    .bind(organization)
    .bind(verified)
    .bind(referral_code)
    .execute(admin)
    .await
    .unwrap();
}

async fn insert_referral(
    admin: &sqlx::PgPool,
    id: Uuid,
    referrer: Uuid,
    referred: Uuid,
    status: &str,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO referrals (id,referrer_id,referred_user_id,referral_code_used,status,created_at) \
         VALUES ($1,$2,$3,'VDX-AAA111',$4,$5::timestamptz)",
    )
    .bind(id)
    .bind(referrer)
    .bind(referred)
    .bind(status)
    .bind(created_at)
    .execute(admin)
    .await
    .unwrap();
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    support: bool,
) -> (u16, Value) {
    use tower::ServiceExt;
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if support {
        builder = builder.header("x-verdaxis-market-support-context", "1");
    }
    let response = app
        .clone()
        .oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn postgres_referral_reads_and_public_resolution() {
    let Ok(admin_url) = std::env::var(ADMIN_URL) else {
        assert!(
            std::env::var(REQUIRE).is_err(),
            "the referral interop database URL is required"
        );
        return;
    };
    let app_url = std::env::var(APP_URL).expect("the app database URL is required");
    // The disposable runner sets the database-level timezone to the oracle
    // timezone, and `connect` refuses any session that disagrees with it.
    let time_zone = std::env::var(TIME_ZONE).unwrap_or_else(|_| "UTC".into());
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url)
        .await
        .unwrap();

    // One deterministic fixture: the database is disposable and owned by the
    // runner, so the reads always start from exactly these rows.
    sqlx::query("TRUNCATE referrals, users, organizations RESTART IDENTITY CASCADE")
        .execute(&admin)
        .await
        .unwrap();
    let organization = Uuid::from_u128(0x1001);
    let actor = Uuid::from_u128(0x2001);
    let referred_active = Uuid::from_u128(0x2002);
    let referred_signed = Uuid::from_u128(0x2003);
    let other_referrer = Uuid::from_u128(0x2004);
    let other_referred = Uuid::from_u128(0x2005);
    let platform_admin = Uuid::from_u128(0x2006);
    insert_organization(&admin, organization, "Buyer Co").await;
    insert_user(
        &admin,
        actor,
        "BUYER",
        Some(organization),
        true,
        Some("VDX-AAA111"),
        Some("Ada"),
        Some("Lovelace"),
    )
    .await;
    insert_user(
        &admin,
        referred_active,
        "BUYER",
        Some(organization),
        true,
        None,
        Some("Bob"),
        None,
    )
    .await;
    insert_user(
        &admin,
        referred_signed,
        "SELLER",
        None,
        true,
        None,
        None,
        None,
    )
    .await;
    insert_user(
        &admin,
        other_referrer,
        "SELLER",
        None,
        true,
        Some("VDX-BBB222"),
        Some("Cara"),
        Some("Dee"),
    )
    .await;
    insert_user(
        &admin,
        other_referred,
        "BUYER",
        None,
        true,
        None,
        Some("Dan"),
        None,
    )
    .await;
    insert_user(
        &admin,
        platform_admin,
        "ADMIN",
        None,
        true,
        Some("VDX-ADMIN1"),
        Some("Root"),
        None,
    )
    .await;
    insert_referral(
        &admin,
        Uuid::from_u128(0x3001),
        actor,
        referred_active,
        "ACTIVE",
        "2026-01-02T00:00:00+00:00",
    )
    .await;
    insert_referral(
        &admin,
        Uuid::from_u128(0x3002),
        actor,
        referred_signed,
        "SIGNED_UP",
        "2026-01-01T00:00:00+00:00",
    )
    .await;
    insert_referral(
        &admin,
        Uuid::from_u128(0x3003),
        other_referrer,
        other_referred,
        "VERIFIED",
        "2026-01-03T00:00:00+00:00",
    )
    .await;

    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "p0_20261016_union".into(),
        database_url: app_url,
        db_session_time_zone: time_zone,
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = verdaxis_read::AppState::new(
        config.clone(),
        pools,
        verdaxis_read::MarketPolicy {
            source_sha: config.release_sha.clone(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let mut settings = ReferralSettings::new(
        verdaxis_read::auth::VerificationSettings::new(
            SECRET.into(),
            None,
            ISSUER.into(),
            AUDIENCE.into(),
        ),
        "https://app.verdaxis.test",
    );
    let app = verdaxis_read::runtime::http::router_with(
        verdaxis_read::crud::referrals::routes(settings.clone()),
        state.clone(),
    );

    let token = access_token(actor);
    let snapshot = |id: Uuid| {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT concat_ws('|', referral_code, password_hash, role, status, email_verified, organization_id, must_change_password) \
                 FROM users WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    };
    let before = snapshot(actor).await;

    // R02: statistics for the actor.
    let (status, body) = send(
        &app,
        "GET",
        "/api/referrals/my-referrals",
        Some(&token),
        false,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(body["verified"], 1);
    assert_eq!(body["active"], 1);
    let referrals = body["referrals"].as_array().unwrap();
    assert_eq!(referrals.len(), 2);
    assert_eq!(referrals[0]["status"], "ACTIVE");
    assert_eq!(referrals[0]["organization_name"], "Buyer Co");
    assert_eq!(referrals[0]["role"], "BUYER");
    assert_eq!(referrals[0]["signed_up_at"], "2026-01-02T00:00:00Z");
    assert_eq!(referrals[1]["status"], "SIGNED_UP");
    assert_eq!(referrals[1]["organization_name"], Value::Null);
    assert_eq!(referrals[1]["role"], "SELLER");
    assert_eq!(referrals[1]["signed_up_at"], "2026-01-01T00:00:00Z");

    // A support context keeps GETs actor-scoped; the body is identical.
    let (support_status, support_body) = send(
        &app,
        "GET",
        "/api/referrals/my-referrals",
        Some(&token),
        true,
    )
    .await;
    assert_eq!(support_status, 200, "{support_body}");
    assert_eq!(support_body, body);

    // R03: the platform leaderboard, ordered by count.
    let (status, body) = send(
        &app,
        "GET",
        "/api/referrals/leaderboard",
        Some(&token),
        false,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["rank"], 1);
    assert_eq!(entries[0]["user_name"], "Ada Lovelace");
    assert_eq!(entries[0]["organization_name"], "Buyer Co");
    assert_eq!(entries[0]["referral_count"], 2);
    assert_eq!(entries[1]["rank"], 2);
    assert_eq!(entries[1]["user_name"], "Cara Dee");
    assert_eq!(entries[1]["organization_name"], Value::Null);
    assert_eq!(entries[1]["referral_count"], 1);

    // R05: a valid user code is resolvable without a token, and a garbage
    // bearer never changes the public answer.
    for bearer in [None, Some("not-a-jwt"), Some(token.as_str())] {
        let (status, body) = send(
            &app,
            "GET",
            "/api/referrals/resolve/VDX-AAA111",
            bearer,
            false,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["valid"], true);
        assert_eq!(body["organization_name"], "Buyer Co");
        assert_eq!(body["organization_type"], "FUEL_BUYER");
        assert_eq!(body["referrer_name"], "Ada Lovelace");
    }
    // ADMIN resolution keeps the platform label.
    let (status, body) = send(
        &app,
        "GET",
        "/api/referrals/resolve/VDX-ADMIN1",
        None,
        false,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["organization_name"], "Verdaxis Exchange");
    assert_eq!(body["organization_type"], Value::Null);
    assert_eq!(body["referrer_name"], Value::Null);
    // Unknown and malformed codes answer with valid=false.
    for code in ["VDX-ZZZ999", "VDX-zzz999", "nope"] {
        let (status, body) = send(
            &app,
            "GET",
            &format!("/api/referrals/resolve/{code}"),
            None,
            false,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["valid"], false);
        assert_eq!(body["organization_name"], Value::Null);
        assert_eq!(body["organization_type"], Value::Null);
        assert_eq!(body["referrer_name"], Value::Null);
    }

    // U routes refuse a missing or unusable bearer.
    for path in ["/api/referrals/my-referrals", "/api/referrals/leaderboard"] {
        let (status, body) = send(&app, "GET", path, None, false).await;
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["detail"], "Not authenticated");
        let (status, body) = send(&app, "GET", path, Some("not-a-jwt"), false).await;
        assert_eq!(status, 401, "{body}");
        assert_eq!(body["detail"]["code"], "AUTH_CREDENTIALS_INVALID");
    }
    // A token whose subject does not exist is refused the same way.
    let (status, body) = send(
        &app,
        "GET",
        "/api/referrals/my-referrals",
        Some(&access_token(Uuid::from_u128(0x9999))),
        false,
    )
    .await;
    assert_eq!(status, 401, "{body}");

    // FastAPI GET routes do not implicitly accept HEAD.
    use tower::ServiceExt;
    for path in [
        "/api/referrals/my-referrals",
        "/api/referrals/leaderboard",
        "/api/referrals/resolve/VDX-AAA111",
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("HEAD")
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 405, "HEAD {path}");
        assert_eq!(response.headers()["allow"], "GET");
    }

    // No read changed the actor row.
    assert_eq!(snapshot(actor).await, before);

    // R01 preserves existing codes and changes no unrelated account field.
    let all_users = || async {
        sqlx::query_scalar::<_, Value>(
            "SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY id), '[]') FROM users u",
        )
        .fetch_one(&admin)
        .await
        .unwrap()
    };
    let unchanged = all_users().await;
    let (status, body) = send(&app, "POST", "/api/referrals/my-code", Some(&token), false).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["referral_code"], "VDX-AAA111");
    assert_eq!(all_users().await, unchanged);

    sqlx::query("UPDATE users SET email_verified = false WHERE id = $1")
        .bind(other_referrer)
        .execute(&admin)
        .await
        .unwrap();
    let unchanged = all_users().await;
    let (status, body) = send(
        &app,
        "POST",
        "/api/referrals/my-code",
        Some(&access_token(other_referrer)),
        false,
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(all_users().await, unchanged);

    let unchanged = all_users().await;
    let new_token = access_token(referred_active);
    let (first, second) = tokio::join!(
        send(
            &app,
            "POST",
            "/api/referrals/my-code",
            Some(&new_token),
            false
        ),
        send(
            &app,
            "POST",
            "/api/referrals/my-code",
            Some(&new_token),
            false
        ),
    );
    assert_eq!(first.0, 200, "{}", first.1);
    assert_eq!(second.0, 200, "{}", second.1);
    assert_eq!(first.1, second.1);
    assert!(code_matches_python_format(
        first.1["referral_code"].as_str().unwrap()
    ));
    let mut expected = unchanged;
    for user in expected.as_array_mut().unwrap() {
        if user["id"] == referred_active.to_string() {
            user["referral_code"] = first.1["referral_code"].clone();
        }
    }
    assert_eq!(all_users().await, expected);

    // Invite provider failure retains exactly the code committed before send.
    async fn invite_request(app: &axum::Router, token: &str, email: &str) -> (u16, Value) {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/referrals/invite")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(json!({"email":email}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    let invite_actor = other_referred;
    let before = all_users().await;
    let (status, body) = invite_request(&app, &access_token(invite_actor), "new@example.com").await;
    assert_eq!(status, 502, "{body}");
    let after = all_users().await;
    let mut expected = before;
    for user in expected.as_array_mut().unwrap() {
        if user["id"] == invite_actor.to_string() {
            let stored = after
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == user["id"])
                .unwrap();
            assert!(code_matches_python_format(
                stored["referral_code"].as_str().unwrap()
            ));
            user["referral_code"] = stored["referral_code"].clone();
        }
    }
    assert_eq!(after, expected);

    // Repeated accepted invites are separate attempts; use only a local stub.
    let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let ledger = attempts.clone();
    let stub = axum::Router::new().route(
        "/emails",
        axum::routing::post(move |axum::Json(payload): axum::Json<Value>| {
            let ledger = ledger.clone();
            async move {
                ledger.lock().unwrap().push(payload);
                axum::http::StatusCode::CREATED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, stub).await.unwrap();
    });
    settings.mail = verdaxis_read::crud::referrals::invite::MailClient::loopback(
        format!("http://{address}/emails"),
        "test-key".into(),
    )
    .unwrap();
    settings.email_from = "Verdaxis <sender@example.com>".into();
    let sending = verdaxis_read::runtime::http::router_with(
        verdaxis_read::crud::referrals::routes(settings),
        state,
    );
    for _ in 0..2 {
        let (status, body) =
            invite_request(&sending, &access_token(invite_actor), "new@example.com").await;
        assert_eq!(status, 200, "{body}");
    }
    assert_eq!(attempts.lock().unwrap().len(), 2);
    assert_eq!(all_users().await, after);

    // Insert ties in reverse identifier order. The stable order must apply
    // before the tenth-place cut, not just to the returned rows.
    sqlx::query("TRUNCATE referrals")
        .execute(&admin)
        .await
        .unwrap();
    let mut referral_number = 0_u128;
    for number in (1_u128..=12).rev() {
        let referrer = Uuid::from_u128(0x6000 + number);
        let name = format!("Referrer {number:02}");
        insert_user(
            &admin,
            referrer,
            "BUYER",
            None,
            true,
            None,
            Some(&name),
            None,
        )
        .await;
        let count = match number {
            1 => 4,
            2 | 3 => 3,
            _ => 2,
        };
        for _ in 0..count {
            referral_number += 1;
            let referred = Uuid::from_u128(0x7000 + referral_number);
            insert_user(&admin, referred, "BUYER", None, true, None, None, None).await;
            insert_referral(
                &admin,
                Uuid::from_u128(0x8000 + referral_number),
                referrer,
                referred,
                "SIGNED_UP",
                "2026-01-01T00:00:00+00:00",
            )
            .await;
        }
    }
    let (status, leaderboard) = send(
        &app,
        "GET",
        "/api/referrals/leaderboard",
        Some(&token),
        false,
    )
    .await;
    assert_eq!(status, 200, "{leaderboard}");
    let expected: Vec<Value> = (1..=10)
        .map(|number| {
            json!({
                "rank": number,
                "user_name": format!("Referrer {number:02}"),
                "organization_name": null,
                "referral_count": if number == 1 { 4 } else if number <= 3 { 3 } else { 2 },
            })
        })
        .collect();
    assert_eq!(leaderboard, json!(expected));
    server.abort();
    let _ = server.await;
}
