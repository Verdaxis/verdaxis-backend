#[path = "../src/kyc/grace.rs"]
mod grace;
#[path = "../src/kyc/policy.rs"]
mod policy;

use chrono::{DateTime, Duration, Utc};
use grace::{Grace, grace_applies, notice_is_due};
use policy::{KycParty, KycPolicy, KycReason, KycStatus, eligibility};
use policy::{KycSqlParty, bind_eligibility_sql};
use uuid::Uuid;

fn now() -> DateTime<Utc> {
    "2026-10-24T00:00:00Z".parse().unwrap()
}

fn party(status: KycStatus) -> KycParty {
    KycParty {
        user_id: Uuid::from_u128(1),
        organization_id: Some(Uuid::from_u128(2)),
        kyc_organization_id: Some(Uuid::from_u128(2)),
        kyc_status: status,
    }
}

fn policy(enabled: bool) -> KycPolicy {
    KycPolicy {
        execution_gate_enabled: enabled,
    }
}

fn grace() -> Grace {
    Grace {
        user_id: Uuid::from_u128(1),
        organization_id: Uuid::from_u128(2),
        expires_at: now() + Duration::seconds(1),
    }
}

#[test]
fn p4_t9_c03() {
    for enabled in [false, true] {
        let result = eligibility(
            &policy(enabled),
            &party(KycStatus::Rejected),
            Some(&grace()),
            now(),
        );
        assert!(!result.eligible);
        assert_eq!(result.reason, KycReason::Rejected);
    }
}

#[test]
fn p4_t9_c04() {
    let mut grace = grace();
    grace.expires_at = now();
    assert!(!grace_applies(&party(KycStatus::Pending), &grace, now()));
    let result = eligibility(
        &policy(true),
        &party(KycStatus::Pending),
        Some(&grace),
        now(),
    );
    assert!(!result.eligible);
    assert_eq!(result.reason, KycReason::Required);
    assert_eq!(result.grace_expires_at, Some(now()));
}

#[test]
fn p4_t9_c05() {
    let mut party = party(KycStatus::Pending);
    party.organization_id = None;
    party.kyc_organization_id = None;
    let result = eligibility(&policy(false), &party, Some(&grace()), now());
    assert!(result.eligible);
    assert_eq!(result.reason, KycReason::NotEnforced);
    assert_eq!(result.grace_expires_at, None);
    party.kyc_organization_id = Some(Uuid::from_u128(3));
    assert!(!eligibility(&policy(false), &party, None, now()).eligible);
    assert_eq!(
        serde_json::to_value(result).unwrap()["reason"],
        "KYC_NOT_ENFORCED"
    );
}

#[test]
fn p4_t9_c08() {
    assert!(KycPolicy::default().execution_gate_enabled);
    assert_eq!(
        serde_json::to_value(KycStatus::Submitted).unwrap(),
        "SUBMITTED"
    );
    for sql_party in [KycSqlParty::USER, KycSqlParty::MEMBER] {
        let mut builder = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT ");
        bind_eligibility_sql(&mut builder, &policy(true), sql_party, now());
        let sql = builder.sql();
        let sql = sql.as_str();
        assert!(sql.contains("$1::boolean"));
        assert!(sql.contains("$2::timestamptz"));
        assert!(!sql.contains("2026"));
    }
}

#[test]
fn p4_t9_c10() {
    for value in [
        serde_json::json!("UNKNOWN"),
        serde_json::json!(""),
        serde_json::json!(null),
    ] {
        assert!(serde_json::from_value::<KycStatus>(value).is_err());
    }
    assert!(
        serde_json::from_value::<KycParty>(serde_json::json!({
            "user_id": Uuid::from_u128(1), "organization_id": null,
            "kyc_organization_id": null
        }))
        .is_err()
    );
    assert_eq!(
        serde_json::from_str::<KycStatus>("\"SUBMITTED\"").unwrap(),
        KycStatus::Submitted
    );
}

