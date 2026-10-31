use super::super::command::{Canonical, LockPlan};
use super::super::locks::{IDEMPOTENCY_KEY_BOUNDS, SliceKey};
use super::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("test uuid")
}

#[derive(Default)]
struct Recording {
    calls: Vec<String>,
    slice_keys: Vec<i64>,
    order_rows: Vec<Uuid>,
    users: Vec<Uuid>,
    organizations: Vec<Uuid>,
    events: Vec<String>,
    receipts: Vec<String>,
    commits: usize,
    rollbacks: usize,
}

#[derive(Default)]
struct Double {
    recording: Arc<Mutex<Recording>>,
    receipt: Option<Receipt>,
    order_row_present: bool,
    failures: BTreeMap<String, WriteError>,
}

impl Double {
    fn record(&self, call: &str) {
        self.recording.lock().unwrap().calls.push(call.to_owned());
    }

    fn failure(&self, call: &str) -> Result<(), WriteError> {
        match self.failures.get(call) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn ok(recording: &Arc<Mutex<Recording>>) -> Self {
        Self {
            recording: recording.clone(),
            order_row_present: true,
            ..Self::default()
        }
    }

    fn failing(recording: &Arc<Mutex<Recording>>, call: &str, error: WriteError) -> Self {
        let mut double = Self::ok(recording);
        double.failures.insert(call.to_owned(), error);
        double
    }
}

impl WriteSession for Double {
    fn lock_bounds<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("bounds");
            self.failure("bounds")
        })
    }

    fn lock_idempotency<'a>(&'a mut self, _lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("idempotency");
            self.failure("idempotency")
        })
    }

    fn lock_slice<'a>(&'a mut self, lock_key: i64) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("slice");
            self.recording.lock().unwrap().slice_keys.push(lock_key);
            self.failure("slice")
        })
    }

    fn load_receipt<'a>(
        &'a mut self,
        _actor_user_id: Uuid,
        _operation: &'a str,
        _idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<Option<Receipt>, WriteError>> {
        Box::pin(async move {
            self.record("receipt");
            self.failure("receipt")?;
            Ok(self.receipt.clone())
        })
    }

    fn lock_order_row<'a>(&'a mut self, order_id: Uuid) -> BoxFuture<'a, Result<bool, WriteError>> {
        Box::pin(async move {
            self.record("order_row");
            self.recording.lock().unwrap().order_rows.push(order_id);
            self.failure("order_row")?;
            Ok(self.order_row_present)
        })
    }

    fn lock_actor_user<'a>(&'a mut self, user_id: Uuid) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("user");
            self.recording.lock().unwrap().users.push(user_id);
            self.failure("user")
        })
    }

    fn lock_organization<'a>(
        &'a mut self,
        organization_id: Uuid,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("organization");
            self.recording
                .lock()
                .unwrap()
                .organizations
                .push(organization_id);
            self.failure("organization")
        })
    }

    fn insert_outbox<'a>(
        &'a mut self,
        event: &'a OutboxEvent,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("outbox");
            self.recording
                .lock()
                .unwrap()
                .events
                .push(event.event_type.clone());
            self.failure("outbox")
        })
    }

    fn insert_receipt<'a>(
        &'a mut self,
        receipt: &'a ReceiptRecordView,
    ) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("receipt_insert");
            self.recording
                .lock()
                .unwrap()
                .receipts
                .push(receipt.idempotency_key.clone());
            self.failure("receipt_insert")
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("commit");
            self.failure("commit")?;
            self.recording.lock().unwrap().commits += 1;
            Ok(())
        })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), WriteError>> {
        Box::pin(async move {
            self.record("rollback");
            self.recording.lock().unwrap().rollbacks += 1;
            self.failure("rollback")
        })
    }
}

/// The receipt row the shell inserts. Aliased so the test reads as data.
type ReceiptRecordView = super::super::command::ReceiptRecord;

fn opener(doubles: Vec<Double>) -> impl FnMut() -> BoxFuture<'static, Result<Double, WriteError>> {
    let mut queue = VecDeque::from(doubles);
    move || {
        let next = queue.pop_front();
        Box::pin(async move { next.ok_or_else(WriteError::other) })
    }
}

fn command_context() -> CommandContext {
    CommandContext {
        actor_user_id: uuid("30000000-0000-4000-8000-0000000000c1"),
        effective_organization_id: Some(uuid("40000000-0000-4000-8000-0000000000d1")),
        support_context_id: None,
        idempotency_key: Some("retry-key".to_owned()),
    }
}

