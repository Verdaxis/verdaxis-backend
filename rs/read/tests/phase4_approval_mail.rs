#[cfg_attr(not(feature = "contract-tests"), allow(dead_code))]
#[path = "../src/business/approval_mail.rs"]
mod approval_mail;

use approval_mail::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-24T09:00:00.123456Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn marker(user_id: Uuid, now: DateTime<Utc>) -> ApprovalMailMarker {
    ApprovalMailMarker {
        user_id,
        transition_id: Uuid::new_v4(),
        payload: serde_json::from_value(serde_json::json!({
            "from": "sender@example.test", "to": ["recipient@example.test"],
            "subject": "Synthetic", "html": "<p>Synthetic</p>"
        }))
        .unwrap(),
        retry_at: now,
    }
}

#[test]
fn p4_t11a_c08() {
    let marker = marker(Uuid::new_v4(), instant());
    let value = serde_json::to_value(&marker.payload).unwrap();
    assert!(value.get("reply_to").is_none());
    assert!(format!("{marker:?}").contains("REDACTED"));
    assert!(!format!("{marker:?}").contains('@'));
    let mut retained = value;
    retained["reply_to"] = serde_json::json!("support@example.test");
    let payload: FrozenApprovalPayload = serde_json::from_value(retained.clone()).unwrap();
    assert!(serde_json::to_value(payload).unwrap() == retained);
    assert_eq!(ApprovalMailBatchSize::default().get(), 20);
    assert!(ApprovalMailBatchSize::new(0).is_err());
    assert!(ApprovalMailBatchSize::new(101).is_err());
    assert_eq!(ApprovalMailBatchSize::new(100).unwrap().get(), 100);
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use chrono::Duration;
    use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    struct Fixture {
        admin: PgConnection,
        pool: sqlx::PgPool,
        runtime: String,
        schema: String,
        user: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let migrator = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL required");
            let runtime = std::env::var("RUST_TEST_DATABASE_URL")
                .expect("explicit disposable runtime URL required");
            let m = url::Url::parse(&migrator).unwrap();
            let r = url::Url::parse(&runtime).unwrap();
            assert_eq!(m.host_str(), Some("127.0.0.1"));
            assert_eq!(r.host_str(), m.host_str());
            assert_eq!(r.port(), m.port());
            assert_eq!(r.path(), m.path());
            assert!(m.path().ends_with("_test"));
            assert_eq!(r.username(), "rust_app");
            assert_ne!(m.username(), r.username());
            let mut admin = PgConnection::connect(&migrator).await.unwrap();
            let schema = format!("p4_mail_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}; \
                 CREATE TABLE users(id uuid PRIMARY KEY, status text NOT NULL, role text NOT NULL, \
                 email_verified boolean NOT NULL, pending_approval_email_transition_id uuid, \
                 pending_approval_email_payload jsonb, pending_approval_email_retry_at timestamptz); \
                 GRANT USAGE ON SCHEMA {schema} TO rust_app; \
                 GRANT SELECT ON users TO rust_app; \
                 GRANT UPDATE(pending_approval_email_transition_id,pending_approval_email_payload,\
                 pending_approval_email_retry_at) ON users TO rust_app;"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            let user = Uuid::new_v4();
            sqlx::query("INSERT INTO users(id,status,role,email_verified) VALUES($1,'APPROVED','BUYER',true)")
                .bind(user).execute(&mut admin).await.unwrap();
            let path = format!("SET search_path TO {schema}");
            let pool = PgPoolOptions::new()
                .max_connections(1)
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
                admin,
                pool,
                runtime,
                schema,
                user,
            }
        }

        /// A pool with its own connection budget, so a second claimer holds a
        /// real second connection instead of queueing behind the first.
        async fn second_pool(&self) -> sqlx::PgPool {
            let path = format!("SET search_path TO {}", self.schema);
            PgPoolOptions::new()
                .max_connections(1)
                .after_connect(move |conn, _| {
                    let path = path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                            .execute(conn)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&self.runtime)
                .await
                .unwrap()
        }

        async fn freeze(&self, marker: &ApprovalMailMarker) {
            let mut tx = self.pool.begin().await.unwrap();
            freeze_approval_mail(&mut tx, marker).await.unwrap();
            tx.commit().await.unwrap();
        }

        async fn claim(
            &self,
            marker: &ApprovalMailMarker,
            now: DateTime<Utc>,
        ) -> Option<ApprovalMailClaim> {
            let mut tx = self.pool.begin().await.unwrap();
            let claim = claim_approval_mail(&mut tx, marker.user_id, marker.transition_id, now)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            claim
        }

        async fn pending(&mut self) -> (Option<Uuid>, Option<DateTime<Utc>>) {
            sqlx::query_as("SELECT pending_approval_email_transition_id,pending_approval_email_retry_at FROM users WHERE id=$1")
                .bind(self.user).fetch_one(&mut self.admin).await.unwrap()
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
    async fn p4_t11a_c01() {
        let mut f = Fixture::new().await;
        let old = marker(f.user, instant());
        f.freeze(&old).await;
        let claimed = f.claim(&old, instant()).await.unwrap();
        let newer = marker(f.user, instant());
        // Models the atomic rejection/reapproval writer while the old provider call is outstanding.
        f.freeze(&newer).await;
        let current = f.claim(&newer, instant()).await.unwrap();
        for accepted in [true, false] {
            let mut tx = f.pool.begin().await.unwrap();
            finalize_approval_mail(&mut tx, &claimed, accepted, instant())
                .await
                .unwrap();
            tx.commit().await.unwrap();
            assert_eq!(
                f.pending().await,
                (Some(newer.transition_id), Some(current.claim_until))
            );
        }
        // A later lease for the same transition also fences the old attempt.
        let next = f.claim(&newer, current.claim_until).await.unwrap();
        let mut tx = f.pool.begin().await.unwrap();
        finalize_approval_mail(&mut tx, &current, true, instant())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(f.pending().await.1, Some(next.claim_until));
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c02() {
        let mut f = Fixture::new().await;
        let marker = marker(f.user, instant());
        f.freeze(&marker).await;
        let pool = f.pool.clone();
        let user = f.user;
        let transition = marker.transition_id;
        let result = deliver_account_approval_mail(&f.pool, user, transition, instant, move |payload, key| async move {
            assert!(key == format!("account-approval/{transition}"));
            assert!(serde_json::to_value(payload).unwrap().get("reply_to").is_none());
            let mut tx = tokio::time::timeout(std::time::Duration::from_secs(3), pool.begin()).await.unwrap().unwrap();
            // A single-connection pool and NOWAIT prove release of connection and row lock.
            let row: (DateTime<Utc>,) = sqlx::query_as("SELECT pending_approval_email_retry_at FROM users WHERE id=$1 FOR UPDATE NOWAIT")
                .bind(user).fetch_one(&mut *tx).await.unwrap();
            assert_eq!(row.0, instant() + Duration::minutes(5));
            tx.rollback().await.unwrap();
            true
        }).await.unwrap();
        assert_eq!(result, ApprovalDelivery::Sent);
        assert_eq!(f.pending().await, (None, None));
        let absent: (bool,) =
            sqlx::query_as("SELECT pending_approval_email_payload IS NULL FROM users WHERE id=$1")
                .bind(user)
                .fetch_one(&mut f.admin)
                .await
                .unwrap();
        assert!(absent.0);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c03() {
        let mut f = Fixture::new().await;
        let marker = marker(f.user, instant());
        f.freeze(&marker).await;
        let expected = serde_json::to_value(&marker.payload).unwrap();
        sqlx::query("SET default_transaction_read_only=on")
            .execute(&f.pool)
            .await
            .unwrap();
        let result = deliver_account_approval_mail(
            &f.pool,
            f.user,
            marker.transition_id,
            instant,
            |payload, _| async move {
                assert!(serde_json::to_value(payload).unwrap() == expected);
                false
            },
        )
        .await
        .unwrap();
        assert_eq!(result, ApprovalDelivery::Deferred);
        assert_eq!(
            f.pending().await,
            (
                Some(marker.transition_id),
                Some(instant() + Duration::hours(1))
            )
        );
        assert!(
            f.claim(
                &marker,
                instant() + Duration::hours(1) - Duration::microseconds(1)
            )
            .await
            .is_none()
        );
        assert!(
            f.claim(&marker, instant() + Duration::hours(1))
                .await
                .is_some()
        );
        f.finish().await;
    }

    /// PostgreSQL blocks a claimant on the row lock the holder keeps open.
    async fn wait_until_blocked(observer: &mut PgConnection, pid: i32) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool =
                    sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                        .bind(pid)
                        .fetch_one(&mut *observer)
                        .await
                        .unwrap();
                if blocked {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second claimer must reach a database lock wait");
    }

    #[tokio::test]
    async fn p4_t11a_c04() {
        let mut f = Fixture::new().await;
        let raced = marker(f.user, instant());
        f.freeze(&raced).await;
        let second = f.second_pool().await;
        let barrier = Arc::new(Barrier::new(2));
        let user = f.user;
        let transition = raced.transition_id;
        // Each claimer owns its own connection, and both transactions are open
        // before either claim runs. A shared single-connection pool would only
        // serialise them and prove nothing about the row lock.
        let attempt = |pool: sqlx::PgPool, barrier: Arc<Barrier>| async move {
            let mut tx = pool.begin().await.unwrap();
            barrier.wait().await;
            let claim = claim_approval_mail(&mut tx, user, transition, instant())
                .await
                .unwrap();
            tx.commit().await.unwrap();
            claim
        };
        let (a, b) = tokio::join!(
            attempt(f.pool.clone(), barrier.clone()),
            attempt(second, barrier)
        );
        assert_eq!(
            usize::from(a.is_some()) + usize::from(b.is_some()),
            1,
            "exactly one concurrent claimer wins the row"
        );
        let claim = a.or(b).unwrap();
        assert!(
            f.claim(&raced, claim.claim_until - Duration::microseconds(1))
                .await
                .is_none()
        );
        assert!(f.claim(&raced, claim.claim_until).await.is_some());

        // A claim the holder has not committed makes the next claimer wait on
        // the row lock, and that claim loses once the holder commits.
        let held = marker(f.user, instant());
        f.freeze(&held).await;
        let mut holder = f.pool.begin().await.unwrap();
        let held_claim = claim_approval_mail(&mut holder, f.user, held.transition_id, instant())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held_claim.claim_until, instant() + Duration::minutes(5));
        let waiting = f.second_pool().await;
        let held_transition = held.transition_id;
        let (pid_tx, pid_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            let mut tx = waiting.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            pid_tx.send(pid).unwrap();
            let claim = claim_approval_mail(&mut tx, user, held_transition, instant()).await;
            tx.commit().await.unwrap();
            claim.unwrap()
        });
        let pid = pid_rx.await.unwrap();
        wait_until_blocked(&mut f.admin, pid).await;
        holder.commit().await.unwrap();
        assert!(waiter.await.unwrap().is_none());
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c05() {
        let mut f = Fixture::new().await;
        let marker = marker(f.user, instant());
        let mut tx = f.pool.begin().await.unwrap();
        freeze_approval_mail(&mut tx, &marker).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(f.pending().await, (None, None));
        f.freeze(&marker).await;
        let mut tx = f.pool.begin().await.unwrap();
        let claim = claim_approval_mail(&mut tx, f.user, marker.transition_id, instant())
            .await
            .unwrap()
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(f.pending().await.1, Some(instant()));
        let claim2 = f.claim(&marker, instant()).await.unwrap();
        assert_eq!(claim.claim_until, claim2.claim_until);
        let mut tx = f.pool.begin().await.unwrap();
        finalize_approval_mail(&mut tx, &claim2, true, instant())
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(f.pending().await.0, Some(marker.transition_id));
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c06() {
        let mut f = Fixture::new().await;
        for (status, role, verified, malformed) in [
            ("REJECTED", "BUYER", true, false),
            ("PENDING", "BUYER", true, false),
            ("APPROVED", "ADMIN", true, false),
            ("APPROVED", "BUYER", false, false),
            ("APPROVED", "BUYER", true, true),
        ] {
            let marker = marker(f.user, instant());
            f.freeze(&marker).await;
            sqlx::query("UPDATE users SET status=$2,role=$3,email_verified=$4,pending_approval_email_payload=CASE WHEN $5 THEN '[]'::jsonb ELSE pending_approval_email_payload END WHERE id=$1")
                .bind(f.user).bind(status).bind(role).bind(verified).bind(malformed).execute(&mut f.admin).await.unwrap();
            let result = deliver_account_approval_mail(
                &f.pool,
                f.user,
                marker.transition_id,
                instant,
                |_, _| async { panic!("unexpected transport invocation") },
            )
            .await
            .unwrap();
            assert_eq!(result, ApprovalDelivery::Skipped);
            assert_eq!(f.pending().await, (None, None));
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c07() {
        let mut f = Fixture::new().await;
        let first_marker = marker(Uuid::from_u128(1), instant() - Duration::seconds(105));
        for n in 0..105 {
            let id = Uuid::from_u128(n + 1);
            sqlx::query("INSERT INTO users(id,status,role,email_verified) VALUES($1,'APPROVED','BUYER',true)")
                .bind(id).execute(&mut f.admin).await.unwrap();
            let marker = if n == 0 {
                first_marker.clone()
            } else {
                marker(id, instant() - Duration::seconds(105 - n as i64))
            };
            f.freeze(&marker).await;
        }
        let mut conn = f.pool.acquire().await.unwrap();
        let default = due_approval_mail(&mut conn, instant(), ApprovalMailBatchSize::default())
            .await
            .unwrap();
        assert_eq!(default.len(), 20);
        assert_eq!(default.first().unwrap().user_id, Uuid::from_u128(1));
        let max = due_approval_mail(
            &mut conn,
            instant(),
            ApprovalMailBatchSize::new(100).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(max.len(), 100);
        assert_eq!(max.last().unwrap().user_id, Uuid::from_u128(100));
        // The due row carries the frozen transition the sender must claim.
        assert_eq!(
            default.first().unwrap().transition_id,
            first_marker.transition_id
        );
        assert!(
            due_approval_mail(
                &mut conn,
                instant() - Duration::hours(1),
                ApprovalMailBatchSize::default()
            )
            .await
            .unwrap()
            .is_empty()
        );
        drop(conn);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t11a_c09() {
        let mut f = Fixture::new().await;
        let marker = marker(f.user, instant());
        f.freeze(&marker).await;
        // A schema the claim does not expect surfaces only its SQLSTATE, never
        // the driver's message.
        sqlx::query("ALTER TABLE users DROP COLUMN pending_approval_email_payload")
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mut tx = f.pool.begin().await.unwrap();
        let error = claim_approval_mail(&mut tx, f.user, marker.transition_id, instant())
            .await
            .unwrap_err();
        tx.rollback().await.unwrap();
        // The SQLSTATE survives for the caller; the driver message never does.
        let MailError::Database(driver) = &error else {
            panic!("expected a database failure");
        };
        assert_eq!(
            driver
                .as_database_error()
                .and_then(|failure| failure.code())
                .as_deref(),
            Some("42703")
        );
        assert_eq!(format!("{error:?}"), "MailError::Database");
        assert_eq!(format!("{error}"), "MailError::Database");
        f.finish().await;
    }

    /// CF4_01 `rules.idempotency_key`: the frozen transition key must reach the
    /// provider on the send, so an uncertain retry collapses at the transport.
    #[tokio::test]
    async fn p4_t11a_c10() {
        use std::sync::Mutex;
        use verdaxis_read::crud::referrals::invite::MailClient;
        let captured = Arc::new(Mutex::new(None::<String>));
        let seen = captured.clone();
        let app = axum::Router::new().route(
            "/emails",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let seen = seen.clone();
                async move {
                    *seen.lock().unwrap() = headers
                        .get("idempotency-key")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    axum::http::StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            MailClient::loopback(format!("http://{address}/emails"), "test-key".into()).unwrap();
        let f = Fixture::new().await;
        let marker = marker(f.user, instant());
        f.freeze(&marker).await;
        let transition = marker.transition_id;
        let expected = format!("account-approval/{transition}");
        let result = deliver_account_approval_mail(
            &f.pool,
            f.user,
            transition,
            instant,
            |payload, key| async move {
                assert_eq!(key, format!("account-approval/{transition}"));
                let payload = serde_json::to_value(payload).unwrap();
                client.send_with_idempotency(&payload, Some(&key)).await
            },
        )
        .await
        .unwrap();
        assert_eq!(result, ApprovalDelivery::Sent);
        assert_eq!(*captured.lock().unwrap(), Some(expected));
        server.abort();
        let _ = server.await;
        f.finish().await;
    }
}
