//! Sequence assignment on one elected physical PostgreSQL session.
//!
//! The tests without a database run in every build of this target. The
//! database tests are ignored here: `tests/postgres/test_phase6_election.py`
//! runs them on the disposable PostgreSQL harness, with the deployed
//! application role, and requires every one of them to pass.

use std::{
    collections::{HashSet, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use futures_util::future::BoxFuture;
use sqlx::{Connection, PgConnection};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Notify, watch},
    time::{Instant, sleep, timeout},
};
use verdaxis_read::{
    Config, compiled_sha,
    streams::election::{
        ASSIGN_BATCH_LIMIT, ActorExit, AssignOutcome, ElectionError, ElectionSession,
        FailureBudget, MAX_CONSECUTIVE_FAILURES, NextStep, POLL_INTERVAL, RETRY_DELAY,
        SEQUENCER_LOCK_KEY, SessionSettings, Stage, StageHook, WAKE_CHANNEL, Wake, run_election,
    },
};

/// The shared key, written out here independently of the library constant.
const SHARED_KEY: i64 = 7_264_537_260_226_625;

/// Database tests share one advisory key and one outbox, so they never overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn shared_sequencer_key() {
    assert_eq!(SEQUENCER_LOCK_KEY, SHARED_KEY);
    assert_eq!(WAKE_CHANNEL, "verdaxis_market_events");
    assert_eq!(ASSIGN_BATCH_LIMIT, 500);
    assert_eq!(POLL_INTERVAL, Duration::from_secs(2));
    assert_eq!(RETRY_DELAY, Duration::from_secs(2));
    assert_eq!(MAX_CONSECUTIVE_FAILURES, 5);
}

#[test]
fn failure_budget_bounds_attempts() {
    let mut budget = FailureBudget::default();
    for _ in 0..4 {
        assert_eq!(
            budget.record_failure(ElectionError::Rejected),
            NextStep::RetryAfter(RETRY_DELAY)
        );
    }
    assert_eq!(
        budget.record_failure(ElectionError::SessionLost),
        NextStep::Stop { attempts: 5 }
    );

    let mut budget = FailureBudget::default();
    for _ in 0..4 {
        budget.record_failure(ElectionError::Connect);
    }
    budget.record_success();
    assert_eq!(
        budget.record_failure(ElectionError::Connect),
        NextStep::RetryAfter(RETRY_DELAY)
    );

    // Serialization failures and deadlocks get one immediate retry, then the
    // normal pause; they still count towards the limit.
    let mut budget = FailureBudget::default();
    let steps: Vec<NextStep> = (0..5)
        .map(|_| budget.record_failure(ElectionError::Retryable))
        .collect();
    assert_eq!(
        steps,
        [
            NextStep::RetryNow,
            NextStep::RetryAfter(RETRY_DELAY),
            NextStep::RetryNow,
            NextStep::RetryAfter(RETRY_DELAY),
            NextStep::Stop { attempts: 5 },
        ]
    );
}

