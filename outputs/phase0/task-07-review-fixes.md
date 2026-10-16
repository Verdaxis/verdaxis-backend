# Task 7 independent review fixes

Date: 2026-10-16. Lane: `feat/phase0-task7`. Base: `7c2cd636`.

## Changes

- **M1 fixed:** use declared `asyncpg`, with read-only transactions and bounded connection/query/close times. Normalize `inet_server_addr()` with `host()` rather than comparing an address with a `/32` suffix. Real PostgreSQL tests execute identity, outbox and rollback-safe checks, including the CLI entry point.
- **M2 fixed:** count every other session on the bound database; exclude only `pg_backend_pid()`. Idle administrator and backup sessions refuse the post-stop gate. No old database grants, schema or settings are changed.
- **M3 fixed:** explicit daemon reload and manager-state checks before source replacement and rollback. Require loaded state, no pending reload, the exact drop-in and all three pre-start commands. Keep actual systemd denial proof in the isolated Linux rehearsal.
- **M4 fixed for local source acceptance:** executable release-specific tuples, distinct synthetic databases, mixed environment/identity publication, real subprocess locks and guards, cached unit replacement, restart/readiness checks, blocked public writes and complete old-tuple restoration. Negative controls detect an unloaded fence and an open write route. This is not a claim of Linux systemd or Caddy proof.
- Added the PM's frontend reopening checks: new entry HTML must be live; the founder sends the refresh note to active pilot users. No forced reload or compatibility release.

## TDD evidence

1. Initial database tests: **3 failed**, missing `psycopg2`.
2. Driver replacement exposed the existing server-address text suffix and Colima NAT mismatch. The first driver commit's success claim was premature; the next commit explicitly corrects that claim.
3. With normalized addresses and test-only exact NAT translation: **1 passed, 2 failed** because administrator and backup sessions were not rejected.
4. Count all other sessions: **3 passed**. Added all database CLI checks: **4 passed**.
5. Runbook tests: **2 failed**, then **2 passed**.
6. Executable tuple tests initially failed for missing fixture support; final suite includes 14 passing cases, including negative controls.

## Verification

| Scope | Result |
| --- | --- |
| Task 7 focused tests | 46 passed, 1 warning |
| Complete monitor suite | 290 passed, 1 platform skip (passwordless nobody unavailable) |
| Complete unit suite | 2,144 passed, 19 known macOS `test_deploy_health.py` failures, 513 warnings |
| Database assertion tests, separate pinned runner | 4 passed |
| Full pinned PostgreSQL runner (foreground, exit 0) | 124 passed, zero failures/skips, 46 warnings; 384.52 seconds |
| Exact identity, ACL and checkpoint regressions | 72 passed |
| `git diff --check`; shell syntax checks | Passed |

The unit failures are Bash 3.2 `mapfile`/lowercase-substitution failures and dependent assertions, not new failures. The first full runner finished with **121 passed, 2 failed** after a test helper signature changed while that run had the old test call in memory. A fresh run uses frozen committed files. Do not use the first full run as acceptance evidence.

All PostgreSQL runs use the existing digest-pinned PostGIS image through Docker's `colima` context, numeric-loopback ephemeral ports and synthetic data. The fixture checks the observed Colima server endpoint exactly before translating the known NAT endpoint for the unchanged production identity validator. It stubs host systemd/peer-sudo and private-file ownership in tests only. No staging or production contact, push, Skill, Agent or subagent was used.

## Commits

- `225c942c`: declared asyncpg driver.
- `b024f97a`: normalized server address and corrected first test claim.
- `f20ca269`: all-other-session drain gate.
- `df810909`: loaded fence, cached-bundle decision and integration checklist.
- `a881f117`: executable interruption and rollback tests.
- `11efcab5`: CLI coverage, CI boundary suite and durable progress notes.

Every fix commit uses the requested Claude Opus 5.5 trailer. Nothing was pushed.

## Resume and integration gates

The previous background rerun had no completion record when the session ended;
it is not acceptance evidence. The final frozen-source run completed in the
foreground with exit 0: **124 passed, zero failures/skips, 46 warnings**.
Log: `work/task7-postgres-foreground-final.log`. Synthetic fixture evidence:
`work/phase0-fixtures/aa3d2a1735544ccba1a5ce58d17e66b2`.
Wait for every test run to finish before ending a turn or recording acceptance.
Supporting logs are
`work/task7-focused-final.log`, `work/task7-monitor-final.log`,
`work/task7-unit-final.log` and `work/task7-cli-final.log`.

Task 4b is on `feat/phase0-convergence-20261016` with `p0_20261016_union`; it was
not merged here. Read-only inspection confirms that branch contains the union
migration and both `kycd_20261015_staging → p0_20261016_union` and
`p0_20261016_union → p0_20261016_union` checkpoint rows.
After PM integration, recheck union bootstrap/copy, exact
production Settings and role/ACL policy, union-to-union deploy, matching runtime
and monitor identity, and combined installed unit digests. Keep the loaded fence
check after installation and before permit issuance. Require separately approved
Linux/systemd/proxy rehearsal, provider isolation, backup restore and complete
switch-back before live cutover. No host operation is authorized by this report.
