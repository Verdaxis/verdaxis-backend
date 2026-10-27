use serde_json::{Value, json};
use sqlx::Connection;
use uuid::Uuid;
use verdaxis_read::crud::alerts::{AlertCreate, authorize_delete, organization_id};

#[test]
fn foreign_alert_delete_is_403() {
    let own = Uuid::from_u128(1);
    let foreign = Uuid::from_u128(2);
    let error = authorize_delete(own, Some(foreign)).unwrap_err();
    assert_eq!(error.status, 403);
    assert_eq!(error.detail, "You can only delete your own alerts");
    assert_eq!(authorize_delete(own, None).unwrap_err().status, 404);
    assert!(authorize_delete(own, Some(own)).is_ok());
}

#[test]
fn missing_organization_is_400() {
    let error = organization_id(None).unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.detail, "User must belong to an organization");
}

#[test]
fn request_validation_matches_python_http_vectors() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase2/alerts.json"
    ))
    .unwrap();
    for vector in fixture["vectors"].as_array().unwrap() {
        let result = AlertCreate::validate(vector["body"].clone());
        if vector["python"]["kind"] == "accepted" {
            let parsed = result.unwrap_or_else(|error| panic!("{}: {error:?}", vector["id"]));
            let expected = &vector["python"]["values"];
            assert_eq!(
                parsed.product_id.to_string(),
                expected["product_id"],
                "{}",
                vector["id"]
            );
            assert_eq!(
                json!(parsed.delivery_point_id),
                expected["delivery_point_id"],
                "{}",
                vector["id"]
            );
            assert_eq!(parsed.direction, expected["direction"], "{}", vector["id"]);
        } else {
            let error = result.expect_err(vector["id"].as_str().unwrap());
            let expected: Value =
                serde_json::from_str(vector["python"]["body"].as_str().unwrap()).unwrap();
            assert_eq!(error.status, 422, "{}", vector["id"]);
            assert_eq!(
                json!({"detail": error.detail}),
                expected,
                "{}",
                vector["id"]
            );
        }
    }
}

