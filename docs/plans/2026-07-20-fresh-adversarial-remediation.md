# Fresh Adversarial Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Resolve all nine fresh backend security findings without changing live state or enabling KYC execution enforcement.

**Architecture:** Extend the existing security-v2 boundaries with one linear migration and narrow shared helpers. Keep browser trust environment-derived, market acceptance transactional, KYC evidence organization-bound but advisory, private stream scope token-bound, operator review read-only unless an explicit existing review action is invoked, and all external ingestion/logging bounded.

**Tech Stack:** FastAPI, Pydantic v2, SQLAlchemy 2 async, PostgreSQL/PostGIS, Alembic, PyJWT, httpx, feedparser, pytest.

---

### Task 1: Browser origin and proxy boundaries

**Files:**
- Modify: `app/config.py`, `app/main.py`, `app/routing.py`, `app/rate_limit.py`, `app/middleware/preauth_rate_limit.py`
- Modify: `.env.example`, `deploy/systemd/verdaxis-backend.service`
- Test: `tests/unit/test_browser_security_boundaries.py`, `tests/unit/test_preauth_rate_limit.py`

1. Add failing tests for exact production/staging CORS sets, staging-origin cookie rejection against production, safe same-origin cookie use, and untrusted direct X-Forwarded-For handling.
2. Run the focused tests and confirm the expected failures.
3. Add an environment-derived origin policy and a shared sensitive-cookie Origin guard; bind login, refresh, logout, and password-change cookie transitions.
4. Make all application rate-limit keys trust forwarded IPs only from loopback proxies and bind the production systemd unit to `127.0.0.1`.
5. Run focused tests to green.

### Task 2: Atomic negotiation capacity

**Files:**
- Modify: `app/routers/negotiations.py`
- Test: `tests/unit/test_negotiation_security.py`, `tests/postgres/test_negotiation_capacity.py`

1. Add failing unit and PostgreSQL tests proving acceptance consumes canonical locked order capacity/status and concurrent negotiations cannot overfill one order.
2. Run tests and confirm failure due to unchanged remaining capacity.
3. Add bounded `FOR UPDATE NOWAIT` handling and decrement every referenced canonical order in the acceptance transaction, updating OPEN/PARTIALLY_FILLED/FILLED status.
4. Run focused unit and disposable-PostgreSQL tests to green.

### Task 3: Organization-bound advisory KYC and review operations

**Files:**
- Create: `alembic/versions/sec_20260720_org_bound_kyc.py`
- Modify: `app/models/user.py`, `app/routers/kyc.py`, `app/routers/auth_simple.py`
- Modify: `app/services/execution_policy.py`, `app/services/audit_actions.py`
- Test: `tests/unit/test_kyc_org_binding.py`, `tests/unit/test_admin_review_queue.py`, `tests/postgres/test_kyc_org_migration.py`

1. Add failing tests for submission/review org binding, membership-change invalidation/audit, legacy unknown preservation, current-org execution checks, advisory-only KYC behavior, and admin queue/detail projections.
2. Run tests and confirm expected failures.
3. Add nullable KYC organization/submission metadata fields. Never infer legacy ownership or alter legacy KYC approval state in migration.
4. Invalidate mismatched org-specific KYC atomically on membership removal/change and audit the transition.
5. Require reviews to match current membership. Expose bounded admin-only queue/detail endpoints with admission, email, organization, requested-organization, KYC, and evidence metadata.
6. Keep KYC out of execution eligibility until product-owner approval, while retaining exact current user/org matching.
7. Run focused tests to green.

### Task 4: Operator preflight and safe migration downgrade

**Files:**
- Create: `app/cli/security_preflight.py`, `scripts/run_security_preflight.sh`
- Modify: `scripts/security_preflight.py`, `alembic/versions/sec_20260720_admission_boundaries.py`
- Modify: `docs/security-hardening-v2.md`
- Test: `tests/unit/test_security_preflight.py`, `tests/postgres/test_security_migrations.py`

1. Add failing tests for repo-root execution, review counts/held actions, unknown KYC ownership, and downgrade with a valid post-upgrade long fuel value.
2. Run tests and confirm direct script/import and narrowing failures.
3. Move preflight implementation into an importable app CLI, provide a root-checked shell entrypoint, and preserve the widened fuel column on downgrade with explicit rationale.
4. Run unit and disposable-PostgreSQL migration tests to green.

### Task 4A: Lifecycle-correct execution invalidation

**Files:**
- Modify: `app/services/execution_invalidation.py` and the canonical order/inventory cancellation helpers
- Modify: `app/routers/auth_simple.py`, `app/routers/kyc.py`
- Test: `tests/unit/test_execution_invalidation.py`, `tests/postgres/test_execution_invalidation_semantics.py`, `tests/postgres/test_execution_invalidation_races.py`

1. Add failing tests proving invalidation never uses bulk status updates, locks canonical slices/orders/inventory in sorted order, releases only unfilled reserved inventory, audits in the transition transaction, commits before publication, and conserves stock under cancellation/fill races.
2. Run tests and confirm failures against the current bulk-update implementation.
3. Extract/reuse the normal cancellation lifecycle so security transitions and owner cancellations share status, reservation-release, audit, and event behavior.
4. Return staged post-commit events from invalidation; callers publish only after a successful commit.
5. Run focused unit and disposable-PostgreSQL race/conservation tests to green.

### Task 5: Organization-bound stream tokens

**Files:**
- Modify: `app/core/security.py`, `app/routers/auth_simple.py`, `app/routers/stream.py`, `app/routers/activity.py`
- Test: `tests/unit/test_stream_tokens.py`, `tests/unit/test_stream_authorization.py`

1. Add failing tests for mandatory `org_id`, exact issuer/audience/environment claims, connect-time org matching, and periodic membership invalidation.
2. Run tests and confirm missing-claim failures.
3. Issue stream tokens only with the current organization and reject any connect/revalidation mismatch.
4. Run focused tests to green.

### Task 6: Bounded RSS and private email logs

**Files:**
- Modify: `app/services/news_feed.py`, `app/services/email.py`
- Modify: `docs/news-refresh-timer.md`
- Test: `tests/unit/test_news_feed.py`, `tests/unit/test_email_logging.py`

1. Add failing tests for streamed byte caps, high-entry truncation, per-run provider/DB caps, and recipient-safe bounded email logs.
2. Run tests and confirm failures.
3. Stream RSS responses with hard byte limits; cap entries per feed/run, provider calls, DB URL queries/inserts, timeout, and concurrency.
4. Hash recipient identity in logs and emit only exception class/status metadata on failure.
5. Run focused tests to green.

### Task 7: Full verification and commit

**Files:**
- Modify: `ARCHITECTURE.md`, `CLAUDE.md`, `docs/security-hardening-v2.md`, `docs/news-refresh-timer.md`

1. Update owned architecture/security/operator documentation with the implemented contracts and explicitly held actions.
2. Run all unit tests with exact counts.
3. Run the complete disposable-PostgreSQL suite, including migrations and concurrency, with exact counts.
4. Run Alembic heads/check and repository-root preflight help/smoke checks.
5. Review the diff for secrets, live-state changes, DRY, edge cases, and security regressions.
6. Commit all changes on `hardening/security-v2` and report SHA, files, test counts, migration head, and held operator actions.
