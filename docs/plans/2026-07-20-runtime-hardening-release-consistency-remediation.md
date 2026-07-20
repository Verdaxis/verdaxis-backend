# Runtime Hardening Release Consistency Remediation Plan

> **For Codex:** Use the executing-plans workflow and keep all execution inside the supplied runtime worktree.

**Goal:** Close the remaining runtime branch blockers with exact object ACL repair, failure-safe code/release identity, meaningful dry-run checks, and production-hardened analytics prune units.

**Architecture:** Treat PostgreSQL object ACL policy as an exact reconstructed set. During deployment, block runtime unit starts before source mutation, select and attest one clean exact remote commit, atomically publish that commit's runtime identity before invoking any executable from the selected tree, and preserve the aligned code/identity plus a fail-closed guard after later failures. Dry-run performs the read-only equivalents of every candidate/config/runtime/Alembic check. Keep integration-owned security and local-monitor work as explicit merge gates only.

**Tech Stack:** Bash, Git, Python 3.12, SQLAlchemy/Alembic, PostgreSQL 17/PostGIS 3.6, systemd, pytest.

---

### Task 1: Exact governed-object ACL repair

**Files:** `tests/postgres/test_runtime_role_policy.py`, `deploy/postgres/bootstrap_roles.sql`, `deploy/postgres/validate_roles.sql`

1. Add a disposable PostgreSQL regression covering unrelated direct grants, grant options, delegated grants, and PUBLIC privileges on governed tables, partitions, and sequences.
2. Run the focused test and prove validation still fails after the current bootstrap.
3. Discover every non-owner ACL grantee with `aclexplode`, revoke all governed object privileges with intentional `CASCADE`, then reconstruct the exact app/backup policy.
4. Rerun bootstrap twice and validation, then assert all unexpected and delegated authority is gone.

### Task 2: Failure-safe deployment provenance and real dry-run

**Files:** `tests/unit/test_deploy_health.py`, `scripts/deploy.sh`, `.gitignore`, `deploy/systemd/verdaxis-*.service`

1. Add behavioral tests using temporary Git remotes and fake runtime commands for preflight, dependency, and Alembic failures after a new remote commit is selected.
2. Assert every failure retains the new exact source and matching atomic release identity under a fail-closed deployment guard; prohibit identity-only rollback.
3. Add dry-run tests proving real branch/source cleanliness, remote-SHA, runtime preflight, dependency resolver/check, Alembic-head, and final-cleanliness checks run while mutations remain skipped and accurately reported.
4. Add the deployment guard to every runtime-owned service so no new-tree executable/timer can start while identity publication or failure recovery is incomplete.

### Task 3: Harden analytics prune ownership

**Files:** `tests/unit/test_product_analytics_prune.py`, `deploy/systemd/verdaxis-product-analytics-prune*.service`, `deploy/systemd/verdaxis-product-analytics-prune*.timer`

1. Add tests for PostgreSQL/network/mount ordering, bounded boot retries, and parity with news-service sandboxing.
2. Run the focused tests red.
3. Add exact prod/staging dependencies, retry bounds, filesystem/device/capability/address-family/system-call restrictions, and deployment guards.
4. Verify all runtime-owned units with `systemd-analyze verify` without installing them.

### Task 4: Documentation and integration gates

**Files:** `docs/runtime-hardening.md`, `docs/news-refresh-timer.md`, `ARCHITECTURE.md`, `README.md`, `CLAUDE.md`, `tasks/lessons.md`

1. Document exact ACL reconstruction and the deploy guard/code/identity failure contract, including dry-run checks and skipped mutations.
2. State that the isolated runtime branch is not independently deployable with security until integration reparents `sec_20260720_identity` to `rh_20260720_runtime_metadata`, preserves the remaining security chain through `sec_20260720_device`, and adds both prod/staging auth-maintenance service/timer pairs to the combined immutable allowlists.
3. Record that local-monitor's identity-only rollback would recreate stale SHA/new bytes and must be removed or paired with atomic code restoration during integration.
4. Keep KYC advisory-only, singleton news ownership, and runtime-normalization-only migration boundaries intact.

### Task 5: Full verification and commit

1. Run all focused tests, the full unit suite, and the full disposable PostgreSQL 17/PostGIS suite.
2. Run shell syntax, Python compile, `git diff --check`, systemd verification, and source-scope/diff checks.
3. Confirm no live state was touched and disposable resources were removed.
4. Commit code, tests, lessons, plan, and owned documentation together; report the full commit SHA, exact files, and test counts.
