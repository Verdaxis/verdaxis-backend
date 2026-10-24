//! Integration round I1: cross-lane composition on the shared branch.
//!
//! These cases exist only to prove that the released components cooperate
//! through their real implementations. They do not replace the lane suites,
//! which keep their own fixtures, assertion sets and test attributes.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tokio::sync::Semaphore;
use uuid::Uuid;
use verdaxis_read::{
    Config, MarketPolicy, auth,
    business::{organizations, transaction},
    kyc::{
        grace::Grace,
        policy::{KycParty, KycPolicy, KycReason, KycStatus, eligibility},
    },
};

fn now() -> DateTime<Utc> {
    "2026-10-24T03:00:00Z".parse().unwrap()
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    org: Uuid,
    member: Uuid,
    order: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL");
        let runtime =
            std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
        let parsed = url::Url::parse(&url).unwrap();
        let app = url::Url::parse(&runtime).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), app.port());
        assert_eq!(parsed.path(), app.path());
        assert_eq!(app.host_str(), Some("127.0.0.1"));
        assert_eq!(app.username(), "rust_app");
        assert_ne!(parsed.username(), app.username());
        assert!(parsed.path().ends_with("_test"));
        assert!(!matches!(parsed.port(), Some(5432 | 8000 | 8001)));
        let mut admin = PgConnection::connect(&url).await.unwrap();
        let schema = format!("i1_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY,type text NOT NULL DEFAULT 'FUEL_BUYER',verification_status text NOT NULL DEFAULT 'PENDING_REVIEW',provenance text NOT NULL DEFAULT 'UNKNOWN',refuse_invalidation boolean NOT NULL DEFAULT false);
             CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations(id),role text,status text NOT NULL DEFAULT 'APPROVED',email_verified boolean NOT NULL DEFAULT true,must_change_password boolean NOT NULL DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,kyc_status text,kyc_organization_id uuid,kyc_rejection_reason text,kyc_external_evidence_reference text,kyc_review_note text,kyc_reviewed_by uuid,kyc_reviewed_at timestamptz);
             CREATE TABLE organization_join_requests(id uuid PRIMARY KEY,user_id uuid NOT NULL REFERENCES users(id),organization_id uuid NOT NULL REFERENCES organizations(id),status text NOT NULL,reviewed_by uuid,reviewed_at timestamptz,review_note text,created_at timestamptz NOT NULL,UNIQUE(user_id,organization_id));
             CREATE TABLE kyc_execution_grace(user_id uuid,organization_id uuid,expires_at timestamptz);
             CREATE TABLE products(id uuid PRIMARY KEY,name text,fuel_type text,fuel_grade text,is_active boolean DEFAULT true);
             CREATE TABLE delivery_points(id uuid PRIMARY KEY,name text,region text,is_active boolean DEFAULT true);
             CREATE TABLE inventory_items(id uuid PRIMARY KEY,current_stock_mt numeric NOT NULL CHECK(current_stock_mt>=0),reserved_stock_mt numeric NOT NULL CHECK(reserved_stock_mt>=0));
             CREATE TABLE orderbook_orders(id uuid PRIMARY KEY,organization_id uuid,side text,product_id uuid,delivery_point_id uuid,availability_window text,status text DEFAULT 'OPEN',quantity_mt numeric DEFAULT 100 CHECK(quantity_mt>=0),remaining_quantity_mt numeric DEFAULT 60 CHECK(remaining_quantity_mt>=0 AND remaining_quantity_mt<=quantity_mt),version int DEFAULT 1 CHECK(version>=1),inventory_item_id uuid,price_per_mt_usd numeric DEFAULT 200,expires_at timestamptz,created_at timestamptz DEFAULT '2026-10-01Z',updated_at timestamptz,provenance text DEFAULT 'REAL',owner_user_id uuid,created_by_actor_user_id uuid,creation_method text DEFAULT 'DIRECT',off_spec boolean DEFAULT false,msds_available boolean DEFAULT true,certification_declared boolean DEFAULT true,certification_scheme text DEFAULT 'test',specification_standard text DEFAULT 'test',carbon_intensity_gco2_mj numeric DEFAULT 10,carbon_intensity_method text,energy_density_mj_kg numeric,feedstock text DEFAULT 'test',origin text DEFAULT 'test',fame_terms jsonb,lng_terms jsonb);
             CREATE INDEX ON orderbook_orders(organization_id,status,id);
             CREATE TABLE trades(id uuid PRIMARY KEY,buyer_id uuid,seller_id uuid,bid_order_id uuid,ask_order_id uuid,product_id uuid,delivery_point_id uuid,availability_window text,status text,quantity_mt numeric,buyer_user_id uuid,seller_user_id uuid,fee_snapshot jsonb,terms_snapshot jsonb);
             CREATE INDEX ON trades(buyer_id,status,id); CREATE INDEX ON trades(seller_id,status,id);
             CREATE TABLE live_slice_benchmarks(id uuid PRIMARY KEY,side varchar(3),market_product varchar(64),delivery_point_id uuid NOT NULL,availability_window varchar(16),benchmark_price_per_mt_usd numeric(10,2),total_remaining_quantity_mt numeric(12,2),order_count int,source varchar(32),created_at timestamptz,updated_at timestamptz,CONSTRAINT uq_live_slice_benchmarks_slice_key UNIQUE(side,market_product,delivery_point_id,availability_window));
             CREATE TABLE watchlist_targets(id uuid PRIMARY KEY,watchlist_id uuid,target_type text,order_id uuid,market_product_code text,delivery_point_id uuid,availability_window_code text,snapshot_price_per_mt_usd double precision,snapshot_quantity_mt double precision,snapshot_remaining_quantity_mt double precision,snapshot_status text,snapshot_side text,snapshot_market_product text,snapshot_delivery_point_name text,snapshot_availability_window text,snapshot_counterparty_label text);
             CREATE TABLE watchlist_events(id uuid PRIMARY KEY,watchlist_id uuid,watchlist_target_id uuid,event_type text,event_payload jsonb,is_read boolean DEFAULT false,created_at timestamptz);
             CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action text,resource_type text,resource_id text,changes jsonb,ip_address text,request_id text,timestamp timestamptz);
             CREATE TABLE market_event_outbox(id uuid PRIMARY KEY,event_type text,aggregate_type text,aggregate_id text,participant_org_ids jsonb,payload jsonb,created_at timestamptz,dispatched_at timestamptz,delivery_attempts int DEFAULT 0,stream_seq bigint);",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        let actor = Uuid::new_v4();
        let org = Uuid::new_v4();
        let member = Uuid::new_v4();
        let order = Uuid::new_v4();
        let product = Uuid::new_v4();
        let point = Uuid::new_v4();
        let inventory = Uuid::new_v4();
        sqlx::query("INSERT INTO products(id,name,fuel_type,fuel_grade) VALUES($1,'fixture-product','Methanol','fixture')")
            .bind(product).execute(&mut admin).await.unwrap();
        sqlx::query("INSERT INTO delivery_points(id,name,region) VALUES($1,'fixture-point','fixture-region')")
            .bind(point).execute(&mut admin).await.unwrap();
        sqlx::query("INSERT INTO organizations(id) VALUES($1)")
            .bind(org)
            .execute(&mut admin)
            .await
            .unwrap();
        for (id, role, organization, status, kyc_status, kyc_org) in [
            (
                actor,
                "ADMIN",
                None::<Uuid>,
                "APPROVED",
                "PENDING",
                None::<Uuid>,
            ),
            (
                member,
                "BUYER",
                Some(org),
                "APPROVED",
                "APPROVED",
                Some(org),
            ),
        ] {
            sqlx::query("INSERT INTO users(id,organization_id,role,status,kyc_status,kyc_organization_id) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(id).bind(organization).bind(role).bind(status).bind(kyc_status).bind(kyc_org)
                .execute(&mut admin).await.unwrap();
        }
        sqlx::query("INSERT INTO inventory_items VALUES($1,20,80)")
            .bind(inventory)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO orderbook_orders(id,organization_id,side,product_id,delivery_point_id,availability_window,remaining_quantity_mt,inventory_item_id) VALUES($1,$2,'ASK',$3,$4,'SPOT',60,$5)")
            .bind(order).bind(org).bind(product).bind(point).bind(inventory)
            .execute(&mut admin).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
             GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&runtime)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            schema,
            actor,
            org,
            member,
            order,
        }
    }

    fn access(&self) -> auth::VerifiedAccess {
        let key = "test-secret-key-for-testing-minimum-32-chars";
        let claims = json!({"sub":self.actor,"iat":Utc::now().timestamp(),"exp":Utc::now().timestamp()+3600,"type":"access","iss":"test","aud":"test"});
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
        )
        .unwrap();
        auth::verify_access(
            &token,
            &auth::VerificationSettings::new(key.into(), None, "test".into(), "test".into()),
            Utc::now(),
        )
        .unwrap()
    }

    fn policy() -> Arc<MarketPolicy> {
        Arc::new(MarketPolicy {
            source_sha: "integration-i1-policy".into(),
            products: vec![],
            points: vec![],
        })
    }

    fn config() -> Arc<Config> {
        Arc::new(Config {
            kyc_execution_gate_enabled: true,
            environment: "test".into(),
            release_sha: verdaxis_read::compiled_sha().into(),
            migration_revision: "kycg_20261014_staging".into(),
            database_url: "postgres://rust_app:fixture@127.0.0.1:12345/verdaxis_rust_test".into(),
            db_session_time_zone: "UTC".into(),
            prepared_budget: None,
            bind: "127.0.0.1:0".parse().unwrap(),
            read_admission_slots: 4,
            pool_max: 5,
            python_budget: 52,
            configured_max_connections: 100,
        })
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

/// Task 8 organization rejection through the real Task 7 invalidator, checked
/// by the real Task 9 policy. No lane double participates in this case.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn i1_t8_rejection_with_real_t7_and_t9() {
    let fixture = Fixture::new().await;
    // The released Task 9 policy admits the bound approved member before the
    // rejection clears the organization link.
    let before = KycParty {
        user_id: fixture.member,
        organization_id: Some(fixture.org),
        kyc_organization_id: Some(fixture.org),
        kyc_status: KycStatus::Approved,
    };
    assert!(eligibility(&KycPolicy::default(), &before, None, now()).eligible);
    let command = organizations::ReviewOrganization {
        organization_id: fixture.org,
        approve: false,
        reason: None,
        now: now(),
        policy: Fixture::policy(),
        config: Fixture::config(),
    };
    let decision = transaction::run_ordered_write(
        fixture.pool.clone(),
        Arc::new(Semaphore::new(4)),
        fixture.access(),
        command,
    )
    .await
    .unwrap();
    assert_eq!(decision.verification_status, "REJECTED");
    let mut conn = fixture.pool.acquire().await.unwrap();
    // Real Task 7 effect: the open remainder is withdrawn, not deleted.
    let (status, version): (String, i32) =
        sqlx::query_as("SELECT status,version FROM orderbook_orders WHERE id=$1")
            .bind(fixture.order)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(status, "CANCELLED");
    assert_eq!(version, 2, "the real invalidator bumps the version once");
    // Real Task 7 effect: one conservative public invalidation event.
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM market_event_outbox WHERE event_type='market_access_invalidated'",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(events, 1);
    // Task 8 effect: exactly one rejection audit, and membership detached.
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action='admin.organization_rejected'",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    let detached: Option<Uuid> =
        sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
            .bind(fixture.member)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(detached, None);
    drop(conn);
    // The real policy refuses the detached member afterwards.
    let after = KycParty {
        user_id: fixture.member,
        organization_id: None,
        kyc_organization_id: Some(fixture.org),
        kyc_status: KycStatus::Approved,
    };
    let result = eligibility(&KycPolicy::default(), &after, None, now());
    assert!(!result.eligible);
    assert_eq!(result.reason, KycReason::Required);
    fixture.finish().await;
}

