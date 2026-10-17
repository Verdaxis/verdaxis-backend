# Task 12b — the dual-service staging deploy transaction

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`. **All Task 12b gates
pass.** Nothing here was run against a real host, a real systemd unit, or a
real database: every proof is a fixture-harness subprocess test.

## What was built

- `scripts/deploy.sh` extended in place — there is still exactly one deploy
  owner. Production (`prod/be`, `DEPLOY_ENVIRONMENT=production`) reads no Rust
  path; its command sequence is byte-identical to the pre-Task-12b sequence
  (proof below).
- `scripts/prepare_rust_runtime.py` — projects ONLY `ENVIRONMENT`,
  `RELEASE_SHA`, `MIGRATION_REVISION`, `DATABASE_URL` (rewritten from
  SQLAlchemy's `postgresql+asyncpg://` to libpq's `postgresql://` after an
  identity check against `verdaxis_app_staging@127.0.0.1:5432/verdaxis_staging`)
  and `DB_SESSION_TIME_ZONE` into `.runtime-rust/runtime.env` at mode 0600,
  atomically. Mail, JWT, provider, and migrator credentials never reach the
  long-running Rust process; the migrator-backed checks stay in the prestart
  control plane.
- `deploy/rust/staging-policy.json` — the committed activation record.
  `rust_enabled` is `false`; Rust is selected only when this committed policy
  says enabled AND the archived `runtime-units.manifest` names
  `verdaxis-rust-staging.service` (Task 12c adds that unit). Two independent
  committed records, no ambient flag.
- `tests/unit/test_rust_release.py` — 28 fixture-harness tests.
- `.gitignore` gains `.runtime-rust/`.

## Transaction order (staging, Rust selected)

1. Pre-lock: the archived staging policy, manifest, and provenance key are
   blob-verified; `RUST_ENABLED` is decided from the COMMITTED policy.
2. Under the main deploy lock (a retried same-SHA deploy replaces a stale
   staging; the lock serializes): the artifact verifier runs the host mode
   against `RUST_ARTIFACT_ARCHIVE` + `RUST_PROVENANCE_RECORD` +
   `RUST_PROVENANCE_SIGNATURE` with the pinned public key (digest-pinned, see
   below), extracting into `.runtime-rust/releases/<SHA>/`. The verified policy
   bytes are staged beside it. Every artifact refusal happens before any state
   write or service stop.
3. `blocked` state, fast-forward, identity publication, preflight, deps,
   budget (`prepare_rust_budget.py`, mode 0600 artifact), migration checkpoint,
   ACL convergence — all unchanged for Python.
4. Rust runtime preparation (`prepare_rust_runtime.py`), then the active
   binary `.runtime-rust/verdaxis-read` is installed by atomic rename from the
   staged release (never a symlink), still under `blocked`.
5. Both locks held → `restart-authorized` → restart BOTH units in one
   `systemctl restart` call → `readiness-pending`.
6. Python public readiness validated, then Rust's direct loopback readiness
   (`http://127.0.0.1:8011/health/ready` from the staged policy) validated
   against the same full SHA and environment. Only then is state cleared.
7. Any failure: both attempted services are stopped, the selected code and
   published identity stay aligned, and durable `blocked` state remains.

## The provenance public-key pin (PM requirement 2)

The transaction reads the expected SHA-256 of
`deploy/rust/artifact-provenance.pub.pem` from the approved tree
(`git show <SHA>:<path> | sha256sum`).

- When `/etc/verdaxis/artifact-provenance.pub.pem` (override:
  `HOST_PROVENANCE_KEY_PATH`) exists, its SHA-256 must EQUAL the reviewed
  digest; that path is then passed to `--provenance-public-key`.
- Otherwise the reviewed tree's own file is passed (its digest is re-verified
  against its committed blob first).
- Any digest difference, missing record/signature/key, or verification failure
  refuses BEFORE any lock, state write, or service stop.

The committed key's digest is
`fef3cc8fd2017173fccaf9e856e17c867d4c5efc6376fe78955c1b57b562c0f8`
(PM-recorded; Step 1 of this task).

## Python-only ledger proof (PM requirement 1)

`GOLDEN_PYTHON_LEDGER` in `tests/unit/test_rust_release.py` records the
pre-Task-12b command sequence (from the Task 12a tree, `9c249ea5`), in its true
execution order:

```
python:scripts/verify_migration_revision.py
python:scripts/preflight_runtime.py
python:-m pip install -r requirements.txt -c constraints.txt
python:-m pip check
python:scripts/apply_migration_checkpoint.py
/scripts/converge_runtime_acls.py --environment staging
sudo:systemctl restart
python:scripts/validate_health_response.py
```

`test_python_only_deploy_matches_the_pre_rust_command_ledger` proves a
staging deploy with `rust_enabled=false` produces exactly this ledger, in
order, with exactly one service restart and zero Rust-side commands
(`prepare_rust_runtime`, `prepare_rust_budget`, `verdaxis-rust`, any
`rust` substring). Trees without the committed policy (every older release,
and production forever) skip the Rust block entirely, which the existing
`test_deploy_health.py` suite re-proves unchanged.

## Refusals tested (each before any service stop unless noted)

