pub use verdaxis_read::*;

#[path = "../src/business/vessels.rs"]
mod vessels;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
#[cfg(feature = "contract-tests")]
use chrono::{Duration, Utc};
use serde_json::Value;
#[cfg(feature = "contract-tests")]
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
#[cfg(feature = "contract-tests")]
use sqlx::{Connection, PgConnection};
use tower::ServiceExt;
#[cfg(feature = "contract-tests")]
use uuid::Uuid;

const SECRET: &str = "vessel-component-test-secret-at-least-32-characters";

fn verification() -> auth::VerificationSettings {
    auth::VerificationSettings::new(
        SECRET.into(),
        None,
        "vessel-test".into(),
        "vessel-test".into(),
    )
}

#[cfg(feature = "contract-tests")]
fn token(id: Uuid) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({"sub":id,"type":"access","iss":"vessel-test","aud":"vessel-test",
            "jti":Uuid::new_v4(),"iat":Utc::now().timestamp(),
            "exp":(Utc::now()+Duration::hours(1)).timestamp()}),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

fn config(url: String) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: url,
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 4,
        pool_max: 5,
        python_budget: 20,
        configured_max_connections: 100,
    }
}

fn app(pool: sqlx::PgPool, url: String) -> Router {
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&url)
        .unwrap();
    let state = AppState::new(
        config(url),
        runtime::db::Pools { read: pool, health },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    vessels::routes(vessels::VesselAuthority::new(
        verification(),
        auth::limits::SharedLimits::default(),
    ))
    .with_state(state)
}

