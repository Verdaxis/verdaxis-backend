//! Phase 4 Task 3: profile and the write-once onboarding survey.
//!
//! The component adapters are exercised through the real routers, the shared
//! authenticated transaction and the Task 2 ordered write owner. The KYC seam
//! and the market-event seam are declarations from CF4_02/Task 6; their doubles
//! here are built strictly from those declarations and never from producer
//! code that has not landed.

pub use verdaxis_read::*;

#[path = "../src/business/profile.rs"]
mod profile;
#[path = "../src/business/survey.rs"]
mod survey;

#[cfg(feature = "contract-tests")]
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(feature = "contract-tests")]
use std::sync::Mutex;

#[cfg(feature = "contract-tests")]
use axum::http::HeaderMap;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
#[cfg(feature = "contract-tests")]
use chrono::Duration;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
#[cfg(feature = "contract-tests")]
use sqlx::{Connection, Row};
use sqlx::{PgConnection, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::auth::AuthError;
use verdaxis_read::business::transaction::{AttemptError, CommandFuture};

use profile::{
    KycEligibilityView, KycProfilePolicy, KycReason, ProfileAuthority, ProfileKycParty,
    ProfileMarketEvents,
};
use survey::SurveyAuthority;

const SECRET: &str = "profile-component-test-secret-at-least-32-characters";
const ISSUER: &str = "profile-test";
const CLIENT: &str = "203.0.113.7:40000";

/// Each fixture account arrives from its own address. The declared `/me` and
/// survey limits are keyed by client, so one account must not spend another
/// account's quota.
#[cfg(feature = "contract-tests")]
fn peer(index: usize) -> String {
    format!("198.51.100.{}:40000", index + 1)
}

fn verification() -> auth::VerificationSettings {
    auth::VerificationSettings::new(SECRET.into(), None, ISSUER.into(), ISSUER.into())
}

/// A token issued at `issued`, valid for the action clock.
#[cfg(feature = "contract-tests")]
fn token_at(id: Uuid, issued: DateTime<Utc>) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "sub": id,
            "type": "access",
            "iss": ISSUER,
            "aud": ISSUER,
            "jti": Uuid::new_v4(),
            "iat": issued.timestamp(),
            "exp": 4_102_444_800u64,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

#[cfg(feature = "contract-tests")]
fn token(id: Uuid) -> String {
    token_at(id, Utc::now() - Duration::minutes(5))
}

fn config(url: String, gate: bool) -> Config {
    Config {
        kyc_execution_gate_enabled: gate,
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

struct Reply {
    status: StatusCode,
    body: Value,
    #[cfg(feature = "contract-tests")]
    headers: HeaderMap,
    #[cfg(feature = "contract-tests")]
    raw: Vec<u8>,
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<&[u8]>,
    peer: &str,
) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(bearer) = bearer {
        builder = builder.header("authorization", format!("Bearer {bearer}"));
    }
    let mut request = builder
        .body(match body {
            Some(body) => Body::from(body.to_vec()),
            None => Body::empty(),
        })
        .unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        peer.parse::<SocketAddr>().unwrap(),
    ));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    #[cfg(feature = "contract-tests")]
    let headers = response.headers().clone();
    let raw = to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap()
    };
    Reply {
        status,
        body,
        #[cfg(feature = "contract-tests")]
        headers,
        #[cfg(feature = "contract-tests")]
        raw,
    }
}

#[cfg(feature = "contract-tests")]
async fn get(app: &Router, path: &str, bearer: Option<&str>) -> Reply {
    send(app, "GET", path, bearer, None, CLIENT).await
}

#[cfg(feature = "contract-tests")]
async fn put(app: &Router, peer: &str, path: &str, bearer: &str, body: &Value) -> Reply {
    send(
        app,
        "PUT",
        path,
        Some(bearer),
        Some(body.to_string().as_bytes()),
        peer,
    )
    .await
}

#[cfg(feature = "contract-tests")]
async fn survey(app: &Router, peer: &str, bearer: Option<&str>, body: &Value) -> Reply {
    send(
        app,
        "POST",
        "/api/auth/survey",
        bearer,
        Some(body.to_string().as_bytes()),
        peer,
    )
    .await
}

/// The CF4_02 truth table, transcribed from the accepted declaration. It is a
/// consumer double only: the real policy lands with Task 9.
struct PolicyDouble;

impl KycProfilePolicy for PolicyDouble {
    fn eligibility<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        party: ProfileKycParty,
        execution_gate_enabled: bool,
        now: DateTime<Utc>,
    ) -> CommandFuture<'a, KycEligibilityView, sqlx::Error> {
        Box::pin(async move {
            const KNOWN: [&str; 4] = ["PENDING", "SUBMITTED", "APPROVED", "REJECTED"];
            let known = KNOWN.contains(&party.kyc_status.as_str());
            let rejected = party.kyc_status == "REJECTED";
            let bound_elsewhere = party.kyc_organization_id.is_some()
                && party.kyc_organization_id != party.organization_id;
            if !execution_gate_enabled {
                // Gate off: advisory rule, no grace deadline is reported.
                return Ok(if !known {
                    KycEligibilityView {
                        eligible: false,
                        reason: KycReason::Required,
                        grace_expires_at: None,
                    }
                } else {
                    let eligible = !rejected && !bound_elsewhere;
                    KycEligibilityView {
                        eligible,
                        reason: if rejected {
                            KycReason::Rejected
                        } else if eligible {
                            KycReason::NotEnforced
                        } else {
                            KycReason::Required
                        },
                        grace_expires_at: None,
                    }
                });
            }
            if !known {
                // Unknown statuses refuse in both switch states.
                return Ok(KycEligibilityView {
                    eligible: false,
                    reason: KycReason::Required,
                    grace_expires_at: None,
                });
            }
            if party.kyc_status == "APPROVED"
                && party.organization_id.is_some()
                && party.kyc_organization_id == party.organization_id
            {
                return Ok(KycEligibilityView {
                    eligible: true,
                    reason: KycReason::Approved,
                    grace_expires_at: None,
                });
            }
            let grace: Option<DateTime<Utc>> = match party.organization_id {
                Some(organization) => {
                    sqlx::query_scalar(
                        "SELECT expires_at FROM kyc_execution_grace WHERE user_id=$1 AND organization_id=$2",
                    )
                    .bind(party.user_id)
                    .bind(organization)
                    .fetch_optional(conn)
                    .await?
                }
                None => None,
            };
            let reason = if rejected {
                KycReason::Rejected
            } else if grace.is_some_and(|expires| now < expires) {
                KycReason::Grace
            } else {
                KycReason::Required
            };
            Ok(KycEligibilityView {
                eligible: reason == KycReason::Grace,
                reason,
                grace_expires_at: grace,
            })
        })
    }
}

