# Runtime Hardening Follow-up Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the remaining default-ACL, deployed-password, prune-unit provenance, and systemd installation TOCTOU gaps without touching live state.

**Architecture:** Normalize default privileges across global and `public` scopes for all protected owners, rebuild only the migrator's intended future-object policy, and use explicit cascading revocation for non-allowlisted database/schema grants. Bind destructive pruning to an explicit deployed environment/release identity. Build a verified archive exclusively from approved Git commit blobs, extract it into a private root-owned staging directory, and use only those staged bytes for syntax checks and installation.

**Tech Stack:** PostgreSQL 17 ACL catalogs/psql, Python 3.12, Pydantic v2, Bash, Git object plumbing, systemd, pytest.

---

### Task 1: Complete ACL normalization

**Files:**
- Modify: `tests/postgres/test_runtime_role_policy.py`
- Modify: `deploy/postgres/bootstrap_roles.sql`
- Modify: `deploy/postgres/validate_roles.sql`

1. Add a disposable regression that creates global and `public` default ACLs for protected owners and unexpected grantees, plus delegated database/schema grants.
2. Run it and verify validation rejects the stale policy and bootstrap cannot fully repair it.
3. Discover default ACL grantees with `aclexplode` for global and `public` rows across protected owners and PostgreSQL 17 object types; revoke each in the correct scope, then reconstruct only intended migrator table/sequence defaults.
4. Add intentional `CASCADE` to complete database/schema resets before allowlisted grants are reconstructed.
5. Validate exact global/public default ACL sets and prove a newly created public table receives only owner/app/backup privileges.

### Task 2: Reject decoded blank deployed passwords

**Files:**
- Modify: `tests/unit/test_runtime_identity_v2.py`
- Modify: `app/config.py`

1. Add app/migrator tests for empty and percent-encoded whitespace passwords in production and staging, including credential-redaction assertions.
2. Run them and confirm decoded blank passwords currently pass.
3. Normalize the decoded password once and reject blank or known placeholder values.
4. Rerun the focused settings tests.

### Task 3: Bind product-analytics pruning to release identity

**Files:**
- Modify: `tests/unit/test_product_analytics_prune.py`
- Modify: `tests/unit/test_deploy_health.py`
- Modify: `scripts/prune_product_analytics.py`
- Modify: `deploy/systemd/verdaxis-product-analytics-prune.service`
- Modify: `deploy/systemd/verdaxis-product-analytics-prune-staging.service`
- Modify: `scripts/install_systemd_units.sh`

1. Add tests requiring both prune service/timer pairs in the exact environment unit sets, `.runtime-release.env` loading, literal environment identity, full release SHA, and refusal of absent/development CLI identity.
2. Run them and observe the missing provenance/configuration controls.
3. Require explicit deployed environment and release arguments before creating the prune engine; compare them with validated settings.
4. Update both services and installer unit allowlists, then rerun focused tests.

### Task 4: Eliminate unit-installation TOCTOU

**Files:**
- Modify: `tests/unit/test_deploy_health.py`
- Modify: `scripts/verify_systemd_source.py`
- Modify: `scripts/install_systemd_units.sh`

1. Add a fault-injection test that mutates worktree unit bytes after checkout attestation and proves the candidate archive still contains the approved commit bytes and exact manifest digests.
2. Run it and confirm the current installer has no immutable candidate artifact.
3. Make the verifier emit a tar archive and SHA-256 manifest built only from `git show` commit blobs.
4. Extract it into a validated private root-owned temporary directory; verify hashes and unit syntax there; compare/install only staged paths; clean it through a guarded trap.
5. Add static assertions that no post-staging path reopens `SOURCE_ROOT/deploy/systemd` and rerun dry-run/apply provenance tests.

### Task 5: Documentation, full verification, and commit

**Files:**
- Modify: `.env.example`
- Modify: `ARCHITECTURE.md`
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: `docs/runtime-hardening.md`
- Modify: `tasks/lessons.md`

1. Document both default-ACL scopes, intentional cascade, prune release identity, and immutable root-owned unit staging.
2. Run shell syntax, Python compilation, `git diff --check`, and systemd unit verification.
3. Run the complete unit suite and disposable PostgreSQL 17/PostGIS suite.
4. Confirm the disposable container is gone, preserve the untracked `venv` symlink, and commit only owned files.
