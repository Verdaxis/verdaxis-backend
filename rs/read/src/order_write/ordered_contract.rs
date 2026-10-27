//! Disposable commands exercise the ordered owner and its typed receipt ports.
use super::super::authority::{ActorRole, TradeSide};
use super::super::locks::{MarketOperation, OperationLockPlan, PartyLockPreview};
use super::*;
use crate::auth::VerifiedAccess;
use sqlx::{Connection, PgPool, postgres::PgPoolOptions};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

type Error = Box<dyn std::error::Error + Send + Sync>;

struct Fixture {
    pool: PgPool,
    admin: PgConnection,
    schema: String,
    actor: Uuid,
    other: Uuid,
    org: Uuid,
    moved_org: Uuid,
    trade: Uuid,
    url: String,
}
impl Fixture {
    async fn new() -> Result<Self, Error> {
        let url = std::env::var("RUST_TEST_DATABASE_URL")?;
        let admin_url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")?;
        let target = url::Url::parse(&url)?;
        let admin_target = url::Url::parse(&admin_url)?;
        assert_eq!(target.host_str(), Some("127.0.0.1"));
        assert_eq!(target.port(), admin_target.port());
        assert_eq!(target.path(), admin_target.path());
        assert!(target.path().ends_with("_test"));
        assert_eq!(target.username(), "rust_app");
        let mut admin = PgConnection::connect(&admin_url).await?;
        let schema = format!("ordered_receipt_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {schema}"
        )))
        .execute(&mut admin)
        .await?;
        sqlx::raw_sql(include_str!(
            "../../tests/fixtures/phase8_ordered_receipts.sql"
        ))
        .execute(&mut admin)
        .await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app; REVOKE UPDATE ON market_command_results,market_event_outbox FROM rust_app"))).execute(&mut admin).await?;
        let actor = Uuid::new_v4();
        let other = Uuid::new_v4();
        let org = Uuid::new_v4();
        let moved_org = Uuid::new_v4();
        let trade = Uuid::new_v4();
        sqlx::query(include_str!(
            "../../tests/fixtures/phase8_ordered_parties.sql"
        ))
        .bind(org)
        .bind(moved_org)
        .execute(&mut admin)
        .await?;
        sqlx::query(include_str!(
            "../../tests/fixtures/phase8_ordered_users.sql"
        ))
        .bind(actor)
        .bind(other)
        .bind(org)
        .execute(&mut admin)
        .await?;
        sqlx::query(include_str!(
            "../../tests/fixtures/phase8_ordered_trade.sql"
        ))
        .bind(trade)
        .bind(org)
        .bind(actor)
        .bind(other)
        .execute(&mut admin)
        .await?;
        let pool = Self::pool(&url, &schema, 5).await?;
        Ok(Self {
            pool,
            admin,
            schema,
            actor,
            other,
            org,
            moved_org,
            trade,
            url,
        })
    }
    async fn pool(url: &str, schema: &str, max: u32) -> Result<PgPool, sqlx::Error> {
        let path = format!("SET search_path TO {schema}");
        PgPoolOptions::new()
            .max_connections(max)
            .after_connect(move |conn, _| {
                let path = path.clone();
                Box::pin(async move {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(path))
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
    }
    fn command(&self) -> Command {
        Command {
            actor: self.actor,
            org: self.org,
            trade: self.trade,
            resource: self.trade,
            key: Some(Uuid::new_v4().to_string()),
            hash: "same".into(),
            support: None,
            fail_response: false,
            abort: None,
            always_abort: false,
            calls: Arc::new(AtomicUsize::new(0)),
            staged: Arc::new(AtomicBool::new(false)),
            collision: None,
        }
    }
    async fn effects(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM market_event_outbox")
            .fetch_one(&self.pool)
            .await
            .unwrap()
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

#[derive(Clone)]
struct Command {
    actor: Uuid,
    org: Uuid,
    trade: Uuid,
    resource: Uuid,
    key: Option<String>,
    hash: String,
    support: Option<Uuid>,
    fail_response: bool,
    abort: Option<&'static str>,
    always_abort: bool,
    calls: Arc<AtomicUsize>,
    staged: Arc<AtomicBool>,
    collision: Option<Arc<Collision>>,
}
struct Collision {
    ready: Notify,
    release: Notify,
    winner: ReceiptRecord,
}
impl MarketCommand for Command {
    type Output = CommittedCommand;
    fn preview<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        actor: Uuid,
    ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
        Box::pin(async move {
            let parties =
                PartyLockPreview::new([self.org], [actor], actor, json!({"trade":self.trade}));
            let mut locks = OperationLockPlan::new(MarketOperation::Deliver);
            locks.parties(&parties)?;
            if let Some(key) = &self.key {
                locks.idempotency(actor, Operation::TradeDeliver.as_str(), key)?;
            }
            locks.trade(self.trade)?;
            Ok(MarketPreview { parties, locks })
        })
    }
    fn replay<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a super::super::AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Option<Self::Output>, MarketAttemptError>> {
        Box::pin(async move {
            let context = tx
                .cleanup_context(actor, self.trade, TradeSide::Seller)
                .await?;
            let Some(key) = &self.key else {
                return Ok(None);
            };
            // The context comes from recorded participation, never current membership.
            let mut result = tx
                .replay_receipt(
                    &context,
                    Operation::TradeDeliver,
                    self.resource,
                    key,
                    &self.hash,
                )
                .await?;
            if self.support.is_some() && result.is_some() {
                return Err(super::super::MarketWriteError::new(
                    super::super::MarketErrorCode::IdempotencyConflict,
                )
                .into());
            }
            if let Some(value) = &mut result {
                value.replayed = true;
            }
            Ok(result)
        })
    }
    fn execute<'a>(
        &'a self,
        tx: &'a mut LockedMarketTransaction<'_>,
        actor: &'a super::super::AuthenticatedActor,
    ) -> BoxFuture<'a, Result<Self::Output, MarketAttemptError>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let context = tx
                .cleanup_context(actor, self.trade, TradeSide::Seller)
                .await?;
            let party = super::super::authority::admission::PartyRef {
                user_id: actor.user_id(),
                organization_id: self.org,
                role: ActorRole::Supplier,
            };
            tx.require_execution(&party, chrono::Utc::now(), false)
                .await?;
            tx.ordered_effect(self.trade).await?;
            self.staged.store(true, Ordering::SeqCst);
            if let Some(state) = self.abort.filter(|_| call == 0 || self.always_abort) {
                tx.ordered_abort(state).await?;
            }
            let response = CommittedCommand::new(
                200,
                json!({"trade": self.trade, "protected": "stored"}),
                false,
            );
            if let Some(key) = &self.key {
                if let Some(collision) = &self.collision {
                    collision.ready.notify_one();
                    collision.release.notified().await;
                }
                tx.record_receipt(
                    &context,
                    &ReceiptRecord {
                        id: Uuid::new_v4(),
                        actor_user_id: self.actor,
                        effective_organization_id: self.org,
                        support_context_id: None,
                        operation: Operation::TradeDeliver,
                        idempotency_key: key.clone(),
                        request_hash: self.hash.clone(),
                        resource_id: self.resource,
                        response_status: 200,
                        response_body: response.response_body.clone(),
                    },
                )
                .await?;
            }
            Ok(response)
        })
    }
    fn prepare_response(&self, value: &Self::Output) -> Result<(), MarketAttemptError> {
        struct Prepared<'a> {
            value: &'a Value,
            fail: bool,
        }
        impl serde::Serialize for Prepared<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                if self.fail {
                    return Err(serde::ser::Error::custom("response assembly unavailable"));
                }
                self.value.serialize(serializer)
            }
        }
        serde_json::to_vec(&Prepared {
            value: &value.response_body,
            fail: self.fail_response,
        })
        .map_err(|_| {
            super::super::MarketWriteError::new(super::super::MarketErrorCode::ValidationFailed)
        })?;
        Ok(())
    }
}
fn access(actor: Uuid) -> VerifiedAccess {
    VerifiedAccess {
        subject: actor,
        issued_at: Some(chrono::Utc::now()),
        issuance_out_of_range: false,
    }
}
async fn run(
    pool: PgPool,
    actor: Uuid,
    command: Command,
) -> Result<CommittedCommand, super::super::MarketWriteError> {
    run_market_command(
        pool,
        Arc::new(Semaphore::new(4)),
        access(actor),
        command,
        super::super::authority::admission::AdmissionClass::Execution,
    )
    .await
}