/// Task 10 evidence declarations are judged by the real Task 9 policy in both
/// switch states, including rejection precedence over grace.
#[test]
fn i1_t10_declarations_with_real_t9_policy() {
    let enabled = KycPolicy {
        execution_gate_enabled: true,
    };
    let advisory = KycPolicy {
        execution_gate_enabled: false,
    };
    let org = Uuid::from_u128(10);
    let other = Uuid::from_u128(11);
    let grace = Grace {
        user_id: Uuid::from_u128(1),
        organization_id: org,
        expires_at: now() + chrono::Duration::hours(1),
    };
    for (status, bound, strict_expect, advisory_expect) in [
        (
            KycStatus::Pending,
            Some(org),
            (true, KycReason::Grace),
            (true, KycReason::NotEnforced),
        ),
        (
            KycStatus::Submitted,
            Some(org),
            (true, KycReason::Grace),
            (true, KycReason::NotEnforced),
        ),
        (
            KycStatus::Approved,
            Some(org),
            (true, KycReason::Approved),
            (true, KycReason::NotEnforced),
        ),
        (
            KycStatus::Approved,
            Some(other),
            (true, KycReason::Grace),
            (false, KycReason::Required),
        ),
        (
            KycStatus::Rejected,
            Some(org),
            (false, KycReason::Rejected),
            (false, KycReason::Rejected),
        ),
    ] {
        let party = KycParty {
            user_id: Uuid::from_u128(1),
            organization_id: Some(org),
            kyc_organization_id: bound,
            kyc_status: status,
        };
        let strict = eligibility(&enabled, &party, Some(&grace), now());
        assert_eq!(
            (strict.eligible, strict.reason),
            strict_expect,
            "strict {status:?} bound={bound:?}"
        );
        let loose = eligibility(&advisory, &party, Some(&grace), now());
        assert_eq!(
            (loose.eligible, loose.reason),
            advisory_expect,
            "advisory {status:?} bound={bound:?}"
        );
    }
    // Grace is current-organization bound and must still be unexpired.
    let pending = KycParty {
        user_id: Uuid::from_u128(1),
        organization_id: Some(org),
        kyc_organization_id: Some(org),
        kyc_status: KycStatus::Pending,
    };
    assert_eq!(
        eligibility(&enabled, &pending, Some(&grace), now()).reason,
        KycReason::Grace
    );
    let expired = Grace {
        expires_at: now(),
        ..grace
    };
    assert_eq!(
        eligibility(&enabled, &pending, Some(&expired), now()).reason,
        KycReason::Required
    );
    let foreign = Grace {
        organization_id: other,
        ..grace
    };
    assert_eq!(
        eligibility(&enabled, &pending, Some(&foreign), now()).reason,
        KycReason::Required
    );
}
