# Task 12c — the staging Rust unit, its manifest entry and the bootstrap

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`. The tree starts from
`b2971573254589485adfbbbd5b0a3c90d4cd2592` (Task 12b, rebased onto the PM's CI
fixes; draft PR #86 runs CI only).

**No staging or production host was contacted.** Every live check ran on the
isolated test host through `verdaxis-prod-test` with disposable containers and
fixtures local to the `verdaxis-test` user, exactly as the earlier Rust tasks
did. No SSH call went to `194.233.68.86` or `169.58.37.164`, no service was
started or installed, and no unit was loaded into a running systemd.

The handoff resumed from `d06e8248` ("feat: start the Rust reads from the
deployed environment"), the commit that carries the deployed-environment seam
this unit starts the released artifact with. Two Task 12b failures in the draft
PR #86 `test` job (run 38998196777, GitHub CI) came first, each in its own
commit and each proven on the test host — `09642af5` (B, the release fixture's
interpreter) then `53e959d1` (A, the runtime-owned `scripts/deploy.sh` blob pin).
The unit, the manifest entry, the tests and these documents then landed as one
commit, `20c9d7dd`, the bootstrap SHA. See *CI follow-ups* below.

## What was built

- `deploy/systemd/verdaxis-rust-staging.service` — the reviewed staging unit.
  It runs as `verdaxis-prod`, binds `127.0.0.1:8011`, and takes every sandbox
  directive from the already-live backend unit, so every directive it uses is
  proven on staging's systemd 249. It loads `.runtime-release.env` and
  `.runtime-rust/runtime.env` and never `.env`; mail, JWT, provider and migrator
  credentials never reach the long-running process.
- `deploy/systemd/runtime-units.manifest` — one new entry,
  `staging	verdaxis-rust-staging.service`. The installer stays manifest-driven:
  no installer code names a Rust unit, and no Rust-owned installer was added.
- `tests/unit/test_rust_units.py` — 14 tests: the unit contract, the deploy
  path agreement, the budget precheck's real flags, the sandbox/version reuse,
  the manifest entry, the file permissions, the two-release bootstrap, and the
  installer's own-checkout rules.
- The Rust runtime seam the reviewed unit needs (see *Deviations*):
  `runtime::config::config_from_deployed_environment` and `deployed_arguments`
  build the same validated `Config` from the flat deployed environment that
  `RUST_CONFIG` builds for the contract harness; `Config.prepared_budget`
  carries the artifact; `db.rs` reads the budget from that field;
  `main.rs` resolves `--bind` and `--budget-file` and reads the deploy-written
  artifact as a size-bounded regular file. `connect(config)` is unchanged.
- `scripts/prepare_rust_runtime.py` now also projects
  `KYC_EXECUTION_GATE_ENABLED` from the deployed `.env` (Python's own switch),
  and refuses an ambiguous value. Without it the Rust reads would answer the
  opposite KYC policy from Python on the same host.

`.gitignore` needed no change: Task 12b already ignores `.runtime-rust/`.

## Digests and identity

| Item | Value |
| --- | --- |
| Bootstrap SHA plan | The inactive first release is the commit that lands these unit bytes and the manifest entry, subject **`feat: add the staged Rust unit and its manifest entry`**. It landed locally as `20c9d7dd`, but read its exact SHA with `git rev-parse` on the reviewed push: the PM rebases this branch onto the remote, so any SHA written here would change. Baseline before this task: `d06e8248`. |
| Unit bytes | `deploy/systemd/verdaxis-rust-staging.service` |
| Unit SHA-256 | `6662a389e04a67ddc7d291a93ae0814b05047ff8f8bb07a55642fdb882de9453` |
| Manifest SHA-256 | `fb25de27f6d0366a0a896a4d12e18a8e86336655a860d6747e0849e2875cd64d` |
| Manifest entry | `staging	verdaxis-rust-staging.service` |
| `deploy/rust/staging-policy.json` `rust_enabled` | `false` (Rust stays inactive in this tree) |
| Compiled revision allowlist SHA-256 | `4aec86c395518a4bda6d1a5c41773fbd222d983cdb0ebee7c2f960175b8d18fd` |
| Migration revision | `kycg_20261014_staging` (the only compiled entry) |
| Release artifact SHA-256 | `f0538798ebb014c3efe0e42e761ec45f248b470e681e3f28ec212bd6e6cddfe3` (a `--release -p verdaxis-read` build of this landing commit, `20c9d7dd…`) |

The artifact digest belongs to a `--release -p verdaxis-read` build of this
tree's Rust source on the test host (`x86_64-unknown-linux-gnu`, glibc floor
`GLIBC_2.34`) with `RELEASE_SHA` set to the commit the artifact was built for
(`20c9d7dd…`), so the digest moves with that commit: the release SHA is compiled
into the binary. It is evidence for the staging-ABI check, not the deploy
artifact: a real deploy
consumes the CI-published archive, whose digest the transaction verifies against
the PM's provenance record (Task 12a/12b).

## Unit contract, test-proven

- `ExecStart` is exactly the binary the transaction installs
  (`.runtime-rust/verdaxis-read`, atomic rename, `chmod 0755`) and the
  transaction's own `RUST_UNIT_NAME`/`RUST_RUNTIME_DIR` variables are asserted
  against the unit's absolute paths, so unit and deploy cannot drift.
- `EnvironmentFile` lines are exactly `.runtime-release.env` and
  `.runtime-rust/runtime.env` (the file the helper writes) — never `.env`.
- The three `ExecStartPre` lines are the deploy start guard (isolated stdlib
  python, `--environment staging --expected-release-sha ${RELEASE_SHA}`), the
  migrator-backed revision check, and
  `prepare_rust_budget.py --verify --environment-file .env --release-sha
  ${RELEASE_SHA} --output .runtime-rust/budget.json` — the prepared artifact,
  with the helper's real flags, in that order before `ExecStart`.
- `--bind` and the policy's `bind`/`health_url`/`service_name` agree with the
  committed `deploy/rust/staging-policy.json`.
- Every directive in the unit already appears in the live
  `verdaxis-backend-staging.service`.

## Permission proof (PM requirement 2)

`deploy.sh` runs as `verdaxis-prod` (`README.md`: `sudo -n -u verdaxis-prod -H
env … ./scripts/deploy.sh`), the same account the unit runs as. Its Rust
staging block runs under `umask 077`; both helpers open their outputs with an
explicit `0600`; the active binary is copied and `chmod 0755`-ed.

A test stages the three files with the **real** helpers under `umask 077` (the
same code `deploy.sh` invokes) and asserts: `runtime.env` and `budget.json` are
mode `0600` with no group/other bit, the runtime directory is `0700`, and the
active binary is a mode `0755` regular file (not a symlink) that the owner can
execute. `test_rust_release.py::test_runtime_environment_carries_only_the_projected_settings`
reads the file the real transaction wrote in the Task 12b fixture.

Live cross-check on the test host (disposable containers, `work/verify/abi.log`):

```
runtime       mode=775 owner=verdaxis-test
runtime.env   mode=600 owner=verdaxis-test
budget.json   mode=600 owner=verdaxis-test
verdaxis-read mode=755 owner=verdaxis-test
OWNER_READ_AND_EXECUTE_OK
cat: /runtime.env: Permission denied
SECOND_USER_READ_DENIED_OK
```

The second run mounted `runtime.env` **as a file**, so the denial comes from the
file's own mode, not from a directory bit.

## systemd syntax verification (PM requirement 3)

The test host is Ubuntu 24.04 with systemd 255; staging is Ubuntu 22.04 with
systemd 249. Unit SHA-256 as verified: `6662a389…9453`.

| Run | Result |
| --- | --- |
| Test host, `systemd-analyze verify` on the committed bytes as reviewed | exit 1 — `Command /home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/verdaxis-read is not executable: Permission denied`. The staging paths do not exist for `verdaxis-test` and cannot be created without root, so this run reports only that. |
| Test host's own systemd 255 (`255.4-1ubuntu8.18`) with the unit's paths materialized in a private fake root (`--root=`) | Rust unit **exit 0, no diagnostics**; Rust unit + backend unit **exit 0, no diagnostics** |
| `ubuntu:22.04` container, `systemd 249 (249.11-0ubuntu3.23)`, same fake root | Rust unit and both units **exit 0, no diagnostics** |
| `ubuntu:24.04` container, `systemd 255 (255.4-1ubuntu8.18)`, same fake root | Rust unit and both units **exit 0, no diagnostics** |

Re-run against the committed bytes (SHA-256 `6662a389…9453`, digest recomputed
before the run) at the landing commit `20c9d7dd`: identical results —
host as-is exit 1 (absent staging path only), host 255 fake root exit 0 and 0,
`ubuntu:22.04` systemd 249 exit 0 (Rust unit and both units), `ubuntu:24.04`
systemd 255 exit 0 (`work/verify/systemd.log`, script exit 0). Every
directive in this unit therefore verifies on systemd 249, the staging version,
so the man-page fallback was not needed.

Blockers found, reported exactly as instructed:

- A private user+mount namespace on the test host is unavailable for this user:
  `unshare: write failed /proc/self/uid_map: Operation not permitted`. The
  `--root=` fake root above is the working substitute; it exercises the host's
  own `/usr/bin/systemd-analyze` (255) against the reviewed bytes.
- A container run of `systemd-analyze verify` needs `systemd-analyze` in the
  image. `ubuntu:22.04` and `ubuntu:24.04` both supply it (`apt-get install
  systemd` succeeded where the image lacked it), so the 249 check **did** run in
  a container and the man-page fallback was not needed.
- The fake root in the first attempt failed with
  `postgresql.service: Service has no ExecStart=… Refusing.` — that was the
  *stub* the verification itself needs (`Requires=postgresql.service`), not the
  unit under test; the stub now has `Type=oneshot` + `ExecStart=/bin/true`.

The fake root only satisfies paths: it creates 0755 regular stubs at the unit's
`ExecStart`, `ExecStartPre`, `EnvironmentFile` and `LoadCredential` source paths,
a valid `postgresql.service` stub, and copies the host's
`/usr/lib/systemd/system` targets. Nothing in it is executed and no systemd
instance loads it.

## Artifact startup on the staging ABI (PM requirement 4)

`work/rust-phase1/t12c-container-check.sh`, all on the test host. Re-run at the
landing commit `20c9d7dd` with the same script (`work/verify/abi.log`, script
exit 0); the values below are that run's, and the container id and the digests
differ from the earlier base-commit run because `RELEASE_SHA` is compiled in.

1. `SQLX_OFFLINE=true cargo build --locked --release -p verdaxis-read` from this
   tree; `--version-json` reports
   `release_sha=20c9d7dd…`, `migration_revision_allowlist=["kycg_20261014_staging"]`,
   `libc_min=2.35`, `arch=x86_64`, `os=linux`; the highest required glibc symbol
   version is `GLIBC_2.34` (≤ the staging host's 2.35).
2. The binary starts in `ubuntu:22.04` (glibc 2.35) and prints its version
   document there — the release artifact really runs on the staging ABI.
3. A disposable pinned `postgis/postgis:17-3.6-alpine` container is the
   database. The binary runs in a second `ubuntu:22.04` container that **shares
   the database container's network namespace**, so the staging identity
   constraint (`127.0.0.1:5432`) holds without touching the host's own
   PostgreSQL and without publishing any port outside that namespace. The
   database has role `verdaxis_app_staging` (LOGIN NOINHERIT NOSUPERUSER, no
   memberships) and database `verdaxis_staging`.
4. `.runtime-rust/runtime.env` and `budget.json` are written by the real helpers
   from a deployed-shaped `.env` (the DB URL is rewritten to
   `postgresql://…@127.0.0.1:5432/verdaxis_staging`, the KYC switch is `false`,
   the revision is the compiled allowlist entry).