pub(super) async fn case(name: &str) -> Value {
    case_result(name).await.unwrap()
}
async fn case_result(name: &str) -> Result<Value, Error> {
    let mut f = Fixture::new().await?;
    let evidence = match name {
        "participation" => {
            let cmd = f.command();
            run(f.pool.clone(), f.actor, cmd.clone()).await?;
            for state in ["rejected", "detached", "moved"] {
                let (status, org) = match state {
                    "rejected" => ("REJECTED", Some(f.org)),
                    "detached" => ("APPROVED", None),
                    _ => ("APPROVED", Some(f.moved_org)),
                };
                sqlx::query("UPDATE users SET status=$1,organization_id=$2 WHERE id=$3")
                    .bind(status)
                    .bind(org)
                    .bind(f.actor)
                    .execute(&mut f.admin)
                    .await?;
                let result = run(f.pool.clone(), f.actor, cmd.clone()).await?;
                assert!(result.replayed);
            }
            let org: Uuid =
                sqlx::query_scalar("SELECT effective_organization_id FROM market_command_results")
                    .fetch_one(&f.pool)
                    .await?;
            json!({"replayed_states":["rejected","detached","moved"],"effects":f.effects().await,"fresh_execution_calls":cmd.calls.load(Ordering::SeqCst),"recorded_organization_preserved":org==f.org})
        }
        "identity" => {
            let cmd = f.command();
            run(f.pool.clone(), f.actor, cmd.clone()).await?;
            let stranger = run(f.pool.clone(), f.other, cmd.clone()).await.unwrap_err();
            let mut changed = cmd.clone();
            changed.resource = Uuid::new_v4();
            let resource = run(f.pool.clone(), f.actor, changed).await.unwrap_err();
            sqlx::query("UPDATE market_command_results SET support_context_id=$1")
                .bind(Uuid::new_v4())
                .execute(&mut f.admin)
                .await?;
            let context = run(f.pool.clone(), f.actor, cmd.clone()).await.unwrap_err();
            sqlx::query("UPDATE market_command_results SET support_context_id=NULL")
                .execute(&mut f.admin)
                .await?;
            let mut changed = cmd;
            changed.hash = "changed".into();
            let hash = run(f.pool.clone(), f.actor, changed).await.unwrap_err();
            json!({"stranger_status":stranger.status.as_u16(),"resource_code":resource.code.as_str(),"context_code":context.code.as_str(),"hash_code":hash.code.as_str(),"protected_body_returned":false,"effects":f.effects().await})
        }
        "collision" => {
            let mut cmd = f.command();
            let key = cmd.key.clone().unwrap();
            let winner = ReceiptRecord {
                id: Uuid::new_v4(),
                actor_user_id: f.actor,
                effective_organization_id: f.org,
                support_context_id: None,
                operation: Operation::TradeDeliver,
                idempotency_key: key.clone(),
                request_hash: cmd.hash.clone(),
                resource_id: cmd.resource,
                response_status: 200,
                response_body: json!({"winner":true}),
            };
            let collision = Arc::new(Collision {
                ready: Notify::new(),
                release: Notify::new(),
                winner,
            });
            cmd.collision = Some(collision.clone());
            let pool = f.pool.clone();
            let actor = f.actor;
            let request = cmd.clone();
            let task = tokio::spawn(async move { run(pool, actor, request).await });
            collision.ready.notified().await;
            insert_winner(&mut f.admin, &collision.winner).await?;
            collision.release.notify_one();
            let result = task.await??;
            assert!(result.replayed);
            assert_eq!(result.response_body, collision.winner.response_body);
            let effects = f.effects().await;
            let mut changed = f.command();
            let mut mismatch = collision.winner.clone();
            mismatch.id = Uuid::new_v4();
            mismatch.idempotency_key = changed.key.clone().unwrap();
            mismatch.resource_id = Uuid::new_v4();
            let mismatch_collision = Arc::new(Collision {
                ready: Notify::new(),
                release: Notify::new(),
                winner: mismatch,
            });
            changed.collision = Some(mismatch_collision.clone());
            let request = changed.clone();
            let pool = f.pool.clone();
            let actor = f.actor;
            let task = tokio::spawn(async move { run(pool, actor, request).await });
            mismatch_collision.ready.notified().await;
            insert_winner(&mut f.admin, &mismatch_collision.winner).await?;
            mismatch_collision.release.notify_one();
            let error = task.await?.unwrap_err();
            assert_eq!(changed.calls.load(Ordering::SeqCst), 1);
            assert_eq!(f.effects().await, effects + 1);
            json!({"receipt_collisions":cmd.calls.load(Ordering::SeqCst),"recovery_execute_calls":cmd.calls.load(Ordering::SeqCst)-1,"effects":effects,"winner_replayed":result.replayed,"mismatch_code":error.code.as_str()})
        }

        "response" => {
            let mut staged = Vec::new();
            let mut calls = 0;
            for keyed in [true, false] {
                let mut cmd = f.command();
                if !keyed {
                    cmd.key = None;
                }
                cmd.fail_response = true;
                let error = run(f.pool.clone(), f.actor, cmd.clone()).await.unwrap_err();
                assert_eq!(error.code.as_str(), "VALIDATION_FAILED");
                staged.push(cmd.staged.load(Ordering::SeqCst));
                calls += cmd.calls.load(Ordering::SeqCst);
            }
            let failed_effects = f.effects().await;
            let failed_receipts: i64 =
                sqlx::query_scalar("SELECT count(*) FROM market_command_results")
                    .fetch_one(&f.pool)
                    .await?;
            let mut postcommit_queries = 0;
            for keyed in [true, false] {
                let mut cmd = f.command();
                if !keyed {
                    cmd.key = None;
                }
                let upstream = url::Url::parse(&f.url)?.port().unwrap();
                let proxy = Proxy::start(upstream, false).await?;
                let mut target = url::Url::parse(&f.url)?;
                target.set_port(Some(proxy.port)).unwrap();
                target.query_pairs_mut().append_pair("sslmode", "disable");
                let pool = Fixture::pool(target.as_str(), &f.schema, 1).await?;
                run(pool.clone(), f.actor, cmd.clone()).await?;
                pool.close().await;
                proxy.task.await?;
                postcommit_queries += proxy.after_commit_queries.load(Ordering::SeqCst);
                calls += cmd.calls.load(Ordering::SeqCst);
            }
            json!({"keyed_staged":staged[0],"unkeyed_staged":staged[1],"failed_effects":failed_effects,"failed_receipts":failed_receipts,"committed_effects":f.effects().await,"queries_after_commit":postcommit_queries,"execution_calls":calls})
        }
        "transport" => transport_case(&f).await?,
        "server_abort" => {
            let mut counts = Vec::new();
            for state in ["40001", "40P01"] {
                let mut cmd = f.command();
                cmd.abort = Some(state);
                run(f.pool.clone(), f.actor, cmd.clone()).await?;
                counts.push(cmd.calls.load(Ordering::SeqCst));
            }
            let mut cmd = f.command();
            cmd.abort = Some("40001");
            cmd.always_abort = true;
            let error = run(f.pool.clone(), f.actor, cmd).await.unwrap_err();
            json!({"serialization_attempts":counts[0],"deadlock_attempts":counts[1],"effects":f.effects().await,"exhausted_code":error.code.as_str()})
        }
        _ => panic!("unknown ordered receipt case"),
    };
    f.finish().await;
    Ok(evidence)
}
async fn insert_winner(conn: &mut PgConnection, r: &ReceiptRecord) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    sqlx::query(
        "INSERT INTO market_event_outbox(id,event_type,payload) VALUES($1,'receipt.winner','{}')",
    )
    .bind(Uuid::new_v4())
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body) VALUES($1,$2,$3,$4,$5,$6,'trade',$7,200,$8::jsonb)")
        .bind(r.id).bind(r.actor_user_id).bind(r.effective_organization_id).bind(r.operation.as_str()).bind(&r.idempotency_key).bind(&r.request_hash).bind(r.resource_id).bind(r.response_body.to_string()).execute(&mut *tx).await?;
    tx.commit().await
}