#[test]
fn p4_t9_c07() {
    let party = party(KycStatus::Pending);
    assert!(grace_applies(&party, &grace(), now()));
    let mut other = grace();
    other.user_id = Uuid::from_u128(4);
    assert!(!grace_applies(&party, &other, now()));
    other = grace();
    other.organization_id = Uuid::from_u128(4);
    assert!(!grace_applies(&party, &other, now()));
    assert!(notice_is_due(&policy(true), false));
    assert!(!notice_is_due(&policy(false), false));
    assert!(!notice_is_due(&policy(true), true));
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection, Postgres, QueryBuilder};

    struct Fixture {
        admin: PgConnection,
        runtime: PgConnection,
        schema: String,
    }

    impl Fixture {
        async fn new() -> Self {
            let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL required");
            let runtime_url = std::env::var("RUST_TEST_DATABASE_URL")
                .expect("explicit disposable runtime URL required");
            let admin_target = url::Url::parse(&admin_url).unwrap();
            let runtime_target = url::Url::parse(&runtime_url).unwrap();
            for target in [&admin_target, &runtime_target] {
                assert_eq!(target.host_str(), Some("127.0.0.1"));
                assert!(target.path().ends_with("_test"));
                assert!(!matches!(target.port(), Some(5432 | 5433)));
            }
            assert_eq!(admin_target.port(), runtime_target.port());
            assert_eq!(admin_target.path(), runtime_target.path());
            assert_eq!(runtime_target.username(), "rust_app");
            assert_ne!(admin_target.username(), runtime_target.username());
            let mut admin = PgConnection::connect(&admin_url).await.unwrap();
            let mut runtime = PgConnection::connect(&runtime_url).await.unwrap();
            let schema = format!("p4_t9_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}; \
                 CREATE TABLE organizations(id uuid PRIMARY KEY,verification_status text,provenance text); \
                 CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid,kyc_organization_id uuid,kyc_status text NOT NULL,role text,status text,email_verified boolean,must_change_password boolean); \
                 CREATE TABLE kyc_execution_grace(user_id uuid,organization_id uuid,expires_at timestamptz,PRIMARY KEY(user_id,organization_id)); \
                 GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app"
            ))).execute(&mut admin).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("SET search_path TO {schema}")))
                .execute(&mut runtime)
                .await
                .unwrap();
            Self {
                admin,
                runtime,
                schema,
            }
        }

        async fn store(&mut self, party: &KycParty, grace: Option<&Grace>) {
            sqlx::raw_sql("TRUNCATE users,kyc_execution_grace")
                .execute(&mut self.admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO users VALUES($1,$2,$3,$4,'BUYER','APPROVED',true,false)")
                .bind(party.user_id)
                .bind(party.organization_id)
                .bind(party.kyc_organization_id)
                .bind(
                    serde_json::to_value(party.kyc_status)
                        .unwrap()
                        .as_str()
                        .unwrap(),
                )
                .execute(&mut self.admin)
                .await
                .unwrap();
            if let Some(grace) = grace {
                sqlx::query("INSERT INTO kyc_execution_grace VALUES($1,$2,$3)")
                    .bind(grace.user_id)
                    .bind(grace.organization_id)
                    .bind(grace.expires_at)
                    .execute(&mut self.admin)
                    .await
                    .unwrap();
            }
        }

        async fn finish(mut self) {
            self.runtime.close().await.unwrap();
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
    async fn p4_t9_c02() {
        let mut fixture = Fixture::new().await;
        for enabled in [false, true] {
            for status in [
                KycStatus::Pending,
                KycStatus::Submitted,
                KycStatus::Approved,
                KycStatus::Rejected,
            ] {
                for organization in [None, Some(Uuid::from_u128(2))] {
                    for binding in [None, Some(Uuid::from_u128(2)), Some(Uuid::from_u128(3))] {
                        for grace_case in 0..7 {
                            let mut party = party(status);
                            party.organization_id = organization;
                            party.kyc_organization_id = binding;
                            let mut grace = grace();
                            match grace_case {
                                2 => grace.expires_at = now(),
                                3 => grace.expires_at = now() - Duration::seconds(1),
                                4 => grace.organization_id = Uuid::from_u128(3),
                                5 => grace.user_id = Uuid::from_u128(3),
                                6 => {
                                    grace.organization_id = Uuid::from_u128(3);
                                    grace.user_id = Uuid::from_u128(3);
                                }
                                _ => {}
                            }
                            let grace = (grace_case != 0).then_some(grace);
                            let rejected = status == KycStatus::Rejected;
                            let approved = status == KycStatus::Approved
                                && organization.is_some()
                                && binding == organization;
                            let matching_grace = grace.as_ref().filter(|g| {
                                Some(g.organization_id) == organization
                                    && g.user_id == party.user_id
                            });
                            let active_grace = matching_grace.is_some_and(|g| now() < g.expires_at);
                            let expected = !rejected
                                && if enabled {
                                    approved || active_grace
                                } else {
                                    binding.is_none() || binding == organization
                                };
                            let reason = if rejected {
                                KycReason::Rejected
                            } else if !enabled && expected {
                                KycReason::NotEnforced
                            } else if enabled && approved {
                                KycReason::Approved
                            } else if enabled && active_grace {
                                KycReason::Grace
                            } else {
                                KycReason::Required
                            };
                            let scalar =
                                eligibility(&policy(enabled), &party, grace.as_ref(), now());
                            assert_eq!(scalar.eligible, expected);
                            assert_eq!(scalar.reason, reason);
                            assert_eq!(
                                scalar.grace_expires_at,
                                if enabled && !approved {
                                    matching_grace.map(|g| g.expires_at)
                                } else {
                                    None
                                }
                            );
                            fixture.store(&party, grace.as_ref()).await;
                            for sql_party in [KycSqlParty::USER, KycSqlParty::MEMBER] {
                                let alias = if sql_party == KycSqlParty::USER {
                                    "u"
                                } else {
                                    "m"
                                };
                                let mut builder = QueryBuilder::<Postgres>::new("SELECT COALESCE(");
                                bind_eligibility_sql(
                                    &mut builder,
                                    &policy(enabled),
                                    sql_party,
                                    now(),
                                );
                                builder.push(format!(",false) FROM users {alias}"));
                                let sql: bool = builder
                                    .build_query_scalar()
                                    .fetch_one(&mut fixture.runtime)
                                    .await
                                    .unwrap();
                                assert_eq!(sql, expected, "{party:?} {grace:?} enabled={enabled}");
                            }
                        }
                    }
                }
            }
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn p4_t9_c09() {
        let mut fixture = Fixture::new().await;
        for enabled in [false, true] {
            for with_grace in [false, true] {
                for status in ["UNKNOWN", "", "pending", "REVIEW_REQUIRED"] {
                    fixture
                        .store(&party(KycStatus::Pending), with_grace.then(grace).as_ref())
                        .await;
                    sqlx::query("UPDATE users SET kyc_status=$1,kyc_organization_id=NULL")
                        .bind(status)
                        .execute(&mut fixture.admin)
                        .await
                        .unwrap();
                    for sql_party in [KycSqlParty::USER, KycSqlParty::MEMBER] {
                        let alias = if sql_party == KycSqlParty::USER {
                            "u"
                        } else {
                            "m"
                        };
                        let mut builder = QueryBuilder::<Postgres>::new("SELECT COALESCE(");
                        bind_eligibility_sql(&mut builder, &policy(enabled), sql_party, now());
                        builder.push(format!(",false) FROM users {alias}"));
                        let admitted: bool = builder
                            .build_query_scalar()
                            .fetch_one(&mut fixture.runtime)
                            .await
                            .unwrap();
                        assert!(
                            !admitted,
                            "status={status:?} enabled={enabled} grace={with_grace}"
                        );
                    }
                }
            }
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn p4_t9_c11() {
        let mut fixture = Fixture::new().await;
        sqlx::query("INSERT INTO organizations VALUES($1,'APPROVED','REAL')")
            .bind(Uuid::from_u128(2))
            .execute(&mut fixture.admin)
            .await
            .unwrap();
        for enabled in [false, true] {
            for status in [KycStatus::Pending, KycStatus::Approved, KycStatus::Rejected] {
                let party = party(status);
                fixture.store(&party, None).await;
                let sql = verdaxis_read::market::visibility::eligibility_sets_sql("$1");
                let statement = format!(
                    "WITH policy_products AS (SELECT 1) {sql} SELECT user_id FROM eligible_owners"
                );
                let ids: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
                    .bind(enabled)
                    .bind(now())
                    .fetch_all(&mut fixture.runtime)
                    .await
                    .unwrap();
                assert_eq!(
                    ids.contains(&party.user_id),
                    eligibility(&policy(enabled), &party, None, now()).eligible
                );
            }
        }
        fixture.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires migrated synthetic market fixture and integration overlay"]
    async fn p4_t9_c01() {
        use axum::{
            body::{Body, to_bytes},
            http::Request,
        };
        use std::collections::BTreeSet;
        use tower::ServiceExt;
        use verdaxis_read::{AppState, Config, MarketPolicy};

        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("RUST_MARKET_TEST_EXPECTED").unwrap()).unwrap(),
        )
        .unwrap();
        let url = std::env::var("RUST_MARKET_TEST_DATABASE_URL").unwrap();
        let target = url::Url::parse(&url).unwrap();
        assert_eq!(target.host_str(), Some("127.0.0.1"));
        assert!(target.path().ends_with("_test"));
        let admin_url = std::env::var("RUST_MARKET_TEST_MIGRATOR_URL").unwrap();
        let admin_target = url::Url::parse(&admin_url).unwrap();
        assert_eq!(admin_target.host_str(), target.host_str());
        assert_eq!(admin_target.port(), target.port());
        assert_eq!(admin_target.path(), target.path());
        let mut admin = PgConnection::connect(&admin_url).await.unwrap();
        let clock: DateTime<Utc> = expected["clock"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            std::env::var("RUST_CONTRACT_CLOCK")
                .unwrap()
                .parse::<DateTime<Utc>>()
                .unwrap(),
            clock
        );
        let sharing = std::env::var("RUST_KYC_SHARING_REQUIRED").as_deref() == Ok("1");
        if sharing {
            assert!(
                verdaxis_read::market::visibility::eligibility_sets_sql("$13")
                    .contains("kyc_status IN")
            );
        }
        for enabled in [false, true] {
            let config = Config {
                kyc_execution_gate_enabled: enabled,
                db_session_time_zone: "Asia/Singapore".into(),
                environment: "test".into(),
                release_sha: verdaxis_read::compiled_sha().into(),
                migration_revision: "kycg_20261014_staging".into(),
                database_url: url.clone(),
                bind: "127.0.0.1:0".parse().unwrap(),
                read_admission_slots: 16,
                pool_max: 17,
                python_budget: 52,
                configured_max_connections: 100,
                prepared_budget: None,
            };
            let pools = verdaxis_read::runtime::db::Pools {
                read: sqlx::postgres::PgPoolOptions::new()
                    .max_connections(16)
                    .connect(&url)
                    .await
                    .unwrap(),
                health: sqlx::postgres::PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&url)
                    .await
                    .unwrap(),
            };
            let market: MarketPolicy =
                serde_json::from_str(include_str!("../../../deploy/rust/market-policy.json"))
                    .unwrap();
            let _ = tracing_subscriber::fmt().with_test_writer().try_init();
            let state = AppState::new(config, pools, market).unwrap();
            let app = verdaxis_read::routes::orderbook_lists::routes().with_state(state);
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/orderbook?limit=100")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: serde_json::Value = serde_json::from_slice(
                &to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap(),
            )
            .unwrap();
            let actual: BTreeSet<String> = body
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].as_str().unwrap().to_owned())
                .collect();
            let run = expected["kyc_gate_runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|run| run["enabled"] == enabled)
                .unwrap();
            let wanted: BTreeSet<String> = run["selected"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(actual, wanted, "baseline public HTTP gate={enabled}");

            // This tenant has one customer member and one assisted order; the
            // administrator's own KYC cannot authorize that customer's tenant.
            let order_id: Uuid = expected["cases"]["assisted_foreign_grace"]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let (member, org): (Uuid, Uuid) = sqlx::query_as("SELECT u.id,u.organization_id FROM users u JOIN orderbook_orders o ON o.organization_id=u.organization_id WHERE o.id=$1 AND u.role='SUPPLIER'")
                .bind(order_id).fetch_one(&mut admin).await.unwrap();
            for (status, bound, grace_offset, grace_bound, strict, advisory) in [
                ("APPROVED", true, None, true, true, true),
                ("APPROVED", false, None, true, false, false),
                ("PENDING", true, Some(1), true, true, true),
                ("PENDING", true, Some(0), true, false, true),
                ("PENDING", true, Some(-1), true, false, true),
                ("PENDING", true, Some(1), false, false, true),
                ("REJECTED", true, Some(1), true, false, false),
                ("SUBMITTED", true, None, true, false, true),
            ] {
                sqlx::query("UPDATE users SET kyc_status=$1,kyc_organization_id=CASE WHEN $2 THEN organization_id ELSE (SELECT id FROM organizations WHERE id<>$3 LIMIT 1) END WHERE id=$4")
                    .bind(status).bind(bound).bind(org).bind(member).execute(&mut admin).await.unwrap();
                sqlx::query("DELETE FROM kyc_execution_grace WHERE user_id=$1")
                    .bind(member)
                    .execute(&mut admin)
                    .await
                    .unwrap();
                if let Some(offset) = grace_offset {
                    sqlx::query("INSERT INTO kyc_execution_grace(id,user_id,organization_id,expires_at,kyc_status_at_enrollment,enrolled_at,created_by) VALUES($1,$2,CASE WHEN $3 THEN $4 ELSE (SELECT id FROM organizations WHERE id<>$4 LIMIT 1) END,$5,'PENDING',$5 - interval '1 day','fixture')")
                        .bind(Uuid::new_v4()).bind(member).bind(grace_bound).bind(org).bind(clock + Duration::microseconds(offset)).execute(&mut admin).await.unwrap();
                }
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/api/orderbook?limit=100")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                let body: serde_json::Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), 4 * 1024 * 1024)
                        .await
                        .unwrap(),
                )
                .unwrap();
                let visible = body
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == order_id.to_string());
                assert_eq!(
                    visible,
                    if enabled { strict } else { advisory },
                    "assisted HTTP status={status} gate={enabled} grace={grace_offset:?}"
                );
            }
            // Restore the frozen fixture before the next switch-state baseline.
            sqlx::query("UPDATE users SET kyc_status='APPROVED',kyc_organization_id=(SELECT id FROM organizations WHERE id<>$1 ORDER BY id LIMIT 1) WHERE id=$2")
                .bind(org).bind(member).execute(&mut admin).await.unwrap();
            sqlx::query("DELETE FROM kyc_execution_grace WHERE user_id=$1")
                .bind(member)
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO kyc_execution_grace(id,user_id,organization_id,expires_at,kyc_status_at_enrollment,enrolled_at,created_by) VALUES($1,$2,$3,$4,'PENDING',$4 - interval '1 day','fixture')")
                .bind(Uuid::new_v4()).bind(member).bind(org).bind(clock + Duration::hours(1)).execute(&mut admin).await.unwrap();
        }
    }

    #[tokio::test]
    async fn p4_t9_c06() {
        let mut fixture = Fixture::new().await;
        fixture
            .store(&party(KycStatus::Pending), Some(&grace()))
            .await;
        for sql in [
            "UPDATE kyc_execution_grace SET expires_at=expires_at",
            "DELETE FROM kyc_execution_grace",
            "INSERT INTO kyc_execution_grace SELECT * FROM kyc_execution_grace",
        ] {
            let error = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .execute(&mut fixture.runtime)
                .await
                .unwrap_err();
            assert_eq!(
                error.as_database_error().and_then(|e| e.code()).as_deref(),
                Some("42501")
            );
        }
        fixture.finish().await;
    }
}