5. `--bind 127.0.0.1:8011 --budget-file …`, with only the six projected
   environment variables, answers
   `{"status":"ok","db":"ok","environment":"staging","release_sha":"20c9d7dd…"}`
   on the first attempt, and the **unmodified** `scripts/validate_health_response.py`
   accepts it (`READINESS_OK staging/20c9d7dd…`). The startup proves the whole
   deployed path: flat env → `Config` validation → budget cross-check → pool
   role/timezone attestation → readiness.

## Bootstrap sequence (two releases)

Recorded in `tests/unit/test_rust_units.py`:

1. **Release one (this tree).** `deploy/rust/staging-policy.json` ships
   `rust_enabled: false`. A staging deploy of this SHA runs the exact pre-Rust
   command ledger — no Rust preparation, no artifact requirement, one service
   restart — and `test_bootstrap_release_deploys_without_the_rust_unit_or_an_artifact`
   proves it. The guarded helper and artifact support land here.
2. **Unit installation.** The operator runs `install_systemd_units.sh
   --dry-run` then `--apply` with `--environment staging --source-ref <SHA>`
   from the fixed clean checkout. The installer requires `HEAD == source-ref`,
   rejects hidden index flags and replacement refs, and installs only
   manifest-selected bytes. `test_installer_requires_its_own_clean_checkout_at_the_source_ref`
   pins that contract; there is no Rust-owned installer.
