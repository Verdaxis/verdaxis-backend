# Task 7 — identity and one-use operator preparation

Date: 2026-10-16. Branch: `feat/phase0-task7`, based on `cab1e0a`.
Implementation commits: `af359a36`, `2c79bc69`. No push or live operation.

## Checks

| Command / scope | Result |
|---|---|
| Initial requested cutover/runtime/deploy-health test run, before implementation | 31 failed, 45 passed; 12 intended Task 7 failures plus 19 known macOS failures |
| Final `pytest tests/phase0/test_cutover.py tests/unit/test_phase0_runtime.py -q` | 30 passed |
| `bash scripts/run_phase0_postgres_tests.sh` | 104 passed, zero skips, 46 warnings; 480.43 seconds |
| Required full `tests/unit -q` with test environment and SQLite URL | 2,142 passed; 19 known `test_deploy_health.py` failures; 513 warnings |
| `pytest tests/monitor -q -rs` | 290 passed; 1 skipped: passwordless `nobody` execution unavailable |
| `bash -n scripts/deploy.sh`, compileall for both new helpers, `git diff --check` | Passed |

The unit failures are confined to Bash 3.2 (`mapfile` absent and `${name,,}` bad
substitution), with dependent deploy assertions. No other final unit failure.
The PostgreSQL gate rebuilt pinned production/staging schemas in its own pinned
PostGIS container, published synthetic evidence under
`work/phase0-fixtures/614c94cbe7b3477c9c6e437af1df8a48`, and removed its container
and volumes through the existing runner. Other lanes' containers were untouched.
The final focused run includes later tuple/late-producer regressions added after
the long PostgreSQL run collected its tests; both passed separately.

The first PostgreSQL run had 93 passes and one false fence denial: access time
changed while reading a file. The reader now compares inode, size and modification/
change timestamps, not access time. The full rerun above passed.

## Delivered boundaries

- Fixed `verdaxis_prod` identity across Settings, ACL convergence, CLI validation,
  backup readers and outbox defaults; unchanged exact roles; both production
  database names explicitly denied by seed paths. Old pinned release unchanged.
- Canonical `staging` selection in both fixed checkout paths. No checkpoint policy,
  existing deploy lock/start guard, unit installer, four-worker or vault gate removed.
- Production canary CORS with real credentialed preflight and staging rejection.
- Both reminder unit pairs retain the 15-minute calendar schedule and deny dangling
  deploy-state paths. No staging reminder or external monitor activation.
- Pinned production monitor/recovery/diagnosis source and tests, with explicit new
  backup/outbox identity defaults. Host-owned configuration remains untouched.
- External root-owned fence guard, exact environment/SHA/database/operator-inode
  permit and read-only assertion CLI. Existing two-lock guard passes independently.
- Separate runtime/monitor publication function under blocked deploy state; suitable
  for the separately reviewed Phase 1 binary transaction extensions.
- One-use operator checklist, drop-in bytes, manual old-tuple restart block and
  append-only journal example in `docs/runbooks/phase0-cutover.md`.

Tests use real subprocess flocks, process termination and directory/file renames.
Restart/readiness boundaries use synthetic markers, not Linux systemd. The
late-producer test uses a real synthetic SQLite transaction to prove why the first
zero is insufficient; PostgreSQL schema/import checks use the existing runner.
These results are not proof of a completed Linux cutover or rollback.

## Deviations and integration-owned work

Minimal additional identity-consumer files and their fixture/blob manifests are
listed in `docs/phase0/progress-task7.md`. `.gitignore` and the deploy test fixture
also ignore the new generated monitor identity. No Task 4b migration/checkpoint
or Task 5 copy file was changed. No new dependency, orchestration engine, installer
or journal parser was added.

Task 4b's final union revision is not present in this lane. After merge, rerun the
bootstrap/union and Task 7 suites, exact production Settings proof and normal
same-revision deploy. Assertion identity deliberately requires `p0_20261016_union`
for the new database and `kycg_20261014_prod` for the pinned old tuple.

Before Task 10, fill and approve host-specific proxy/egress commands, all cron and
writer/recovery inventories, scratch-cluster binding, backup producer changes,
monitor reader wiring and effective root drop-in checks. Test the complete
switch-back only in that authorized isolated rehearsal. The assertion unit list
is a minimum, not a substitute for live host inventory. The backup producer is
absent from this repository; its exact new inventory/prefix and restore proof are
mandatory external gates, not an unimplemented local producer.

No live host, real data, key recovery, external provider, systemd installation or
full host switch-back test ran: the user forbids host contact in Task 7. Public
mutations must remain closed until the signed lossless rollback gate; after new
customer writes, preserve both databases and obtain reconciliation approval.
