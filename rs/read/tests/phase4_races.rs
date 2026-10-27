//! Integrated race and atomicity suite (single-stack part).
//!
//! Every race runs on independent connections against the real commands of the
//! business modules (account admission, organization and join review, KYC
//! submission and review, invitations, subscriptions) and the market
//! invalidation they call. A command is paused inside its transaction after the
//! ordered owner took every lock, a contender is started on its own connection,
//! and the suite waits until PostgreSQL reports the contender blocked before it
//! lets the first command continue. Nothing waits on a sleep. Each case runs in
//! both acquisition orders where two commands contend, with serialization
//! failures and deadlocks (40001 and 40P01) raised inside the transaction, and
//! covers success, retry exhaustion and domain refusal. Every failure asserts
//! that no stock, history, event, audit row, membership or session changed.
//!
//! Races against Python writers are kept only where a Python writer keeps
//! running after its area switches to Rust and touches the same rows as a Rust
//! command: the approval-mail retry timer and the authentication maintenance
//! command (both in the `auth_maintenance` job) and the trade-lifecycle
//! requests (until the market phases). Their statements are modelled in
//! `phase4_races/python_writers.rs`. Python routes that move with their area
//! have no coexistence proof here. Refresh rotation and invitation acceptance
//! are routes of the Phase 3 branch; the races against them use stand-ins that
//! take the same locks through the same store primitives.

#![cfg(feature = "contract-tests")]

#[path = "phase4_races/commands.rs"]
mod commands;
#[path = "phase4_races/python_writers.rs"]
mod python_writers;
#[path = "phase4_races/world.rs"]
mod world;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{Connection, QueryBuilder};
use tokio::sync::Semaphore;
use uuid::Uuid;
use verdaxis_read::{
    analytics::{
        AnalyticsActivity, AnalyticsAudience, AnalyticsQuery, StatusFacts,
        aggregates::load_status_facts,
    },
    auth::{AccountState, AuthError, TokenIssuance, cutoff_refusal, validate_account},
    business::{
        admission::RejectionRoute,
        approval_mail::{
            ApprovalDelivery, claim_approval_mail, deliver_account_approval_mail,
            finalize_approval_mail,
        },
        transaction::{
            CompletionControl, CompletionPoint, run_controlled_write, run_ordered_write,
        },
    },
    kyc::policy::{KycParty, KycPolicy, KycReason, KycSqlParty, bind_eligibility_sql, eligibility},
    order_write::market_slice_lock_key,
};

use commands::*;
use python_writers::{DueMail, Finalized};
use world::*;

/// One command's observed outcome, ignoring its payload.
async fn run_to_end<C: verdaxis_read::business::transaction::BusinessCommand>(
    world: &World,
    actor: Uuid,
    command: C,
) -> Verdict {
    finished(spawn_write(&world.pool, actor, command)).await
}

async fn row_json(world: &World, sql: &'static str, id: Uuid) -> Value {
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(&world.pool)
        .await
        .unwrap()
}

async fn audits_of(world: &World, action: &str, resource: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action=$1 AND resource_id=$2")
        .bind(action)
        .bind(resource.to_string())
        .fetch_one(&world.pool)
        .await
        .unwrap()
}

async fn status_of(world: &World, sql: &'static str, id: Uuid) -> String {
    world.scalar_text(sql, id).await.expect("a stored status")
}

async fn organization_status(world: &World, organization: Uuid) -> String {
    status_of(
        world,
        "SELECT verification_status FROM organizations WHERE id=$1",
        organization,
    )
    .await
}

async fn members_of(world: &World, organization: Uuid) -> i64 {
    world
        .count_for(
            "SELECT count(*) FROM users WHERE organization_id=$1",
            organization,
        )
        .await
}

async fn user_organization(world: &World, user: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap()
}

