# Runtime Hardening Final Review Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the four final runtime correctness gaps with exact PostgreSQL ACL sets, release-bound systemd source provenance, one locked external news owner per environment, and symmetric deployed password validation.

**Architecture:** Keep the existing runtime identity and disposable-test boundaries. Normalize database/schema ACLs by expanded privilege set, bind each environment's unit installation independently to its own explicit Git SHA and clean checkout, invoke news refresh only through source-controlled systemd timers plus a PostgreSQL advisory lock, and share one placeholder-password predicate across app and migrator URLs.

**Tech Stack:** Bash, Python 3.12, Pydantic v2, FastAPI, SQLAlchemy 2 async, PostgreSQL 17/PostGIS 3.6, systemd, pytest.

---

### Task 1: Exact database and schema ACLs

**Files:** `tests/postgres/test_runtime_role_policy.py`, `deploy/postgres/bootstrap_roles.sql`, `deploy/postgres/validate_roles.sql`

1. Add a disposable regression that grants an unrelated role database CREATE/TEMP and public-schema CREATE.
2. Run it and prove the current bootstrap/validator allow the stale authority.
3. Revoke every non-allowlisted database/schema ACL grantee and reconstruct exact expected privileges.
4. Validate expanded database/schema ACL rows by set equality and rerun the regression.

### Task 2: Release-bound systemd unit provenance

**Files:** `tests/unit/test_deploy_health.py`, `scripts/install_systemd_units.sh`, `scripts/verify_systemd_source.py`

1. Add tests for dirty source, wrong source SHA, changed unit bytes, and independent environment refs in dry-run/apply paths.
2. Run them and confirm the current installer cannot satisfy the contract.
3. Require one environment and full `--source-ref` per invocation; source units only from that environment's fixed clean checkout.
4. Verify each unit's working-tree digest against the exact Git blob at the approved SHA before any copy.

### Task 3: One locked external news owner per environment

**Files:** `tests/unit/test_news_refresh_cli.py`, `tests/postgres/test_news_refresh_lock.py`, `tests/unit/test_runtime_identity_v2.py`, `app/cli/refresh_news.py`, `app/routers/news.py`, `app/services/news_feed.py`, `deploy/systemd/verdaxis-news-refresh*.{service,timer}`, `docs/news-refresh-timer.md`

1. Add tests proving no public manual refresh, no worker scheduler, one CLI entrypoint, one timer owner per environment, and PostgreSQL overlap rejection.
2. Run focused unit tests to observe missing CLI/timers and the public route.
3. Add the CLI, transaction advisory lock, and exact prod/staging service/timer artifacts; remove the POST refresh route.
4. Run unit and disposable lock tests.

### Task 4: Symmetric deployed password validation

**Files:** `tests/unit/test_runtime_identity_v2.py`, `app/config.py`, `.env.example`, `docs/runtime-hardening.md`, `ARCHITECTURE.md`, `README.md`, `CLAUDE.md`

1. Add parameterized app/migrator placeholder-password rejection tests without exposing values in errors.
2. Run the tests and observe migrator placeholders pass.
3. Apply one shared non-default password predicate to both deployed URLs and update operator documentation.
4. Run focused tests, the full unit suite, and the full disposable PostgreSQL 17/PostGIS suite.

### Task 5: Final verification and commit

1. Run shell syntax, Python compile, `git diff --check`, and source-scope checks.
2. Confirm no live service/database action occurred and the disposable container was removed.
3. Commit all code, tests, lessons, and owned documentation together.