async fn send(app: &Router, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(path);
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg(feature = "contract-tests")]
fn ids(body: &Value) -> Vec<Uuid> {
    let mut ids: Vec<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|row| Uuid::parse_str(row["id"].as_str().unwrap()).unwrap())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn p4_t12_c05() {
    let url = "postgres://offline_app:offline_password@127.0.0.1:1/offline_test";
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(url)
        .unwrap();
    let app = app(pool, url.into());
    for bearer in [None, Some("invalid")] {
        let mut refused = None;
        for path in [
            "/api/vessels",
            "/api/vessels/not-a-uuid",
            "/api/vessels/00000000-0000-0000-0000-000000000001",
        ] {
            let result = send(&app, path, bearer).await;
            assert_eq!(result.0, StatusCode::UNAUTHORIZED);
            assert!(result.1["detail"]["code"].is_string());
            assert!(result.1["detail"]["message"].is_string());
            if let Some(ref expected) = refused {
                assert_eq!(&result, expected);
            }
            refused = Some(result);
        }
    }
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;

    /// One row of the raw geography snapshot compared across repeated reads.
    type SnapshotRow = (
        Uuid,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        chrono::DateTime<Utc>,
    );

    struct Fixture {
        admin: PgConnection,
        pool: sqlx::PgPool,
        app: Router,
        schema: String,
        orgs: [Uuid; 3],
        users: [Uuid; 6],
        vessels: [Uuid; 4],
    }

    impl Fixture {
        async fn new() -> Self {
            let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL");
            let runtime =
                std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
            let m = url::Url::parse(&migrator).unwrap();
            let r = url::Url::parse(&runtime).unwrap();
            assert_eq!(m.host_str(), Some("127.0.0.1"));
            assert_eq!(r.host_str(), m.host_str());
            assert_eq!(r.port(), m.port());
            assert_eq!(r.path(), m.path());
            assert!(r.path().ends_with("_test"));
            assert_eq!(r.username(), "rust_app");
            assert_ne!(m.username(), r.username());
            let mut admin = PgConnection::connect(&migrator).await.unwrap();
            let schema = format!("p4_vessels_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema},public"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql("CREATE TABLE organizations(id uuid PRIMARY KEY); \
                CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations,role text,status text,\
                email_verified boolean NOT NULL DEFAULT true,must_change_password boolean NOT NULL DEFAULT false,\
                password_changed_at timestamptz,authentication_revoked_at timestamptz); \
                CREATE TABLE vessels(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations,name text NOT NULL,\
                imo_number text NOT NULL UNIQUE,vessel_type text,flag_state text,dwt numeric,cii_rating varchar(1),\
                eu_ets_status text,fueleu_status text,current_location geography(POINT,4326),\
                previous_location geography(POINT,4326),updated_at timestamptz NOT NULL)")
                .execute(&mut admin).await.unwrap();
            let orgs = std::array::from_fn(|_| Uuid::new_v4());
            let users = std::array::from_fn(|_| Uuid::new_v4());
            let vessels = std::array::from_fn(|_| Uuid::new_v4());
            for org in orgs {
                sqlx::query("INSERT INTO organizations VALUES($1)")
                    .bind(org)
                    .execute(&mut admin)
                    .await
                    .unwrap();
            }
            for (index, user) in users.iter().enumerate() {
                let org = match index {
                    0 => Some(orgs[0]),
                    1 => Some(orgs[1]),
                    4 => Some(orgs[2]),
                    _ => None,
                };
                let role = match index {
                    2 => Some("ADMIN"),
                    5 => None,
                    _ => Some("BUYER"),
                };
                sqlx::query(
                    "INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,$3,'APPROVED')",
                )
                .bind(user)
                .bind(org)
                .bind(role)
                .execute(&mut admin)
                .await
                .unwrap();
            }
            for (index, vessel) in vessels.iter().enumerate() {
                let org = match index {
                    0 | 1 => Some(orgs[0]),
                    2 => Some(orgs[1]),
                    _ => None,
                };
                sqlx::query("INSERT INTO vessels(id,organization_id,name,imo_number,vessel_type,flag_state,dwt,cii_rating,eu_ets_status,fueleu_status,current_location,previous_location,updated_at) \
                    VALUES($1,$2,'Fixture vessel',$3,'Tanker','NL',12345.5,'B','COMPLIANT','COMPLIANT',\
                    CASE WHEN $4 THEN ST_SetSRID(ST_MakePoint(4.5,51.9),4326)::geography ELSE NULL END,\
                    CASE WHEN $4 THEN ST_SetSRID(ST_MakePoint(-1.2,50.8),4326)::geography ELSE NULL END,'2026-10-20T00:00:00Z')")
                    .bind(vessel).bind(org).bind(index.to_string()).bind(index % 2 == 0)
                    .execute(&mut admin).await.unwrap();
            }
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; \
                GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app; GRANT UPDATE ON {schema}.users TO rust_app")))
                .execute(&mut admin).await.unwrap();
            let search = format!("SET search_path TO {schema},public");
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .after_connect(move |conn, _| {
                    let search = search.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(search))
                            .execute(conn)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&runtime)
                .await
                .unwrap();
            let app = app(pool.clone(), runtime);
            Self {
                admin,
                pool,
                app,
                schema,
                orgs,
                users,
                vessels,
            }
        }

        async fn get(&self, index: usize, path: &str) -> (StatusCode, Value) {
            send(&self.app, path, Some(&token(self.users[index]))).await
        }

        async fn finish(mut self) {
            drop(self.app);
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn p4_t12_c01() {
        let f = Fixture::new().await;
        for user in [3, 5] {
            assert_eq!(
                f.get(user, "/api/vessels").await,
                (StatusCode::OK, json!([]))
            );
            for id in f.vessels {
                let (status, body) = f.get(user, &format!("/api/vessels/{id}")).await;
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert_eq!(body["detail"]["code"], "VESSEL_NOT_FOUND");
            }
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c02() {
        let f = Fixture::new().await;
        for user in [0, 2, 3] {
            let (status, body) = f.get(user, "/api/vessels/not-a-uuid").await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(body["detail"]["code"], "INVALID_VESSEL_ID");
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c03() {
        let mut f = Fixture::new().await;
        let snapshot = "SELECT id,ST_AsEWKB(current_location::geometry),ST_AsEWKB(previous_location::geometry),updated_at FROM vessels ORDER BY id";
        let before: Vec<SnapshotRow> = sqlx::query_as(snapshot)
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
        for _ in 0..2 {
            let (status, body) = f.get(0, &format!("/api/vessels/{}", f.vessels[0])).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["name"], "Fixture vessel");
            assert_eq!(body["imo_number"], "0");
            assert_eq!(body["vessel_type"], "Tanker");
            assert_eq!(body["flag_state"], "NL");
            assert_eq!(body["dwt"], 12345.5);
            assert_eq!(body["cii_rating"], "B");
            assert_eq!(body["eu_ets_status"], "COMPLIANT");
            assert_eq!(body["fueleu_status"], "COMPLIANT");
            assert_eq!(body["organization_id"], f.orgs[0].to_string());
            for (key, value) in [
                ("lat", 51.9),
                ("lng", 4.5),
                ("prev_lat", 50.8),
                ("prev_lng", -1.2),
            ] {
                assert!((body[key].as_f64().unwrap() - value).abs() < 1e-9);
            }
            assert_eq!(body["current_location"], Value::Null);
            assert_eq!(body["previous_location"], Value::Null);
            assert!(
                body["updated_at"]
                    .as_str()
                    .unwrap()
                    .starts_with("2026-10-20T00:00:00")
            );
            let (_, list) = f.get(0, "/api/vessels").await;
            let null = list
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == f.vessels[1].to_string())
                .unwrap();
            for key in [
                "lat",
                "lng",
                "prev_lat",
                "prev_lng",
                "current_location",
                "previous_location",
            ] {
                assert_eq!(null[key], Value::Null);
            }
        }
        let after: Vec<SnapshotRow> = sqlx::query_as(snapshot)
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(before, after);
        let can_update: bool =
            sqlx::query_scalar("SELECT has_table_privilege('rust_app',$1,'UPDATE')")
                .bind(format!("{}.vessels", f.schema))
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert!(!can_update);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c04() {
        let f = Fixture::new().await;
        for (user, expected) in [
            (0, vec![f.vessels[0], f.vessels[1]]),
            (1, vec![f.vessels[2]]),
            (2, f.vessels.to_vec()),
            (4, vec![]),
        ] {
            let (status, body) = f.get(user, "/api/vessels").await;
            assert_eq!(status, StatusCode::OK);
            let mut sorted = expected.clone();
            sorted.sort();
            assert_eq!(ids(&body), sorted);
            for id in f.vessels {
                let (status, body) = f.get(user, &format!("/api/vessels/{id}")).await;
                if expected.contains(&id) {
                    assert_eq!(status, StatusCode::OK);
                    assert_eq!(body["id"], id.to_string());
                } else {
                    assert_eq!(status, StatusCode::NOT_FOUND);
                    assert_eq!(body["detail"]["code"], "VESSEL_NOT_FOUND");
                }
            }
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c06() {
        let mut f = Fixture::new().await;
        let bearer = token(f.users[0]);
        for assignment in [
            "status='PENDING'",
            "status='REJECTED'",
            "status='APPROVED',must_change_password=true",
            "must_change_password=false,authentication_revoked_at=now()+interval '1 minute'",
            "authentication_revoked_at=NULL,password_changed_at=now()+interval '1 minute'",
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "UPDATE users SET {assignment} WHERE id='{}'",
                f.users[0]
            )))
            .execute(&mut f.admin)
            .await
            .unwrap();
            let mut refused = None;
            for path in [
                "/api/vessels".into(),
                format!("/api/vessels/{}", f.vessels[0]),
                format!("/api/vessels/{}", Uuid::new_v4()),
                "/api/vessels/not-a-uuid".into(),
            ] {
                let result = send(&f.app, &path, Some(&bearer)).await;
                assert!(matches!(
                    result.0,
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ));
                assert!(result.1["detail"]["code"].is_string());
                if let Some(ref expected) = refused {
                    assert_eq!(&result, expected);
                }
                refused = Some(result);
            }
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c07() {
        let mut f = Fixture::new().await;
        sqlx::query("DROP TABLE vessels")
            .execute(&mut f.admin)
            .await
            .unwrap();
        for path in [
            "/api/vessels".into(),
            format!("/api/vessels/{}", f.vessels[0]),
        ] {
            let (status, body) = f.get(0, &path).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["detail"]["code"], "DATABASE_ERROR");
            assert!(!body.to_string().contains(&f.schema));
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t12_c08() {
        let mut f = Fixture::new().await;
        let mut change = f.admin.begin().await.unwrap();
        sqlx::query("UPDATE users SET organization_id=$1 WHERE id=$2")
            .bind(f.orgs[1])
            .bind(f.users[0])
            .execute(&mut *change)
            .await
            .unwrap();
        let app = f.app.clone();
        let bearer = token(f.users[0]);
        let request = tokio::spawn(async move { send(&app, "/api/vessels", Some(&bearer)).await });
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND usename='rust_app' AND wait_event_type='Lock' AND query LIKE '%FROM users%')")
                    .fetch_one(&f.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("route must reach the locked account projection");
        assert!(!request.is_finished());
        change.commit().await.unwrap();
        let (status, body) = request.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids(&body), vec![f.vessels[2]]);
        assert_eq!(
            f.get(0, &format!("/api/vessels/{}", f.vessels[0])).await.0,
            StatusCode::NOT_FOUND
        );
        f.finish().await;
    }
}