async fn kyc_of(world: &World, user: Uuid) -> (String, Option<Uuid>) {
    sqlx::query_as("SELECT kyc_status,kyc_organization_id FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap()
}

async fn order_status(world: &World, order: Uuid) -> String {
    status_of(
        world,
        "SELECT status FROM orderbook_orders WHERE id=$1",
        order,
    )
    .await
}

const TRADE_ROW: &str = "SELECT to_jsonb(t) FROM trades t WHERE id=$1";

async fn live_sessions(world: &World, user: Uuid) -> i64 {
    world
        .count_for(
            "SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked",
            user,
        )
        .await
}

async fn account_state(world: &World, user: Uuid) -> AccountState {
    let (status, must_change, changed, revoked): (
        String,
        bool,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
    ) = sqlx::query_as("SELECT status,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap();
    AccountState {
        exists: true,
        status,
        must_change_password: must_change,
        password_changed_at: changed.map(|at| at.timestamp_micros()),
        authentication_revoked_at: revoked.map(|at| at.timestamp_micros()),
    }
}

// ===========================================================================
// join_approval_and_org_rejection_do_not_deadlock
// ===========================================================================

/// Repetitions of each acquisition order.
const JOIN_REVIEW_REPETITIONS: usize = 50;

/// Join approval and organization rejection contend for the same organization
/// row. Whichever starts first pauses after its canonical locks; the other
/// command waits on its own connection and is observed blocked before the first
/// continues. In both orders neither deadlocks, a rejected organization never
/// keeps or gains a member, and the pre-existing trade, the stock and the audit
/// trail come out exactly once.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn join_approval_and_org_rejection_do_not_deadlock() {
    let mut world = World::new().await;
    for join_first in [true, false] {
        for _ in 0..JOIN_REVIEW_REPETITIONS {
            let organization = world.organization().await;
            let member = world.user(Some(organization)).await;
            world.bind_kyc(member, "APPROVED", Some(organization)).await;
            let applicant = world.user(None).await;
            let join = world.join_request(applicant, organization).await;
            let listing = world.stocked_listing(organization).await;
            let counterparty_org = world.organization().await;
            let counterparty = world.user(Some(counterparty_org)).await;
            let trade = world
                .trade(
                    (counterparty_org, counterparty),
                    (organization, member),
                    Some(listing.order),
                    "CONFIRMED",
                )
                .await;
            let trade_before = row_json(&world, TRADE_ROW, trade).await;

            let gate = Gate::closed();
            let (holder, holder_pid, contender) = if join_first {
                let holder = spawn_write(
                    &world.pool,
                    world.actor,
                    Gated::new(review_join(join, true), &gate),
                );
                let pid = gate.arrived().await;
                let contender = spawn_write(
                    &world.pool,
                    world.actor,
                    review_organization(&world, organization, false),
                );
                (holder, pid, contender)
            } else {
                let holder = spawn_write(
                    &world.pool,
                    world.actor,
                    Gated::new(review_organization(&world, organization, false), &gate),
                );
                let pid = gate.arrived().await;
                let contender = spawn_write(&world.pool, world.actor, review_join(join, true));
                (holder, pid, contender)
            };
            wait_blocked(&mut world.watcher, holder_pid, 1).await;
            gate.open();
            let holder = finished(holder).await;
            let contender = finished(contender).await;

            assert_eq!(organization_status(&world, organization).await, "REJECTED");
            assert_eq!(
                members_of(&world, organization).await,
                0,
                "a rejected organization kept or gained a member"
            );
            assert_eq!(user_organization(&world, applicant).await, None);
            assert_eq!(
                kyc_of(&world, member).await,
                ("PENDING".to_string(), None),
                "the member's review stayed bound to the rejected organization"
            );
            assert_eq!(
                row_json(&world, TRADE_ROW, trade).await,
                trade_before,
                "the rejection changed a pre-existing trade"
            );
            assert_eq!(order_status(&world, listing.order).await, "CANCELLED");
            assert_eq!(
                world.stock(listing.inventory).await,
                (Decimal::from(80), Decimal::from(20)),
                "only the uncommitted reservation is released"
            );
            assert_eq!(
                audits_of(&world, "admin.organization_rejected", organization).await,
                1
            );
            assert_eq!(
                audits_of(&world, "kyc.membership_invalidated", member).await,
                1
            );
            let invalidations: i64 = sqlx::query_scalar("SELECT count(*) FROM market_event_outbox WHERE event_type='market_access_invalidated' AND aggregate_id=$1")
                .bind(organization.to_string())
                .fetch_one(&world.pool)
                .await
                .unwrap();
            assert_eq!(invalidations, 1);
            if join_first {
                // The approval won the organization row; the rejection then saw
                // the new member, retried from a fresh transaction and unlinked it.
                holder.expect("join approval committed");
                contender.expect("organization rejection committed");
                assert_eq!(
                    status_of(
                        &world,
                        "SELECT status FROM organization_join_requests WHERE id=$1",
                        join
                    )
                    .await,
                    "APPROVED"
                );
                assert_eq!(
                    audits_of(&world, "organization.join_approved", join).await,
                    1
                );
            } else {
                // The rejection won; the approval refuses a rejected organization.
                holder.expect("organization rejection committed");
                assert_eq!(
                    refusal(&contender),
                    (409, "ORGANIZATION_JOIN_ORG_INELIGIBLE".into())
                );
                assert_eq!(
                    status_of(
                        &world,
                        "SELECT status FROM organization_join_requests WHERE id=$1",
                        join
                    )
                    .await,
                    "PENDING"
                );
                assert_eq!(
                    audits_of(&world, "organization.join_approved", join).await,
                    0
                );
            }
        }
    }
    world.finish().await;
}

// ===========================================================================
// kyc_submit_vs_membership_change_revalidates
// ===========================================================================

/// A KYC submission is parsed against the user's current organization. The
/// ordered owner re-reads membership under the canonical locks, so the outcome
/// is either the current binding stored or a 409 — never evidence bound to an
/// organization the user has left.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn kyc_submit_vs_membership_change_revalidates() {
    let mut world = World::new().await;

    // Membership moves while the submission waits for the user row.
    let origin = world.organization().await;
    let target = world.organization().await;
    let user = world.user(Some(origin)).await;
    let mut mover = world.runtime_connection().await;
    let mover_pid = backend_pid(&mut mover).await;
    sqlx::query("BEGIN").execute(&mut mover).await.unwrap();
    sqlx::query("SELECT 1 FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .execute(&mut mover)
        .await
        .unwrap();
    let submit = spawn_write(&world.pool, user, submit_kyc(user, origin).await);
    wait_blocked(&mut world.watcher, mover_pid, 1).await;
    sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
        .bind(user)
        .bind(target)
        .execute(&mut mover)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut mover).await.unwrap();
    assert_eq!(
        refusal(&finished(submit).await),
        (409, "KYC_MEMBERSHIP_CHANGED".into())
    );
    assert_eq!(kyc_of(&world, user).await, ("PENDING".to_string(), None));
    assert_eq!(
        world
            .count_for("SELECT count(*) FROM kyc_documents WHERE user_id=$1", user)
            .await,
        0
    );
    assert_eq!(audits_of(&world, "kyc.submitted", user).await, 0);

    // The submission commits first; the organization is then rejected, which
    // resets the review it just bound. The evidence rows stay.
    let origin = world.organization().await;
    let user = world.user(Some(origin)).await;
    let gate = Gate::closed();
    let submit = spawn_write(
        &world.pool,
        user,
        Gated::new(submit_kyc(user, origin).await, &gate),
    );
    let holder = gate.arrived().await;
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        review_organization(&world, origin, false),
    );
    wait_blocked(&mut world.watcher, holder, 1).await;
    gate.open();
    finished(submit).await.expect("the submission committed");
    finished(rejection).await.expect("the rejection committed");
    assert_eq!(kyc_of(&world, user).await, ("PENDING".to_string(), None));
    assert_eq!(user_organization(&world, user).await, None);
    assert_eq!(
        world
            .count_for("SELECT count(*) FROM kyc_documents WHERE user_id=$1", user)
            .await,
        2,
        "retained evidence is never deleted"
    );
    assert_eq!(audits_of(&world, "kyc.submitted", user).await, 1);
    assert_eq!(
        audits_of(&world, "kyc.membership_invalidated", user).await,
        1
    );

    // A review decision racing the rejection of the organization it is bound
    // to: approved first, the rejection resets it; rejected first, the approval
    // finds no current binding. No order leaves an approval bound to the
    // rejected organization.
    for approve_first in [true, false] {
        let origin = world.organization().await;
        let user = world.user(Some(origin)).await;
        world.bind_kyc(user, "SUBMITTED", Some(origin)).await;
        let gate = Gate::closed();
        let (holder, pid, contender) = if approve_first {
            let holder = spawn_write(
                &world.pool,
                world.actor,
                Gated::new(review_kyc(user, true), &gate),
            );
            let pid = gate.arrived().await;
            let contender = spawn_write(
                &world.pool,
                world.actor,
                review_organization(&world, origin, false),
            );
            (holder, pid, contender)
        } else {
            let holder = spawn_write(
                &world.pool,
                world.actor,
                Gated::new(review_organization(&world, origin, false), &gate),
            );
            let pid = gate.arrived().await;
            let contender = spawn_write(&world.pool, world.actor, review_kyc(user, true));
            (holder, pid, contender)
        };
        wait_blocked(&mut world.watcher, pid, 1).await;
        gate.open();
        let holder = finished(holder).await;
        let contender = finished(contender).await;
        holder.expect("the first command committed");
        if approve_first {
            contender.expect("the rejection committed");
            assert_eq!(audits_of(&world, "kyc.approved", user).await, 1);
        } else {
            assert_eq!(
                refusal(&contender),
                (409, "KYC_ORGANIZATION_MISMATCH".into())
            );
            assert_eq!(audits_of(&world, "kyc.approved", user).await, 0);
        }
        assert_eq!(organization_status(&world, origin).await, "REJECTED");
        assert_eq!(
            kyc_of(&world, user).await,
            ("PENDING".to_string(), None),
            "an approval stayed bound to the rejected organization"
        );
        assert_eq!(members_of(&world, origin).await, 0);
    }
    world.finish().await;
}

// ===========================================================================
// reject_vs_refresh_revokes_successor
// ===========================================================================

/// Account rejection and a refresh rotation serialize on the user row.
/// Rejection first: the rotation finds the rejected status and the cutoff and
/// issues nothing. Rotation first: the rejection revokes the successor session
/// the rotation just issued, so no refresh family of the user stays live.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn reject_vs_refresh_revokes_successor() {
    let mut world = World::new().await;

    // Rejection first.
    let organization = world.organization().await;
    let user = world.user(Some(organization)).await;
    let session = world.live_session(user).await;
    let gate = Gate::closed();
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        Gated::new(
            reject_account(user, RejectionRoute::Auth, Utc::now()),
            &gate,
        ),
    );
    let holder = gate.arrived().await;
    let refresh = tokio::spawn({
        let pool = world.pool.clone();
        async move {
            lock_for_rotation(&pool, user, &session)
                .await
                .finish(Utc::now() - Duration::minutes(10))
                .await
        }
    });
    wait_blocked(&mut world.watcher, holder, 1).await;
    gate.open();
    finished(rejection).await.expect("the rejection committed");
    assert_eq!(
        refresh.await.unwrap(),
        None,
        "a refresh outlived the rejection"
    );
    assert_eq!(live_sessions(&world, user).await, 0);
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
                user
            )
            .await,
        1,
        "no successor session exists"
    );

    // Rotation first.
    let user = world.user(Some(organization)).await;
    let session = world.live_session(user).await;
    let rotation = lock_for_rotation(&world.pool, user, &session).await;
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        reject_account(user, RejectionRoute::Auth, Utc::now()),
    );
    wait_blocked(&mut world.watcher, rotation.backend, 1).await;
    let successor = rotation.finish(Utc::now()).await;
    assert!(successor.is_some(), "the rotation ran before the rejection");
    finished(rejection).await.expect("the rejection committed");
    assert_eq!(
        live_sessions(&world, user).await,
        0,
        "the successor survived"
    );
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
                user
            )
            .await,
        2
    );
    world.finish().await;
}

