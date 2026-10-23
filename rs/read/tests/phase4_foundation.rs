use std::{future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;
use verdaxis_read::{
    auth::{CurrentActor, VerifiedAccess, store::revoke_user_authentication},
    business::{
        audit::{AuditFact, append_audit},
        error::database_failure,
        status::{AccountStatus, StatusFact, append_status_transition},
        transaction::{BusinessCommand, OrganizationWritePlan, run_ordered_write},
    },
};

type RevocationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DateTime<Utc>, sqlx::Error>> + Send + 'a>>;

#[test]
fn p4_t2_c06() {
    fn interface(conn: &mut PgConnection, id: Uuid, now: DateTime<Utc>) -> RevocationFuture<'_> {
        Box::pin(revoke_user_authentication(conn, id, now))
    }
    let _ = interface;
}

#[tokio::test]
async fn p4_t2_c04() {
    use verdaxis_read::auth::lock::{OrgWriteError, retry_org_write};
    use verdaxis_read::business::transaction::AttemptError;
    let mut attempts = 0;
    let result: Result<(), OrgWriteError<AttemptError>> = retry_org_write(|_| {
        attempts += 1;
        std::future::ready(Err(AttemptError::Database(sqlx::Error::Protocol(
            "unclassified failure".into(),
        ))))
    })
    .await;
    assert!(matches!(result, Err(OrgWriteError::Failed(_))));
    assert_eq!(attempts, 1);
    use axum::http::StatusCode;
    for (state, status, code) in [
        ("55P03", StatusCode::CONFLICT, "MARKET_BUSY"),
        (
            "57014",
            StatusCode::SERVICE_UNAVAILABLE,
            "TRANSACTION_TIMEOUT",
        ),
        ("XX999", StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
    ] {
        let error = database_failure(Some(state));
        assert_eq!(error.status, status);
        assert_eq!(error.detail["code"], code);
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    organization: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
            .expect("explicit disposable migrator URL required");
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        let mut admin = PgConnection::connect(&url).await.unwrap();
        // Identifier bytes are a fixed prefix and locally generated UUID hex only.
        let schema = format!("p4_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE organizations(id uuid PRIMARY KEY); \
             CREATE TABLE users(id uuid PRIMARY KEY, organization_id uuid, role text, status text, \
             email_verified boolean DEFAULT true, must_change_password boolean DEFAULT false, \
             password_changed_at timestamptz, authentication_revoked_at timestamptz, email text, password_hash text, email_verification_token_hash text, email_verification_token_expires_at timestamptz, referral_code text, pending_approval_email_transition_id uuid,pending_approval_email_payload jsonb,pending_approval_email_retry_at timestamptz); \
             CREATE TABLE organization_join_requests(id uuid PRIMARY KEY); \
             CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100), \
             resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45), \
             request_id varchar(36),timestamp timestamptz); \
             CREATE TABLE user_status_transitions(id uuid PRIMARY KEY,user_id uuid,organization_id uuid, \
             role text,from_status text,to_status text,effective_at timestamptz,provenance text); \
             CREATE TABLE refresh_sessions(id uuid PRIMARY KEY,user_id uuid,revoked boolean); \
             CREATE TABLE market_event_outbox(id uuid PRIMARY KEY); \
             CREATE TABLE effects(id uuid PRIMARY KEY)",
        ).execute(&mut admin).await.unwrap();
        let actor = Uuid::new_v4();
        let organization = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations VALUES($1)")
            .bind(organization)
            .execute(&mut admin)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'ADMIN','APPROVED')",
        )
        .bind(actor)
        .bind(organization)
        .execute(&mut admin)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app; REVOKE UPDATE ON {schema}.audit_logs,{schema}.user_status_transitions,{schema}.market_event_outbox FROM rust_app")))
            .execute(&mut admin).await.unwrap();
        let runtime_url = std::env::var("RUST_TEST_DATABASE_URL")
            .expect("explicit disposable runtime URL required");
        let runtime = url::Url::parse(&runtime_url).unwrap();
        assert_eq!(runtime.host_str(), Some("127.0.0.1"));
        assert_eq!(runtime.port(), parsed.port());
        assert_eq!(runtime.path(), parsed.path());
        assert_eq!(runtime.username(), "rust_app");
        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .after_connect(move |conn, _| {
                let sql = search_path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&runtime_url)
            .await
            .unwrap();
        Self {
            pool,
            admin,
            schema,
            actor,
            organization,
        }
    }

    async fn finish(mut self) {
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {} CASCADE", self.schema)))
            .execute(&mut self.admin)
            .await
            .unwrap();
    }
}

struct Command {
    actor: Uuid,
    organization: Uuid,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    pause: bool,
}