| Refusal | Test |
| --- | --- |
| Missing provenance record | `test_missing_provenance_record_refuses_before_any_stop` |
| Missing signature | `test_missing_signature_refuses_before_any_stop` |
| Missing record AND signature | `test_missing_record_and_signature_refuses_before_any_stop` |
| Host key digest differs from the reviewed digest | `test_divergent_host_key_refuses_before_any_stop` |
| Tampered archive bytes | `test_tampered_archive_refuses` |
| Record bound to a different source SHA | `test_wrong_source_sha_record_refuses` |
| Signature verification failure | `test_failed_signature_verification_refuses` |
| Moved remote SHA | `test_moved_sha_refuses_before_source_mutation` |
| Wrong migration revision | `test_wrong_revision_refuses_before_source_mutation` |
| Changed budget | `test_changed_budget_refuses_and_blocks` (blocked state, no stop) |
| Rust preparation failure | `test_rust_failure_keeps_selected_code_identity_and_blocked_state` |
| Rust readiness never OK | `test_both_health_results_are_required` (stops BOTH) |
| Pre-mutation failure phases | `test_failures_retain_blocked_state_and_stop_attempted_services` |
| Dry-run executes nothing | `test_dry_run_never_executes_build_code` |

Positive paths proven: dual restart in one `systemctl restart`, state cleared
only after both health gates, `runtime.env` keys limited to the five allowed,
mode 0600, active binary a regular file installed by rename, host key
preferred when its digest matches, tree key used when the host key is absent,
loopback Rust health URL from the staged policy.

## Interruption matrix

| Interruption point | Selected code | Published identity (`.runtime-release.env`) | Rust staging/active | Deploy state | Locks | Attempted services | Recovery |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Before the deploy lock (any refusal) | old | old | none written | absent | released | none started/stopped | fix input, re-deploy |
| During artifact extraction (under lock) | old | old | partial `releases/<SHA>/` | absent | released on exit | none | same-SHA retry replaces the stale staging |
| After `blocked`, before checkout | old | old (or none) | none | `blocked` | released on exit | none | re-deploy (state rewritten) |
| After fast-forward, before identity | new | old | none | `blocked` | released | none | forward-correct; identity still names old SHA until publication |
| After identity publication, pre-migration | new | new | none | `blocked` | released | none | fix forward; a unit start is guard-denied |
| During budget/preparation | new | new | partial `.runtime-rust/` | `blocked` | released | none | re-deploy; preparation is idempotent per SHA |
| SIGKILL inside the restart window | new | new | staged + active installed | `restart-authorized` (retained) | kernel-released | possibly restarting | start guard denies a backend start until a corrected deploy clears state |
| After restart, during Python readiness | new | new | active installed | `readiness-pending` | held by live deploy; released if killed | both attempted | trap/killed: state stays, both stopped by trap; re-deploy forward |
| Rust readiness fails | new | new | active installed | `blocked` (trap) | released | both stopped by trap | forward correction; never restore old metadata over new bytes |
| Success | new | new | active installed | cleared | released | both running | — |

Key invariants proven by tests: a stale `restart-authorized` never authorizes
a unit start (`test_stale_authorization_does_not_authorize_a_unit_start`);
SIGKILL mid-window retains state and releases both locks
(`test_interrupted_restart_window_releases_locks_and_keeps_state`); a
concurrent deploy is refused while the main lock is held
(`test_concurrent_deploy_refuses_while_lock_held`); identity is published
before any selected-tree execution and never rolled back independently.

## Gates (test host, 17 Oct)

| Gate | Result |
| --- | --- |
| `venv/bin/python -m pytest tests/unit/test_rust_release.py -q` | **28 passed** |
| Existing deploy/start-guard/budget/artifact/identity suites combined | **165 passed** |
| Full Python unit suite (`ENVIRONMENT=test … pytest tests/unit -q`) | **2391 passed**, 514 warnings |
| `SQLX_OFFLINE=true cargo test --workspace` | **ok** (all targets, 0 failed) |
| `cargo fmt --all -- --check` | **pass** |
| Strict `clippy` (`--all-targets -- -D warnings`, `RELEASE_SHA` set) | **pass** |
| `tests/rust_contract/test_routing.py` | **306 passed** |
| `shellcheck scripts/deploy.sh` | **not run: shellcheck is not installed on the test host** |

Logs: `work/rust-phase1/t12b-*.log` (run35, clippy2, final-gates hold the
green evidence; earlier t12b logs record the red-to-green iterations).

## Deviations from the plan's literal text

1. The plan's restart block shows two separate `systemctl restart` invocations
   would follow from its snippet; the implemented transaction restarts both
   units in ONE `systemctl restart "$SERVICE_NAME" "$RUST_UNIT_NAME"` call
   inside the window, which the fixture pins exactly.
2. The plan names `deploy/rust/staging-policy.json` and this task creates it;
   `rust_enabled` ships `false`. Task 12c owns the unit bytes and the manifest
   entry, so no deploy can select Rust until that reviewed unit exists — the
   two-record activation the Task 12c plan section already requires.
3. `prepare_rust_runtime.py` gained `--source-root` and `--migration-revision`
   beyond the plan's three flags: the policy is read from the selected tree
   (not the ambient checkout), and `MIGRATION_REVISION` must be projected into
   the Rust environment for its compiled allowlist check, exactly as the Rust
   unit's `EnvironmentFile` design in Task 12c expects.

## What Task 12c still owns

The audited `deploy/systemd/verdaxis-rust-staging.service` bytes, the manifest
line `staging\tverdaxis-rust-staging.service`, `.runtime-rust/` ignore policy
(done here), the two-release bootstrap, and flipping `rust_enabled` to `true`
in a reviewed release. No live deploy, even a dry run, is authorized by this
task's completion.