3. **Release two.** With the unit installed, a reviewed release flips
   `rust_enabled` to `true`. `test_activation_requires_the_manifest_unit_and_a_verified_artifact`
   proves that activation then needs the verified artifact (a missing one
   refuses before any service stops) and that the deploy restarts both units in
   one `systemctl restart` and clears state only after both readiness gates.
   `test_committed_tree_is_the_inactive_first_bootstrap_release` pins that this
   tree is still the inactive first release.

Activation is read from two committed records (the policy and the manifest), not
an ambient flag. Timers stay quiesced through the transition per
`docs/runtime-hardening.md`.

## Deviations from the plan's literal text

1. **The unit starts the binary from the deployed environment.** The plan's unit
   names only `--bind`, but the released binary read its whole configuration
   from a `RUST_CONFIG` JSON document that no deploy artifact writes, and it
   read the budget from the `RUST_PREPARED_BUDGET` environment variable — which,
   as shipped, nothing set on the host. A reviewed unit could therefore not have
   started the released artifact at all. Task 12c adds
   `config_from_deployed_environment`/`deployed_arguments` (same validated
   `Config`, same project defaults), moves the budget source into
   `Config.prepared_budget` (`--budget-file` in the unit, `RUST_CONFIG` still
   accepted for the contract harness), and leaves `connect(config)` unchanged.
   This is also what PM requirement 1 needs: the unit's paths are the files the
   Task 12b transaction writes.