/// A rotation that already holds the user row when a rejection request starts
/// can issue its successor access token after the instant the request was made.
/// The rejection then commits, and the token must still fall under its
/// revocation cutoff: otherwise a later reapproval of the account would revive a
/// token issued before the rejection committed.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn rejection_cutoff_covers_a_successor_issued_while_it_waited() {
    let mut world = World::new().await;
    let organization = world.organization().await;
    let user = world.user(Some(organization)).await;
    let session = world.live_session(user).await;
    let rotation = lock_for_rotation(&world.pool, user, &session).await;
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        reject_account(user, RejectionRoute::Auth, Utc::now()),
    );
    wait_blocked(&mut world.watcher, rotation.backend, 1).await;
    let successor_issued_at = Utc::now();
    rotation
        .finish(successor_issued_at)
        .await
        .expect("the rotation ran before the rejection");
    finished(rejection).await.expect("the rejection committed");

    world.set_status(user, "APPROVED").await;
    let state = account_state(&world, user).await;
    let issuance = TokenIssuance {
        issued_at: Some(successor_issued_at),
        out_of_range: false,
    };
    assert_eq!(
        cutoff_refusal(issuance, &state),
        Some(AuthError::SessionRevoked),
        "a token issued before the rejection committed passes the cutoff after reapproval"
    );
    world.finish().await;
}

/// A Python maintenance delete of an expired session holds that row while the
/// rejection revokes the user's sessions: the revocation waits for the delete,
/// and the stale row ends deleted and every live session revoked.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn auth_maintenance_delete_vs_rejection_keeps_both_effects() {
    let mut world = World::new().await;
    let organization = world.organization().await;
    let user = world.user(Some(organization)).await;
    world.session(user, Utc::now() - Duration::hours(1)).await;
    world.live_session(user).await;
    let mut maintenance = world.runtime_connection().await;
    let maintenance_pid = backend_pid(&mut maintenance).await;
    sqlx::query("BEGIN")
        .execute(&mut maintenance)
        .await
        .unwrap();
    let deleted =
        python_writers::delete_stale_refresh_sessions(&mut maintenance, Utc::now(), 1000).await;
    assert_eq!(deleted, 1, "the expired session is the maintenance batch");
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        reject_account(user, RejectionRoute::Auth, Utc::now()),
    );
    wait_blocked(&mut world.watcher, maintenance_pid, 1).await;
    sqlx::query("COMMIT")
        .execute(&mut maintenance)
        .await
        .unwrap();
    finished(rejection).await.expect("the rejection committed");
    assert_eq!(live_sessions(&world, user).await, 0);
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
                user
            )
            .await,
        1,
        "the maintenance delete and the revocation each kept their effect"
    );
    assert!(
        account_state(&world, user)
            .await
            .authentication_revoked_at
            .is_some()
    );
    world.finish().await;
}

// ===========================================================================
// approval_email_finalize_preserves_newer_transition
// ===========================================================================

/// The Python retry timer claims a due approval mail, then rejection and
/// reapproval replace the marker while its send is in flight. The old finalize
/// matches no row and cannot clear the new lease or payload; the Rust sender
/// then delivers the newer transition exactly once and the timer finds nothing.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn approval_email_finalize_preserves_newer_transition() {
    let mut world = World::new().await;
    let organization = world.organization().await;
    let user = world.user(Some(organization)).await;
    world.set_status(user, "PENDING").await;
    let marker = |world: &World| {
        let pool = world.pool.clone();
        async move {
            sqlx::query_as::<_, (Option<Uuid>, Option<Value>, Option<DateTime<Utc>>)>(
                "SELECT pending_approval_email_transition_id,pending_approval_email_payload,pending_approval_email_retry_at FROM users WHERE id=$1",
            )
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    run_to_end(&world, world.actor, approve_account(user, Utc::now()))
        .await
        .expect("the approval committed");
    let (first, ..) = marker(&world).await;
    let first = first.expect("the approval froze a marker");

    // The timer claims the first transition and its send is delayed.
    let mut timer = world.runtime_connection().await;
    assert!(matches!(
        python_writers::lock_due_approval_mail(&mut timer, user, first, Utc::now()).await,
        DueMail::Locked
    ));
    let lease = python_writers::commit_lease(&mut timer, user, Utc::now()).await;

    // Rejection clears the marker; reapproval freezes a new transition.
    run_to_end(
        &world,
        world.actor,
        reject_account(user, RejectionRoute::Auth, Utc::now()),
    )
    .await
    .expect("the rejection committed");
    assert_eq!(marker(&world).await.0, None, "rejection clears the marker");
    world.set_status(user, "PENDING").await;
    run_to_end(&world, world.actor, approve_account(user, Utc::now()))
        .await
        .expect("the reapproval committed");
    let (second, payload, retry_at) = marker(&world).await;
    let second = second.expect("the reapproval froze a marker");
    assert_ne!(first, second);

    // The old send returns and finalizes against its stale lease.
    assert_eq!(
        python_writers::finalize_approval_mail(&mut timer, user, first, lease, true, Utc::now())
            .await,
        Finalized::Skipped
    );
    let (kept, kept_payload, kept_retry) = marker(&world).await;
    assert_eq!(
        kept,
        Some(second),
        "the old finalize cleared the newer transition"
    );
    assert_eq!(
        kept_payload, payload,
        "the old finalize cleared the new payload"
    );
    assert_eq!(kept_retry, retry_at, "the old finalize moved the new lease");

    // The newer transition is sent once, by the Rust sender.
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = deliver_account_approval_mail(&world.pool, user, second, Utc::now, {
        let sends = Arc::clone(&sends);
        move |_payload, key| async move {
            assert_eq!(key, format!("account-approval/{second}"));
            sends.fetch_add(1, Ordering::SeqCst);
            true
        }
    })
    .await
    .unwrap();
    assert_eq!(sent, ApprovalDelivery::Sent);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(marker(&world).await.0, None);
    assert!(matches!(
        python_writers::lock_due_approval_mail(&mut timer, user, second, Utc::now()).await,
        DueMail::Skipped
    ));

    // One lease, two senders: whichever commits its lease first sends; the
    // other finds the marker no longer due and skips. Both orders.
    for python_first in [true, false] {
        let user = world.user(Some(organization)).await;
        world.set_status(user, "PENDING").await;
        run_to_end(&world, world.actor, approve_account(user, Utc::now()))
            .await
            .unwrap();
        let transition: Uuid = sqlx::query_scalar(
            "SELECT pending_approval_email_transition_id FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap();
        let mut timer = world.runtime_connection().await;
        let timer_pid = backend_pid(&mut timer).await;
        let rust_sends = Arc::new(AtomicUsize::new(0));
        if python_first {
            assert!(matches!(
                python_writers::lock_due_approval_mail(&mut timer, user, transition, Utc::now())
                    .await,
                DueMail::Locked
            ));
            let rust = tokio::spawn({
                let pool = world.pool.clone();
                let rust_sends = Arc::clone(&rust_sends);
                async move {
                    deliver_account_approval_mail(
                        &pool,
                        user,
                        transition,
                        Utc::now,
                        move |_, _| async move {
                            rust_sends.fetch_add(1, Ordering::SeqCst);
                            true
                        },
                    )
                    .await
                    .unwrap()
                }
            });
            wait_blocked(&mut world.watcher, timer_pid, 1).await;
            let lease = python_writers::commit_lease(&mut timer, user, Utc::now()).await;
            assert_eq!(rust.await.unwrap(), ApprovalDelivery::Skipped);
            assert_eq!(
                rust_sends.load(Ordering::SeqCst),
                0,
                "a mail was sent twice"
            );
            assert_eq!(
                python_writers::finalize_approval_mail(
                    &mut timer,
                    user,
                    transition,
                    lease,
                    true,
                    Utc::now()
                )
                .await,
                Finalized::Sent
            );
        } else {
            let mut sender = world.pool.acquire().await.unwrap();
            let mut claim_tx = sender.begin().await.unwrap();
            let claim = claim_approval_mail(&mut claim_tx, user, transition, Utc::now())
                .await
                .unwrap()
                .expect("the due transition is claimable");
            let sender_pid = backend_pid(&mut claim_tx).await;
            let python = tokio::spawn(async move {
                let due = python_writers::lock_due_approval_mail(
                    &mut timer,
                    user,
                    transition,
                    Utc::now(),
                )
                .await;
                (due, timer)
            });
            wait_blocked(&mut world.watcher, sender_pid, 1).await;
            claim_tx.commit().await.unwrap();
            let (due, _timer) = python.await.unwrap();
            assert!(
                matches!(due, DueMail::Skipped),
                "the lease was claimed twice"
            );
            let mut finish_tx = sender.begin().await.unwrap();
            finalize_approval_mail(&mut finish_tx, &claim, true, Utc::now())
                .await
                .unwrap();
            finish_tx.commit().await.unwrap();
        }
        let cleared: Option<Uuid> = sqlx::query_scalar(
            "SELECT pending_approval_email_transition_id FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap();
        assert_eq!(cleared, None, "the delivered marker was not cleared");
    }
    world.finish().await;
}

// ===========================================================================
// invitation_rotation_vs_acceptance_is_serialized
// ===========================================================================

async fn invitee_digest(world: &World, user: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT password_reset_token_hash FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap()
}

/// Reissuing an invitation and accepting it serialize on the invited user's
/// row. Acceptance first: the secret is spent, the rotation refuses the
/// verified account and writes no second secret. Rotation first: the old secret
/// no longer resolves and the new one is spendable exactly once.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn invitation_rotation_vs_acceptance_is_serialized() {
    let mut world = World::new().await;
    let organization = world.supplier_organization().await;
    let create = |email: &str| invite_supplier(email, organization);

    // Acceptance first.
    let created = run_ordered_write(
        world.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(world.actor),
        create("accept-first@customer.example"),
    )
    .await
    .expect("the invitation was created");
    let invitee = created.user_id;
    let digest = invitee_digest(&world, invitee)
        .await
        .expect("a stored digest");
    let acceptance = lock_for_acceptance(&world.pool, &digest)
        .await
        .expect("the current secret resolves");
    let rotation = spawn_write(
        &world.pool,
        world.actor,
        create("accept-first@customer.example"),
    );
    wait_blocked(&mut world.watcher, acceptance.backend, 1).await;
    acceptance.consume().await;
    assert_eq!(
        refusal(&finished(rotation).await),
        (409, "EMAIL_ALREADY_REGISTERED".into()),
        "a rotation reissued an invitation that was already spent"
    );
    assert_eq!(invitee_digest(&world, invitee).await, None);
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
                invitee
            )
            .await,
        1,
        "one acceptance issues one session"
    );
    assert_eq!(
        audits_of(&world, "user.invitation_accepted", invitee).await,
        1
    );
    assert_eq!(audits_of(&world, "admin.user_invited", invitee).await, 1);

    // Rotation first.
    let created = run_ordered_write(
        world.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(world.actor),
        create("rotate-first@customer.example"),
    )
    .await
    .expect("the invitation was created");
    let invitee = created.user_id;
    let old_digest = invitee_digest(&world, invitee)
        .await
        .expect("a stored digest");
    let gate = Gate::closed();
    let rotation = spawn_write(
        &world.pool,
        world.actor,
        Gated::new(create("rotate-first@customer.example"), &gate),
    );
    let holder = gate.arrived().await;
    let stale = tokio::spawn({
        let pool = world.pool.clone();
        let digest = old_digest.clone();
        async move { lock_for_acceptance(&pool, &digest).await }
    });
    wait_blocked(&mut world.watcher, holder, 1).await;
    gate.open();
    finished(rotation).await.expect("the rotation committed");
    assert!(
        stale.await.unwrap().is_none(),
        "the rotated-out secret was still accepted"
    );
    let new_digest = invitee_digest(&world, invitee)
        .await
        .expect("a rotated digest");
    assert_ne!(new_digest, old_digest);
    lock_for_acceptance(&world.pool, &new_digest)
        .await
        .expect("the new secret resolves")
        .consume()
        .await;
    assert!(
        lock_for_acceptance(&world.pool, &new_digest)
            .await
            .is_none(),
        "a spent secret resolved again"
    );
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
                invitee
            )
            .await,
        1
    );
    assert_eq!(
        audits_of(&world, "user.invitation_accepted", invitee).await,
        1
    );
    world.finish().await;
}

