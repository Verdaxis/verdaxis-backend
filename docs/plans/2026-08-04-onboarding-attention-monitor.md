# Onboarding Attention Monitor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Send deduplicated Telegram alerts when a real production user reaches an actionable onboarding state, including a two-hour approved-without-login threshold.

**Architecture:** A production-only systemd timer invokes a small backend CLI every five minutes. The CLI reads authoritative onboarding records through the existing application database role, classifies one stage per candidate, and atomically persists identifier-only delivery state so Telegram failures retry without affecting signup.

**Tech Stack:** Python 3.12, SQLAlchemy async, standard-library JSON/atomic files and HTTP, pytest, systemd.

---

### Task 1: Classification contract

**Files:**
- Create: `app/services/onboarding_attention.py`
- Create: `tests/unit/test_onboarding_attention.py`

1. Write table-driven tests for rejected, expiring organization setup,
   unverified expiry, approval required, first-login overdue at two hours,
   complete, and excluded identities.
2. Run `pytest tests/unit/test_onboarding_attention.py -q`; expect failure.
3. Add immutable candidate/stage dataclasses and one explicit priority-ordered
   classifier with `FIRST_LOGIN_OVERDUE = timedelta(hours=2)`.
4. Rerun the focused test; expect pass.

### Task 2: Database projection and alert formatting

**Files:**
- Modify: `app/services/onboarding_attention.py`
- Modify: `tests/unit/test_onboarding_attention.py`

1. Test bounded candidate projection, final approval timestamp selection,
   canary/admin/provenance exclusions, and messages containing only approved
   operator fields.
2. Add one SQLAlchemy-backed loader for current production candidates and
   format one concise Telegram message per stage.
3. Run the focused test; expect pass.

### Task 3: Stateful CLI and Telegram delivery

**Files:**
- Create: `app/cli/onboarding_attention.py`
- Create: `tests/unit/test_onboarding_attention_cli.py`

1. Test dry-run no-send behavior, one delivery per stage, unchanged-stage
   suppression, delivery-failure retry, atomic state, and recovery delivery.
2. Implement `--dry-run`, `--state-file`, production-environment enforcement,
   required Telegram configuration, bounded HTTP delivery, atomic state, and a
   one-time `--bootstrap` that silently baselines historical stages.
3. Run both focused test files; expect pass.

### Task 4: Production timer and operations contract

**Files:**
- Create: `deploy/external_monitor/systemd/verdaxis-onboarding-attention.service`
- Create: `deploy/external_monitor/systemd/verdaxis-onboarding-attention.timer`
- Modify: `deploy/external_monitor/README.md`
- Modify: `ARCHITECTURE.md`

1. Add a production-fixed oneshot using the production checkout, existing
   backend and Telegram environment files, `StateDirectory`, five-minute
   timeout, and strict write boundaries.
2. Add a five-minute persistent timer and exact install/dry-run commands.
3. Verify units with `systemd-analyze verify` and Python with `py_compile`.

### Task 5: Release and activation

1. Run focused tests, backend unit tests, and `git diff --check`.
2. Commit docs, implementation, units, and tests together; push production and
   cherry-pick to staging without activating a staging timer.
3. Deploy the production backend through the approved immutable release flow.
4. Run the CLI against production with `--dry-run`, inspect the candidate
   messages, then record the historical baseline with `--bootstrap`.
5. Install and enable the production units, start one normal run, inspect
   journal/state, and confirm API readiness and clean worktrees.
