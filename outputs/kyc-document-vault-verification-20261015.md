# KYC vault verification — 15 October 2026

## Delivered

Encrypted document rows, strict key-ring loading, atomic submission/status/audit,
metadata history, audited ADMIN downloads, retention checks, migration/checkpoint,
append-only ACL policy, both systemd credential directives, and operator guidance.
KYC calls no external provider. DeepSeek is planned only. The initial blocker note
was merged into `docs/security-hardening-v2.md` and removed as requested.

## Verification

- Full requested unit suite: **1957 passed, 19 failed**, 496 warnings. All failures
  are in `tests/unit/test_deploy_health.py`; local `/bin/bash` lacks `mapfile` and
  `${value,,}`. Logs include `scripts/deploy.sh: line 479: mapfile: command not found`
  and `line 580: ... bad substitution`. No newer Bash is installed. These are local
  execution failures, not a passing deployment gate.
- Unit suite excluding that module: **1914 passed**, 496 warnings.
- Focused `test_kyc_document*` suite: **45 passed**, 11 warnings.
- PostgreSQL migration and role-policy selection: **13 skipped**. No disposable
  database is configured; `pg_isready -h 127.0.0.1 -p 5432` reports no response and
  Docker reports no daemon socket. Migration upgrade/downgrade and real role ACL
  enforcement remain unverified here. No live database was contacted.
- Locked dependency audit: `pip-audit --strict -r constraints.txt --no-deps
  --disable-pip`: **No known vulnerabilities found**. `cryptography==50.0.0`
  matches the repository's existing transitive lock (not the unrelated global
  Python installation's 49.0.0).
- Exact requirements audit command failed before auditing: its temporary Python
  `ensurepip` process terminated with SIGABRT. Do not treat that gate as passed.
- `compileall` and `git diff --check` passed. No systemd execution was possible on
  macOS. v249/v255 source verification and exact source links are in the design.
- Codesight helper dry-run passed. Full regeneration was not used because its
  bootstrap rewrites unrelated managed files; the KYC navigation page was updated
  directly. No agents or background jobs were started.

## Deletion inventory

- `app/routers/monitor.py:53-86`: user and empty-organization deletes; retention
  checks return stable HTTP 409, and row locks serialize account deletion.
- `app/admin.py:48-71`: SQLAdmin user/organization deletion uses the same retention
  check under a row lock. SQLAdmin catches exceptions and displays its error banner;
  its generic delete handler does not propagate HTTP 409. No document view exists.
- No user/account delete endpoint exists in `app/routers/auth_simple.py`.
- `app/seeds/market_seed.py:134-145`, `app/services/demo_activity.py`, and
  `app/services/market_quarantine.py`: cleanup removes market records, not users or
  organizations. Pending registration/session maintenance does not delete users.
- Test-only deletes: `tests/unit/test_orderbook_marketplace_filters.py:202,424`;
  `tests/postgres/test_runtime_role_policy.py:998-1001`; PostgreSQL fixture
  TRUNCATE CASCADE in `tests/postgres/conftest.py:114-116,150-151` and
  `tests/postgres/test_market_integrity_concurrency.py:81`. Retention migration tests
  add explicit rejected deletes. No foreign key into users/organizations cascades
  their deletion from a parent; document ownership itself is RESTRICT.

## Remaining acceptance gates

Run the complete unit and PostgreSQL runners on the supported Linux toolchain,
and both CI dependency audits. Test installed credentials on disposable systemd
249 and 255 if runtime proof beyond source inspection is required. Deployment,
real keys, retention-period policy and operator document deletion remain outside
this task. No push or PR was made. `work/` and `.claude/` are not committed.