fn command() -> OrderCommand {
    OrderCommand {
        operation: Operation::OrderAmend,
        resource_id: uuid("10000000-0000-4000-8000-0000000000a1"),
        payload: Canonical::Object(BTreeMap::from([
            (
                "operation".to_owned(),
                Canonical::Text("order.amend".to_owned()),
            ),
            (
                "order_id".to_owned(),
                Canonical::Uuid(uuid("10000000-0000-4000-8000-0000000000a1")),
            ),
        ])),
        locks: LockPlan {
            slices: vec![
                SliceKey {
                    product_id: uuid("10000000-0000-4000-8000-0000000000a1"),
                    delivery_point_id: None,
                    availability_window: "2026-Q1".to_owned(),
                },
                SliceKey {
                    product_id: uuid("10000000-0000-4000-8000-0000000000a1"),
                    delivery_point_id: None,
                    availability_window: "SPOT".to_owned(),
                },
            ],
            order_rows: vec![
                uuid("10000000-0000-4000-8000-0000000000a2"),
                uuid("10000000-0000-4000-8000-0000000000a3"),
            ],
            actor_users: vec![
                uuid("30000000-0000-4000-8000-0000000000c2"),
                uuid("30000000-0000-4000-8000-0000000000c1"),
            ],
            organizations: vec![
                uuid("40000000-0000-4000-8000-0000000000d2"),
                uuid("40000000-0000-4000-8000-0000000000d1"),
            ],
        },
        events: vec![OutboxEvent {
            event_type: "ORDER_UPDATED".to_owned(),
            aggregate_type: "order".to_owned(),
            aggregate_id: "10000000-0000-4000-8000-0000000000a1".to_owned(),
            participant_org_ids: vec!["40000000-0000-4000-8000-0000000000d1".to_owned()],
            payload: serde_json::json!({"order_id": "10000000-0000-4000-8000-0000000000a1"}),
        }],
        response_status: 200,
        response_body: serde_json::json!({"id": "10000000-0000-4000-8000-0000000000a1"}),
    }
}

fn stored_receipt(context: &CommandContext, command: &OrderCommand) -> Receipt {
    Receipt {
        request_hash: command_request_hash(&command.payload),
        effective_organization_id: context.effective_organization_id.unwrap(),
        support_context_id: context.support_context_id,
        resource_type: command.operation.resource_type().to_owned(),
        resource_id: command.resource_id,
        response_status: 201,
        response_body: serde_json::json!({"id": "stored"}),
    }
}

#[tokio::test]
async fn the_boundary_takes_every_lock_in_the_reviewed_order() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let result = run_write_unit(
        opener(vec![Double::ok(&recording)]),
        command_context(),
        command(),
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        CommittedCommand {
            response_status: 200,
            response_body: serde_json::json!({"id": "10000000-0000-4000-8000-0000000000a1"}),
            replayed: false,
        }
    );
    let recorded = recording.lock().unwrap();
    assert_eq!(
        recorded.calls,
        vec![
            "bounds",
            "idempotency",
            "receipt",
            "slice",
            "slice",
            "order_row",
            "order_row",
            "user",
            "user",
            "organization",
            "organization",
            "outbox",
            "receipt_insert",
            "commit",
        ]
    );
    // Bounds are set before the first lock; the idempotency lock is the first.
    assert_eq!(recorded.commits, 1);
    assert_eq!(recorded.rollbacks, 0);
    assert_eq!(recorded.events, vec!["ORDER_UPDATED".to_owned()]);
    assert_eq!(recorded.receipts, vec!["retry-key".to_owned()]);
    // Sorted slice keys, then the declared row order, then ascending actor ids.
    let expected_slices = ordered_slice_lock_keys(&command().locks.slices).unwrap();
    assert_eq!(recorded.slice_keys, expected_slices);
    assert_eq!(
        recorded.order_rows,
        vec![
            uuid("10000000-0000-4000-8000-0000000000a2"),
            uuid("10000000-0000-4000-8000-0000000000a3"),
        ]
    );
    assert_eq!(
        recorded.users,
        vec![
            uuid("30000000-0000-4000-8000-0000000000c1"),
            uuid("30000000-0000-4000-8000-0000000000c2"),
        ]
    );
    assert_eq!(
        recorded.organizations,
        vec![
            uuid("40000000-0000-4000-8000-0000000000d1"),
            uuid("40000000-0000-4000-8000-0000000000d2"),
        ]
    );
}

