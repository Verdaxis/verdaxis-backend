# Account Approval Email Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reliably send a Verdaxis sign-in email when an administrator genuinely approves a user account.

**Architecture:** Persist the exact pending approval-transition UUID, immutable provider payload, and due time on the user in the admission transaction. After commit, lock and re-read the user before replaying that payload with the transition UUID as the Resend idempotency key. Extend the existing bounded auth-maintenance job to retry due markers fairly and discard notifications invalidated by rejection.

**Tech Stack:** Python 3.12, FastAPI, SQLAlchemy async, httpx/Resend, pytest.

---

### Task 1: Encode provider idempotency and copy contracts

**Files:**
- Modify: `tests/unit/test_email_logging.py`
- Modify: `app/services/email.py`

1. Add failing tests that assert the approval subject, `FRONTEND_URL` sign-in
   link, escaped recipient name, and `Idempotency-Key` request header.
2. Run `pytest tests/unit/test_email_logging.py -q` and confirm the missing
   service function fails.
3. Add an approval-payload builder and sender using the existing branded
   conventions and optional `_send_email` idempotency header.
4. Rerun the focused test and confirm it passes.

### Task 2: Persist transition-scoped pending delivery

**Files:**
- Modify: `tests/unit/test_organization_join_review.py`
- Modify: `app/models/user.py`
- Create: `alembic/versions/ae_20260811_account_approval_email.py`
- Modify: `deploy/migration-checkpoints.tsv`
- Modify: `app/routers/auth_simple.py`

1. Add failing tests proving `PENDING -> APPROVED` and `REJECTED -> APPROVED`
   persist a transition marker, frozen payload, and due time; an
   already-approved account does not replace them; and provider failure does
   not fail or reverse approval.
2. Run the focused tests and confirm they fail before implementation.
3. Add the nullable marker, JSON payload, due time, and due-work index with no
   historical backfill; wire the migration checkpoint; and populate them
   before approval commit.
4. Run both focused files and confirm they pass.

### Task 3: Deliver immediately and retry durably

**Files:**
- Create: `app/services/account_approval_email.py`
- Modify: `app/routers/auth_simple.py`
- Modify: `app/cli/auth_maintenance.py`
- Modify: `tests/unit/test_auth_maintenance.py`
- Modify: `docs/auth-maintenance-timer.md`

1. Add failing tests for frozen payload replay, pre-send eligibility checks,
   provider failure deferral, successful acknowledgement, stale-transition
   protection, null-role eligibility, fair bounded retry, and CLI counts.
2. Implement row-locked immediate delivery, exact-marker acknowledgement,
   retry scheduling, and bounded maintenance retries.
3. Keep provider failures as normal pending work; propagate database failures
   from maintenance while isolating them from the already-committed API action.
4. Run focused approval, email, auth-maintenance, and migration tests.

### Task 4: Verify, release, and repair the missed notification

**Files:**
- Modify if required by ownership check: `ARCHITECTURE.md`
- Modify if required by source pin: `tests/monitor/test_source_ownership.py`

1. Run `git diff --check`, focused tests, monitor/source-ownership tests, and
   the backend unit suite.
2. Review the diff for duplicate sends, premature sends, leaked PII, and email
   failure coupling.
3. Commit and push the feature to `staging`; deploy the exact staging release
   and verify readiness.
4. Promote the reviewed commit to `prod`; deploy the exact production release
   and verify readiness plus the external monitor.
5. Select the missed recipient from the production approval audit event and
   invoke the committed email service once. Confirm a successful provider
   response without logging the raw address.
6. Remove the temporary worktree and leave staging and production clean and
   synchronized with GitHub.