impl BusinessCommand for Command {
    type Output = ();

    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        OrganizationWritePlan,
                        verdaxis_read::business::transaction::AttemptError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let organization: Option<Uuid> =
                sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                    .bind(self.actor)
                    .fetch_one(conn)
                    .await?;
            Ok(OrganizationWritePlan::new(
                [self.organization],
                [self.actor],
                [],
                serde_json::json!({"organization": organization}),
            ))
        })
    }

    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(), verdaxis_read::business::transaction::AttemptError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            assert_eq!(actor.id, self.actor);
            sqlx::query("INSERT INTO effects VALUES($1)")
                .bind(self.actor)
                .execute(&mut *conn)
                .await?;
            sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
                .bind(self.actor)
                .execute(&mut *conn)
                .await?;
            append_status_transition(
                conn,
                &StatusFact {
                    id: Uuid::new_v4(),
                    user_id: self.actor,
                    organization_id: Some(self.organization),
                    role: Some("ADMIN".into()),
                    from: Some(AccountStatus::Approved),
                    to: AccountStatus::Rejected,
                    at: Utc::now(),
                },
            )
            .await?;
            append_audit(
                conn,
                &AuditFact {
                    id: Uuid::new_v4(),
                    actor_id: Some(self.actor),
                    action: "account.review".into(),
                    resource_type: "user".into(),
                    resource_id: Some(self.actor.to_string()),
                    changes: None,
                    ip_address: None,
                    request_id: None,
                    at: Utc::now(),
                },
            )
            .await?;
            sqlx::query("INSERT INTO market_event_outbox VALUES($1)")
                .bind(self.actor)
                .execute(&mut *conn)
                .await?;
            self.entered.notify_one();
            if self.pause {
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t2_c02() {
    let fixture = Fixture::new().await;
    let mut holder = fixture.pool.acquire().await.unwrap();
    let mut held = holder.begin().await.unwrap();
    sqlx::query("SELECT id FROM organizations WHERE id=$1 FOR UPDATE")
        .bind(fixture.organization)
        .execute(&mut *held)
        .await
        .unwrap();
    let pool = fixture.pool.clone();
    let id = fixture.actor;
    let cmd = command(&fixture, false);
    let caller = tokio::spawn(async move {
        run_ordered_write(pool, Arc::new(Semaphore::new(1)), access(id), cmd).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND cardinality(pg_blocking_pids(pid)) > 0 AND query LIKE 'SELECT id FROM organizations%')")
                .fetch_one(&mut *held).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    // This update completes while the business owner waits for its first lock.
    sqlx::query("UPDATE users SET status='REJECTED' WHERE id=$1")
        .bind(fixture.actor)
        .execute(&mut *held)
        .await
        .unwrap();
    held.commit().await.unwrap();
    drop(holder);
    let result = caller.await.unwrap();
    assert_eq!(result.unwrap_err().status.as_u16(), 403);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM effects")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        0
    );
    fixture.finish().await;
}

fn access(id: Uuid) -> VerifiedAccess {
    let key = "test-secret-key-for-testing-minimum-32-chars";
    let claims = serde_json::json!({"sub":id,"iat":Utc::now().timestamp(),"exp":Utc::now().timestamp()+3600,"type":"access","iss":"test","aud":"test"});
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
    )
    .unwrap();
    verdaxis_read::auth::verify_access(
        &token,
        &verdaxis_read::auth::VerificationSettings::new(
            key.into(),
            None,
            "test".into(),
            "test".into(),
        ),
        Utc::now(),
    )
    .unwrap()
}