#[test]
fn session_settings_reject_unsafe_targets() {
    for url in [
        "postgresql://fixture:fixture-secret@10.0.0.5:5432/verdaxis_test",
        "postgresql://fixture:fixture-secret@127.0.0.1:5432/verdaxis_live",
        "postgresql://fixture:fixture-secret@127.0.0.1:5432/verdaxis_test?sslmode=disable",
        "postgresql://fixture@127.0.0.1:5432/verdaxis_test",
    ] {
        assert_eq!(
            SessionSettings::from_config(&config_for(url)).err(),
            Some(ElectionError::Configuration),
            "{url}"
        );
    }
    let mut config = config_for("postgresql://fixture:fixture-secret@127.0.0.1:5432/verdaxis_test");
    assert!(SessionSettings::from_config(&config).is_ok());
    config.db_session_time_zone = "+08:00".into();
    assert_eq!(
        SessionSettings::from_config(&config).err(),
        Some(ElectionError::Configuration)
    );
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn python_lock_blocks_assignment() {
    let mut db = Database::open().await;
    let marker = "python_lock_blocks_assignment";
    // Under the harness's Python mode the real Python dispatcher already
    // holds the key; otherwise a plain session takes it.
    let python_holds_key = std::env::var("STREAM_ELECTION_LOCK_HOLDER").as_deref() == Ok("python");
    let holder = if python_holds_key {
        None
    } else {
        drain(&db.settings()).await;
        let mut connection = connect(&db.url).await;
        hold_key(&mut connection).await;
        Some(connection)
    };
    insert_rows(&mut db.observer, marker, 2).await;
    let mut session = open(&db.settings()).await;
    for _ in 0..3 {
        assert_eq!(
            session.assign_one_batch().await,
            Ok(AssignOutcome::NotElected)
        );
    }
    assert_eq!(pending(&mut db.observer, marker).await, 2);
    let holder_pid = lock_holder(&mut db.observer)
        .await
        .expect("the key stays held");
    assert_ne!(holder_pid, pid(session.backend_pid()));
    if let Some(mut connection) = holder {
        assert!(unlock(&mut connection).await);
        assert_eq!(
            session.assign_one_batch().await,
            Ok(AssignOutcome::Committed { assigned: 2 })
        );
        assert_eq!(pending(&mut db.observer, marker).await, 0);
        assert_eq!(lock_holder(&mut db.observer).await, None);
    }
    session.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn session_lock_count_balanced() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "session_lock_count_balanced";
    let probe = Arc::new(LockProbe {
        observer: tokio::sync::Mutex::new(connect(&db.url).await),
        seen: Mutex::new(Vec::new()),
    });
    let mut session = open(&db.settings().with_stage_hook(probe.clone())).await;
    let own = pid(session.backend_pid());
    for round in 0..3 {
        insert_rows(&mut db.observer, marker, 2).await;
        assert_eq!(
            session.assign_one_batch().await,
            Ok(AssignOutcome::Committed { assigned: 2 })
        );
        assert_eq!(
            lock_holder(&mut db.observer).await,
            None,
            "round {round} kept an acquisition"
        );
        assert_eq!(
            session.assign_one_batch().await,
            Ok(AssignOutcome::NothingPending)
        );
    }
    // The key was held by this session while each batch ran.
    let seen = probe.seen.lock().expect("probe record").clone();
    assert_eq!(seen.len(), 6);
    assert!(seen.iter().all(|holder| *holder == Some(own)), "{seen:?}");

    // A refused try-lock takes no acquisition.
    let mut contender = connect(&db.url).await;
    hold_key(&mut contender).await;
    insert_rows(&mut db.observer, marker, 1).await;
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::NotElected)
    );
    assert!(unlock(&mut contender).await);
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );
    assert_eq!(lock_holder(&mut db.observer).await, None);
    session.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn committed_rows_only() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let mut session = open(&db.settings()).await;
    let mut producer = connect(&db.url).await;

    // A producer's uncommitted row is invisible to the sequencer.
    let mut late = producer.begin().await.expect("producer transaction");
    insert_rows(&mut late, "committed_rows_only_late", 1).await;
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::NothingPending)
    );
    insert_rows(&mut db.observer, "committed_rows_only_early", 2).await;
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 2 })
    );
    late.commit().await.expect("producer commit");
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );
    let early = assigned(&mut db.observer, "committed_rows_only_early").await;
    let late = assigned(&mut db.observer, "committed_rows_only_late").await;
    assert_eq!((early.len(), late.len()), (2, 1));
    assert!(
        early.iter().all(|sequence| *sequence < late[0]),
        "a late commit must receive a later sequence: {early:?} {late:?}"
    );

    // A rolled-back producer row never receives a sequence.
    let mut rolled_back = producer.begin().await.expect("producer transaction");
    insert_rows(&mut rolled_back, "committed_rows_only_rolled_back", 1).await;
    rolled_back.rollback().await.expect("producer rollback");
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::NothingPending)
    );
    assert!(
        assigned(&mut db.observer, "committed_rows_only_rolled_back")
            .await
            .is_empty()
    );
    session.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn lock_and_assignment_share_backend() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "lock_and_assignment_share_backend";
    insert_rows(&mut db.observer, marker, 2).await;
    let probe = Arc::new(BackendProbe {
        marker,
        observer: tokio::sync::Mutex::new(connect(&db.url).await),
        seen: Mutex::new(None),
    });
    let mut session = open(&db.settings().with_stage_hook(probe.clone())).await;
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 2 })
    );
    let seen = probe
        .seen
        .lock()
        .expect("probe record")
        .clone()
        .expect("the probe ran inside the batch");
    let own = pid(session.backend_pid());
    assert_eq!(seen.statement_backend, own);
    assert_eq!(seen.hook_backend, own);
    assert_eq!(seen.lock_holder, Some(own));
    assert!(seen.lock_held_by_statement_backend);
    // The assignment is visible inside the session's open transaction only.
    assert_eq!(seen.assigned_in_session, 2);
    assert_eq!(seen.assigned_elsewhere, 0);
    assert_eq!(assigned(&mut db.observer, marker).await.len(), 2);
    assert_eq!(lock_holder(&mut db.observer).await, None);
    session.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn full_transaction_retry() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "full_transaction_retry";
    insert_rows(&mut db.observer, marker, 3).await;
    let faults = FaultPlan::new([Fault::Serialization, Fault::Deadlock], None);
    let mut session = open(&db.settings().with_stage_hook(faults.clone())).await;
    for _ in 0..2 {
        assert_eq!(
            session.assign_one_batch().await,
            Err(ElectionError::Retryable)
        );
        // The whole batch rolled back and its lock acquisition was released.
        assert_eq!(pending(&mut db.observer, marker).await, 3);
        assert_eq!(lock_holder(&mut db.observer).await, None);
    }
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 3 })
    );
    assert_eq!(delivery_attempts(&mut db.observer, marker).await, [1, 1, 1]);
    let sequences = assigned(&mut db.observer, marker).await;
    assert_eq!(sequences.iter().collect::<HashSet<_>>().len(), 3);
    // Every attempt ran on the same clean session, elected afresh each time.
    assert_eq!(faults.backends(), vec![session.backend_pid(); 3]);
    session.close().await;

    // The actor retries once at once, then pauses before the next attempt.
    let marker = "full_transaction_retry_actor";
    insert_rows(&mut db.observer, marker, 2).await;
    let faults = FaultPlan::new(
        [Fault::Serialization, Fault::Serialization, Fault::Deadlock],
        None,
    );
    let hub_wake = Arc::new(Notify::new());
    let (stop, shutdown) = watch::channel(false);
    let actor = tokio::spawn(run_election(
        db.settings().with_stage_hook(faults.clone()),
        hub_wake.clone(),
        shutdown,
    ));
    wait_for_pending(&mut db.observer, marker, 0).await;
    timeout(Duration::from_secs(5), hub_wake.notified())
        .await
        .expect("the hub is woken after the commit");
    stop.send(true).expect("actor listens");
    let exit = timeout(Duration::from_secs(5), actor)
        .await
        .expect("bounded shutdown")
        .expect("actor task");
    assert_eq!(exit, ActorExit::Shutdown);
    let attempts = faults.attempts();
    assert_eq!(attempts.len(), 4);
    assert!(
        attempts
            .iter()
            .all(|(backend, _)| *backend == attempts[0].0)
    );
    assert!(attempts[2].1 - attempts[1].1 >= RETRY_DELAY);
    assert_eq!(delivery_attempts(&mut db.observer, marker).await, [1, 1]);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn unlock_false_discards_session() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "unlock_false_discards_session";
    insert_rows(&mut db.observer, marker, 1).await;
    let mut session = open(&db.settings().with_stage_hook(Arc::new(ReleaseEarly))).await;
    let backend = session.backend_pid();
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::UnlockFailed)
    );
    // The assignment committed before the release was refused.
    assert_eq!(assigned(&mut db.observer, marker).await.len(), 1);
    // The session does no further work, even after its connection is gone.
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    session.close().await;
    wait_until_gone(&mut db.observer, backend).await;
    wait_for_election_backends(&mut db.observer, &[]).await;
    assert_eq!(lock_holder(&mut db.observer).await, None);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn unlock_error_discards_session() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "unlock_error_discards_session";
    insert_rows(&mut db.observer, marker, 1).await;
    let mut session = open(&db.settings().with_stage_hook(Arc::new(AbortBeforeRelease))).await;
    let backend = session.backend_pid();
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::UnlockFailed)
    );
    assert_eq!(assigned(&mut db.observer, marker).await.len(), 1);
    // The failed release left the acquisition held; only closing the session
    // releases it.
    assert_eq!(lock_holder(&mut db.observer).await, Some(pid(backend)));
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    session.close().await;
    wait_until_gone(&mut db.observer, backend).await;
    assert_eq!(lock_holder(&mut db.observer).await, None);
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn repeated_assignment_failure_stops_spinning() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "repeated_assignment_failure";
    insert_rows(&mut db.observer, marker, 1).await;

    // Every attempt reaches the assignment and the server refuses it.
    let faults = FaultPlan::new([], Some(Fault::Refusal));
    let (_stop, shutdown) = watch::channel(false);
    let started = Instant::now();
    let exit = timeout(
        Duration::from_secs(60),
        run_election(
            db.settings().with_stage_hook(faults.clone()),
            Arc::new(Notify::new()),
            shutdown,
        ),
    )
    .await
    .expect("the actor stops by itself");
    assert_eq!(
        exit,
        ActorExit::Exhausted {
            attempts: MAX_CONSECUTIVE_FAILURES
        }
    );
    // Four pauses separate the five attempts: the actor does not spin.
    assert!(started.elapsed() >= RETRY_DELAY * 4);
    let backends = faults.backends();
    assert_eq!(backends.len(), 5);
    assert_eq!(
        backends.iter().collect::<HashSet<_>>().len(),
        5,
        "a failed session was used again: {backends:?}"
    );
    for backend in backends {
        wait_until_gone(&mut db.observer, backend).await;
    }
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(pending(&mut db.observer, marker).await, 1);

    // Shutdown ends the pause after a failure promptly.
    let faults = FaultPlan::new([], Some(Fault::Refusal));
    let (stop, shutdown) = watch::channel(false);
    let actor = tokio::spawn(run_election(
        db.settings().with_stage_hook(faults.clone()),
        Arc::new(Notify::new()),
        shutdown,
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while faults.backends().is_empty() {
        assert!(Instant::now() < deadline, "the actor made no attempt");
        sleep(Duration::from_millis(20)).await;
    }
    wait_until_gone(&mut db.observer, faults.backends()[0]).await;
    stop.send(true).expect("actor listens");
    let exit = timeout(Duration::from_secs(1), actor)
        .await
        .expect("shutdown interrupts the pause")
        .expect("actor task");
    assert_eq!(exit, ActorExit::Shutdown);
    for backend in faults.backends() {
        wait_until_gone(&mut db.observer, backend).await;
    }
    assert_eq!(lock_holder(&mut db.observer).await, None);
    drain(&db.settings()).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn session_replacement_requires_election() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "session_replacement_requires_election";

    // A connection lost during the wait is reported at once.
    let mut lost = open(&db.settings()).await;
    let lost_backend = lost.backend_pid();
    let (waited, ()) = tokio::join!(
        timeout(
            Duration::from_secs(10),
            lost.wait_for_wake(Duration::from_secs(60))
        ),
        async {
            sleep(Duration::from_millis(100)).await;
            terminate(&mut db.observer, lost_backend).await;
        },
    );
    assert_eq!(
        waited.expect("the loss ends the wait"),
        Err(ElectionError::SessionLost)
    );
    wait_until_gone(&mut db.observer, lost_backend).await;

    // While no Rust session lives, another contender takes the key.
    let mut contender = connect(&db.url).await;
    hold_key(&mut contender).await;
    insert_rows(&mut db.observer, marker, 1).await;
    // The lost session neither reconnects nor assigns.
    assert_eq!(
        lost.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    wait_for_election_backends(&mut db.observer, &[]).await;
    lost.close().await;

    // The replacement is a new attested session that must win the key itself.
    let mut replacement = open(&db.settings()).await;
    assert_ne!(replacement.backend_pid(), lost_backend);
    assert_eq!(
        replacement.assign_one_batch().await,
        Ok(AssignOutcome::NotElected)
    );
    assert!(unlock(&mut contender).await);
    assert_eq!(
        replacement.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );

    // A connection lost after the lock and before the assignment assigns
    // nothing, and its acquisition ends with the session.
    let killer = Arc::new(TerminateAt {
        stage: Stage::Elected,
        observer: tokio::sync::Mutex::new(connect(&db.url).await),
    });
    insert_rows(&mut db.observer, marker, 1).await;
    let mut doomed = open(&db.settings().with_stage_hook(killer)).await;
    let doomed_backend = doomed.backend_pid();
    assert_eq!(
        doomed.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    assert_eq!(pending(&mut db.observer, marker).await, 1);
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(
        doomed.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    wait_for_election_backends(&mut db.observer, &[pid(replacement.backend_pid())]).await;
    doomed.close().await;
    assert_ne!(doomed_backend, replacement.backend_pid());
    assert_eq!(
        replacement.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );
    replacement.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn unknown_commit_outcome() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "unknown_commit_outcome";
    insert_rows(&mut db.observer, marker, 2).await;

    // The commit reaches the server, but its reply is lost with the connection.
    let relay = Relay::start(upstream(&db.url)).await;
    let cut = Arc::new(RelayModeAt {
        stage: Stage::Assigned,
        mode: relay.mode.clone(),
        value: LOSE_REPLY,
    });
    let mut session = open(&settings_for(&relay.url(&db.url)).with_stage_hook(cut)).await;
    let backend = session.backend_pid();
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    wait_until_gone(&mut db.observer, backend).await;
    let committed = assigned(&mut db.observer, marker).await;
    assert_eq!(committed.len(), 2, "the server committed the batch");
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    session.close().await;
    relay.close();

    // A new session neither sequences those rows again nor skips new ones.
    insert_rows(&mut db.observer, "unknown_commit_outcome_after", 1).await;
    let mut next = open(&db.settings()).await;
    assert_eq!(
        next.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );
    assert_eq!(assigned(&mut db.observer, marker).await, committed);
    assert_eq!(delivery_attempts(&mut db.observer, marker).await, [1, 1]);
    let after = assigned(&mut db.observer, "unknown_commit_outcome_after").await;
    assert!(committed.iter().all(|sequence| *sequence < after[0]));
    next.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn connection_deadlines() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;

    // Every election session carries explicit server-side bounds.
    let probe = Arc::new(SettingsProbe::default());
    let mut session = open(&db.settings().with_stage_hook(probe.clone())).await;
    assert_eq!(
        probe.seen.lock().expect("probe record").clone(),
        ["5s", "500ms", "10s", "off", "UTC"]
    );

    // A blocked statement ends at the lock timeout.
    let marker = "connection_deadlines";
    insert_rows(&mut db.observer, marker, 1).await;
    let mut blocker = connect(&db.url).await;
    let mut blocking = blocker.begin().await.expect("blocking transaction");
    sqlx::query("LOCK TABLE market_event_outbox IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocking)
        .await
        .expect("block the outbox");
    let started = Instant::now();
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::Rejected)
    );
    assert!(started.elapsed() < Duration::from_secs(4));
    blocking.rollback().await.expect("release the outbox");
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    session.close().await;

    // A stalled network ends at the client deadline; the server never commits.
    let relay = Relay::start(upstream(&db.url)).await;
    let stall = Arc::new(RelayModeAt {
        stage: Stage::Assigned,
        mode: relay.mode.clone(),
        value: STALL,
    });
    let mut session = open(&settings_for(&relay.url(&db.url)).with_stage_hook(stall)).await;
    let backend = session.backend_pid();
    let started = Instant::now();
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(15),
        "{waited:?}"
    );
    relay.close();
    session.close().await;
    wait_until_gone(&mut db.observer, backend).await;
    assert_eq!(pending(&mut db.observer, marker).await, 1);
    assert_eq!(lock_holder(&mut db.observer).await, None);

    // Opening a session is bounded too.
    let relay = Relay::start(upstream(&db.url)).await;
    relay.mode.store(STALL, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(
        ElectionSession::open(&settings_for(&relay.url(&db.url)))
            .await
            .err(),
        Some(ElectionError::Connect)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    relay.close();
    drain(&db.settings()).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn session_close_releases_lock() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let marker = "session_close_releases_lock";
    insert_rows(&mut db.observer, marker, 1).await;

    // A batch abandoned while it holds the key.
    let hold = Arc::new(HoldAt::new(Stage::Elected));
    let mut session = open(&db.settings().with_stage_hook(hold.clone())).await;
    let backend = session.backend_pid();
    tokio::select! {
        outcome = session.assign_one_batch() => panic!("the batch finished: {outcome:?}"),
        () = hold.reached.notified() => {}
    }
    assert_eq!(lock_holder(&mut db.observer).await, Some(pid(backend)));
    assert_eq!(
        session.assign_one_batch().await,
        Err(ElectionError::SessionLost)
    );
    session.close().await;
    wait_until_gone(&mut db.observer, backend).await;
    assert_eq!(lock_holder(&mut db.observer).await, None);
    assert_eq!(pending(&mut db.observer, marker).await, 1);

    // Actor shutdown while a batch holds the key and its row locks.
    let hold = Arc::new(HoldAt::new(Stage::Assigned));
    let (stop, shutdown) = watch::channel(false);
    let actor = tokio::spawn(run_election(
        db.settings().with_stage_hook(hold.clone()),
        Arc::new(Notify::new()),
        shutdown,
    ));
    timeout(Duration::from_secs(10), hold.reached.notified())
        .await
        .expect("the actor reaches the assignment");
    let holder = lock_holder(&mut db.observer)
        .await
        .expect("the actor holds the key");
    stop.send(true).expect("actor listens");
    let exit = timeout(Duration::from_secs(5), actor)
        .await
        .expect("bounded shutdown")
        .expect("actor task");
    assert_eq!(exit, ActorExit::Shutdown);
    wait_until_gone(
        &mut db.observer,
        u32::try_from(holder).expect("backend process id"),
    )
    .await;
    assert_eq!(lock_holder(&mut db.observer).await, None);
    // The abandoned transaction rolled back.
    assert_eq!(pending(&mut db.observer, marker).await, 1);
    drain(&db.settings()).await;
}

#[tokio::test]
#[ignore = "requires the disposable PostgreSQL harness"]
async fn wake_follows_other_sessions() {
    let mut db = Database::open().await;
    drain(&db.settings()).await;
    let mut session = open(&db.settings()).await;
    // The session's own commit notice does not wake it again.
    insert_rows(&mut db.observer, "wake_follows_other_sessions", 1).await;
    assert_eq!(
        session.assign_one_batch().await,
        Ok(AssignOutcome::Committed { assigned: 1 })
    );
    assert_eq!(
        session.wait_for_wake(Duration::from_millis(500)).await,
        Ok(Wake::PollElapsed)
    );
    // Another session's notice ends the wait before the poll interval.
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(WAKE_CHANNEL)
        .execute(&mut db.observer)
        .await
        .expect("notify");
    let woken = timeout(
        Duration::from_secs(10),
        session.wait_for_wake(Duration::from_secs(60)),
    )
    .await
    .expect("woken before the poll interval");
    assert_eq!(woken, Ok(Wake::Notified));
    session.close().await;
}

// ---------------------------------------------------------------------------
// Fixture

struct Database {
    url: String,
    observer: PgConnection,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl Database {
    async fn open() -> Self {
        let serial = SERIAL.lock().await;
        let url = std::env::var("STREAM_ELECTION_APP_URL")
            .expect("run through tests/postgres/test_phase6_election.py");
        let observer = connect(&url).await;
        Self {
            url,
            observer,
            _serial: serial,
        }
    }

    fn settings(&self) -> SessionSettings {
        settings_for(&self.url)
    }
}

fn config_for(url: &str) -> Config {
    Config {
        kyc_execution_gate_enabled: true,
        environment: "test".into(),
        release_sha: compiled_sha().into(),
        migration_revision: "test".into(),
        database_url: url.into(),
        db_session_time_zone: "UTC".into(),
        prepared_budget: None,
        bind: "127.0.0.1:0".parse().expect("loopback bind"),
        read_admission_slots: 1,
        pool_max: 2,
        python_budget: 0,
        configured_max_connections: 100,
    }
}

fn settings_for(url: &str) -> SessionSettings {
    SessionSettings::from_config(&config_for(url)).expect("disposable database settings")
}

async fn open(settings: &SessionSettings) -> ElectionSession {
    ElectionSession::open(settings)
        .await
        .expect("open an election session")
}

async fn connect(url: &str) -> PgConnection {
    PgConnection::connect(url)
        .await
        .expect("fixture connection")
}

fn pid(backend: u32) -> i32 {
    i32::try_from(backend).expect("backend process id")
}

/// Sequence every pending row so a test starts from its own rows only.
async fn drain(settings: &SessionSettings) {
    let mut session = open(settings).await;
    loop {
        match session.assign_one_batch().await.expect("drain batch") {
            AssignOutcome::NothingPending => break,
            AssignOutcome::Committed { .. } => {}
            AssignOutcome::NotElected => panic!("the shared key is held outside the test"),
        }
    }
    session.close().await;
}

async fn insert_rows(connection: &mut PgConnection, marker: &str, count: i32) {
    sqlx::query(
        "INSERT INTO market_event_outbox \
         (id, event_type, aggregate_type, aggregate_id, participant_org_ids, payload) \
         SELECT gen_random_uuid(), 'fixture_event', 'phase6_election', $1, \
         json_build_array(gen_random_uuid()::text), json_build_object('schema_version', 1) \
         FROM generate_series(1, $2)",
    )
    .bind(marker)
    .bind(count)
    .execute(connection)
    .await
    .expect("insert fixture rows");
}

async fn pending(connection: &mut PgConnection, marker: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM market_event_outbox \
         WHERE aggregate_type = 'phase6_election' AND aggregate_id = $1 \
         AND stream_seq IS NULL",
    )
    .bind(marker)
    .fetch_one(connection)
    .await
    .expect("count pending rows")
}

async fn wait_for_pending(connection: &mut PgConnection, marker: &str, expected: i64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while pending(connection, marker).await != expected {
        assert!(
            Instant::now() < deadline,
            "{marker} did not reach {expected}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

/// Sequence values of the marker's assigned rows, ascending.
async fn assigned(connection: &mut PgConnection, marker: &str) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT stream_seq FROM market_event_outbox \
         WHERE aggregate_type = 'phase6_election' AND aggregate_id = $1 \
         AND stream_seq IS NOT NULL ORDER BY stream_seq",
    )
    .bind(marker)
    .fetch_all(connection)
    .await
    .expect("read assigned rows")
}

async fn delivery_attempts(connection: &mut PgConnection, marker: &str) -> Vec<i32> {
    sqlx::query_scalar(
        "SELECT delivery_attempts FROM market_event_outbox \
         WHERE aggregate_type = 'phase6_election' AND aggregate_id = $1 ORDER BY id",
    )
    .bind(marker)
    .fetch_all(connection)
    .await
    .expect("read delivery attempts")
}

/// Backend that holds the shared key, if any.
async fn lock_holder(connection: &mut PgConnection) -> Option<i32> {
    sqlx::query_scalar(
        "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND granted \
         AND pid IS NOT NULL AND objsubid = 1 \
         AND classid::bigint = ($1::bigint >> 32) \
         AND objid::bigint = ($1::bigint & 4294967295)",
    )
    .bind(SHARED_KEY)
    .fetch_optional(connection)
    .await
    .expect("read the key holder")
}

async fn hold_key(connection: &mut PgConnection) {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SHARED_KEY)
        .execute(connection)
        .await
        .expect("hold the shared key");
}

async fn unlock(connection: &mut PgConnection) -> bool {
    sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(SHARED_KEY)
        .fetch_one(connection)
        .await
        .expect("release the shared key")
}

/// Wait until exactly `expected` election sessions are open. A lost session
/// that reconnected on its own would keep this from ever matching.
async fn wait_for_election_backends(connection: &mut PgConnection, expected: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let open: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity \
             WHERE application_name = 'verdaxis-stream-election' ORDER BY pid",
        )
        .fetch_all(&mut *connection)
        .await
        .expect("read election sessions");
        if open == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "election sessions {open:?}, expected {expected:?}"
        );
        sleep(Duration::from_millis(50)).await;
    }
}

async fn terminate(connection: &mut PgConnection, backend: u32) {
    let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid(backend))
        .fetch_one(connection)
        .await
        .expect("terminate a session");
    assert!(terminated, "session {backend} was not running");
}

