# Local Monitor v2 Monitor-Only Remediation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce `hardening/local-monitor-v2` to monitor-owned, source-only artifacts while closing backup-generation, environment-isolation, alert-proof, readiness, and retirement boundaries.

**Architecture:** The branch owns credential-free readers, a least-privilege alert consumer, independent demo activity unit definitions, hostile-state handling, and a guarded legacy-retirement validator. The canonical runtime owner remains solely responsible for application configuration/readiness production, deploy transactions, immutable installation, service activation, and producer integration. A static exact artifact inventory is the only installation seam. The backup verifier consumes—but never creates—a strict latest-attempt journal written durably by the external producer owner.

**Tech Stack:** Python 3.12 standard library, pytest, systemd unit files, JSON contracts, Git subprocess verification.

---

## Failed-review remediation addendum

- Legacy retirement requires a valid durable alert state whose current state is
  healthy and, for every configured destination, a recovery receipt strictly
  newer than its latest failure receipt. Missing destination history is never
  treated as health.
- Every hostile/external JSON reader rejects duplicate object keys, including
  backup status and attempts, retirement evidence, alert state, monitor status,
  readiness, and disposable-target attestation.
- A gzip member is only plausible backup evidence when it expands to at least
  1 KiB and its bounded prefix contains a PostgreSQL plain-dump marker. Valid
  but empty, tiny, or unrelated gzip content fails verification.
- Demo units use one launcher operation that archives the attested commit into
  a private read-only runtime snapshot and executes the demo script from that
  snapshot. There is no separate mutable-checkout preflight followed by exec.
- Execute-mode retirement checks the complete timer set once during the live
  proof sequence and rechecks the complete set as the final operation before
  issuing the fixed disable command.
- `tests/disposable_server.py` is the source-tree producer for the disposable
  identity endpoint and wraps the ordinary test ASGI application on a numeric
  loopback ephemeral port.
- Runtime-owner source regression compares exact working-tree bytes to the
  fixed integration base `f31736d`, never a branch-relative baseline.
- Alert deduplication is fixed at one hour. The unused force override,
  configurable repeat interval, redundant same-file endpoint inventory
  validator, and one-line status-loader wrappers are removed.

## Approved ownership decisions

- Restore `app/config.py`, `app/main.py`, `scripts/deploy.sh`, and all other runtime identity/deploy transaction files to the integration base; do not alter `app/database.py`.
- Delete the local installer, installer transaction, producer implementation/service, and their promotion/transaction tests. Document only the external producer's durable attempt-record seam and complete responsibility boundary.
- Supply one exact JSON artifact inventory containing source path, destination, mode, and SHA-256 for monitor-owned artifacts. It is data for the canonical immutable installer, not an activation command.
- Require an external latest-attempt marker for backup recovery. A started or failed newest attempt keeps verification red even when an older `status.json` and artifacts remain valid. Only a newer matching successful attempt can recover.
- Make production and staging demo services independently scheduled and deployment-guarded. Neither unit names, waits for, loads, writes, or schedules the other environment.
- Derive retirement alert proof from configured key presence only. Compare exact event/destination receipt IDs and timestamps with durable dispatcher state, then recheck current status and timer state immediately before the fixed legacy disable command.
- Require the exact four-key readiness response: `status`, `db`, `environment`, and `release_sha`. Keep a machine-readable corpus beside the integration notes for the runtime owner.
- Delete `PILOT-RUNBOOK.patch`; source docs describe an outstanding owner integration and explicitly deny activation/readiness claims.

### Task 1: Encode ownership and inventory failures

**Files:**
- Create: `tests/monitor/test_source_ownership.py`
- Replace: `tests/monitor/test_install.py`
- Modify: `tests/monitor/test_systemd_units.py`

1. Assert runtime/app owner paths have no branch diff from the integration base.
2. Assert installer/transaction/bootstrap and backup producer service are absent.
3. Assert the static inventory is exact, byte-attested, destination-unique, mode-bounded, and excludes producer/reference/activation artifacts.
4. Assert every remaining unit is inventoried and no inventory consumer or root promotion path exists.
5. Run only these tests with the explicit disposable test environment and observe the expected failures.

### Task 2: Encode exact readiness and backup-attempt truth

**Files:**
- Modify: `tests/monitor/test_local_health_check.py`
- Modify: `tests/monitor/test_backup_verify.py`
- Create: `deploy/monitor/runtime-v2-readiness-corpus.json`