#[cfg(feature = "contract-tests")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppendCall {
    user: Uuid,
    organization: Uuid,
    public: bool,
}

#[derive(Default)]
struct RecordingEvents {
    #[cfg(feature = "contract-tests")]
    visible: Mutex<VecDeque<bool>>,
    #[cfg(feature = "contract-tests")]
    visible_calls: Mutex<Vec<Uuid>>,
    #[cfg(feature = "contract-tests")]
    appends: Mutex<Vec<AppendCall>>,
}

impl RecordingEvents {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    #[cfg(feature = "contract-tests")]
    fn push_visible(&self, value: bool) {
        self.visible.lock().unwrap().push_back(value);
    }
    #[cfg(feature = "contract-tests")]
    fn visible_calls(&self) -> Vec<Uuid> {
        self.visible_calls.lock().unwrap().clone()
    }
    #[cfg(feature = "contract-tests")]
    fn appends(&self) -> Vec<AppendCall> {
        self.appends.lock().unwrap().clone()
    }
}

impl ProfileMarketEvents for RecordingEvents {
    fn visible<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            #[cfg(feature = "contract-tests")]
            self.visible_calls.lock().unwrap().push(user);
            #[cfg(not(feature = "contract-tests"))]
            let _ = user;
            #[cfg(feature = "contract-tests")]
            {
                Ok(self.visible.lock().unwrap().pop_front().unwrap_or(false))
            }
            #[cfg(not(feature = "contract-tests"))]
            {
                Ok(false)
            }
        })
    }
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO market_event_outbox(id,aggregate_type,aggregate_id,participant_org_ids,payload) \
                 VALUES($1,'user',$2,$3,$4)",
            )
            .bind(Uuid::new_v4())
            .bind(user.to_string())
            .bind(json!([organization.to_string()]))
            .bind(json!({"schema_version":1,"user_id":user.to_string()}))
            .execute(conn)
            .await?;
            #[cfg(feature = "contract-tests")]
            self.appends.lock().unwrap().push(AppendCall {
                user,
                organization,
                public,
            });
            #[cfg(not(feature = "contract-tests"))]
            let _ = public;
            Ok(())
        })
    }
}

/// One fixture row: index, role, status, organization, KYC status, bound KYC
/// organization, grace offset in days.
#[cfg(feature = "contract-tests")]
type ProfileRow<'a> = (
    usize,
    Option<&'a str>,
    &'a str,
    Option<Uuid>,
    &'a str,
    Option<Uuid>,
    Option<i64>,
);

#[cfg(feature = "contract-tests")]
struct Fixture {
    admin: PgConnection,
    pool: sqlx::PgPool,
    app: Router,
    schema: String,
    organizations: [Uuid; 2],
    users: [Uuid; 16],
}