#[tokio::test]
async fn postgres_crud() {
    use verdaxis_read::crud::alerts::{create, delete, list};
    let Ok(url) = std::env::var("ALERT_TEST_DATABASE_URL") else {
        eprintln!("PostgreSQL component coverage requires the test_phase2_alert_interop.py driver");
        return;
    };
    let mut connection = sqlx::PgConnection::connect(&url).await.unwrap();
    let fixture_id = |name| Uuid::parse_str(&std::env::var(name).unwrap()).unwrap();
    let organization = fixture_id("ALERT_TEST_ORGANIZATION");
    let foreign = fixture_id("ALERT_TEST_FOREIGN");
    let product = fixture_id("ALERT_TEST_PRODUCT");
    let point = fixture_id("ALERT_TEST_POINT");
    let policy = verdaxis_read::MarketPolicy {
        source_sha: String::new(),
        products: vec![],
        points: vec![],
    };
    let input = |threshold: &str| {
        AlertCreate::validate(json!({
        "product_id":product,"delivery_point_id":point,"direction":"above","threshold_usd":threshold
    })).unwrap()
    };
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    let first = create(&mut connection, organization, input("12.345"), &policy)
        .await
        .unwrap();
    assert_eq!(first["threshold_usd"], "12.35");
    assert_eq!(first["product_name"], "Alert legacy fixture");
    assert!(first["market_product"].is_null());
    let id = Uuid::parse_str(first["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        delete(&mut connection, foreign, id)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        list(&mut connection, foreign, &policy).await.unwrap(),
        json!([])
    );
    for _ in 0..4 {
        create(&mut connection, organization, input("1"), &policy)
            .await
            .unwrap();
    }
    assert_eq!(
        create(&mut connection, organization, input("1"), &policy)
            .await
            .unwrap_err()
            .status,
        403
    );
    sqlx::query("UPDATE price_alerts SET is_active=false WHERE id=$1")
        .bind(id)
        .execute(&mut connection)
        .await
        .unwrap();
    let replacement = create(&mut connection, organization, input("0.0001"), &policy)
        .await
        .unwrap();
    assert_eq!(replacement["threshold_usd"], "0.00");
    assert_eq!(
        list(&mut connection, organization, &policy)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        6
    );
    sqlx::query("DELETE FROM price_alerts WHERE org_id=$1")
        .bind(organization)
        .execute(&mut connection)
        .await
        .unwrap();
    for (threshold, invalid_fk) in [("99999999.995", false), ("1", true)] {
        sqlx::query("SAVEPOINT invalid_alert")
            .execute(&mut connection)
            .await
            .unwrap();
        let mut bad = input(threshold);
        if invalid_fk {
            bad.product_id = Uuid::new_v4();
        }
        assert!(
            create(&mut connection, organization, bad, &policy)
                .await
                .is_err()
        );
        sqlx::query("ROLLBACK TO SAVEPOINT invalid_alert")
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::query("SAVEPOINT invalid_point")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut bad = input("1");
    bad.delivery_point_id = Some(Uuid::new_v4());
    assert!(
        create(&mut connection, organization, bad, &policy)
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK TO SAVEPOINT invalid_point")
        .execute(&mut connection)
        .await
        .unwrap();
    let final_alert = create(&mut connection, organization, input("12.345"), &policy)
        .await
        .unwrap();
    let removable = create(&mut connection, organization, input("1"), &policy)
        .await
        .unwrap();
    let removable = Uuid::parse_str(removable["id"].as_str().unwrap()).unwrap();
    delete(&mut connection, organization, removable)
        .await
        .unwrap();
    assert_eq!(
        delete(&mut connection, organization, removable)
            .await
            .unwrap_err()
            .status,
        404
    );
    assert_ne!(first["id"], final_alert["id"]);
    sqlx::query("COMMIT")
        .execute(&mut connection)
        .await
        .unwrap();
    postgres_http_admission(&url, organization, product, point).await;
}

async fn postgres_http_admission(url: &str, organization: Uuid, product: Uuid, point: Uuid) {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    use verdaxis_read::auth::{VerificationSettings, limits::SharedLimits};
    use verdaxis_read::{AppState, Config, MarketPolicy};
    let config = Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        compliance_eur_usd_rate: None,
        fleet_artifact_path: "data/fleet_demand.json".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 16,
        pool_max: 17,
        python_budget: 52,
        configured_max_connections: 100,
        phase2: Default::default(),
    };
    let pools = verdaxis_read::connect(&config).await.unwrap();
    let state = AppState::new(
        config,
        pools,
        MarketPolicy {
            source_sha: String::new(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let key = "synthetic-alert-http-key-minimum-32-characters";
    let app = verdaxis_read::crud::alerts::router(
        state,
        VerificationSettings::new(key.into(), None, "alert-test".into(), "alert-test".into()),
        SharedLimits::default(),
    );
    let actors: Vec<String> = std::env::var("ALERT_TEST_ACTORS")
        .unwrap()
        .split(',')
        .map(str::to_owned)
        .collect();
    let token = |actor: &str| {
        let now = chrono::Utc::now().timestamp();
        jsonwebtoken::encode(&jsonwebtoken::Header::default(), &json!({
            "sub":actor,"type":"access","iss":"alert-test","aud":"alert-test","iat":now,"exp":now+300
        }), &jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap()
    };
    let existing = sqlx::query_scalar::<_, Uuid>("SELECT id FROM price_alerts WHERE org_id=$1")
        .bind(organization)
        .fetch_one(&mut sqlx::PgConnection::connect(url).await.unwrap())
        .await
        .unwrap();
    for (method, path, actor, payload, status) in [
        (
            "POST",
            "/api/alerts".to_owned(),
            3,
            json!({"product_id":product,"delivery_point_id":point,"direction":"above","threshold_usd":"1"}),
            400,
        ),
        ("GET", "/api/alerts".to_owned(), 3, Value::Null, 400),
        (
            "DELETE",
            format!("/api/alerts/{existing}"),
            3,
            Value::Null,
            400,
        ),
        (
            "DELETE",
            format!("/api/alerts/{existing}"),
            2,
            Value::Null,
            403,
        ),
        ("DELETE", "/api/alerts/bad".to_owned(), 0, Value::Null, 422),
        ("GET", "/api/alerts".to_owned(), 1, Value::Null, 200),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {}", token(&actors[actor])))
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if method == "GET" && status == 200 {
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(body.as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn positive_threshold_does_not_narrow_before_postgres_rounding() {
    for threshold in [
        "0.0001",
        "99999999.995",
        "123456789012345678901234567890.123",
        "1e100",
    ] {
        let parsed = AlertCreate::validate(json!({
            "product_id": Uuid::from_u128(1), "direction": "below", "threshold_usd": threshold
        }))
        .unwrap();
        assert!(!parsed.threshold_usd.is_empty());
    }
}