fn command(fixture: &Fixture, pause: bool) -> Command {
    Command {
        actor: fixture.actor,
        organization: fixture.organization,
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        pause,
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t2_c03() {
    let fixture = Fixture::new().await;
    let mut conn = fixture.pool.acquire().await.unwrap();
    let mut transaction = conn.begin().await.unwrap();
    let fact = AuditFact {
        id: Uuid::new_v4(),
        actor_id: Some(fixture.actor),
        action: "account.review".into(),
        resource_type: "user".into(),
        resource_id: Some(fixture.actor.to_string()),
        changes: None,
        ip_address: None,
        request_id: None,
        at: Utc::now(),
    };
    append_audit(&mut transaction, &fact).await.unwrap();
    let status = StatusFact {
        id: Uuid::new_v4(),
        user_id: fixture.actor,
        organization_id: Some(fixture.organization),
        role: Some("ADMIN".into()),
        from: Some(AccountStatus::Pending),
        to: AccountStatus::Approved,
        at: Utc::now(),
    };
    assert_eq!(
        append_status_transition(&mut transaction, &status)
            .await
            .unwrap(),
        Some(status.id)
    );
    let no_change = StatusFact {
        from: Some(AccountStatus::Approved),
        ..status
    };
    assert_eq!(
        append_status_transition(&mut transaction, &no_change)
            .await
            .unwrap(),
        None
    );
    transaction.rollback().await.unwrap();
    drop(conn);
    for table in ["audit_logs", "user_status_transitions"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            0
        );
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t2_c05() {
    let fixture = Fixture::new().await;
    let cmd = command(&fixture, true);
    let entered = cmd.entered.clone();
    let release = cmd.release.clone();
    let permits = Arc::new(Semaphore::new(1));
    let pool = fixture.pool.clone();
    let id = fixture.actor;
    let capacity = permits.clone();
    let caller =
        tokio::spawn(async move { run_ordered_write(pool, capacity, access(id), cmd).await });
    entered.notified().await;
    caller.abort();
    let _ = caller.await;
    assert_eq!(permits.available_permits(), 0);
    release.notify_one();
    let permit = tokio::time::timeout(std::time::Duration::from_secs(10), permits.acquire())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM effects")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        1
    );
    drop(permit);
    fixture.finish().await;
}

#[cfg(feature = "contract-tests")]
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t2_c09() {
    use verdaxis_read::business::transaction::{
        CompletionControl, CompletionPoint, run_controlled_write,
    };
    for (point, lost_ack, cancel) in [
        (CompletionPoint::BeforeCommit, false, true),
        (CompletionPoint::AfterCommit, false, true),
        (CompletionPoint::AfterCommit, true, false),
    ] {
        let fixture = Fixture::new().await;
        let control = CompletionControl::new(point, lost_ack);
        let gate = control.clone();
        let permits = Arc::new(Semaphore::new(1));
        let capacity = permits.clone();
        let pool = fixture.pool.clone();
        let id = fixture.actor;
        let cmd = command(&fixture, false);
        let caller = tokio::spawn(async move {
            run_controlled_write(pool, capacity, access(id), cmd, gate).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), control.reached())
            .await
            .unwrap();
        assert_eq!(permits.available_permits(), 0);
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM effects")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(
            before,
            if point == CompletionPoint::BeforeCommit {
                0
            } else {
                1
            }
        );
        if cancel {
            caller.abort();
        }
        control.release();
        let result = caller.await;
        if lost_ack {
            assert_eq!(
                result.unwrap().unwrap_err().detail["code"],
                "COMMIT_OUTCOME_UNKNOWN"
            );
        }
        let permit = tokio::time::timeout(std::time::Duration::from_secs(10), permits.acquire())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM effects")
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            1
        );
        for table in [
            "audit_logs",
            "user_status_transitions",
            "market_event_outbox",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id=$1")
                .bind(id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            "REJECTED"
        );
        assert!(control.discarded());
        drop(permit);
        fixture.finish().await;
    }
}

struct Events;
impl verdaxis_read::business::admission::AdmissionEvents for Events {
    fn visible<'a>(
        &'a self,
        _: &'a mut PgConnection,
        _: Uuid,
    ) -> verdaxis_read::business::transaction::CommandFuture<
        'a,
        bool,
        verdaxis_read::business::transaction::AttemptError,
    > {
        Box::pin(async { Ok(false) })
    }
    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        _: Uuid,
        _: bool,
    ) -> verdaxis_read::business::transaction::CommandFuture<
        'a,
        (),
        verdaxis_read::business::transaction::AttemptError,
    > {
        Box::pin(async move {
            sqlx::query("INSERT INTO market_event_outbox VALUES($1)")
                .bind(user)
                .execute(conn)
                .await?;
            Ok(())
        })
    }
}

#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn p4_t2_c10() {
    use verdaxis_read::business::admission::RejectAccount;
    let fixture = Fixture::new().await;
    let target = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,organization_id,role,status,email,password_hash) VALUES($1,$2,'BUYER','APPROVED','synthetic@example.test','synthetic')")
        .bind(target).bind(fixture.organization).execute(&fixture.pool).await.unwrap();
    sqlx::query("INSERT INTO refresh_sessions VALUES($1,$2,false)")
        .bind(Uuid::new_v4())
        .bind(target)
        .execute(&fixture.pool)
        .await
        .unwrap();
    let result = run_ordered_write(
        fixture.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(fixture.actor),
        RejectAccount {
            user_id: target,
            now: Utc::now(),
            reason: "review".into(),
            events: Events,
        },
    )
    .await
    .unwrap();
    assert_eq!(result.user_id, target);
    let cutoff: DateTime<Utc> =
        sqlx::query_scalar("SELECT authentication_revoked_at FROM users WHERE id=$1")
            .bind(target)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(cutoff, result.cutoff);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM refresh_sessions WHERE NOT revoked")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        0
    );
    for table in [
        "audit_logs",
        "user_status_transitions",
        "market_event_outbox",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            1
        );
    }
    fixture.finish().await;
}