impl LockedMarketTransaction<'_> {
    async fn ordered_effect(&mut self, trade: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO market_event_outbox(id,event_type,payload) VALUES($1,'receipt.ordered',$2::json)").bind(Uuid::new_v4()).bind(json!({"trade":trade}).to_string()).execute(&mut *self.conn).await?;
        Ok(())
    }
    async fn ordered_abort(&mut self, state: &str) -> Result<(), sqlx::Error> {
        sqlx::query(include_str!(
            "../../tests/fixtures/phase8_ordered_abort.sql"
        ))
        .bind(Uuid::new_v4())
        .bind(state)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }
}

struct Proxy {
    port: u16,
    commits: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    done: Arc<Notify>,
    after_commit_queries: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Proxy {
    async fn start(upstream: u16, drop_ack: bool) -> Result<Self, Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let commits = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let done = Arc::new(Notify::new());
        let after_commit_queries = Arc::new(AtomicUsize::new(0));
        let query_count = after_commit_queries.clone();
        let committed = Arc::new(AtomicBool::new(false));
        let committed_reader = committed.clone();
        let c = commits.clone();
        let close = closed.clone();
        let finished = done.clone();
        let task = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = tokio::net::TcpStream::connect(("127.0.0.1", upstream))
                .await
                .unwrap();
            let len = client.read_u32().await.unwrap();
            let mut startup = vec![0; len as usize - 4];
            client.read_exact(&mut startup).await.unwrap();
            server.write_u32(len).await.unwrap();
            server.write_all(&startup).await.unwrap();
            let (mut cr, mut cw) = client.into_split();
            let (mut sr, mut sw) = server.into_split();
            let armed = Arc::new(AtomicBool::new(false));
            let send_armed = armed.clone();
            let sending = tokio::spawn(async move {
                loop {
                    let kind = cr.read_u8().await?;
                    if committed_reader.load(Ordering::SeqCst) && matches!(kind, b'Q' | b'P' | b'E')
                    {
                        query_count.fetch_add(1, Ordering::SeqCst);
                    }
                    let len = cr.read_u32().await?;
                    let mut bytes = vec![0; len as usize - 4];
                    cr.read_exact(&mut bytes).await?;
                    if kind == b'Q'
                        && bytes
                            .strip_suffix(&[0])
                            .is_some_and(|sql| sql.eq_ignore_ascii_case(b"COMMIT"))
                    {
                        send_armed.store(true, Ordering::SeqCst);
                        c.fetch_add(1, Ordering::SeqCst);
                    }
                    sw.write_u8(kind).await?;
                    sw.write_u32(len).await?;
                    sw.write_all(&bytes).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), std::io::Error>(())
            });
            while let Ok(kind) = sr.read_u8().await {
                let len = sr.read_u32().await.unwrap();
                let mut bytes = vec![0; len as usize - 4];
                sr.read_exact(&mut bytes).await.unwrap();
                if armed.load(Ordering::SeqCst) && drop_ack {
                    // Consume the server's actual completion, but sever the client
                    // transport before any COMMIT response reaches SQLx.
                    if kind == b'Z' {
                        close.store(true, Ordering::SeqCst);
                        cw.shutdown().await.unwrap();
                        finished.notify_one();
                        break;
                    }
                } else if cw.write_u8(kind).await.is_err()
                    || cw.write_u32(len).await.is_err()
                    || cw.write_all(&bytes).await.is_err()
                {
                    break;
                }
                if armed.load(Ordering::SeqCst) && kind == b'Z' {
                    committed.store(true, Ordering::SeqCst);
                }
            }
            sending.abort();
            let _ = sending.await;
        });
        Ok(Self {
            port,
            commits,
            closed,
            done,
            after_commit_queries,
            task,
        })
    }
}
async fn transport_case(f: &Fixture) -> Result<Value, Error> {
    let upstream = url::Url::parse(&f.url)?
        .port()
        .ok_or("explicit disposable port required")?;
    let mut results = Vec::new();
    let mut keyed_effects = 0;
    for keyed in [true, false] {
        let proxy = Proxy::start(upstream, true).await?;
        let mut target = url::Url::parse(&f.url)?;
        target.set_port(Some(proxy.port)).unwrap();
        target.query_pairs_mut().append_pair("sslmode", "disable");
        let pool = Fixture::pool(target.as_str(), &f.schema, 1).await?;
        let mut cmd = f.command();
        if !keyed {
            cmd.key = None;
        }
        let result = run(pool.clone(), f.actor, cmd.clone()).await.unwrap_err();
        tokio::time::timeout(Duration::from_secs(10), proxy.done.notified()).await?;
        proxy.task.await?;
        let discarded = pool.size() == 0;
        let mut replayed = false;
        if keyed {
            replayed = run(f.pool.clone(), f.actor, cmd.clone()).await?.replayed;
        }
        if keyed {
            keyed_effects = f.effects().await;
            assert_eq!(keyed_effects, 1);
        } else {
            assert_eq!(f.effects().await, keyed_effects + 1);
        }
        results.push((
            result,
            cmd.calls.load(Ordering::SeqCst),
            proxy.commits.load(Ordering::SeqCst),
            proxy.closed.load(Ordering::SeqCst),
            discarded,
            replayed,
        ));
        pool.close().await;
    }
    Ok(
        json!({"commit_frames_forwarded":results[0].2,"socket_closed_before_ack":results[0].3,"status":results[0].0.status.as_u16(),"code":results[0].0.code.as_str(),"execution_calls_before_retry":results[0].1,"discarded_connection":results[0].4,"retry_replayed":results[0].5,"effects":keyed_effects,"unkeyed_status":results[1].0.status.as_u16(),"unkeyed_execution_calls":results[1].1}),
    )
}
