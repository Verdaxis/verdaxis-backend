# Phase 0 progress — Task 6 (make database time UTC)

Task 6 is complete. Commits:

- `f23dd07` `test: widen Phase 0 runner container readiness window`
- `f8d8e13` `ops: standardize canonical database sessions on UTC`

## Artifacts

- `deploy/phase0/set_timezone.sql` — the policy. It sets the database default
  and the role-in-database override for the app, migrator and backup roles,
  refuses an unexplained role-wide timezone override, and requires exactly the
  four published UTC settings. Applied from a connection to `postgres` with the
  psql variables `database_name`, `app_role`, `migrator_role`, `backup_role`.
- `tests/postgres/test_phase0_timezone.py` — `test_timezone_reconnect_and_role_override`.
- `docs/runtime-hardening.md` — the "Database roles and session timeouts"
  section now states the UTC session contract and removes the implicit
  "the database default is fine" assumption.

## Check results

- `bash scripts/run_phase0_postgres_tests.sh tests/postgres/test_phase0_timezone.py`
  — 1 passed.
- `bash scripts/run_phase0_postgres_tests.sh` — 16 passed.
- `pytest tests/unit -q` — 2020 passed, 19 failed. All 19 are the known
  `tests/unit/test_deploy_health.py` failures on macOS Bash 3.2
  (`${VAR,,}` bad substitution and dependent assertions). No other failure.

## Recorded before/after values

Boundary instants in UTC: `2026-10-16 15:59:59Z`, `16:00:00Z`, `23:59:59Z`.

| Bucket | Singapore session (before) | UTC session (after) |
|---|---|---|
| price reference `func.date(confirmed_at)` | 10-16, 10-17, 10-17 | 10-16, 10-16, 10-16 |
| admin daily `cast(created_at, Date)` | 10-16, 10-17, 10-17 | 10-16, 10-16, 10-16 |
| explicit `timezone('UTC', …)` login fact | 10-16, 10-16, 10-16 | 10-16, 10-16, 10-16 |

`SHOW TimeZone` after the policy is `UTC` for the app, migrator and backup
roles. A session opened before the change stays `Asia/Singapore` until its pool
is recycled. A role-wide override added after the policy is beaten by the
role-in-database value. A connection `timezone` setting outranks the policy and
is invisible to the server. Stored `timestamptz` epochs are unchanged. The FAME
Singapore delivery/certificate date rule is unchanged.

## Deviations

- `tests/phase0/runner.py` (owned by Task 1, extended by Task 9): the 60-second
  container readiness window is too tight for the emulated amd64 image on a
  loaded host, and made the runner fail intermittently. Raised to 240 seconds
  (commit `f23dd07`). No other behavior changed. The configured formatter also
  sorted that file's imports.

## Notes for other lanes

- The Rust runtime needs a `DB_SESSION_TIME_ZONE` setting (Rust decision 21)
  that must be `UTC` after Phase 0. SQLx (Rust) sends its own `TimeZone` at
  startup, so it is out of scope for this task; this task covers Python,
  migrations and CLIs.
- Apply the policy to the canonical target databases only: `verdaxis_prod`
  (Task 7 owns that identity change) and `verdaxis_staging`. Never to the
  retired `verdaxis`; the SQL refuses it.
- Recycle the API pools and every CLI process after applying. A database
  default does not change an already-open session.
- Task 5: stored naive timestamps copy literally; do not guess a zone. Task 10
  repeats this on the host scratch cluster; Task 11 checks the fresh live pools.

## Resume

Task 6 is done at `f8d8e13`. Downstream tasks consume
`deploy/phase0/set_timezone.sql` and the UTC checks in
`tests/postgres/test_phase0_timezone.py`. No live operation or push was
performed.