async fn wait_until_gone(connection: &mut PgConnection, backend: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let alive: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE pid = $1)")
                .bind(pid(backend))
                .fetch_one(&mut *connection)
                .await
                .expect("read sessions");
        if !alive {
            return;
        }
        assert!(Instant::now() < deadline, "session {backend} is still open");
        sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Stage hooks

/// Records the key holder seen from outside while each batch holds the key.
struct LockProbe {
    observer: tokio::sync::Mutex<PgConnection>,
    seen: Mutex<Vec<Option<i32>>>,
}

impl StageHook for LockProbe {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        _connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if matches!(stage, Stage::Elected | Stage::Committed) {
                let mut observer = self.observer.lock().await;
                let holder = lock_holder(&mut observer).await;
                self.seen.lock().expect("probe record").push(holder);
            }
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
struct BackendView {
    statement_backend: i32,
    hook_backend: i32,
    lock_held_by_statement_backend: bool,
    lock_holder: Option<i32>,
    assigned_in_session: i64,
    assigned_elsewhere: i64,
}

/// Looks at the open assignment from inside the session and from outside.
struct BackendProbe {
    marker: &'static str,
    observer: tokio::sync::Mutex<PgConnection>,
    seen: Mutex<Option<BackendView>>,
}

impl StageHook for BackendProbe {
    fn at<'a>(
        &'a self,
        stage: Stage,
        backend: u32,
        connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage != Stage::Assigned {
                return Ok(());
            }
            let (statement_backend, lock_held_by_statement_backend, assigned_in_session): (
                i32,
                bool,
                i64,
            ) = sqlx::query_as(
                "SELECT pg_backend_pid(), \
                 EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted \
                 AND pid = pg_backend_pid() AND objsubid = 1 \
                 AND classid::bigint = ($1::bigint >> 32) \
                 AND objid::bigint = ($1::bigint & 4294967295)), \
                 (SELECT count(*) FROM market_event_outbox \
                 WHERE aggregate_type = 'phase6_election' AND aggregate_id = $2 \
                 AND stream_seq IS NOT NULL)",
            )
            .bind(SHARED_KEY)
            .bind(self.marker)
            .fetch_one(&mut *connection)
            .await?;
            let mut observer = self.observer.lock().await;
            let holder = lock_holder(&mut observer).await;
            let assigned_elsewhere =
                i64::try_from(assigned(&mut observer, self.marker).await.len()).expect("row count");
            *self.seen.lock().expect("probe record") = Some(BackendView {
                statement_backend,
                hook_backend: pid(backend),
                lock_held_by_statement_backend,
                lock_holder: holder,
                assigned_in_session,
                assigned_elsewhere,
            });
            Ok(())
        })
    }
}