/// The Python maintenance command clears invitation digests whose expiry has
/// passed. An administrator reissuing an expired invitation races it: the
/// reissued digest must survive.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn auth_maintenance_cannot_clear_a_reissued_invitation_digest() {
    let mut world = World::new().await;
    let organization = world.supplier_organization().await;
    let created = run_ordered_write(
        world.pool.clone(),
        Arc::new(Semaphore::new(1)),
        access(world.actor),
        invite_supplier("expired@customer.example", organization),
    )
    .await
    .expect("the invitation was created");
    let invitee = created.user_id;
    sqlx::query("UPDATE users SET password_reset_expires=now() - interval '1 hour' WHERE id=$1")
        .bind(invitee)
        .execute(&mut world.admin)
        .await
        .unwrap();
    let expired = invitee_digest(&world, invitee)
        .await
        .expect("a stored digest");

    let gate = Gate::closed();
    let reissue = spawn_write(
        &world.pool,
        world.actor,
        Gated::new(
            invite_supplier("expired@customer.example", organization),
            &gate,
        ),
    );
    let holder = gate.arrived().await;
    let mut maintenance = world.runtime_connection().await;
    let maintenance = tokio::spawn(async move {
        python_writers::clear_expired_reset_hashes(&mut maintenance, Utc::now(), 1000).await
    });
    wait_blocked(&mut world.watcher, holder, 1).await;
    gate.open();
    finished(reissue).await.expect("the reissue committed");
    maintenance.await.unwrap();
    let kept = invitee_digest(&world, invitee).await;
    assert!(
        kept.as_deref().is_some_and(|digest| digest != expired),
        "the maintenance command cleared the digest the administrator just issued"
    );
    world.finish().await;
}

// ===========================================================================
// The command matrix: retry, exhaustion and atomic refusal.
// ===========================================================================

#[derive(Clone, Copy, Debug)]
enum Subject {
    ApproveAccount,
    RejectAccount,
    RejectAccountAnalytics,
    ApproveJoin,
    RejectOrganization,
    ApproveOrganization,
    ApproveKyc,
    RejectKyc,
    SubmitKyc,
    BootstrapSubscription,
}

struct Scene {
    actor: Uuid,
    organization: Uuid,
    user: Uuid,
    join: Uuid,
    listing: Option<Listing>,
}