#[tokio::test]
async fn a_serialization_failure_reruns_the_whole_command_once() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut first = Double::ok(&recording);
    first.failures.insert(
        "slice".to_owned(),
        WriteError {
            sqlstate: Some("40001".to_owned()),
            ambiguous_commit: false,
        },
    );
    let result = run_write_unit(
        opener(vec![first, Double::ok(&recording)]),
        command_context(),
        command(),
    )
    .await
    .unwrap();
    assert!(!result.replayed);
    let recorded = recording.lock().unwrap();
    assert_eq!(recorded.commits, 1);
    // Exactly one rollback per failed attempt.
    assert_eq!(recorded.rollbacks, 1);
    assert_eq!(
        recorded
            .calls
            .iter()
            .filter(|call| *call == "bounds")
            .count(),
        2,
        "the second attempt starts a fresh transaction"
    );
    assert_eq!(
        recorded.events,
        vec!["ORDER_UPDATED".to_owned()],
        "a rolled-back attempt emits nothing twice"
    );
}

#[tokio::test]
async fn the_attempt_bound_is_two_attempts() {
    assert_eq!(DEFAULT_MARKET_TRANSACTION_ATTEMPTS, 2);
    let recording = Arc::new(Mutex::new(Recording::default()));
    let retryable = || WriteError {
        sqlstate: Some("40P01".to_owned()),
        ambiguous_commit: false,
    };
    let mut first = Double::ok(&recording);
    first.failures.insert("commit".to_owned(), retryable());
    let mut second = Double::ok(&recording);
    second.failures.insert("commit".to_owned(), retryable());
    let error = run_write_unit(opener(vec![first, second]), command_context(), command())
        .await
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 503);
    assert_eq!(
        error.detail,
        serde_json::json!(SERIALIZATION_FAILED_MESSAGE)
    );
    let recorded = recording.lock().unwrap();
    assert_eq!(recorded.rollbacks, 2);
    assert_eq!(recorded.commits, 0);
}

#[tokio::test]
async fn each_contention_failure_keeps_its_own_refusal() {
    let cases: [(WriteError, &str, u16, &str); 4] = [
        (
            WriteError {
                sqlstate: Some("55P03".to_owned()),
                ambiguous_commit: false,
            },
            "slice",
            409,
            SLICE_BUSY_MESSAGE,
        ),
        (
            WriteError {
                sqlstate: Some("57014".to_owned()),
                ambiguous_commit: false,
            },
            "slice",
            503,
            SLICE_TIMEOUT_MESSAGE,
        ),
        (
            WriteError {
                sqlstate: Some("55P03".to_owned()),
                ambiguous_commit: false,
            },
            "idempotency",
            503,
            IDEMPOTENCY_BUSY_MESSAGE,
        ),
        (
            WriteError {
                sqlstate: Some("57014".to_owned()),
                ambiguous_commit: false,
            },
            "idempotency",
            503,
            IDEMPOTENCY_TIMEOUT_MESSAGE,
        ),
    ];
    for (error, call, status, detail) in cases {
        let recording = Arc::new(Mutex::new(Recording::default()));
        let refused = run_write_unit(
            opener(vec![Double::failing(&recording, call, error)]),
            command_context(),
            command(),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.status.as_u16(), status, "{call}");
        assert_eq!(refused.detail, serde_json::json!(detail), "{call}");
        let recorded = recording.lock().unwrap();
        assert_eq!(
            recorded.rollbacks, 1,
            "{call}: one rollback per failed attempt"
        );
        assert_eq!(recorded.commits, 0, "{call}");
        assert!(
            recorded.events.is_empty(),
            "{call}: a refused command emits nothing"
        );
    }
}

#[tokio::test]
async fn an_unmapped_failure_is_a_database_refusal() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let refused = run_write_unit(
        opener(vec![Double::failing(
            &recording,
            "outbox",
            WriteError {
                sqlstate: Some("23505".to_owned()),
                ambiguous_commit: false,
            },
        )]),
        command_context(),
        command(),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.status.as_u16(), 500);
    assert_eq!(
        refused.detail,
        serde_json::json!("Database operation failed.")
    );
}

#[tokio::test]
async fn a_failed_commit_is_ambiguous_and_never_rerun() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let refused = run_write_unit(
        opener(vec![Double::failing(
            &recording,
            "commit",
            WriteError::ambiguous(),
        )]),
        command_context(),
        command(),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.status.as_u16(), 500);
    let recorded = recording.lock().unwrap();
    assert_eq!(
        recorded.rollbacks, 0,
        "an ambiguous outcome is never rolled back"
    );
    assert_eq!(
        recorded
            .calls
            .iter()
            .filter(|call| *call == "bounds")
            .count(),
        1,
        "an ambiguous outcome is never replayed"
    );
}

