# Runtime Hardening V2 Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close every runtime hardening review gap with exact deployed identity, least-privilege PostgreSQL policy, immutable release health gates, safe operator tooling, and PostgreSQL-backed downgrade proof.

**Architecture:** Keep the existing Settings/database/deploy/role-policy boundaries. Tighten each boundary to exact environment-owned identities and exact ACLs, add one small JSON health validator and one non-starting systemd installer, and prove destructive database behavior only in the disposable PostgreSQL 17/PostGIS harness.

**Tech Stack:** Python 3.12, Pydantic v2, SQLAlchemy 2 async, FastAPI, Alembic, Bash, PostgreSQL 17/PostGIS 3.6, pytest.

---

### Task 1: Exact deployed runtime and migration identity

**Files:** `tests/unit/test_runtime_hardening.py`, `app/config.py`, `app/database.py`, `alembic/env.py`

1. Add failing tests for exact production/staging database and app/migrator roles, distinct identities, no SQLite/default JWT/bypass, current database, least-privilege role properties, membership rejection, topology count, and maintenance reserve.
2. Run the focused tests and confirm the expected failures.
3. Implement exact Settings and connection attestation, rejecting all URL query parameters on migration targets.
4. Set `hide_parameters=True` on application and migration engines and rerun focused tests.

### Task 2: Exact PostgreSQL ownership and ACL policy

**Files:** `tests/postgres/test_runtime_role_policy.py`, `deploy/postgres/bootstrap_roles.sql`, `deploy/postgres/validate_roles.sql`, `scripts/run_product_analytics_postgres_tests.sh`, `.github/workflows/backend-ci.yml`

1. Add PostgreSQL tests proving memberships, stale direct/default ACLs, Alembic/PostGIS mutation, ownership, and backup INSERT are rejected.
2. Run the disposable role-policy tests and confirm failures.
3. Make bootstrap remove memberships and stale ACLs, transfer database/schema/app-object ownership, and grant only exact app-object privileges.
4. Make validation compare exact role properties, memberships, owners, object ACLs, default ACLs, and excluded-object mutation rights.
5. Rerun the disposable role-policy tests.

### Task 3: Release, health, and operator gates

**Files:** `tests/unit/test_runtime_hardening.py`, `tests/unit/test_deploy_health.py`, `scripts/validate_health_response.py`, `scripts/deploy.sh`, `scripts/install_systemd_units.sh`, `deploy/systemd/*.service`

1. Add failing tests for wrong health environment/SHA, dirty artifacts, release ordering, unit installation/daemon reload, and preflight behavior.
2. Implement strict JSON health validation and clean-tree immutable release handling.
3. Add an explicit idempotent systemd install path that validates units and runtime environment/migration preflights but never starts services.
4. Document forward-only rollback via a new revert release identity; never automate Alembic downgrade.

### Task 4: Migration, seed routing, and sanitized database errors

**Files:** `tests/unit/test_seed_safety.py`, `tests/unit/test_db_errors.py`, `tests/postgres/test_runtime_migration_roundtrip.py`, `app/seeds/safety.py`, `app/services/db_errors.py`, `app/main.py`, `alembic/versions/rh_20260720_runtime_metadata.py`

1. Add failing tests for URL routing query rejection, bounded log fields, hidden parameters, and down-revision schema equivalence including `commissions.match_id NOT NULL`.
2. Implement the smallest fixes and rerun focused tests.

### Task 5: Documentation and verification

**Files:** `docs/runtime-hardening.md`, `CLAUDE.md`, `ARCHITECTURE.md`, `.env.example`

1. Document exact identities, mandatory credential rotation (history rewrite is not remediation), operator-held systemd installation, immutable topology, exact health/release behavior, and forward-only rollback procedure.
2. Run the full unit suite.
3. Run the complete disposable PostgreSQL 17/PostGIS suite.
4. Review the full diff for secrets, scope, DRY, ownership, and doc sync; commit once all gates pass.