impl Subject {
    /// The tables the command writes after its first row change, in the order
    /// that matters for a failure injected there.
    fn tables(self) -> &'static [&'static str] {
        match self {
            Self::ApproveAccount | Self::RejectAccount | Self::RejectAccountAnalytics => &[
                "user_status_transitions",
                "market_event_outbox",
                "audit_logs",
            ],
            Self::ApproveJoin => &["audit_logs"],
            Self::RejectOrganization | Self::ApproveOrganization => {
                &["market_event_outbox", "audit_logs"]
            }
            Self::ApproveKyc | Self::RejectKyc => &["market_event_outbox", "audit_logs"],
            Self::SubmitKyc => &["kyc_documents", "audit_logs"],
            Self::BootstrapSubscription => &["subscriptions"],
        }
    }

    /// The table whose insert fails in the retry cases: the last write of the
    /// command, after every earlier change of the same attempt.
    fn injection_table(self) -> &'static str {
        match self {
            Self::RejectOrganization => "market_event_outbox",
            Self::BootstrapSubscription => "subscriptions",
            _ => "audit_logs",
        }
    }

    async fn seed(self, world: &mut World) -> Scene {
        let organization = match self {
            Self::ApproveOrganization => world.pending_organization().await,
            _ => world.organization().await,
        };
        let user = match self {
            Self::ApproveJoin => world.user(None).await,
            _ => world.user(Some(organization)).await,
        };
        let join = match self {
            Self::ApproveJoin => world.join_request(user, organization).await,
            _ => Uuid::nil(),
        };
        let mut listing = None;
        match self {
            Self::ApproveAccount => world.set_status(user, "PENDING").await,
            Self::RejectAccount | Self::RejectAccountAnalytics => {
                world.live_session(user).await;
            }
            Self::RejectOrganization => {
                world.bind_kyc(user, "APPROVED", Some(organization)).await;
                listing = Some(world.stocked_listing(organization).await);
            }
            Self::ApproveKyc | Self::RejectKyc => {
                world.bind_kyc(user, "SUBMITTED", Some(organization)).await;
            }
            _ => {}
        }
        let actor = match self {
            Self::SubmitKyc | Self::BootstrapSubscription => user,
            _ => world.actor,
        };
        Scene {
            actor,
            organization,
            user,
            join,
            listing,
        }
    }

    async fn run(self, world: &World, scene: &Scene) -> Verdict {
        let now = Utc::now();
        let (actor, organization, user) = (scene.actor, scene.organization, scene.user);
        match self {
            Self::ApproveAccount => run_to_end(world, actor, approve_account(user, now)).await,
            Self::RejectAccount => {
                run_to_end(
                    world,
                    actor,
                    reject_account(user, RejectionRoute::Auth, now),
                )
                .await
            }
            Self::RejectAccountAnalytics => {
                run_to_end(
                    world,
                    actor,
                    reject_account(user, RejectionRoute::Analytics, now),
                )
                .await
            }
            Self::ApproveJoin => run_to_end(world, actor, review_join(scene.join, true)).await,
            Self::RejectOrganization => {
                run_to_end(
                    world,
                    actor,
                    review_organization(world, organization, false),
                )
                .await
            }
            Self::ApproveOrganization => {
                run_to_end(world, actor, review_organization(world, organization, true)).await
            }
            Self::ApproveKyc => run_to_end(world, actor, review_kyc(user, true)).await,
            Self::RejectKyc => run_to_end(world, actor, review_kyc(user, false)).await,
            Self::SubmitKyc => run_to_end(world, actor, submit_kyc(user, organization).await).await,
            Self::BootstrapSubscription => {
                run_to_end(world, actor, subscription_bootstrap(user)).await
            }
        }
    }

    /// The effects of exactly one committed run.
    async fn assert_committed_once(self, world: &World, scene: &Scene) {
        let (organization, user) = (scene.organization, scene.user);
        match self {
            Self::ApproveAccount => {
                assert_eq!(
                    status_of(world, "SELECT status FROM users WHERE id=$1", user).await,
                    "APPROVED"
                );
                assert_eq!(
                    world
                        .count_for(
                            "SELECT count(*) FROM user_status_transitions WHERE user_id=$1",
                            user
                        )
                        .await,
                    1
                );
                assert_eq!(
                    world.count_for("SELECT count(*) FROM users WHERE id=$1 AND pending_approval_email_transition_id IS NOT NULL", user).await,
                    1,
                    "the approval froze exactly one mail marker"
                );
                assert_eq!(audits_of(world, "admin.user_approved", user).await, 1);
            }
            Self::RejectAccount | Self::RejectAccountAnalytics => {
                assert_eq!(
                    status_of(world, "SELECT status FROM users WHERE id=$1", user).await,
                    "REJECTED"
                );
                assert_eq!(
                    world
                        .count_for(
                            "SELECT count(*) FROM user_status_transitions WHERE user_id=$1",
                            user
                        )
                        .await,
                    1
                );
                assert_eq!(live_sessions(world, user).await, 0);
                assert_eq!(audits_of(world, "admin.user_rejected", user).await, 1);
            }
            Self::ApproveJoin => {
                assert_eq!(user_organization(world, user).await, Some(organization));
                assert_eq!(
                    audits_of(world, "organization.join_approved", scene.join).await,
                    1
                );
            }
            Self::RejectOrganization => {
                let listing = scene.listing.as_ref().expect("a stocked listing");
                assert_eq!(organization_status(world, organization).await, "REJECTED");
                assert_eq!(user_organization(world, user).await, None);
                assert_eq!(order_status(world, listing.order).await, "CANCELLED");
                assert_eq!(
                    world.stock(listing.inventory).await,
                    (Decimal::from(80), Decimal::from(20)),
                    "the uncommitted reservation is released exactly once"
                );
                assert_eq!(
                    audits_of(world, "admin.organization_rejected", organization).await,
                    1
                );
            }
            Self::ApproveOrganization => {
                assert_eq!(organization_status(world, organization).await, "APPROVED");
                assert_eq!(
                    audits_of(world, "admin.organization_approved", organization).await,
                    1
                );
            }
            Self::ApproveKyc => {
                assert_eq!(
                    kyc_of(world, user).await,
                    ("APPROVED".to_string(), Some(organization))
                );
                assert_eq!(audits_of(world, "kyc.approved", user).await, 1);
            }
            Self::RejectKyc => {
                assert_eq!(
                    kyc_of(world, user).await,
                    ("REJECTED".to_string(), Some(organization))
                );
                assert_eq!(audits_of(world, "kyc.rejected", user).await, 1);
            }
            Self::SubmitKyc => {
                assert_eq!(
                    kyc_of(world, user).await,
                    ("SUBMITTED".to_string(), Some(organization))
                );
                assert_eq!(
                    world
                        .count_for("SELECT count(*) FROM kyc_documents WHERE user_id=$1", user)
                        .await,
                    2
                );
                assert_eq!(audits_of(world, "kyc.submitted", user).await, 1);
            }
            Self::BootstrapSubscription => {
                assert_eq!(
                    world
                        .count_for(
                            "SELECT count(*) FROM subscriptions WHERE org_id=$1",
                            organization
                        )
                        .await,
                    1
                );
            }
        }
    }
}

const ALL_SUBJECTS: [Subject; 10] = [
    Subject::ApproveAccount,
    Subject::RejectAccount,
    Subject::RejectAccountAnalytics,
    Subject::ApproveJoin,
    Subject::RejectOrganization,
    Subject::ApproveOrganization,
    Subject::ApproveKyc,
    Subject::RejectKyc,
    Subject::SubmitKyc,
    Subject::BootstrapSubscription,
];

/// Serialization failures and deadlocks raised inside the transaction. Two
/// failures are absorbed by the bounded retry, which restarts from a fresh
/// transaction and commits the effects exactly once. A failure that never
/// clears ends in the busy refusal with every row unchanged.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn org_write_injected_conflicts_retry_or_refuse_atomically() {
    let mut world = World::new().await;
    for subject in ALL_SUBJECTS {
        let table = subject.injection_table();
        for sqlstate in ["40001", "40P01"] {
            // Recovery within the attempt bound.
            let scene = subject.seed(&mut world).await;
            let fault = Fault::first(&mut world.admin, table, sqlstate, 2).await;
            subject.run(&world, &scene).await.unwrap_or_else(|error| {
                panic!("{subject:?}: retry did not absorb {sqlstate}: {error:?}")
            });
            assert_eq!(
                fault.hits(&mut world.admin).await,
                3,
                "{subject:?} {sqlstate}: attempts"
            );
            subject.assert_committed_once(&world, &scene).await;
            fault.remove(&mut world.admin).await;

            // Exhaustion.
            let scene = subject.seed(&mut world).await;
            let before = world.snapshot().await;
            let fault = Fault::always(&mut world.admin, table, sqlstate).await;
            let verdict = subject.run(&world, &scene).await;
            let error = verdict.as_ref().expect_err("the failure never clears");
            assert_eq!(
                (
                    error.status.as_u16(),
                    error.detail["code"].as_str(),
                    error.retry_after
                ),
                (503, Some("TRANSACTION_RETRY_EXHAUSTED"), Some(1)),
                "{subject:?} {sqlstate}"
            );
            assert_eq!(
                fault.hits(&mut world.admin).await,
                3,
                "{subject:?} {sqlstate}: attempts"
            );
            assert_eq!(
                world.snapshot().await,
                before,
                "{subject:?} {sqlstate}: a partial write survived the exhausted retry"
            );
            fault.remove(&mut world.admin).await;
        }
    }
    world.finish().await;
}

/// A failure after the writes and before the commit — the audit row, the event
/// or the status history — rolls the whole command back and is not retried:
/// no status, membership, session, stock, evidence, event or audit change.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn outbox_audit_failure_rolls_back_admission() {
    let mut world = World::new().await;
    for subject in ALL_SUBJECTS {
        if matches!(subject, Subject::BootstrapSubscription) {
            continue;
        }
        for table in subject.tables() {
            let scene = subject.seed(&mut world).await;
            let before = world.snapshot().await;
            let fault = Fault::always(&mut world.admin, table, "23514").await;
            let verdict = subject.run(&world, &scene).await;
            assert_eq!(
                refusal(&verdict),
                (500, "DATABASE_ERROR".into()),
                "{subject:?} failing {table}"
            );
            assert_eq!(
                fault.hits(&mut world.admin).await,
                1,
                "{subject:?} failing {table}: the failure was retried or never fired"
            );
            assert_eq!(
                world.snapshot().await,
                before,
                "{subject:?} failing {table}: a partial write survived"
            );
            fault.remove(&mut world.admin).await;
        }
    }
    world.finish().await;
}

// ===========================================================================
// org_rejection_bound_refuses_atomically
// ===========================================================================