#[cfg(feature = "contract-tests")]
impl Fixture {
    async fn new(gate: bool, events: Arc<RecordingEvents>) -> Self {
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
        assert_eq!(r.username(), "rust_app");
        assert_ne!(m.username(), r.username());
        let mut admin = PgConnection::connect(&migrator).await.unwrap();
        let schema = format!("p4_profile_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY, name text NOT NULL DEFAULT 'Org', verification_status text NOT NULL DEFAULT 'APPROVED', provenance text NOT NULL DEFAULT 'REAL', created_at timestamptz NOT NULL DEFAULT now()); \
             CREATE TABLE users(\
               id uuid PRIMARY KEY,\
               email text NOT NULL,\
               first_name text,\
               last_name text,\
               role text,\
               status text NOT NULL DEFAULT 'PENDING',\
               organization_id uuid REFERENCES organizations(id),\
               referral_code text,\
               must_change_password boolean NOT NULL DEFAULT false,\
               email_verified boolean NOT NULL DEFAULT true,\
               password_changed_at timestamptz,\
               authentication_revoked_at timestamptz,\
               kyc_status varchar(20) NOT NULL DEFAULT 'PENDING',\
               kyc_organization_id uuid REFERENCES organizations(id),\
               onboarding_use_case varchar(50),\
               onboarding_referral_source varchar(500)); \
             CREATE TABLE kyc_execution_grace(user_id uuid NOT NULL REFERENCES users(id), organization_id uuid NOT NULL REFERENCES organizations(id), expires_at timestamptz NOT NULL, PRIMARY KEY(user_id, organization_id)); \
             CREATE TABLE market_event_outbox(id uuid PRIMARY KEY, aggregate_type text, aggregate_id text, participant_org_ids jsonb, payload jsonb, created_at timestamptz NOT NULL DEFAULT now()); \
             CREATE SEQUENCE survey_updates",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        // A sequence counts real survey writes: nextval is not transactional, so
        // a rolled-back statement cannot inflate it and a no-op UPDATE never
        // reaches it.
        sqlx::raw_sql(
            "CREATE FUNCTION count_survey_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('survey_updates'); RETURN NEW; END $$; \
             CREATE TRIGGER count_survey_update AFTER UPDATE OF onboarding_use_case ON users \
               FOR EACH ROW WHEN (OLD.onboarding_use_case IS DISTINCT FROM NEW.onboarding_use_case) \
               EXECUTE FUNCTION count_survey_update()",
        )
        .execute(&mut admin)
        .await
        .unwrap();
        let organizations = [Uuid::new_v4(), Uuid::new_v4()];
        for organization in organizations {
            sqlx::query("INSERT INTO organizations(id) VALUES($1)")
                .bind(organization)
                .execute(&mut admin)
                .await
                .unwrap();
        }
        let users: [Uuid; 16] = std::array::from_fn(|_| Uuid::new_v4());
        let [o1, o2] = organizations;
        // index, role, status, org, kyc status, kyc org, grace offset in days.
        let rows: [ProfileRow<'_>; 12] = [
            (
                0,
                Some("BUYER"),
                "APPROVED",
                Some(o1),
                "APPROVED",
                Some(o1),
                None,
            ),
            (
                1,
                Some("SUPPLIER"),
                "APPROVED",
                Some(o1),
                "PENDING",
                Some(o1),
                Some(7),
            ),
            (
                2,
                Some("BUYER"),
                "APPROVED",
                Some(o1),
                "REJECTED",
                Some(o1),
                Some(7),
            ),
            (
                3,
                Some("BUYER"),
                "APPROVED",
                Some(o1),
                "PENDING",
                Some(o1),
                Some(-1),
            ),
            (
                4,
                Some("BUYER"),
                "APPROVED",
                Some(o1),
                "APPROVED",
                Some(o2),
                None,
            ),
            (5, None, "APPROVED", Some(o1), "PENDING", None, None),
            (6, Some("BUYER"), "APPROVED", None, "PENDING", None, None),
            (7, Some("BUYER"), "APPROVED", Some(o1), "WEIRD", None, None),
            (8, Some("BUYER"), "PENDING", Some(o1), "PENDING", None, None),
            (
                9,
                Some("BUYER"),
                "REJECTED",
                Some(o1),
                "PENDING",
                None,
                None,
            ),
            (14, None, "APPROVED", Some(o1), "PENDING", None, None),
            (
                15,
                Some("BUYER"),
                "APPROVED",
                Some(o2),
                "PENDING",
                None,
                None,
            ),
        ];
        for (index, role, status, organization, kyc_status, kyc_organization, grace_days) in rows {
            sqlx::query(
                "INSERT INTO users(id,email,first_name,last_name,role,status,organization_id,kyc_status,kyc_organization_id) \
                 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            )
            .bind(users[index])
            .bind(format!("user{index}@example.test"))
            .bind(format!("User{index}"))
            .bind(format!("Family{index}"))
            .bind(role)
            .bind(status)
            .bind(organization)
            .bind(kyc_status)
            .bind(kyc_organization)
            .execute(&mut admin)
            .await
            .unwrap();
            if let Some(days) = grace_days {
                sqlx::query("INSERT INTO kyc_execution_grace VALUES($1,$2,$3)")
                    .bind(users[index])
                    .bind(o1)
                    .bind(Utc::now() + Duration::days(days))
                    .execute(&mut admin)
                    .await
                    .unwrap();
            }
        }
        // Account-state and cutoff cases.
        for (index, must_change) in [(10usize, true), (11, false)] {
            sqlx::query(
                "INSERT INTO users(id,email,role,status,organization_id,must_change_password) VALUES($1,$2,'BUYER','APPROVED',$3,$4)",
            )
            .bind(users[index])
            .bind(format!("user{index}@example.test"))
            .bind(o1)
            .bind(must_change)
            .execute(&mut admin)
            .await
            .unwrap();
        }
        let issued =
            DateTime::from_timestamp((Utc::now() - Duration::minutes(5)).timestamp(), 0).unwrap();
        sqlx::query("INSERT INTO users(id,email,role,status,organization_id,password_changed_at) VALUES($1,$2,'BUYER','APPROVED',$3,$4)")
            .bind(users[12])
            .bind("user12@example.test")
            .bind(o1)
            .bind(issued)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users(id,email,role,status,organization_id,authentication_revoked_at) VALUES($1,$2,'BUYER','APPROVED',$3,$4)")
            .bind(users[13])
            .bind("user13@example.test")
            .bind(o1)
            .bind(issued)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "GRANT USAGE ON SCHEMA {schema} TO rust_app; \
             GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app; \
             GRANT USAGE,SELECT ON SEQUENCE {schema}.survey_updates TO rust_app"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        let search = format!("SET search_path TO {schema}");
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
        let health = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&runtime)
            .unwrap();
        let state = AppState::new(
            config(runtime.clone(), gate),
            runtime::db::Pools {
                read: pool.clone(),
                health,
            },
            MarketPolicy {
                source_sha: compiled_sha().into(),
                products: vec![],
                points: vec![],
            },
        )
        .unwrap();
        let limits = auth::limits::SharedLimits::default();
        let app = Router::new()
            .merge(profile::routes(ProfileAuthority::new(
                verification(),
                limits.clone(),
                Arc::new(PolicyDouble) as Arc<dyn KycProfilePolicy>,
                events.clone() as Arc<dyn ProfileMarketEvents>,
            )))
            .merge(survey::routes(SurveyAuthority::new(verification(), limits)))
            .with_state(state);
        Self {
            admin,
            pool,
            app,
            schema,
            organizations,
            users,
        }
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(&self.pool)
            .await
            .unwrap()
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

#[cfg(feature = "contract-tests")]
fn eligibility(body: &Value) -> (bool, &str, Option<&str>) {
    let eligibility = &body["execution_eligibility"];
    (
        eligibility["eligible"].as_bool().unwrap(),
        eligibility["reason"].as_str().unwrap(),
        eligibility["grace_expires_at"].as_str(),
    )
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;

    /// A01: the exact client projection, current-account cutoffs and the shared
    /// KYC eligibility decision, through the real adapter.
    #[tokio::test]
    async fn p4_t3_c01() {
        for gate in [true, false] {
            let f = Fixture::new(gate, RecordingEvents::new()).await;
            let read = |index: usize, path: &'static str| {
                let bearer = token(f.users[index]);
                let app = f.app.clone();
                async move { get(&app, path, Some(&bearer)).await }
            };
            for index in [0usize, 1, 2, 3, 4, 5, 6, 7] {
                let reply = read(index, "/api/auth/me").await;
                assert_eq!(reply.status, StatusCode::OK, "user {index}");
                let mut keys: Vec<&str> = reply
                    .body
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                keys.sort_unstable();
                assert_eq!(
                    keys,
                    [
                        "email",
                        "execution_eligibility",
                        "first_name",
                        "id",
                        "kyc_status",
                        "last_name",
                        "must_change_password",
                        "organization_id",
                        "referral_code",
                        "role",
                        "status"
                    ]
                );
                assert_eq!(reply.body["email"], format!("user{index}@example.test"));
                assert_eq!(reply.body["must_change_password"], json!(false));
                assert_eq!(reply.body["referral_code"], Value::Null);
                assert_eq!(reply.body["status"], "APPROVED");
                assert_eq!(reply.body["kyc_status"], {
                    match index {
                        0 | 4 => "APPROVED",
                        1 | 3 => "PENDING",
                        2 => "REJECTED",
                        7 => "WEIRD",
                        _ => "PENDING",
                    }
                });
                assert_eq!(reply.body["organization_id"], {
                    if index == 6 {
                        Value::Null
                    } else {
                        json!(f.organizations[0].to_string())
                    }
                });
                let (eligible, reason, grace) = eligibility(&reply.body);
                if gate {
                    match index {
                        0 => {
                            assert!(eligible);
                            assert_eq!(reason, "KYC_APPROVED");
                            assert_eq!(grace, None);
                        }
                        1 => {
                            assert!(eligible);
                            assert_eq!(reason, "KYC_GRACE");
                            assert!(grace.unwrap().starts_with('2'));
                        }
                        2 => {
                            assert!(!eligible);
                            assert_eq!(reason, "KYC_REJECTED");
                            assert!(grace.is_some());
                        }
                        3 => {
                            assert!(!eligible);
                            assert_eq!(reason, "KYC_REQUIRED");
                            assert!(grace.is_some());
                        }
                        4 | 7 => {
                            assert!(!eligible);
                            assert_eq!(reason, "KYC_REQUIRED");
                            assert_eq!(grace, None);
                        }
                        _ => {
                            assert!(!eligible);
                            assert_eq!(reason, "NOT_APPLICABLE");
                            assert_eq!(grace, None);
                        }
                    }
                } else {
                    match index {
                        0 | 1 | 3 => {
                            assert!(eligible);
                            assert_eq!(reason, "KYC_NOT_ENFORCED");
                            assert_eq!(grace, None);
                        }
                        2 => {
                            assert!(!eligible);
                            assert_eq!(reason, "KYC_REJECTED");
                            assert_eq!(grace, None);
                        }
                        4 => {
                            assert!(!eligible);
                            assert_eq!(reason, "KYC_REQUIRED");
                            assert_eq!(grace, None);
                        }
                        7 => assert!(!eligible),
                        _ => {
                            assert!(!eligible);
                            assert_eq!(reason, "NOT_APPLICABLE");
                        }
                    }
                }
            }
            // Role and names round-trip exactly as stored, including nulls.
            let reply = read(5, "/api/auth/me").await;
            assert_eq!(reply.body["role"], Value::Null);
            assert_eq!(reply.body["first_name"], "User5");
            assert_eq!(reply.body["last_name"], "Family5");
            // Anonymous and malformed credentials are the shared envelope.
            for bearer in [None, Some("not-a-token")] {
                let reply = get(&f.app, "/api/auth/me", bearer).await;
                assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
                assert!(reply.body["detail"]["code"].is_string());
                assert!(reply.body["detail"]["message"].is_string());
                assert_eq!(reply.headers.get("www-authenticate").unwrap(), "Bearer");
            }
            // Current-account state, not the token, decides admission.
            for (index, code, status) in [
                (8usize, "ACCOUNT_PENDING", StatusCode::FORBIDDEN),
                (9, "ACCOUNT_REJECTED", StatusCode::FORBIDDEN),
                (10, "PASSWORD_CHANGE_REQUIRED", StatusCode::FORBIDDEN),
                (11, "INVALID_CREDENTIALS", StatusCode::UNAUTHORIZED),
                (12, "AUTH_PASSWORD_CHANGED", StatusCode::UNAUTHORIZED),
                (13, "AUTH_SESSION_REVOKED", StatusCode::UNAUTHORIZED),
            ] {
                if index == 11 {
                    // A token with an unusable subject never reaches the account.
                    let reply = get(&f.app, "/api/auth/me", Some("garbage")).await;
                    assert_eq!(reply.status, status);
                    assert_eq!(reply.body["detail"]["code"], code);
                    continue;
                }
                // Users 12 and 13 carry cutoffs written five minutes before
                // the fixture finished; a token from the default helper is
                // issued at read time, which can fall after the cutoff and
                // dodge the rule on wall-clock luck. Their tokens are ten
                // minutes old, strictly before either cutoff.
                let reply = if index >= 12 {
                    let bearer = token_at(f.users[index], Utc::now() - Duration::minutes(10));
                    let app = f.app.clone();
                    get(&app, "/api/auth/me", Some(&bearer)).await
                } else {
                    read(index, "/api/auth/me").await
                };
                assert_eq!(reply.status, status, "user {index}");
                assert_eq!(reply.body["detail"]["code"], code, "user {index}");
            }
            // An expired token is refused before any account read.
            let expired = jsonwebtoken::encode(
                &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
                &json!({
                    "sub": f.users[0], "type": "access", "iss": ISSUER, "aud": ISSUER,
                    "jti": Uuid::new_v4(), "iat": 1_000_000_000u64, "exp": 1_000_000_001u64
                }),
                &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
            )
            .unwrap();
            assert_eq!(
                get(&f.app, "/api/auth/me", Some(&expired)).await.status,
                StatusCode::UNAUTHORIZED
            );
            // The route never writes.
            assert_eq!(f.count("SELECT count(*) FROM market_event_outbox").await, 0);
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM users WHERE onboarding_use_case IS NOT NULL"
                )
                .fetch_one(&f.pool)
                .await
                .unwrap(),
                0
            );
            f.finish().await;
        }
    }

