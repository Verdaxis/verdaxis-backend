pub use verdaxis_read::{ApiError, AppState, auth, business};

#[allow(dead_code)]
#[path = "../src/subscriptions/fees.rs"]
mod fees;
#[allow(dead_code)]
#[path = "../src/subscriptions/routes.rs"]
mod routes;
#[allow(dead_code)]
#[path = "../src/subscriptions/store.rs"]
mod store;

#[cfg(feature = "contract-tests")]
use chrono::{TimeDelta, Utc};
use fees::FeeConfig;
#[cfg(feature = "contract-tests")]
use fees::SubscriptionTier;
use rust_decimal::Decimal;
use serde_json::json;

#[test]
fn p4_t13_c05() {
    for value in ["NaN", "Infinity", "-1", "100000.01", "1.001", "1e2"] {
        assert!(
            store::SubscriptionUpdate::parse(
                json!({"tier":"enterprise","seller_fee_per_mt_usd":value})
            )
            .is_err()
        );
    }
    assert!(FeeConfig::new(Decimal::new(-1, 0), Decimal::ONE).is_err());
    assert!(FeeConfig::new(Decimal::new(1001, 3), Decimal::ONE).is_err());
}

#[test]
fn p4_t13_c07() {
    let omitted = store::SubscriptionUpdate::parse(json!({"tier":"standard"})).unwrap();
    let cleared =
        store::SubscriptionUpdate::parse(json!({"tier":"standard","seller_fee_per_mt_usd":null}))
            .unwrap();
    assert_ne!(omitted.fee, cleared.fee);
    assert!(
        store::SubscriptionUpdate::parse(json!({"tier":"free","seller_fee_per_mt_usd":true}))
            .is_err()
    );
    let schedule = serde_json::to_value(FeeConfig::default().schedule()).unwrap();
    assert_eq!(schedule["currency"], "USD");
    assert_eq!(schedule["buyer_fee_per_mt_usd"], "0");
    assert_eq!(schedule["seller_fee_per_mt_usd"]["enterprise"], json!(null));
    assert_eq!(schedule["seller_fee_per_mt_usd"]["free"], "2");
}