#[tokio::test]
async fn a_replay_returns_the_stored_result_and_emits_nothing() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let context = command_context();
    let command = command();
    let mut replay = Double::ok(&recording);
    replay.receipt = Some(stored_receipt(&context, &command));
    let result = run_write_unit(opener(vec![replay]), context, command)
        .await
        .unwrap();
    assert_eq!(
        result,
        CommittedCommand {
            response_status: 201,
            response_body: serde_json::json!({"id": "stored"}),
            replayed: true,
        }
    );
    let recorded = recording.lock().unwrap();
    assert_eq!(
        recorded.calls,
        vec!["bounds", "idempotency", "receipt", "rollback"]
    );
    assert!(recorded.events.is_empty());
    assert!(recorded.receipts.is_empty());
    assert_eq!(recorded.commits, 0);
}

#[tokio::test]
async fn a_reused_key_with_a_different_payload_is_refused() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let context = command_context();
    let command = command();
    let mut different = stored_receipt(&context, &command);
    different.request_hash = "0".repeat(64);
    let mut replay = Double::ok(&recording);
    replay.receipt = Some(different);
    let refused = run_write_unit(opener(vec![replay]), context, command)
        .await
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(refused.detail, serde_json::json!(REPLAY_CONFLICT_MESSAGE));
    let recorded = recording.lock().unwrap();
    assert_eq!(recorded.rollbacks, 1);
    assert!(recorded.events.is_empty());
}

#[tokio::test]
async fn a_missing_order_row_is_refused_before_anything_is_written() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut missing = Double::ok(&recording);
    missing.order_row_present = false;
    let refused = run_write_unit(opener(vec![missing]), command_context(), command())
        .await
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 404);
    assert_eq!(refused.detail, serde_json::json!(ORDER_NOT_FOUND_MESSAGE));
    let recorded = recording.lock().unwrap();
    assert!(recorded.events.is_empty());
    assert!(recorded.receipts.is_empty());
    assert_eq!(recorded.rollbacks, 1);
}

#[tokio::test]
async fn a_command_without_an_organization_is_refused() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut context = command_context();
    context.effective_organization_id = None;
    let refused = run_write_unit(opener(vec![Double::ok(&recording)]), context, command())
        .await
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 403);
    assert_eq!(
        refused.detail,
        serde_json::json!(MARKET_ORGANIZATION_REQUIRED)
    );
    let recorded = recording.lock().unwrap();
    assert!(recorded.calls.is_empty(), "no transaction is opened");
}

#[tokio::test]
async fn a_command_without_a_key_takes_no_receipt_and_still_emits_events() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut context = command_context();
    context.idempotency_key = None;
    run_write_unit(opener(vec![Double::ok(&recording)]), context, command())
        .await
        .unwrap();
    let recorded = recording.lock().unwrap();
    assert!(!recorded.calls.contains(&"idempotency".to_owned()));
    assert!(!recorded.calls.contains(&"receipt".to_owned()));
    assert_eq!(recorded.events, vec!["ORDER_UPDATED".to_owned()]);
    assert!(recorded.receipts.is_empty());
    assert_eq!(recorded.commits, 1);
}

#[tokio::test]
async fn a_blank_key_is_refused_before_the_transaction_opens() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut context = command_context();
    context.idempotency_key = Some("   ".to_owned());
    let refused = run_write_unit(opener(vec![Double::ok(&recording)]), context, command())
        .await
        .unwrap_err();
    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(refused.detail, serde_json::json!(IDEMPOTENCY_KEY_BOUNDS));
    assert!(recording.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn an_invalid_slice_window_is_refused_before_the_transaction_opens() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut command = command();
    command.locks.slices[0].availability_window = "2026-13".to_owned();
    let refused = run_write_unit(
        opener(vec![Double::ok(&recording)]),
        command_context(),
        command,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.status.as_u16(), 422);
    assert!(recording.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn every_attempt_opens_its_own_session() {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut first = Double::ok(&recording);
    first.failures.insert(
        "order_row".to_owned(),
        WriteError {
            sqlstate: Some("40P01".to_owned()),
            ambiguous_commit: false,
        },
    );
    run_write_unit(
        opener(vec![first, Double::ok(&recording)]),
        command_context(),
        command(),
    )
    .await
    .unwrap();
    let recorded = recording.lock().unwrap();
    // The failed attempt records its first lock; the fresh attempt restarts the plan.
    assert_eq!(
        recorded.order_rows,
        vec![
            uuid("10000000-0000-4000-8000-0000000000a2"),
            uuid("10000000-0000-4000-8000-0000000000a2"),
            uuid("10000000-0000-4000-8000-0000000000a3"),
        ]
    );
}