2. **The budget precheck uses the helper's real flags**
   (`--verify --environment-file … --release-sha … --output …`) instead of the
   plan's illustrative `--check … --budget-file`.
3. **`runtime.env` carries six keys, not five**: the plan list omitted
   `KYC_EXECUTION_GATE_ENABLED`. The Rust reads bind that switch, so a Rust side
   that guessed the value would answer against a different KYC policy than
   Python on the same host and corpus. It is projected from the deployed `.env`
   by the Task 12b helper, with Python's default when the key is absent.
4. **No `.gitignore` change** — Task 12b already ignores `.runtime-rust/`.

## CI follow-ups from Task 12b (draft PR #86, `test` job, run 38998196777)

Task 12b's tests passed on the test host but failed in GitHub CI. Both causes
are fixed here, each in its own commit, and each is reproduced and re-proven on
the test host.

**A. The runtime-owned source pin (`fix: repin the deploy script blob after the
Rust staging extension`).** `tests/monitor/test_source_ownership.py` pins each
runtime-owned source file by git blob, so a stage that changes one updates its
pin in the same reviewed commit. Task 12b extended `scripts/deploy.sh` in place
and left the pre-Rust pin in place. The pin now names the committed bytes:
`git hash-object scripts/deploy.sh` =
`3e75a493b502cfd065dbbf99c0b9e3da8e4c5622`. The other four pins are unchanged
and recomputed. Evidence: with the pre-fix pin the monitor ownership test fails
on the test host (`AssertionError: scripts/deploy.sh`), and with this pin
`tests/monitor/test_source_ownership.py` passes 6/6 inside the full monitor run
below; the pinned value, `git hash-object scripts/deploy.sh` and
`git rev-parse HEAD:scripts/deploy.sh` all agree.