#[derive(Clone, Copy)]
enum Fault {
    Serialization,
    Deadlock,
    Refusal,
}

impl Fault {
    /// A server-raised error inside the assignment transaction.
    fn statement(self) -> &'static str {
        match self {
            Self::Serialization => {
                "DO $$BEGIN RAISE EXCEPTION USING ERRCODE = 'serialization_failure'; END$$"
            }
            Self::Deadlock => {
                "DO $$BEGIN RAISE EXCEPTION USING ERRCODE = 'deadlock_detected'; END$$"
            }
            Self::Refusal => "DO $$BEGIN RAISE EXCEPTION USING ERRCODE = 'raise_exception'; END$$",
        }
    }
}

/// Fails the next assignments as planned and records each attempt.
struct FaultPlan {
    planned: Mutex<VecDeque<Fault>>,
    after_plan: Option<Fault>,
    attempts: Mutex<Vec<(u32, Instant)>>,
}

impl FaultPlan {
    fn new<const N: usize>(planned: [Fault; N], after_plan: Option<Fault>) -> Arc<Self> {
        Arc::new(Self {
            planned: Mutex::new(planned.into()),
            after_plan,
            attempts: Mutex::new(Vec::new()),
        })
    }

    fn attempts(&self) -> Vec<(u32, Instant)> {
        self.attempts.lock().expect("attempt record").clone()
    }