1. Add corpus-driven exact-four-key readiness cases, including missing, extra, wrong type, wrong environment, and wrong SHA.
2. Add strict external attempt-marker cases for started/crashed, failed, malformed, stale-success/newer-failure, and matching newer success.
3. Preserve real slow-drip deadline, symlink, compressed-bomb, corruption, timestamp, and TOCTOU regressions.
4. Run focused tests and observe the expected failures before implementation.

### Task 3: Encode environment-independent demo boundaries

**Files:**
- Modify: `tests/monitor/test_systemd_units.py`
- Modify: `tests/monitor/test_runtime_identity.py`
- Create: `deploy/monitor/verdaxis-demo-activity.timer`
- Create: `deploy/monitor/verdaxis-demo-activity-staging.timer`

1. Assert each timer targets only its own service and each service has its own deployment-state guard.
2. Assert production source contains no staging service/path/credential references and vice versa.
3. Use two disposable Git repositories and real Git subprocesses to prove each verifier is target-local; a staging failure cannot affect a production verification result.
4. Preserve exact fixed `safe.directory`, disabled ambient/system config, exact root, and exact HEAD attestation.

### Task 4: Encode durable receipt matrix and immediate retirement rechecks

**Files:**
- Modify: `tests/monitor/test_alert_dispatch.py`
- Modify: `tests/monitor/test_legacy_retirement.py`

1. Require independent receipt IDs/timestamps per destination and event state, persisted after each successful channel attempt.
2. Preserve transition plus hourly failure/warning reminder semantics and partial-channel dedupe.
3. Parameterize every Telegram/health-Healthchecks/backup-Healthchecks key-presence combination without exposing values.
4. Require evidence to match the complete configured destination/event matrix and durable dispatcher receipts.
5. Assert current monitor statuses, all v2 timers, and the legacy timer are rechecked after evidence validation and immediately before disable; any mismatch refuses execution.

### Task 5: Delete competing ownership and implement minimal monitor seams

**Files:**
- Restore: `app/config.py`, `app/main.py`, `scripts/deploy.sh`
- Delete: `scripts/preflight_runtime.py`
- Delete: `deploy/monitor/runtime_identity.py`
- Delete: `deploy/monitor/install.sh`
- Delete: `deploy/monitor/install_transaction.py`
- Delete: `deploy/monitor/verdaxis-backup.service`
- Delete: `deploy/monitor/PILOT-RUNBOOK.patch`
- Delete: redundant deploy/install/producer-permission tests and artifacts
- Modify: monitor reader, verifier, alert, retirement, unit, sysusers/tmpfiles, and inventory files only as required by failing tests

1. Apply exact base restoration to runtime-owned paths.
2. Remove the competing activation and producer artifacts.
3. Parse the exact monitor identity keys locally in the health reader; leave identity publication to runtime integration.
4. Add read-only strict backup-attempt evidence validation.
5. Centralize status construction and atomic publication only where it removes duplicate code.
6. Implement independent demo units and receipt/retirement checks with no secret values in proof data.

### Task 6: Synchronize owner documentation

**Files:**
- Rewrite: `deploy/monitor/README.md`
- Modify: `docs/demo-activity-and-canary.md`
- Modify: `ARCHITECTURE.md`
- Modify: `CLAUDE.md`
- Modify: `tasks/lessons.md`
- Update: `deploy/monitor/retirement-evidence.example.json`

1. State exact monitor/runtime/backup/public-monitor ownership and no activation.
2. Document the producer attempt-marker and exact readiness corpus integration seams.
3. Document independent scheduler ownership and the dual-run prohibition on legacy retirement.
4. Remove every claim that this branch deploys, installs, replaces backup responsibilities, or fixes the live pilot runbook.

### Task 7: Verify source gate and hand off the dirty worktree

1. Run focused monitor tests under the explicit disposable target environment.
2. Run the full unit/monitor source suite under the same environment; do not collect integration/E2E tests.
3. Run the dual-repository Git executable proof explicitly; the externally owned backup producer is not executed or tested from this branch.
4. Run Python compilation, shell syntax for remaining shell files, `systemd-analyze verify` for remaining units/timers, static hardening checks, inventory digest verification, and JSON parsing.
5. Confirm owner paths have zero diff from the integration base, no promotion semantics remain, docs are synchronized, and the worktree is otherwise clean.
6. Review the final diff for credentials and unrelated changes.
7. Do not commit, push, checkout, or reset during failed-review remediation.
   Report exact test counts, changed files, unfinished work, held live actions,
   and explicitly state that no commits were made.