/// An organization with more than 100 executable orders, or more than 100
/// uncancelled trades, is refused whole: the organization, its members, the
/// stock and the market state stay exactly as they were.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn org_rejection_bound_refuses_atomically() {
    let mut world = World::new().await;
    for by_trades in [false, true] {
        let organization = world.organization().await;
        let member = world.user(Some(organization)).await;
        world.bind_kyc(member, "APPROVED", Some(organization)).await;
        world.live_session(member).await;
        let listing = world.stocked_listing(organization).await;
        if by_trades {
            let counterparty_org = world.organization().await;
            let counterparty = world.user(Some(counterparty_org)).await;
            for _ in 0..101 {
                world
                    .trade(
                        (counterparty_org, counterparty),
                        (organization, member),
                        Some(listing.order),
                        "PENDING_CONFIRMATION",
                    )
                    .await;
            }
        } else {
            for _ in 0..100 {
                world.open_order(organization).await;
            }
        }
        let before = world.snapshot().await;
        let verdict = run_to_end(
            &world,
            world.actor,
            review_organization(&world, organization, false),
        )
        .await;
        assert_eq!(refusal(&verdict), (409, "MARKET_INVALIDATION_LIMIT".into()));
        assert_eq!(world.snapshot().await, before, "by_trades={by_trades}");
        assert_eq!(organization_status(&world, organization).await, "APPROVED");
    }
    world.finish().await;
}

// ===========================================================================
// analytics_status_history_is_atomic
// ===========================================================================

async fn status_facts(world: &mut World, query: &AnalyticsQuery) -> StatusFacts {
    load_status_facts(&mut world.admin, query)
        .await
        .expect("status facts")
}

/// The status change and its history row commit together or not at all, and
/// the analytics facts read only committed history: a failure between the
/// status write and the history insert leaves the facts as they were, and the
/// rerun counts the approval once.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn analytics_status_history_is_atomic() {
    let mut world = World::new().await;
    let organization = world.organization().await;
    let user = world.user(Some(organization)).await;
    world.set_status(user, "PENDING").await;
    let instant = Utc.with_ymd_and_hms(2026, 10, 20, 10, 0, 0).unwrap();
    let query = AnalyticsQuery {
        start: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
        previous: None,
        compare: false,
        audience: AnalyticsAudience::All,
        activity: AnalyticsActivity::Live,
        product_id: None,
        delivery_point_id: None,
        availability_window: None,
    };
    let before = world.snapshot().await;
    assert_eq!(status_facts(&mut world, &query).await.approved_current, 0);

    let fault = Fault::always(&mut world.admin, "user_status_transitions", "23514").await;
    let verdict = run_to_end(&world, world.actor, approve_account(user, instant)).await;
    assert_eq!(refusal(&verdict), (500, "DATABASE_ERROR".into()));
    assert_eq!(
        world.snapshot().await,
        before,
        "the status write persisted alone"
    );
    let observed = status_facts(&mut world, &query).await;
    assert_eq!(observed.approved_current, 0);
    assert_eq!(
        observed.coverage_start, None,
        "uncommitted history was counted"
    );
    fault.remove(&mut world.admin).await;

    run_to_end(&world, world.actor, approve_account(user, instant))
        .await
        .expect("the rerun committed");
    let observed = status_facts(&mut world, &query).await;
    assert_eq!(observed.approved_current, 1);
    assert_eq!(observed.coverage_start, Some(instant));
    assert_eq!(
        world
            .count_for(
                "SELECT count(*) FROM user_status_transitions WHERE user_id=$1",
                user
            )
            .await,
        1
    );
    world.finish().await;
}

// ===========================================================================
// organization_write_cancellation_does_not_duplicate_commit
// ===========================================================================

/// The detached ordered owner commits at most once across caller cancellation
/// before or after the commit and across a lost commit acknowledgement: the
/// admission commits one status change, one history row, one audit row and one
/// event; the ambiguous connection is closed and never reused; the command
/// runs once; and the permit returns only after the commit settles.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn organization_write_cancellation_does_not_duplicate_commit() {
    for (point, lost_ack, cancel) in [
        (CompletionPoint::BeforeCommit, false, true),
        (CompletionPoint::AfterCommit, false, true),
        (CompletionPoint::AfterCommit, true, false),
    ] {
        let mut world = World::new().await;
        let organization = world.organization().await;
        let user = world.user(Some(organization)).await;
        world.live_session(user).await;
        let control = CompletionControl::new(point, lost_ack);
        let observed = Gate::closed();
        observed.open();
        let permits = Arc::new(Semaphore::new(1));
        let caller = tokio::spawn({
            let (pool, permits, control) =
                (world.pool.clone(), Arc::clone(&permits), control.clone());
            let actor = world.actor;
            let command = Gated::new(
                reject_account(user, RejectionRoute::Auth, Utc::now()),
                &observed,
            );
            async move { run_controlled_write(pool, permits, access(actor), command, control).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(60), control.reached())
            .await
            .expect("the command reached its completion point");
        assert_eq!(
            permits.available_permits(),
            0,
            "the admission permit is held"
        );
        let history_before_commit = world
            .count_for(
                "SELECT count(*) FROM user_status_transitions WHERE user_id=$1",
                user,
            )
            .await;
        assert_eq!(
            history_before_commit,
            i64::from(point == CompletionPoint::AfterCommit)
        );
        if cancel {
            caller.abort();
        }
        control.release();
        let outcome = caller.await;
        if lost_ack {
            assert_eq!(
                outcome.unwrap().unwrap_err().detail["code"],
                "COMMIT_OUTCOME_UNKNOWN"
            );
        }
        let permit = tokio::time::timeout(std::time::Duration::from_secs(60), permits.acquire())
            .await
            .expect("the permit returns after the commit settles")
            .unwrap();
        drop(permit);

        assert_eq!(observed.attempts(), 1, "the command ran more than once");
        assert_eq!(
            world
                .count_for(
                    "SELECT count(*) FROM user_status_transitions WHERE user_id=$1",
                    user
                )
                .await,
            1
        );
        assert_eq!(audits_of(&world, "admin.user_rejected", user).await, 1);
        assert_eq!(
            world.events("account_admission_changed").await,
            1,
            "the event was written more than once"
        );
        assert_eq!(
            status_of(&world, "SELECT status FROM users WHERE id=$1", user).await,
            "REJECTED"
        );
        assert_eq!(live_sessions(&world, user).await, 0);
        let backend = observed.backends()[0];
        wait_backend_gone(&mut world.watcher, backend).await;
        world.finish().await;
    }
}

// ===========================================================================
// concurrent_subscription_bootstrap_is_one_row
// ===========================================================================

/// Two members bootstrapping the organization's free subscription and an
/// administrator negotiating its enterprise tier contend for the organization
/// row. In every order exactly one row exists, no command fails, and the
/// negotiated tier and fee are never replaced by the free bootstrap.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn concurrent_subscription_bootstrap_is_one_row() {
    let mut world = World::new().await;
    for administrator_first in [false, true] {
        let organization = world.organization().await;
        let first = world.user(Some(organization)).await;
        let second = world.user(Some(organization)).await;
        let gate = Gate::closed();
        let holder = if administrator_first {
            spawn_write(
                &world.pool,
                world.actor,
                Gated::new(subscription_negotiation(world.actor, organization), &gate),
            )
        } else {
            spawn_write(
                &world.pool,
                first,
                Gated::new(subscription_bootstrap(first), &gate),
            )
        };
        gate.arrived().await;
        let mut contenders = vec![spawn_write(
            &world.pool,
            second,
            subscription_bootstrap(second),
        )];
        if administrator_first {
            contenders.push(spawn_write(
                &world.pool,
                first,
                subscription_bootstrap(first),
            ));
        } else {
            contenders.push(spawn_write(
                &world.pool,
                world.actor,
                subscription_negotiation(world.actor, organization),
            ));
        }
        wait_blocked_backends(&mut world.watcher, 2).await;
        gate.open();
        finished(holder).await.expect("the first command committed");
        for contender in contenders {
            finished(contender).await.expect("a contender failed");
        }
        assert_eq!(
            world
                .count_for(
                    "SELECT count(*) FROM subscriptions WHERE org_id=$1",
                    organization
                )
                .await,
            1,
            "the bootstrap created more than one row"
        );
        let (tier, fee): (String, Option<Decimal>) =
            sqlx::query_as("SELECT tier,seller_fee_per_mt_usd FROM subscriptions WHERE org_id=$1")
                .bind(organization)
                .fetch_one(&world.pool)
                .await
                .unwrap();
        assert_eq!(
            tier, "enterprise",
            "a bootstrap downgraded the negotiated tier"
        );
        assert_eq!(fee, Some(Decimal::new(75, 2)));
        assert_eq!(
            audits_of(&world, "subscription.updated", organization).await,
            1
        );
    }
    world.finish().await;
}