    fn backends(&self) -> Vec<u32> {
        self.attempts()
            .into_iter()
            .map(|(backend, _)| backend)
            .collect()
    }
}

impl StageHook for FaultPlan {
    fn at<'a>(
        &'a self,
        stage: Stage,
        backend: u32,
        connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage != Stage::Assigned {
                return Ok(());
            }
            self.attempts
                .lock()
                .expect("attempt record")
                .push((backend, Instant::now()));
            let fault = self
                .planned
                .lock()
                .expect("fault plan")
                .pop_front()
                .or(self.after_plan);
            if let Some(fault) = fault {
                sqlx::raw_sql(fault.statement()).execute(connection).await?;
            }
            Ok(())
        })
    }
}

/// Releases the batch's acquisition before the session does.
struct ReleaseEarly;

impl StageHook for ReleaseEarly {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == Stage::Committed {
                let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
                    .bind(SHARED_KEY)
                    .fetch_one(connection)
                    .await?;
                assert!(released, "the batch held the key");
            }
            Ok(())
        })
    }
}

/// Leaves the session in a failed transaction, so the release statement fails.
struct AbortBeforeRelease;

impl StageHook for AbortBeforeRelease {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == Stage::Committed {
                sqlx::raw_sql("BEGIN").execute(&mut *connection).await?;
                let failed = sqlx::raw_sql("SELECT 1 / 0")
                    .execute(&mut *connection)
                    .await;
                assert!(failed.is_err(), "the transaction must be aborted");
            }
            Ok(())
        })
    }
}