**B. The release fixture in a clean checkout (`fix: run the release fixture from
the test interpreter`).** The fixture's `venv/bin/python` wrapper exec'd the
repository `venv/bin/python` when that file existed and `/usr/bin/python3`
otherwise. GitHub CI installs the dependencies into the pytest interpreter and
carries no repository `venv/`, so eight tests failed there with
`ModuleNotFoundError: No module named 'dotenv'` from the real artifact, runtime
and budget helpers the fixture dispatches. The wrapper now execs
`sys.executable`, the interpreter that runs the tests, so a clean checkout
imports the dependency set under test.

The second half of the CI report — `Cannot infer backend environment from path
…/deploy-checkout` — is the expected notice `scripts/deploy.sh` prints when the
checkout is not a canonical `<env>/be` layout; it appeared in the CI log as part
of the failing deploys' sanitised stderr, not as a separate defect. The fixture
must stay outside that layout: a canonical path makes `deploy.sh` reset `PATH`,
which would replace the fixture's fake `sudo`, `curl` and `systemctl` with the
host's real commands. The fixture therefore uses the documented override
(`TARGET_BRANCH`, `SERVICE_NAME`, `HEALTH_URL`, `DEPLOY_ENVIRONMENT`), and a new
test pins both halves of that contract: the script demands all four variables
for a non-canonical path, and the fixture supplies all four.

Evidence, on the test host, in a copy of this tree made by excluding `venv/`
(so `ROOT/venv/bin/python` is absent, exactly as in CI), run with the shared
interpreter outside the copy (`work/verify/clean-b.log`):

| Run | Result |
| --- | --- |
| Before the fix, `tests/unit/test_rust_release.py` (the `d06e8248` fixture) | **8 failed**, 25 passed — the same eight tests as CI, same `dotenv` cause |
| After the fix, `tests/unit/test_rust_release.py` | **34 passed**, exit 0 |
| After the fix, `tests/unit/test_rust_release.py tests/unit/test_rust_units.py` | **48 passed**, exit 0 |
| The `test` job's exact command (`ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=… DATABASE_URL=sqlite… pytest tests/unit tests/monitor -q`) | 2702 passed, 1 skipped, exit 0 — see *Gate results* |

## Gate results

All on the test host through `verdaxis-prod-test`, against the landing commit
`20c9d7dd`; logs in `work/verify/*.log` (not committed). Every command's own
exit code is captured; each is 0 unless the table says otherwise.