// ===========================================================================
// org_rejection_vs_python_take_preserves_stock
// ===========================================================================

struct TradeScene {
    organization: Uuid,
    member: Uuid,
    listing: Listing,
    trade: Uuid,
    lifecycle: python_writers::TradeLifecycle,
}

async fn trade_scene(world: &mut World) -> TradeScene {
    let organization = world.organization().await;
    let member = world.user(Some(organization)).await;
    world.bind_kyc(member, "APPROVED", Some(organization)).await;
    let listing = world.stocked_listing(organization).await;
    let counterparty_org = world.organization().await;
    let counterparty = world.user(Some(counterparty_org)).await;
    let trade = world
        .trade(
            (counterparty_org, counterparty),
            (organization, member),
            Some(listing.order),
            "CONFIRMED",
        )
        .await;
    let lifecycle = python_writers::TradeLifecycle {
        slice_key: market_slice_lock_key(world.product, Some(world.point), "SPOT"),
        trade,
        order: listing.order,
        users: vec![counterparty, member],
        organizations: vec![counterparty_org, organization],
    };
    TradeScene {
        organization,
        member,
        listing,
        trade,
        lifecycle,
    }
}

/// A Python trade-lifecycle request takes the market slice and rows first, then
/// the parties' user and organization rows; the Rust organization rejection
/// takes organizations and users first, then the slice. The orders are
/// inverted. The suite does not hide that: with the rejection holding its
/// organization and user locks, the Python request holds the slice and waits
/// for the user row, and the rejection refuses as busy after its bounded slice
/// wait, rolls back whole, and succeeds when repeated. The other order — the
/// rejection inside its market stage, the Python request waiting for the slice —
/// commits both. In both, stock is conserved and the pre-existing trade keeps
/// its participants.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn org_rejection_vs_python_take_preserves_stock() {
    // The rejection reaches the slice lock while the Python request holds it.
    let mut world = World::new().await;
    let scene = trade_scene(&mut world).await;
    let before = world.snapshot().await;
    let gate = Gate::closed();
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        Gated::new(
            review_organization(&world, scene.organization, false),
            &gate,
        ),
    );
    let rust_backend = gate.arrived().await;
    let python = tokio::spawn(scene.lifecycle.deliver(world.runtime_connection().await));
    wait_blocked(&mut world.watcher, rust_backend, 1).await;
    gate.open();
    assert_eq!(
        refusal(&finished(rejection).await),
        (409, "MARKET_BUSY".into()),
        "the inverted wait must refuse as busy, not hang"
    );
    python.await.unwrap().expect("the Python request committed");
    let mut expected = before.clone();
    expected["trades"][0]["status"] = Value::String("DELIVERED".into());
    assert_eq!(
        world.snapshot().await,
        expected,
        "only the Python request's own write may differ after the refusal"
    );
    run_to_end(
        &world,
        world.actor,
        review_organization(&world, scene.organization, false),
    )
    .await
    .expect("the repeated rejection committed");
    assert_eq!(
        organization_status(&world, scene.organization).await,
        "REJECTED"
    );
    assert_eq!(order_status(&world, scene.listing.order).await, "CANCELLED");
    let (available, reserved) = world.stock(scene.listing.inventory).await;
    assert_eq!(
        available + reserved,
        Decimal::from(100),
        "stock is conserved"
    );
    assert_eq!(reserved, Decimal::from(20));
    let trade = row_json(&world, TRADE_ROW, scene.trade).await;
    assert_eq!(trade["status"], "DELIVERED");
    assert_eq!(
        trade["seller_user_id"],
        Value::String(scene.member.to_string())
    );
    world.finish().await;

    // The Python request arrives while the rejection is inside its market stage.
    let mut world = World::new().await;
    let scene = trade_scene(&mut world).await;
    sqlx::raw_sql(include_str!("fixtures/phase4_races_stage_gate.sql"))
        .execute(&mut world.admin)
        .await
        .unwrap();
    let mut gate_holder = world.runtime_connection().await;
    let gate_pid = backend_pid(&mut gate_holder).await;
    sqlx::query("BEGIN")
        .execute(&mut gate_holder)
        .await
        .unwrap();
    sqlx::query(include_str!(
        "fixtures/market_invalidation_advisory_lock.sql"
    ))
    .bind(7_240_001_i64)
    .execute(&mut gate_holder)
    .await
    .unwrap();
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        review_organization(&world, scene.organization, false),
    );
    let rust_backend = wait_blocked(&mut world.watcher, gate_pid, 1).await[0];
    let python = tokio::spawn(scene.lifecycle.deliver(world.runtime_connection().await));
    wait_blocked(&mut world.watcher, rust_backend, 1).await;
    sqlx::query("COMMIT")
        .execute(&mut gate_holder)
        .await
        .unwrap();
    finished(rejection).await.expect("the rejection committed");
    python.await.unwrap().expect("the Python request committed");
    assert_eq!(
        organization_status(&world, scene.organization).await,
        "REJECTED"
    );
    assert_eq!(order_status(&world, scene.listing.order).await, "CANCELLED");
    let (available, reserved) = world.stock(scene.listing.inventory).await;
    assert_eq!(
        (available, reserved),
        (Decimal::from(80), Decimal::from(20))
    );
    let trade = row_json(&world, TRADE_ROW, scene.trade).await;
    assert_eq!(trade["status"], "DELIVERED");
    assert_eq!(
        trade["seller_user_id"],
        Value::String(scene.member.to_string())
    );
    world.finish().await;
}

// ===========================================================================
// auth_maintenance_vs_ordered_write_lock_order
// ===========================================================================

async fn recorded_deadlocks(watcher: &mut sqlx::PgConnection) -> i64 {
    sqlx::query_scalar("SELECT deadlocks FROM pg_stat_database WHERE datname = current_database()")
        .fetch_one(watcher)
        .await
        .unwrap()
}

/// The statistics collector reports a deadlock after the detecting backend
/// flushes its counters, so the count is awaited as observed state.
async fn wait_for_deadlock_count(watcher: &mut sqlx::PgConnection, expected: i64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while recorded_deadlocks(watcher).await < expected {
        assert!(
            std::time::Instant::now() < deadline,
            "the database never recorded the deadlock this race forms"
        );
        tokio::task::yield_now().await;
    }
}

/// The Python maintenance command clears expired digests user by user in
/// storage order, without an organization lock; the Rust organization write
/// locks its users in id order. With two members in opposite orders the
/// database reports a real deadlock (40P01) after its detection delay. The
/// victim is whichever backend's detector fires first. Either way the Rust
/// write commits within its bounded retry, and the maintenance transaction
/// either committed both rows or neither.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn auth_maintenance_vs_ordered_write_lock_order() {
    let mut world = World::new().await;
    let organization = world.organization().await;
    let low = Uuid::from_u128(0x1000);
    let high = Uuid::from_u128(0x2000);
    for id in [low, high] {
        sqlx::query("INSERT INTO users(id,organization_id,role,password_reset_token_hash,password_reset_expires) VALUES($1,$2,'BUYER','stale-digest',now() - interval '1 hour')")
            .bind(id)
            .bind(organization)
            .execute(&mut world.admin)
            .await
            .unwrap();
    }
    let deadlocks_before = recorded_deadlocks(&mut world.watcher).await;
    let mut maintenance = world.runtime_connection().await;
    let maintenance_pid = backend_pid(&mut maintenance).await;
    sqlx::query("BEGIN")
        .execute(&mut maintenance)
        .await
        .unwrap();
    python_writers::clear_reset_hash_of(&mut maintenance, high)
        .await
        .unwrap();
    let rejection = spawn_write(
        &world.pool,
        world.actor,
        review_organization(&world, organization, false),
    );
    // The rejection holds the organization and the lower user and waits for the higher.
    wait_blocked(&mut world.watcher, maintenance_pid, 1).await;
    let python = tokio::spawn(async move {
        let cleared_low = python_writers::clear_reset_hash_of(&mut maintenance, low).await;
        match cleared_low {
            Ok(_) => sqlx::query("COMMIT")
                .execute(&mut maintenance)
                .await
                .is_ok(),
            Err(_) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut maintenance).await;
                false
            }
        }
    });
    finished(rejection)
        .await
        .expect("the rejection commits within its bounded retry");
    let committed = python.await.unwrap();
    wait_for_deadlock_count(&mut world.watcher, deadlocks_before + 1).await;
    let cleared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE id = ANY($1) AND password_reset_token_hash IS NULL",
    )
    .bind(vec![low, high])
    .fetch_one(&world.pool)
    .await
    .unwrap();
    assert_eq!(
        cleared,
        if committed { 2 } else { 0 },
        "the maintenance transaction applied half of its rows"
    );
    assert_eq!(organization_status(&world, organization).await, "REJECTED");
    assert_eq!(members_of(&world, organization).await, 0);
    world.finish().await;
}

