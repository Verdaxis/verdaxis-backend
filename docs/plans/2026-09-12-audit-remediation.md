# Verdaxis Backend Audit Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Correct confirmed backend availability and maintenance gaps and implement the owner's seller-fee policy without changing live services.

**Architecture:** Reuse the existing email claim sequence, authentication locks, provider adapter, and monitor. Work from the production base in an isolated branch. Use synthetic tests and disposable PostgreSQL only; normal test-provider mocks must prevent external sends.

**Tech Stack:** FastAPI, Python, SQLAlchemy async, PostgreSQL, pytest, AnyIO, existing Gemini adapter.

---

## Boundaries

Base `1b88213f884948b9c218143c8a0540cec96e7f57`, branch `fix/audit-20260912`. No production or staging mutations, deployments (including deploy dry-runs), service changes, live provider calls, email sends, or customer-row experiments. Preserve existing dirty lesson files. Unanswered-trade expiry and settlement authority remain unanswered product choices; do not infer them. The owner authorized parallel implementation from 05:31:27 to 07:01:27 UTC on 12 September 2026, with separate file ownership and independent review.

## Task 1 — Approval email claim outside customer lock (R5)

Files: `app/services/account_approval_email.py`, `tests/unit/test_account_approval_email.py`, `tests/postgres/test_account_approval_email_concurrency.py`, `ARCHITECTURE.md`.

1. Read all callers and the existing `order_expiry_reminders.py` claim/commit/send/finalize sequence.
2. Change the existing concurrency test to require that another transaction can acquire the user lock while provider I/O is blocked. Add a focused exact-transition replacement/duplicate-claim regression using existing fixtures.
3. Run the regression to establish failure. Use the existing pending retry field for a short persisted claim; commit before awaiting the provider. Snapshot the immutable payload and transition. Re-lock and revalidate that same claim when finalizing, so stale work cannot clear a newer transition.
4. Preserve skip/discard/retry behavior, provider idempotency, preference/eligibility rules, and crash retry. Do not introduce a second queue or new schema unless the existing fields cannot express the invariant.
5. Run unit and disposable PostgreSQL checks. Update the architecture description. Obtain independent specification review, then code review. Commit only this batch.

## Task 2 — Bounded async password work (R6)

Files: `app/core/security.py`, actual async password-helper callers, existing authentication unit tests, and one focused event-loop/capacity check.

1. Trace every hash/verification caller; keep synchronous helpers for synchronous tools.
2. Add the smallest shared async wrapper using the installed thread-offload facility and a documented capacity bound. Keep session/device lock ordering and account revalidation. Do not weaken rate limits or auth checks.
3. Verify no async request path performs bcrypt on the event loop. Verify a running password task does not block unrelated async work and cancellation does not release capacity before the worker finishes.
4. Run the relevant auth tests and independent specification/code review before committing.

## Task 3 — Event backlog monitoring source (R7)

Files: `deploy/monitor/outbox_backlog_probe.py`, canonical `deploy/external_monitor` source and its actual contract tests/docs.

1. Trace how the active public monitor executes bounded local checks and whether the existing read-only probe has the needed identity/config contract.
2. Add the existing backlog check to the canonical source path, preserving fixed environment targeting and sanitized output. Alert on sustained pending age, not zero rows or demo traffic.
3. Test healthy, pending/stalled, and unavailable probe states using synthetic inputs. Keep recovery/activation outside this change. Update exact artifact manifests if required by the existing source contract.
4. Review and commit the source change. Mark activation as pending operator release; never claim the live alert is armed from source edits alone.

## Task 4 — Supported Gemini client (R8)

Files: `app/services/gemini_provider.py`, its actual callers if needed, `requirements.txt`, existing provider-bound tests.

1. Verify the supported official `google-genai` API and installed dependency compatibility.
2. Extend existing tests for the shared adapter's request/response shape, native timeout, retry, cancellation, and capacity semantics using fakes only.
3. Replace the discontinued SDK inside the shared adapter and pin the new dependency. Preserve KYC advisory-only behavior, chat/news output contracts, and missing-key behavior.
4. Install dependencies only in a repair-owned environment, run all affected provider tests, then independent specification/code review.

## Task 5 — Configurable seller fees (R9)

The owner confirmed: seller pays, buyers always free; Pilot $2/MT and Professional $1.50/MT by default. Use validated Decimal settings for those rates and the existing admin subscription path for a negotiated Enterprise rate. Resolve only the seller's effective plan and snapshot rate and plan on each new manual or matched trade. Retain the old percentage calculation for historical rows without a per-MT snapshot. Apply delivered quantity to the stored rate, never today's settings. Add nullable columns without a fee backfill, database integrity checks, public schedule and response fields, and synthetic plus disposable PostgreSQL regressions. No fee charge or subscription change is made against live data.

## Deferred decisions and capacity work

- Historical fee repricing is not authorized. Existing trade snapshots remain unchanged.
- Pending confirmation expiry and bilateral settlement: product decisions, not approved automatic transitions.
- Trade-order indexes: follow with a disposable production-shaped query-plan check before adding a migration; no evidence of current query latency.
- Outbox pruning: zero current rows; do not build speculative retention work.
- Low-priority collection-contract changes must preserve consumers and can follow the first repair pass.

## Verification command and final gate

Run tests from this worktree with explicit `ENVIRONMENT=test`, `RELEASE_SHA=test`, a synthetic JWT secret, and `DATABASE_URL=sqlite+aiosqlite:///:memory:`. Never load a live `.env`. PostgreSQL checks must point to a disposable database and meet the repository's test guard. No integration/E2E suite may default to live localhost ports. Finish with the applicable full unit suite, diff review, and a record of commit IDs and remaining release gates.

## Progress

- [x] Isolated repair branch created.
- [x] R5 approval-email lock repair: specification, quality, and disposable PostgreSQL checks passed; commit `5e51042`.
- [x] R6 bounded password offload: specification and quality checks passed, including queued/running cancellation; commit `5464a75`.
- [x] R7 monitoring source: all 223 monitor tests, specification, and installation-prerequisite quality review passed; commit `4139b0e`. Activation remains separate.
- [x] R8 supported Gemini SDK: isolated dependency checks, 24 focused adapter tests, specification, and quality review passed; commit `5274aa4`. Release must plan a clean venv or approved retired-SDK cleanup.
- [x] R9 seller fees: unit and disposable PostgreSQL route/migration/concurrency checks, specification, and quality review passed; commit `b19d28b`. Active Enterprise sellers require reviewed negotiated rates before rollout.
- [x] Final regression gate: 1,390 unit tests and 223 monitor tests passed on the final source; diff check passed. Remaining provider test-double signatures match the reviewed deadline argument.
- [x] Final complete PostgreSQL trade-route suite: 16 passed on committed source; the pinned disposable harness completed migration, ACL, and cleanup checks without live database access.
- [ ] Separate operator release authorized (not part of this request).