| Gate | Result |
| --- | --- |
| `pytest tests/unit/test_rust_units.py tests/unit/test_rust_release.py -q` | **48 passed** (14 unit-contract + 34 release-fixture), exit 0 |
| existing deploy / start-guard / installer / budget / artifact / identity / KYC-key-preflight / migration-checkpoint / ACL-convergence / monitor-systemd suites | **213 passed**, exit 0 |
| `pytest tests/monitor -q` | **291 passed**, 1 skipped, exit 0 |
| the draft PR #86 `test` job's exact command (`ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=… DATABASE_URL="sqlite+aiosqlite:///:memory:" pytest tests/unit tests/monitor -q`) | **2702 passed**, 1 skipped, exit 0 |
| `SQLX_OFFLINE=true cargo test --manifest-path rs/Cargo.toml --locked --workspace` | all 34 targets ok, 0 failed, exit 0 |
| `cargo fmt --manifest-path rs/Cargo.toml --all -- --check` | pass, exit 0 |
| strict `cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings` | pass, exit 0 |
| ShellCheck on changed shell files | none changed — no repository shell file is in this diff (`git status --porcelain -- '*.sh'` empty) — so there is no new-warning delta |
| `git diff -- deploy/postgres/app_acl_policy.sql` | empty (0 lines) |
| `systemd-analyze verify` (test host 255 with a private fake root, ubuntu:22.04 249, ubuntu:24.04 255) | exit 0, no diagnostics; the test host run on the bare staging paths exits 1 with only the absent-path notice — see *systemd syntax verification* |
| release artifact start on the staging ABI (ubuntu:22.04, glibc 2.35) | readiness accepted by the unmodified validator, exit 0 — see *Artifact startup* |

Red-to-green evidence (earlier stages, `work/rust-phase1/`): `t12c-units-red.log`
(the new suite failed with `FileNotFoundError` for the absent unit before Step
3), `t12c-py1.log` and `t12c-py2.log` (two fixture/projection defects found and
fixed: the rustfmt and deployed-`.env` shape), `t12c-cargo1.log` (formatting),
then the green runs `t12c-py3.log`, `t12c-gates1.log`, `t12c-gates3.log`,
`t12c-flake2.log`. The checks in the table above were re-run against the landing
commit `20c9d7dd` and are in `work/verify/`: `ci-command.log`, `focused.log`,
`changed-suites.log`, `monitor.log`, `rust-gates.log`, `systemd.log`, `abi.log`
and `clean-b.log` (the clean-copy, no-`venv/` proof of fix B: 8 failed before,
34 then 48 passed after).

**One intermittent failure observed, reported as-is.**
`test_rust_release.py::test_host_key_matching_the_pinned_digest_is_preferred`
(a Task 12b fixture test, untouched by Task 12c) failed once in one of six runs
of the two changed suites with the deploy trap's
`Deployment failed; code and published identity remain aligned …` message; the
`--tb=short` capture did not retain the failing phase. It passed 5/5 in
isolation (`t12c-flake.log`) and both changed suites then passed 2/2
(`t12c-flake2.log`). Nothing in this task's diff touches the deploy
transaction, the fixture harness, or the health path, so this reads as a
timing-sensitive fixture flake on the 2-CPU test slice, not a Task 12c defect.
It is left for the PM to weigh; no retry wrapper or weakened assertion was added.

**Re-run against the landing commit:** it did not recur. `20c9d7dd` passed the
two changed suites twice (48 each: `work/t12c-commit-suites.log`,
`work/verify/changed-suites.log`), the focused suites (213,
`work/verify/focused.log`), `tests/monitor` (291 passed, 1 skipped,
`work/verify/monitor.log`), the `test` job's exact command (2702 passed,
1 skipped, `work/verify/ci-command.log`) and the clean-copy, no-`venv/` runs of
the two changed suites (48 passed, `work/verify/clean-b.log`). The observed rate
stays 1 in the Task 12b runs, still unreproduced in isolation.

## What remains outside this task

Live activation needs separate operator approval: the two-release bootstrap on
the staging host, the artifact provenance record/signature from the PM
workstation, the unit installation, and flipping `rust_enabled` in a reviewed
release. This task authorizes no live command, not even a dry run.