/// Terminates the session's own backend at one stage.
struct TerminateAt {
    stage: Stage,
    observer: tokio::sync::Mutex<PgConnection>,
}

impl StageHook for TerminateAt {
    fn at<'a>(
        &'a self,
        stage: Stage,
        backend: u32,
        _connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == self.stage {
                let mut observer = self.observer.lock().await;
                terminate(&mut observer, backend).await;
                wait_until_gone(&mut observer, backend).await;
            }
            Ok(())
        })
    }
}

/// Never finishes one stage, so the test can abandon the batch there.
struct HoldAt {
    stage: Stage,
    reached: Notify,
}

impl HoldAt {
    fn new(stage: Stage) -> Self {
        Self {
            stage,
            reached: Notify::new(),
        }
    }
}

impl StageHook for HoldAt {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        _connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == self.stage {
                self.reached.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
}

/// Reads the session bounds when the session opens.
#[derive(Default)]
struct SettingsProbe {
    seen: Mutex<Vec<String>>,
}

impl StageHook for SettingsProbe {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == Stage::Opened {
                let (statement, lock, idle, read_only, time_zone): (
                    String,
                    String,
                    String,
                    String,
                    String,
                ) = sqlx::query_as(
                    "SELECT current_setting('statement_timeout'), \
                     current_setting('lock_timeout'), \
                     current_setting('idle_in_transaction_session_timeout'), \
                     current_setting('default_transaction_read_only'), \
                     current_setting('TimeZone')",
                )
                .fetch_one(connection)
                .await?;
                *self.seen.lock().expect("probe record") =
                    vec![statement, lock, idle, read_only, time_zone];
            }
            Ok(())
        })
    }
}

