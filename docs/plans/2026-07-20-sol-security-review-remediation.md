# Sol Security Review Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the remaining browser-session ordering, migration-autogenerate, invalidation-contention, operator-preflight, review-queue, and PostgreSQL-version findings without enabling runtime KYC enforcement.

**Architecture:** Every browser receives an opaque path-scoped HttpOnly device cookie whose SHA-256 hash, never its raw value, is stored on refresh-session rows. Login, refresh, and logout acquire one transaction-scoped PostgreSQL advisory lock derived from that hash before touching refresh families; login and logout revoke all device families. Legacy NULL-device rows remain nullable for schema compatibility but are revoked during migration and never rebound, forcing a safe fresh sign-in. API invalidation callers share one rollback-and-retry response adapter, and Alembic discovers extension-owned relations from PostgreSQL catalogs while limiting autogenerate to the public application schema.

**Tech Stack:** FastAPI, SQLAlchemy asyncio, Alembic, PostgreSQL 17, PostGIS 3.5, pytest, Docker.

---

### Task 1: Browser device refresh serialization

**Files:**
- Modify: `app/models/refresh_session.py`
- Modify: `app/routers/auth_simple.py`
- Create: `alembic/versions/sec_20260720_device_sessions.py`
- Modify: `tests/unit/test_auth_cookies.py`
- Modify: `tests/unit/test_refresh_error_contract.py`
- Modify: `tests/postgres/test_refresh_session_concurrency.py`

1. Add failing cookie/model tests proving the identifier is opaque, HttpOnly, Secure, SameSite, path scoped, absent from response JSON/logs, and represented only by a hash in storage.
2. Add failing real PostgreSQL races for refresh-first/login-second, login-first/refresh-second, refresh-first/logout-second, and logout-first/refresh-second.
3. Add nullable `device_id_hash` migration state, revoke legacy NULL-device rows, and never guess a backfill.
4. Acquire `pg_advisory_xact_lock` from the device hash before user/session rows in login, refresh, and logout.
5. Revoke all device families on login/logout, permit only one-way legacy revocation under the lock, and fail closed when refresh lacks a valid device cookie or presents a NULL-device family.
6. Re-run focused unit and PostgreSQL tests.

### Task 2: PostgreSQL 17/PostGIS Alembic authority

**Files:**
- Modify: `alembic/env.py`
- Modify: `alembic/README`
- Modify: `.github/workflows/backend-ci.yml`
- Modify: `docker-compose.yml`
- Modify: `scripts/run_product_analytics_postgres_tests.sh`
- Modify: `tests/postgres/conftest.py`
- Create: `tests/postgres/test_alembic_check.py`
- Modify: PostgreSQL-version comments in tests/docs

1. Add a failing disposable-PG assertion that `alembic check` exits zero after upgrade.
2. Add a drift probe and assert `alembic check` becomes nonzero for a real public application column.
3. Query `pg_depend`, `pg_extension`, `pg_class`, and `pg_namespace` to discover extension-owned public relations dynamically.
4. Exclude every non-public schema through Alembic name filtering while retaining public app drift detection.
5. Pin CI, compose, and the disposable harness to `postgis/postgis:17-3.5`.

### Task 3: Stable execution-invalidation contention response

**Files:**
- Modify: `app/services/execution_invalidation.py`
- Modify: `app/routers/auth_simple.py`
- Modify: `app/routers/admin_analytics.py`
- Modify: `app/routers/kyc.py`
- Modify: `app/routers/orderbook.py`
- Create: `tests/postgres/test_invalidation_route_contention.py`
- Modify: `tests/unit/test_execution_invalidation.py`

1. Add route-level PostgreSQL races that hold canonical order locks while invoking KYC, user, organization, and membership review transitions.
2. Assert one `409 EXECUTION_INVALIDATION_BUSY` contract with `Retry-After: 1`, a rolled-back review transition, unchanged executable rows, and no publication.
3. Add one shared request adapter that recognizes SQLSTATE `55P03`, rolls back, and raises the stable response; re-raise unexpected database failures.
4. Route every admin/KYC/membership invalidation call through the adapter.

### Task 4: Operator preflight and fair review projection

**Files:**
- Modify: `scripts/security_preflight.py`
- Modify: `app/routers/auth_simple.py`
- Modify: `tests/unit/test_security_preflight.py`
- Modify: `tests/unit/test_organization_join_review.py`
- Create: `tests/postgres/test_security_preflight_postgres.py`

1. Add a no-organization approved/verified user and prove preflight counts it and reports `BLOCKED`.
2. Replace negated nullable conjunctions with explicit null-safe OR branches and include the count in blockers.
3. Add heavy and single-candidate join histories and prove each receives its own cap.
4. Select ranked join rows with `row_number() over (partition by user_id ...)` before projection.

### Task 5: Documentation, full verification, and commit

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `CLAUDE.md`
- Modify: `docs/security-hardening-v2.md`
- Modify: `docs/runbooks/security-v2-operator-review.md`
- Modify: `tasks/lessons.md`

1. Document device lock ordering, legacy transition, retryable contention contract, dynamic extension filtering, and PostgreSQL 17 authority.
2. Confirm Gemini and KYC eligibility remain advisory-only.
3. Run focused tests, all unit tests, the complete disposable PostgreSQL suite, `alembic heads`, and clean-PG `alembic check`.
4. Run diff/security review checks and commit the complete remediation.
