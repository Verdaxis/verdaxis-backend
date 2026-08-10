# Account Approval Email Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Send a Verdaxis sign-in email exactly once when an administrator genuinely approves a user account.

**Architecture:** Add one branded template to the existing Resend email service and invoke it after the canonical account-approval transaction commits. Gate delivery on the prior account status so repeat requests are idempotent, and keep email failure isolated from account admission.

**Tech Stack:** Python 3.12, FastAPI, SQLAlchemy async, httpx/Resend, pytest.

---

### Task 1: Encode the email contract

**Files:**
- Modify: `tests/unit/test_email_logging.py`
- Modify: `app/services/email.py`

1. Add a failing test that captures the low-level send call and asserts the
   approval subject, `FRONTEND_URL` sign-in link, and escaped recipient name.
2. Run `pytest tests/unit/test_email_logging.py -q` and confirm the missing
   service function fails.
3. Add `send_account_approved_email(to_email, name)` using the existing branded
   email conventions and `_send_email` helper.
4. Rerun the focused test and confirm it passes.

### Task 2: Wire the canonical approval transition

**Files:**
- Modify: `tests/unit/test_organization_join_review.py`
- Modify: `app/routers/auth_simple.py`

1. Add failing tests proving a `PENDING -> APPROVED` transition sends after
   commit, an already-approved account does not send, and provider failure does
   not fail or reverse approval.
2. Run the focused tests and confirm they fail before implementation.
3. Import and call `send_account_approved_email` after commit only when
   `previous_status != UserStatus.APPROVED`.
4. Run both focused files and confirm they pass.

### Task 3: Verify, release, and repair the missed notification

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