/// Switches the relay mode when the session reaches one stage.
struct RelayModeAt {
    stage: Stage,
    mode: Arc<AtomicU8>,
    value: u8,
}

impl StageHook for RelayModeAt {
    fn at<'a>(
        &'a self,
        stage: Stage,
        _backend: u32,
        _connection: &'a mut PgConnection,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            if stage == self.stage {
                self.mode.store(self.value, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// Loopback relay with injectable network faults

/// Forward bytes in both directions.
const FORWARD: u8 = 0;
/// Hold bytes in both directions: a network that stops delivering.
const STALL: u8 = 1;
/// Keep sending client bytes, then drop the next server reply and close both
/// connections: the server's work completes, but its answer is lost.
const LOSE_REPLY: u8 = 2;

struct Relay {
    address: SocketAddr,
    mode: Arc<AtomicU8>,
    stop: Arc<watch::Sender<bool>>,
}

impl Relay {
    async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("relay bind");
        let address = listener.local_addr().expect("relay address");
        let mode = Arc::new(AtomicU8::new(FORWARD));
        let stop = Arc::new(watch::channel(false).0);
        let (accept_mode, accept_stop) = (mode.clone(), stop.clone());
        tokio::spawn(async move {
            let mut stopped = accept_stop.subscribe();
            loop {
                let client = tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((client, _)) => client,
                        Err(_) => return,
                    },
                    _ = stopped.wait_for(|stop| *stop) => return,
                };
                let Ok(server) = TcpStream::connect(upstream).await else {
                    return;
                };
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                tokio::spawn(pump(
                    client_read,
                    server_write,
                    false,
                    accept_mode.clone(),
                    accept_stop.clone(),
                ));
                tokio::spawn(pump(
                    server_read,
                    client_write,
                    true,
                    accept_mode.clone(),
                    accept_stop.clone(),
                ));
            }
        });
        Self {
            address,
            mode,
            stop,
        }
    }

    fn url(&self, database_url: &str) -> String {
        let mut url = url::Url::parse(database_url).expect("database URL");
        url.set_port(Some(self.address.port())).expect("relay port");
        url.to_string()
    }

    /// Close every relayed connection and stop accepting new ones.
    fn close(&self) {
        self.stop.send_replace(true);
    }
}

async fn pump(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    server_reply: bool,
    mode: Arc<AtomicU8>,
    stop: Arc<watch::Sender<bool>>,
) {
    let mut stopped = stop.subscribe();
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = tokio::select! {
            read = from.read(&mut buffer) => read,
            _ = stopped.wait_for(|stop| *stop) => return,
        };
        let count = match read {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        loop {
            match mode.load(Ordering::SeqCst) {
                STALL => tokio::select! {
                    () = sleep(Duration::from_millis(10)) => {}
                    _ = stopped.wait_for(|stop| *stop) => return,
                },
                LOSE_REPLY if server_reply => {
                    stop.send_replace(true);
                    return;
                }
                _ => break,
            }
        }
        if to.write_all(&buffer[..count]).await.is_err() {
            return;
        }
    }
}

fn upstream(database_url: &str) -> SocketAddr {
    let url = url::Url::parse(database_url).expect("database URL");
    let host: IpAddr = url
        .host_str()
        .expect("database host")
        .trim_matches(['[', ']'])
        .parse()
        .expect("numeric loopback host");
    SocketAddr::new(host, url.port().unwrap_or(5432))
}