// ===========================================================================
// rejected_party_new_execution_refused_but_existing_deliver_pay_allowed
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// A08, `PUT /api/auth/reject/{user_id}`.
    Account,
    /// N01, `PUT /api/admin/analytics/users/{user_id}/reject`.
    Analytics,
    /// A10, `PUT /api/auth/organization/{organization_id}/reject`.
    Organization,
    /// K06, `PUT /api/kyc/admin/{user_id}/reject`.
    Kyc,
}

async fn eligible_in_sql(world: &World, user: Uuid) -> bool {
    let mut builder = QueryBuilder::new("SELECT count(*) FROM users u WHERE u.id = ");
    builder.push_bind(user);
    builder.push(" AND (");
    bind_eligibility_sql(
        &mut builder,
        &KycPolicy::default(),
        KycSqlParty::USER,
        Utc::now(),
    );
    builder.push(")");
    let count: i64 = builder
        .build_query_scalar()
        .fetch_one(&world.pool)
        .await
        .unwrap();
    count == 1
}

async fn kyc_party(world: &World, user: Uuid) -> KycParty {
    let (organization, kyc_organization, status): (Option<Uuid>, Option<Uuid>, String) =
        sqlx::query_as(
            "SELECT organization_id,kyc_organization_id,kyc_status FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&world.pool)
        .await
        .unwrap();
    KycParty {
        user_id: user,
        organization_id: organization,
        kyc_organization_id: kyc_organization,
        kyc_status: serde_json::from_value(Value::String(status)).expect("a closed KYC status"),
    }
}

/// Each of the four rejection routes leaves every pre-existing trade exactly as
/// it was — participants, lifecycle state, quantity and fee and term snapshots —
/// whatever the rejected party's current membership, and the settlement
/// reservation stays. The authority of an existing party is its immutable
/// participation in the trade, not its current membership. Meanwhile new
/// execution is refused: authentication stops for a rejected account, and the
/// execution policy and its SQL mirror refuse an unlinked or rejected party.
/// The deliver and pay requests themselves run in Python until the market
/// phases and are not driven here.
#[tokio::test]
#[ignore = "requires explicit disposable PostgreSQL authority"]
async fn rejected_party_new_execution_refused_but_existing_deliver_pay_allowed() {
    let mut world = World::new().await;
    for route in [
        Route::Account,
        Route::Analytics,
        Route::Organization,
        Route::Kyc,
    ] {
        for unlinked in [false, true] {
            let case = format!("{route:?} unlinked={unlinked}");
            let organization = world.organization().await;
            let member = world.user(Some(organization)).await;
            world.bind_kyc(member, "APPROVED", Some(organization)).await;
            world.live_session(member).await;
            let listing = world.stocked_listing(organization).await;
            let counterparty_org = world.organization().await;
            let counterparty = world.user(Some(counterparty_org)).await;
            let mut trades = Vec::new();
            for status in ["PENDING_CONFIRMATION", "CONFIRMED", "DELIVERED"] {
                trades.push(
                    world
                        .trade(
                            (counterparty_org, counterparty),
                            (organization, member),
                            Some(listing.order),
                            status,
                        )
                        .await,
                );
            }
            if unlinked {
                sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
                    .bind(member)
                    .execute(&mut world.admin)
                    .await
                    .unwrap();
            }
            let mut trades_before = Vec::new();
            for trade in &trades {
                trades_before.push(row_json(&world, TRADE_ROW, *trade).await);
            }
            // The rejection's cutoff, set well before "now" so the token cases
            // below are unambiguous at one-second token resolution.
            let rejected_at = Utc::now() - Duration::seconds(30);

            let verdict = match route {
                Route::Account => {
                    run_to_end(
                        &world,
                        world.actor,
                        reject_account(member, RejectionRoute::Auth, rejected_at),
                    )
                    .await
                }
                Route::Analytics => {
                    run_to_end(
                        &world,
                        world.actor,
                        reject_account(member, RejectionRoute::Analytics, rejected_at),
                    )
                    .await
                }
                Route::Organization => {
                    run_to_end(
                        &world,
                        world.actor,
                        review_organization(&world, organization, false),
                    )
                    .await
                }
                Route::Kyc => run_to_end(&world, world.actor, review_kyc(member, false)).await,
            };
            if route == Route::Kyc && unlinked {
                // The review is bound to an organization the user no longer belongs to.
                assert_eq!(
                    refusal(&verdict),
                    (409, "KYC_ORGANIZATION_MISMATCH".into()),
                    "{case}"
                );
            } else {
                verdict.unwrap_or_else(|error| panic!("{case}: {error:?}"));
            }

            // Existing trades: immutable participants, lifecycle and snapshots.
            for (trade, before) in trades.iter().zip(&trades_before) {
                assert_eq!(
                    &row_json(&world, TRADE_ROW, *trade).await,
                    before,
                    "{case}: a trade changed"
                );
            }
            let authorized: i64 = sqlx::query_scalar("SELECT count(*) FROM trades WHERE id = ANY($1) AND $2 IN (buyer_user_id, seller_user_id)")
                .bind(&trades)
                .bind(member)
                .fetch_one(&world.pool)
                .await
                .unwrap();
            assert_eq!(
                authorized, 3,
                "{case}: the party's participation no longer identifies it"
            );

            // The settlement reservation of existing trades is untouched; only
            // an organization rejection withdraws the uncommitted remainder.
            let (available, reserved) = world.stock(listing.inventory).await;
            assert_eq!(
                available + reserved,
                Decimal::from(100),
                "{case}: stock is conserved"
            );
            if route == Route::Organization {
                assert_eq!(
                    order_status(&world, listing.order).await,
                    "CANCELLED",
                    "{case}"
                );
                assert_eq!(
                    (available, reserved),
                    (Decimal::from(80), Decimal::from(20)),
                    "{case}"
                );
            } else {
                assert_eq!(order_status(&world, listing.order).await, "OPEN", "{case}");
                assert_eq!(
                    (available, reserved),
                    (Decimal::from(20), Decimal::from(80)),
                    "{case}"
                );
            }

            // New execution is refused.
            let state = account_state(&world, member).await;
            match route {
                Route::Account | Route::Analytics => {
                    assert_eq!(live_sessions(&world, member).await, 0, "{case}");
                    assert_eq!(
                        validate_account(
                            &access_issued_at(member, rejected_at - Duration::minutes(1)),
                            &state
                        ),
                        Err(AuthError::SessionRevoked),
                        "{case}: a token issued before the rejection still works"
                    );
                    assert_eq!(
                        validate_account(&access_issued_at(member, Utc::now()), &state),
                        Err(AuthError::Rejected),
                        "{case}: a rejected account authenticated"
                    );
                }
                Route::Organization | Route::Kyc => {
                    let party = kyc_party(&world, member).await;
                    let decision = eligibility(&KycPolicy::default(), &party, None, Utc::now());
                    assert!(!decision.eligible, "{case}: {decision:?}");
                    let expected = if route == Route::Kyc && !unlinked {
                        KycReason::Rejected
                    } else {
                        KycReason::Required
                    };
                    assert_eq!(decision.reason, expected, "{case}");
                    assert!(
                        !eligible_in_sql(&world, member).await,
                        "{case}: the SQL mirror admits the party"
                    );
                }
            }
        }
    }
    world.finish().await;
}