fn component(pool: sqlx::PgPool, health: sqlx::PgPool) -> axum::Router {
    let config = verdaxis_read::Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: verdaxis_read::compiled_sha().into(),
        migration_revision: "kycg_20261014_staging".into(),
        database_url: "postgres://rust_app:disposable_app_pw@127.0.0.1:54329/verdaxis_rust_test"
            .into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().unwrap(),
        read_admission_slots: 6,
        pool_max: 7,
        python_budget: 52,
        configured_max_connections: 100,
    };
    let state = AppState::new(
        config,
        verdaxis_read::runtime::db::Pools { read: pool, health },
        verdaxis_read::MarketPolicy {
            source_sha: verdaxis_read::compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    routes::router(
        state,
        std::sync::Arc::new(auth::VerificationSettings::new(
            "test-secret-key-for-testing-minimum-32-chars".into(),
            None,
            "test".into(),
            "test".into(),
        )),
        FeeConfig::default(),
    )
}

#[tokio::test]
async fn p4_t13_c09() {
    use tower::ServiceExt;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect_lazy("postgres://rust_app:test@127.0.0.1:54329/verdaxis_rust_test")
        .unwrap();
    let health = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://rust_app:test@127.0.0.1:54329/verdaxis_rust_test")
        .unwrap();
    let app = component(pool, health);
    for (method, path, status) in [
        ("GET", "/api/subscriptions/fees", 200),
        ("GET", "/api/subscriptions/me", 401),
        ("GET", "/api/admin/subscriptions", 401),
        (
            "GET",
            "/api/admin/subscriptions/00000000-0000-0000-0000-000000000001",
            401,
        ),
        (
            "PUT",
            "/api/admin/subscriptions/00000000-0000-0000-0000-000000000001",
            401,
        ),
    ] {
        let request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(axum::body::Body::from("{\"tier\":\"free\"}"))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{method} {path}");
        if status == 401 {
            let body = axum::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(value["detail"]["code"].is_string());
        }
    }
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use uuid::Uuid;
    use verdaxis_read::{auth::VerifiedAccess, business::transaction::run_ordered_write};

    struct Fixture {
        admin: PgConnection,
        pool: sqlx::PgPool,
        schema: String,
        actor: Uuid,
        org: Uuid,
    }
    impl Fixture {
        async fn new() -> Self {
            let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("disposable migrator URL required");
            let app_url =
                std::env::var("RUST_TEST_DATABASE_URL").expect("disposable runtime URL required");
            let a = url::Url::parse(&admin_url).unwrap();
            let b = url::Url::parse(&app_url).unwrap();
            assert_eq!(a.host_str(), Some("127.0.0.1"));
            assert_eq!(a.host_str(), b.host_str());
            assert_eq!(a.port(), b.port());
            assert_eq!(a.path(), b.path());
            assert!(a.path().ends_with("_test"));
            assert_eq!(b.username(), "rust_app");
            assert_ne!(a.username(), b.username());
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            let schema = format!("p4_t13_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}; SET search_path TO {schema};
                CREATE TABLE organizations(id uuid PRIMARY KEY);
                CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations,role text,status text,email_verified boolean DEFAULT true,must_change_password boolean DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz);
                CREATE TABLE organization_join_requests(id uuid PRIMARY KEY);
                CREATE TABLE subscriptions(id uuid PRIMARY KEY,org_id uuid NOT NULL UNIQUE REFERENCES organizations,tier text NOT NULL,started_at timestamptz DEFAULT now(),expires_at timestamptz,is_active boolean NOT NULL,seller_fee_per_mt_usd numeric CHECK(seller_fee_per_mt_usd IS NULL OR (seller_fee_per_mt_usd>=0 AND seller_fee_per_mt_usd<=100000 AND seller_fee_per_mt_usd*100=trunc(seller_fee_per_mt_usd*100))));
                CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100),resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz);
                GRANT USAGE ON SCHEMA {schema} TO rust_app;
                GRANT SELECT,INSERT,UPDATE,DELETE ON subscriptions TO rust_app;
                GRANT SELECT,UPDATE ON organizations,users,organization_join_requests TO rust_app;
                GRANT SELECT,INSERT ON audit_logs TO rust_app;"))).execute(&mut admin).await.unwrap();
            let actor = Uuid::new_v4();
            let org = Uuid::new_v4();
            sqlx::query("INSERT INTO organizations VALUES($1)")
                .bind(org)
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'ADMIN','APPROVED')").bind(actor).bind(org).execute(&mut admin).await.unwrap();
            let path = format!("SET search_path TO {schema}");
            let pool = PgPoolOptions::new()
                .max_connections(6)
                .after_connect(move |c, _| {
                    let path = path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(path)).execute(c).await?;
                        Ok(())
                    })
                })
                .connect(&app_url)
                .await
                .unwrap();
            Self {
                admin,
                pool,
                schema,
                actor,
                org,
            }
        }
        fn access(&self) -> VerifiedAccess {
            let key = "test-secret-key-for-testing-minimum-32-chars";
            let now = Utc::now();
            let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":self.actor,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
            verdaxis_read::auth::verify_access(
                &token,
                &verdaxis_read::auth::VerificationSettings::new(
                    key.into(),
                    None,
                    "test".into(),
                    "test".into(),
                ),
                now,
            )
            .unwrap()
        }
        async fn command(
            &self,
            operation: routes::Operation,
        ) -> Result<routes::CommandOutput, verdaxis_read::ApiError> {
            run_ordered_write(
                self.pool.clone(),
                Arc::new(Semaphore::new(6)),
                self.access(),
                routes::SubscriptionCommand::new(self.actor, operation, Utc::now()),
            )
            .await
        }
        async fn finish(mut self) {
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
    async fn p4_t13_c01() {
        let mut f = Fixture::new().await;
        sqlx::query(
            "INSERT INTO organizations SELECT md5(i::text)::uuid FROM generate_series(1,502) i",
        )
        .execute(&mut f.admin)
        .await
        .unwrap();
        sqlx::query("INSERT INTO subscriptions(id,org_id,tier,is_active) SELECT id,id,'free',true FROM organizations WHERE id<>$1").bind(f.org).execute(&mut f.admin).await.unwrap();
        let output = f
            .command(routes::Operation::List(store::Page::new(500, 0).unwrap()))
            .await
            .unwrap();
        assert_eq!(output.body.as_array().unwrap().len(), 500);
        assert_eq!(output.truncated, Some(true));
        use tower::ServiceExt;
        let health = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&std::env::var("RUST_TEST_DATABASE_URL").unwrap())
            .unwrap();
        let app = component(f.pool.clone(), health.clone());
        let key = "test-secret-key-for-testing-minimum-32-chars";
        let now = Utc::now();
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":f.actor,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
        let request = axum::http::Request::builder()
            .uri("/api/admin/subscriptions?limit=500")
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.headers()["x-result-truncated"], "true");
        let body = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            500
        );
        health.close().await;
        let ids: Vec<_> = output
            .body
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["org_id"].as_str().unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let tail = f
            .command(routes::Operation::List(store::Page::new(500, 500).unwrap()))
            .await
            .unwrap();
        assert_eq!(tail.body.as_array().unwrap().len(), 2);
        assert_eq!(tail.truncated, Some(false));
        assert!(store::Page::new(501, 0).is_err());
        assert!(store::Page::new(100, 10001).is_err());
        sqlx::query("UPDATE users SET role='BUYER' WHERE id=$1")
            .bind(f.actor)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.command(routes::Operation::List(store::Page::default()))
                .await
                .unwrap_err()
                .status
                .as_u16(),
            403
        );
        sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
            .bind(f.actor)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert!(f.command(routes::Operation::Mine).await.is_err());
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t13_c02() {
        let mut f = Fixture::new().await;
        let member = Uuid::new_v4();
        let second_member = Uuid::new_v4();
        for id in [member, second_member] {
            sqlx::query("INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'SUPPLIER','APPROVED')")
                .bind(id).bind(f.org).execute(&mut f.admin).await.unwrap();
        }
        assert_ne!(member, second_member);
        assert_ne!(member, f.actor);
        assert_ne!(second_member, f.actor);
        let key = "test-secret-key-for-testing-minimum-32-chars";
        let now = Utc::now();
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":member,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
        let access = auth::verify_access(
            &token,
            &auth::VerificationSettings::new(key.into(), None, "test".into(), "test".into()),
            now,
        )
        .unwrap();
        let members: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE organization_id=$1 AND role<>'ADMIN' AND status='APPROVED'")
            .bind(f.org).fetch_one(&mut f.admin).await.unwrap();
        assert_eq!(members, 2, "two independent nonadmin users required");
        let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":second_member,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
        let second_access = auth::verify_access(
            &token,
            &auth::VerificationSettings::new(key.into(), None, "test".into(), "test".into()),
            now,
        )
        .unwrap();
        // Hold the organization row outside the competing commands. All three
        // identities must reach PostgreSQL lock contention before release.
        let mut holder = f.pool.acquire().await.unwrap();
        let mut held = holder.begin().await.unwrap();
        let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(f.org)
            .fetch_one(&mut *held)
            .await
            .unwrap();
        let start = Arc::new(tokio::sync::Barrier::new(4));
        let mut workers = Vec::new();
        for (actor, access) in [
            (member, access),
            (second_member, second_access),
            (f.actor, f.access()),
        ] {
            let pool = f.pool.clone();
            let start = start.clone();
            let org = f.org;
            let admin = f.actor;
            workers.push(tokio::spawn(async move {
                start.wait().await;
                let first=run_ordered_write(pool.clone(),Arc::new(Semaphore::new(1)),access,routes::SubscriptionCommand::new(actor,routes::Operation::Mine,now)).await?;
                if actor==admin {
                    let token=jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":actor,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
                    let access=auth::verify_access(&token,&auth::VerificationSettings::new(key.into(),None,"test".into(),"test".into()),now).unwrap();
                    run_ordered_write(pool,Arc::new(Semaphore::new(1)),access,routes::SubscriptionCommand::new(actor,routes::Operation::Update(org,store::SubscriptionUpdate::parse(json!({"tier":"enterprise","seller_fee_per_mt_usd":"0.75"})).unwrap()),now)).await?;
                }
                Ok::<_,ApiError>(first)
            }));
        }
        start.wait().await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            // pg_locks is independent of the holder's transaction snapshot.
            // Each contender holds a granted relation RowShareLock while its
            // SELECT FOR UPDATE waits on the held organization row.
            let waiting:i64=sqlx::query_scalar("SELECT count(DISTINCT l.pid) FROM pg_locks l WHERE l.relation=($1::text)::regclass AND l.mode='RowShareLock' AND l.granted AND cardinality(pg_blocking_pids(l.pid))>0 AND l.pid<>$2")
                .bind(format!("{}.organizations",f.schema)).bind(holder_pid).fetch_one(&mut f.admin).await.unwrap();
            if waiting == 3 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "three subscription actors did not reach organization lock contention"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscriptions")
                .fetch_one(&mut f.admin)
                .await
                .unwrap(),
            0
        );
        held.commit().await.unwrap();
        drop(holder);
        for worker in workers {
            worker.await.unwrap().unwrap();
        }
        let audits: Vec<(Uuid, String)> = sqlx::query_as("SELECT user_id,action FROM audit_logs")
            .fetch_all(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(audits, vec![(f.actor, "subscription.updated".into())]);
        let mut conn = f.pool.acquire().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscriptions")
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            1
        );
        let row = store::find(&mut conn, f.org).await.unwrap().unwrap();
        assert_eq!(row.tier, SubscriptionTier::Enterprise);
        assert_eq!(row.seller_fee_per_mt_usd, Some(Decimal::new(75, 2)));
        drop(conn);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t13_c03() {
        let f = Fixture::new().await;
        let mut holder = f.pool.acquire().await.unwrap();
        let mut tx = holder.begin().await.unwrap();
        sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
            .bind(f.org)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let pool = f.pool.clone();
        let access = f.access();
        let actor = f.actor;
        let waiter = tokio::spawn(async move {
            run_ordered_write(
                pool,
                Arc::new(Semaphore::new(1)),
                access,
                routes::SubscriptionCommand::new(actor, routes::Operation::Mine, Utc::now()),
            )
            .await
        });
        store::update(
            &mut tx,
            f.org,
            &store::SubscriptionUpdate::parse(
                json!({"tier":"enterprise","seller_fee_per_mt_usd":"1.25"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND usename='rust_app' AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0)")
                .fetch_one(&f.pool).await.unwrap();
            if blocked {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "waiter did not reach lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tx.commit().await.unwrap();
        let row = waiter.await.unwrap().unwrap();
        assert_eq!(row.body["tier"], "enterprise");
        drop(holder);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t13_c04() {
        let mut f = Fixture::new().await;
        let absent = Uuid::new_v4();
        let update = store::SubscriptionUpdate::parse(json!({"tier":"free"})).unwrap();
        assert_eq!(
            f.command(routes::Operation::Update(absent, update))
                .await
                .unwrap_err()
                .status
                .as_u16(),
            404
        );
        sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
            .bind(f.actor)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.command(routes::Operation::Mine)
                .await
                .unwrap_err()
                .status
                .as_u16(),
            403
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(count, 0);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t13_c06() {
        let f = Fixture::new().await;
        let now = Utc::now();
        let mut conn = f.pool.acquire().await.unwrap();
        let mut tx = conn.begin_with("BEGIN READ ONLY").await.unwrap();
        let config = FeeConfig::default();
        assert_eq!(
            fees::resolve_seller_fee(&mut tx, f.org, now, &config)
                .await
                .unwrap()
                .seller_fee_per_mt_usd,
            Decimal::from(2)
        );
        tx.rollback().await.unwrap();
        let mut tx = conn.begin().await.unwrap();
        assert_eq!(
            fees::resolve_seller_fee(&mut tx, f.org, now, &config)
                .await
                .unwrap()
                .plan,
            SubscriptionTier::Free
        );
        store::bootstrap(&mut tx, f.org).await.unwrap();
        sqlx::query("UPDATE subscriptions SET tier='standard',expires_at=$1")
            .bind(now)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            fees::resolve_seller_fee(&mut tx, f.org, now, &config)
                .await
                .unwrap()
                .plan,
            SubscriptionTier::Free
        );
        let before =
            fees::resolve_seller_fee(&mut tx, f.org, now - TimeDelta::microseconds(1), &config)
                .await
                .unwrap();
        assert_eq!(before.plan, SubscriptionTier::Standard);
        assert_eq!(before.seller_fee_per_mt_usd, Decimal::new(150, 2));
        sqlx::query("UPDATE subscriptions SET is_active=false,expires_at=NULL")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            fees::resolve_seller_fee(&mut tx, f.org, now, &config)
                .await
                .unwrap()
                .plan,
            SubscriptionTier::Free
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM subscriptions")
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
            0
        );
        drop(conn);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t13_c08() {
        let mut f = Fixture::new().await;
        let set = || {
            store::SubscriptionUpdate::parse(
                json!({"tier":"enterprise","seller_fee_per_mt_usd":"0.99"}),
            )
            .unwrap()
        };
        f.command(routes::Operation::Update(f.org, set()))
            .await
            .unwrap();
        let row = f
            .command(routes::Operation::Update(
                f.org,
                store::SubscriptionUpdate::parse(json!({"tier":"enterprise"})).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(row.body["seller_fee_per_mt_usd"], "0.99");
        assert_eq!(
            f.command(routes::Operation::Update(
                f.org,
                store::SubscriptionUpdate::parse(
                    json!({"tier":"enterprise","seller_fee_per_mt_usd":null})
                )
                .unwrap()
            ))
            .await
            .unwrap_err()
            .status
            .as_u16(),
            422
        );
        sqlx::raw_sql("ALTER TABLE audit_logs ADD CONSTRAINT audit_gate CHECK(false) NOT VALID")
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert!(
            f.command(routes::Operation::Update(
                f.org,
                store::SubscriptionUpdate::parse(
                    json!({"tier":"standard","seller_fee_per_mt_usd":null})
                )
                .unwrap()
            ))
            .await
            .is_err()
        );
        let stored = sqlx::query("SELECT tier,seller_fee_per_mt_usd FROM subscriptions")
            .fetch_one(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(stored.get::<String, _>("tier"), "enterprise");
        assert_eq!(
            stored.get::<Decimal, _>("seller_fee_per_mt_usd"),
            Decimal::new(99, 2)
        );
        sqlx::raw_sql("ALTER TABLE audit_logs DROP CONSTRAINT audit_gate")
            .execute(&mut f.admin)
            .await
            .unwrap();
        f.command(routes::Operation::Update(
            f.org,
            store::SubscriptionUpdate::parse(json!({"tier":"standard"})).unwrap(),
        ))
        .await
        .unwrap();
        let preserved =
            sqlx::query("SELECT tier,seller_fee_per_mt_usd FROM subscriptions WHERE org_id=$1")
                .bind(f.org)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(preserved.get::<String, _>("tier"), "standard");
        assert_eq!(
            preserved.get::<Decimal, _>("seller_fee_per_mt_usd"),
            Decimal::new(99, 2)
        );
        let cleared = f
            .command(routes::Operation::Update(
                f.org,
                store::SubscriptionUpdate::parse(
                    json!({"tier":"standard","seller_fee_per_mt_usd":null}),
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        assert!(cleared.body["seller_fee_per_mt_usd"].is_null());
        let persisted =
            sqlx::query("SELECT tier,seller_fee_per_mt_usd FROM subscriptions WHERE org_id=$1")
                .bind(f.org)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert_eq!(persisted.get::<String, _>("tier"), "standard");
        assert_eq!(
            persisted.get::<Option<Decimal>, _>("seller_fee_per_mt_usd"),
            None
        );
        let changes: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT changes FROM audit_logs WHERE action='subscription.updated' AND resource_id=$1",
        )
        .bind(f.org.to_string())
        .fetch_all(&mut f.admin)
        .await
        .unwrap();
        assert_eq!(changes.len(), 4);
        assert!(
            changes
                .iter()
                .any(|change| change["tier"]["to"] == "standard"
                    && change.get("seller_fee_per_mt_usd").is_none())
        );
        assert_eq!(changes.iter().filter(|change|change["seller_fee_per_mt_usd"]==json!({"from":"0.99","to":null})).count(),1);
        f.finish().await;
    }
}