    /// A02: the corrected `/me` limits, at the route boundary and per client.
    #[tokio::test]
    async fn p4_t3_c02() {
        let f = Fixture::new(true, RecordingEvents::new()).await;
        let bearer = token(f.users[14]);
        for _ in 0..120 {
            assert_eq!(
                get(&f.app, "/api/auth/me", Some(&bearer)).await.status,
                StatusCode::OK
            );
        }
        let refused = get(&f.app, "/api/auth/me", Some(&bearer)).await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.body["detail"]["code"], "RATE_LIMITED");
        let retry_after: u64 = refused
            .headers
            .get("retry-after")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&retry_after));
        // A different client has its own window.
        assert_eq!(
            send(
                &f.app,
                "GET",
                "/api/auth/me",
                Some(&bearer),
                None,
                "198.51.100.9:40000"
            )
            .await
            .status,
            StatusCode::OK
        );
        // The PUT window is separate and smaller.
        for index in 0..10 {
            let reply = put(
                &f.app,
                CLIENT,
                "/api/auth/me",
                &bearer,
                &json!({"first_name": format!("Name{index}")}),
            )
            .await;
            assert_eq!(reply.status, StatusCode::OK, "put {index}");
        }
        let refused = put(
            &f.app,
            CLIENT,
            "/api/auth/me",
            &bearer,
            &json!({"first_name": "Refused"}),
        )
        .await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.body["detail"]["code"], "RATE_LIMITED");
        assert!(refused.headers.contains_key("retry-after"));
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT first_name FROM users WHERE id=$1")
                .bind(f.users[14])
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            "Name9"
        );
        // Unauthorized calls never consume a client's quota.
        for _ in 0..130 {
            assert_eq!(
                send(
                    &f.app,
                    "GET",
                    "/api/auth/me",
                    Some("bad"),
                    None,
                    "192.0.2.44:40000"
                )
                .await
                .status,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            send(
                &f.app,
                "GET",
                "/api/auth/me",
                Some(&bearer),
                None,
                "192.0.2.44:40000"
            )
            .await
            .status,
            StatusCode::OK
        );
        for _ in 0..11 {
            assert_eq!(
                send(
                    &f.app,
                    "PUT",
                    "/api/auth/me",
                    Some("bad"),
                    Some(br#"{"first_name":"Nope"}"#),
                    "192.0.2.45:40000"
                )
                .await
                .status,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            send(
                &f.app,
                "PUT",
                "/api/auth/me",
                Some(&bearer),
                Some(br#"{"first_name":"Allowed"}"#),
                "192.0.2.45:40000"
            )
            .await
            .status,
            StatusCode::OK
        );
        f.finish().await;
    }

    /// A02: one role assignment, never ADMIN, and its event is part of the same
    /// commit as the profile write.
    #[tokio::test]
    async fn p4_t3_c03() {
        let events = RecordingEvents::new();
        let f = Fixture::new(true, events.clone()).await;
        let plain = token(f.users[14]);
        let other = token(f.users[15]);
        // A role that is already assigned cannot change, for the user themselves
        // or across tenants.
        assert_eq!(
            put(
                &f.app,
                &peer(15),
                "/api/auth/me",
                &other,
                &json!({"role": "SUPPLIER"})
            )
            .await
            .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>("SELECT role FROM users WHERE id=$1")
                .bind(f.users[15])
                .fetch_one(&f.pool)
                .await
                .unwrap()
                .as_deref(),
            Some("BUYER")
        );
        // ADMIN is never assignable, even from an empty role.
        let reply = put(
            &f.app,
            &peer(14),
            "/api/auth/me",
            &plain,
            &json!({"role": "ADMIN"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert_eq!(
            reply.body["detail"]["code"],
            "PROFILE_ROLE_CHANGE_NOT_ALLOWED"
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>("SELECT role FROM users WHERE id=$1")
                .bind(f.users[14])
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            None
        );
        // Unknown role labels are a typed refusal, not a value.
        assert_eq!(
            put(
                &f.app,
                &peer(14),
                "/api/auth/me",
                &plain,
                &json!({"role": "admin"})
            )
            .await
            .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            put(
                &f.app,
                &peer(14),
                "/api/auth/me",
                &plain,
                &json!({"role": "OWNER"})
            )
            .await
            .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        // An organization-bound profile gets its admission event exactly once.
        events.push_visible(false);
        events.push_visible(true);
        let reply = put(
            &f.app,
            &peer(14),
            "/api/auth/me",
            &plain,
            &json!({"role": "BUYER", "first_name": "Ada", "last_name": "Lovelace"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.body["role"], "BUYER");
        assert_eq!(reply.body["first_name"], "Ada");
        assert_eq!(reply.body["last_name"], "Lovelace");
        // Python builds the response after the commit, so the new role is
        // already part of the projection: the policy runs and a PENDING
        // review without grace is KYC_REQUIRED, not NOT_APPLICABLE.
        assert_eq!(
            reply.body["execution_eligibility"]["reason"],
            "KYC_REQUIRED"
        );
        assert_eq!(events.visible_calls(), vec![f.users[14], f.users[14]]);
        assert_eq!(
            events.appends(),
            vec![AppendCall {
                user: f.users[14],
                organization: f.organizations[0],
                public: true,
            }]
        );
        assert_eq!(f.count("SELECT count(*) FROM market_event_outbox").await, 1);
        // Repeating the same role is a no-op for the role: no event, names still
        // update, and ADMIN remains refused.
        let appends = events.appends().len();
        let reply = put(
            &f.app,
            &peer(14),
            "/api/auth/me",
            &plain,
            &json!({"role": "BUYER", "first_name": "Ada2"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.body["role"], "BUYER");
        assert_eq!(reply.body["first_name"], "Ada2");
        assert_eq!(events.appends().len(), appends);
        assert_eq!(events.visible_calls().len(), 2);
        assert_eq!(
            put(
                &f.app,
                &peer(14),
                "/api/auth/me",
                &plain,
                &json!({"role": "ADMIN"})
            )
            .await
            .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            put(
                &f.app,
                &peer(14),
                "/api/auth/me",
                &plain,
                &json!({"role": "SUPPLIER"})
            )
            .await
            .status,
            StatusCode::FORBIDDEN
        );
        // No public projection: the role is still assigned and no event exists.
        let no_projection = token(f.users[5]);
        events.push_visible(false);
        events.push_visible(false);
        let reply = put(
            &f.app,
            &peer(5),
            "/api/auth/me",
            &no_projection,
            &json!({"role": "SUPPLIER"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.body["role"], "SUPPLIER");
        assert_eq!(events.appends().len(), 1);
        assert_eq!(events.visible_calls().len(), 4);
        // An orgless profile takes the role without a projection probe.
        let orgless = token(f.users[6]);
        let reply = put(
            &f.app,
            &peer(6),
            "/api/auth/me",
            &orgless,
            &json!({"role": "BUYER"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.body["role"], "BUYER");
        assert_eq!(events.visible_calls().len(), 4);
        // Two distinct users each assign their own role independently: both
        // succeed, one event per user, no cross-tenant effect.
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        for id in [first, second] {
            sqlx::query(
                "INSERT INTO users(id,email,status,organization_id) VALUES($1,$2,'APPROVED',$3)",
            )
            .bind(id)
            .bind(format!("{id}@example.test"))
            .bind(f.organizations[0])
            .execute(&f.pool)
            .await
            .unwrap();
        }
        events.push_visible(false);
        events.push_visible(false);
        events.push_visible(false);
        events.push_visible(false);
        let a = token(first);
        let b = token(second);
        let app = f.app.clone();
        let (left, right) = tokio::join!(
            async {
                put(
                    &app,
                    &peer(20),
                    "/api/auth/me",
                    &a,
                    &json!({"role": "BUYER"}),
                )
                .await
            },
            async {
                put(
                    &app,
                    &peer(21),
                    "/api/auth/me",
                    &b,
                    &json!({"role": "SUPPLIER"}),
                )
                .await
            }
        );
        assert_eq!(left.status, StatusCode::OK);
        assert_eq!(right.status, StatusCode::OK);
        let roles: Vec<Option<String>> =
            sqlx::query_scalar("SELECT role FROM users WHERE id=ANY($1) ORDER BY id")
                .bind(vec![first, second])
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(roles.iter().filter(|role| role.is_some()).count(), 2);
        assert_eq!(events.visible_calls().len(), 8);
        // Same-row concurrent first assignment: exactly one answer wins.
        let third = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,email,status) VALUES($1,$2,'APPROVED')")
            .bind(third)
            .bind(format!("{third}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        let app = f.app.clone();
        let a = token(third);
        let b = token(third);
        let (left, right) = tokio::join!(
            async {
                put(
                    &app,
                    &peer(22),
                    "/api/auth/me",
                    &a,
                    &json!({"role": "BUYER"}),
                )
                .await
            },
            async {
                put(
                    &app,
                    &peer(22),
                    "/api/auth/me",
                    &b,
                    &json!({"role": "SUPPLIER"}),
                )
                .await
            }
        );
        // Both attempts are admitted by a valid token; the row lock serializes
        // the two ordered writes, so one commits its role and the other then
        // loses the once-only rule. Two accepted writes would mean the second
        // overwrote the first.
        let mut statuses = [left.status, right.status];
        statuses.sort_by_key(|status| status.as_u16());
        assert_eq!(statuses, [StatusCode::OK, StatusCode::FORBIDDEN]);
        assert!(matches!(
            sqlx::query_scalar::<_, Option<String>>("SELECT role FROM users WHERE id=$1")
                .bind(third)
                .fetch_one(&f.pool)
                .await
                .unwrap()
                .as_deref(),
            Some("BUYER" | "SUPPLIER")
        ));
        f.finish().await;
    }

    /// A02: a refused role change is an authority refusal (PD4-T8-1); it must
    /// not spend the caller's quota, and the accepted writes then still fit.
    #[tokio::test]
    async fn p4_t3_c10() {
        let f = Fixture::new(true, RecordingEvents::new()).await;
        let bearer = token(f.users[15]);
        // The account already has BUYER, so every one of these is refused.
        for _ in 0..20 {
            let reply = put(
                &f.app,
                CLIENT,
                "/api/auth/me",
                &bearer,
                &json!({"role": "SUPPLIER"}),
            )
            .await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
            assert_eq!(
                reply.body["detail"]["code"],
                "PROFILE_ROLE_CHANGE_NOT_ALLOWED"
            );
        }
        // The 10/minute bucket is still whole, and the next call is the one
        // that exhausts it. This fixture row has no role, so these writes are
        // plain profile updates with no event work.
        for index in 0..10 {
            let reply = put(
                &f.app,
                CLIENT,
                "/api/auth/me",
                &bearer,
                &json!({"first_name": format!("Name{index}")}),
            )
            .await;
            assert_eq!(reply.status, StatusCode::OK, "accepted write {index}");
        }
        let refused = put(
            &f.app,
            CLIENT,
            "/api/auth/me",
            &bearer,
            &json!({"first_name": "Over"}),
        )
        .await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.body["detail"]["code"], "RATE_LIMITED");
        f.finish().await;
    }

    /// An event write that fails at commit takes the profile write with it.
    #[tokio::test]
    async fn p4_t3_c07() {
        let events = RecordingEvents::new();
        let mut f = Fixture::new(true, events.clone()).await;
        sqlx::raw_sql(
            "CREATE OR REPLACE FUNCTION refuse_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic event failure' USING ERRCODE='23514'; END $$; \
             CREATE CONSTRAINT TRIGGER refuse_event AFTER INSERT ON market_event_outbox DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_event()",
        )
        .execute(&mut f.admin)
        .await
        .unwrap();
        events.push_visible(true);
        events.push_visible(true);
        let reply = put(
            &f.app,
            &peer(14),
            "/api/auth/me",
            &token(f.users[14]),
            &json!({"role": "BUYER", "first_name": "Never"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(reply.body["detail"]["code"], "DATABASE_ERROR");
        let row = sqlx::query("SELECT role,first_name FROM users WHERE id=$1")
            .bind(f.users[14])
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(row.get::<Option<String>, _>("role"), None);
        assert_eq!(
            row.get::<Option<String>, _>("first_name").as_deref(),
            Some("User14")
        );
        assert_eq!(f.count("SELECT count(*) FROM market_event_outbox").await, 0);
        f.finish().await;
    }

    /// A03: the declared bounds refuse before any write, and the authenticated
    /// account is the only row the survey can touch.
    #[tokio::test]
    async fn p4_t3_c04() {
        let f = Fixture::new(true, RecordingEvents::new()).await;
        let bearer = token(f.users[14]);
        let stored = || {
            let pool = f.pool.clone();
            async move {
                sqlx::query(
                    "SELECT onboarding_use_case,onboarding_referral_source FROM users WHERE id=$1",
                )
                .bind(f.users[14])
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let long: String = "é".repeat(501);
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&bearer),
            &json!({"use_case": "buyer", "referral_source": long}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            reply.body["detail"]["code"],
            "SURVEY_REFERRAL_SOURCE_TOO_LONG"
        );
        // The refusal happens before any write: the account is still empty.
        let row = stored().await;
        assert_eq!(row.get::<Option<String>, _>("onboarding_use_case"), None);
        assert_eq!(
            row.get::<Option<String>, _>("onboarding_referral_source"),
            None
        );
        // Character count, not byte count: 500 non-ASCII characters fit.
        let exact: String = "é".repeat(500);
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&bearer),
            &json!({"use_case": "buyer", "referral_source": exact}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert!(reply.raw.is_empty());
        let row = stored().await;
        assert_eq!(
            row.get::<Option<String>, _>("onboarding_use_case")
                .as_deref(),
            Some("buyer")
        );
        assert_eq!(
            row.get::<Option<String>, _>("onboarding_referral_source")
                .as_deref(),
            Some(exact.as_str())
        );
        // A finite use_case and a closed body shape.
        for body in [
            json!({"use_case": "pirate"}),
            json!({"use_case": ""}),
            json!({"use_case": 3}),
            json!({"use_case": "buyer", "referral_source": 7}),
            json!({"use_case": "buyer", "unexpected": true}),
            json!({}),
            json!([1, 2]),
        ] {
            let reply = survey(&f.app, CLIENT, Some(&bearer), &body).await;
            assert!(
                matches!(
                    reply.status,
                    StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
                ),
                "{body} -> {:?}",
                reply.status
            );
            assert!(reply.body["detail"]["code"].is_string());
        }
        // The invalid use_case path is 400 and writes nothing anywhere.
        let fresh = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users(id,email,status,organization_id) VALUES($1,$2,'APPROVED',$3)",
        )
        .bind(fresh)
        .bind(format!("{fresh}@example.test"))
        .bind(f.organizations[0])
        .execute(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            survey(
                &f.app,
                CLIENT,
                Some(&token(fresh)),
                &json!({"use_case": "pirate"})
            )
            .await
            .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT onboarding_use_case FROM users WHERE id=$1"
            )
            .bind(fresh)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            None
        );
        // The address key is refused like any undeclared field, so the body
        // cannot retarget the write through a foreign value: nothing is
        // recorded for either account, and a clean submit still records.
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&token(fresh)),
            &json!({
                "email": "user15@example.test",
                "use_case": "supplier",
                "referral_source": "web"
            }),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            reply.body
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT onboarding_use_case FROM users WHERE id=$1"
            )
            .bind(fresh)
            .fetch_one(&f.pool)
            .await
            .unwrap()
            .as_deref(),
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT onboarding_use_case FROM users WHERE id=$1"
            )
            .bind(f.users[15])
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            None
        );
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&token(fresh)),
            &json!({"use_case": "supplier", "referral_source": "web"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT onboarding_use_case FROM users WHERE id=$1"
            )
            .bind(fresh)
            .fetch_one(&f.pool)
            .await
            .unwrap()
            .as_deref(),
            Some("supplier")
        );
        let unauth = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,email,status) VALUES($1,$2,'APPROVED')")
            .bind(unauth)
            .bind(format!("{unauth}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        // The declared body limit refuses before parsing or writing.
        let huge = format!(
            "{{\"use_case\":\"buyer\",\"referral_source\":\"{}\"}}",
            "x".repeat(140_000)
        );
        let reply = send(
            &f.app,
            "POST",
            "/api/auth/survey",
            Some(&token(unauth)),
            Some(huge.as_bytes()),
            CLIENT,
        )
        .await;
        assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(reply.body["detail"]["code"], "BODY_TOO_LARGE");
        // Malformed JSON is a typed refusal, not a crash.
        let reply = send(
            &f.app,
            "POST",
            "/api/auth/survey",
            Some(&token(unauth)),
            Some(b"{"),
            CLIENT,
        )
        .await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT onboarding_use_case FROM users WHERE id=$1"
            )
            .bind(unauth)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            None
        );
        f.finish().await;
    }

    /// A03: write once. A repeat keeps the first answer, and concurrent first
    /// submits leave exactly one stored answer from one real update.
    #[tokio::test]
    async fn p4_t3_c05() {
        let f = Fixture::new(true, RecordingEvents::new()).await;
        let bearer = token(f.users[14]);
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&bearer),
            &json!({"use_case": "buyer", "referral_source": "conference"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        // The typed model refuses every field it does not declare, including
        // the address key an older client still sends; the answer is not
        // recorded and a later clean submit still succeeds.
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&bearer),
            &json!({"use_case": "buyer", "email": "someone@example.test"}),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            reply.body
        );
        let reply = survey(
            &f.app,
            CLIENT,
            Some(&bearer),
            &json!({"use_case": "supplier", "referral_source": "search"}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
        let row = sqlx::query(
            "SELECT onboarding_use_case,onboarding_referral_source FROM users WHERE id=$1",
        )
        .bind(f.users[14])
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("onboarding_use_case")
                .as_deref(),
            Some("buyer")
        );
        assert_eq!(
            row.get::<Option<String>, _>("onboarding_referral_source")
                .as_deref(),
            Some("conference")
        );
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT is_called FROM survey_updates")
                .fetch_one(&f.pool)
                .await
                .unwrap()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT last_value FROM survey_updates")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        // Independent requests on independent pool connections race the first
        // submit. Each accepted request is its own transaction and socket.
        for iteration in 0..5 {
            let user = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO users(id,email,status,organization_id) VALUES($1,$2,'APPROVED',$3)",
            )
            .bind(user)
            .bind(format!("{user}@example.test"))
            .bind(f.organizations[0])
            .execute(&f.pool)
            .await
            .unwrap();
            let before: i64 = sqlx::query_scalar("SELECT last_value FROM survey_updates")
                .fetch_one(&f.pool)
                .await
                .unwrap();
            let app = f.app.clone();
            let a = token(user);
            let b = token(user);
            // Both requests arrive from one address, which has its own bucket.
            let client = peer(30 + iteration);
            let (left, right) = tokio::join!(
                async {
                    survey(
                        &app,
                        &client,
                        Some(&a),
                        &json!({"use_case": "buyer", "referral_source": "left"}),
                    )
                    .await
                },
                async {
                    survey(
                        &app,
                        &client,
                        Some(&b),
                        &json!({"use_case": "supplier", "referral_source": "right"}),
                    )
                    .await
                }
            );
            assert_eq!(left.status, StatusCode::NO_CONTENT);
            assert_eq!(right.status, StatusCode::NO_CONTENT);
            let row = sqlx::query(
                "SELECT onboarding_use_case,onboarding_referral_source FROM users WHERE id=$1",
            )
            .bind(user)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            let answer = row.get::<Option<String>, _>("onboarding_use_case").unwrap();
            let source = row
                .get::<Option<String>, _>("onboarding_referral_source")
                .unwrap();
            assert!(
                matches!(
                    (answer.as_str(), source.as_str()),
                    ("buyer", "left") | ("supplier", "right")
                ),
                "{answer} {source}"
            );
            let after: i64 = sqlx::query_scalar("SELECT last_value FROM survey_updates")
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(after, before + 1, "exactly one survey update");
        }
        f.finish().await;
    }
}

/// The unauthenticated boundary needs no database connection at all.
#[tokio::test]
async fn p4_t3_c08() {
    let url = "postgres://offline_app:offline_password@127.0.0.1:1/offline_test";
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(url)
        .unwrap();
    let health = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(url)
        .unwrap();
    let state = AppState::new(
        config(url.into(), true),
        runtime::db::Pools { read: pool, health },
        MarketPolicy {
            source_sha: compiled_sha().into(),
            products: vec![],
            points: vec![],
        },
    )
    .unwrap();
    let limits = auth::limits::SharedLimits::default();
    let app = Router::new()
        .merge(profile::routes(ProfileAuthority::new(
            verification(),
            limits.clone(),
            Arc::new(PolicyDouble) as Arc<dyn KycProfilePolicy>,
            RecordingEvents::new() as Arc<dyn ProfileMarketEvents>,
        )))
        .merge(survey::routes(SurveyAuthority::new(verification(), limits)))
        .with_state(state);
    for (method, path) in [
        ("GET", "/api/auth/me"),
        ("PUT", "/api/auth/me"),
        ("POST", "/api/auth/survey"),
    ] {
        for bearer in [None, Some("invalid")] {
            let reply = send(
                &app,
                method,
                path,
                bearer,
                Some(br#"{"role":"BUYER"}"#),
                CLIENT,
            )
            .await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert!(reply.body["detail"]["code"].is_string());
            assert!(reply.body["detail"]["message"].is_string());
        }
    }
    // A wrong method is not silently accepted by another route.
    let reply = send(&app, "DELETE", "/api/auth/me", None, None, CLIENT).await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    let reply = send(&app, "GET", "/api/auth/survey", None, None, CLIENT).await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
}

/// The declared limit keys and windows for the two profile routes are in the
/// frozen inventory; the Task 2 counter is the authority for them.
#[test]
fn p4_t3_c09() {
    let limits = auth::limits::SharedLimits::default();
    let keys = auth::limits::BusinessLimitKeys {
        client: CLIENT,
        user: Some("user"),
        organization: Some("org"),
        token: Some("token"),
    };
    for _ in 0..120 {
        limits
            .check_business("GET", "/api/auth/me", keys, 1_000)
            .unwrap();
    }
    assert!(
        limits
            .check_business("GET", "/api/auth/me", keys, 1_000)
            .is_err()
    );
    for _ in 0..10 {
        limits
            .check_business("PUT", "/api/auth/me", keys, 1_000)
            .unwrap();
    }
    let refused = limits
        .check_business("PUT", "/api/auth/me", keys, 1_010)
        .unwrap_err();
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.detail["code"], "RATE_LIMITED");
    assert_eq!(refused.retry_after, Some(50));
    for _ in 0..5 {
        limits
            .check_business("POST", "/api/auth/survey", keys, 1_000)
            .unwrap();
    }
    assert!(
        limits
            .check_business("POST", "/api/auth/survey", keys, 1_000)
            .is_err()
    );
    // An unowned method/template pair is refused, never silently unlimited.
    assert_eq!(
        limits
            .check_business("DELETE", "/api/auth/me", keys, 1_000)
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    let _ = AuthError::Credentials;
}
