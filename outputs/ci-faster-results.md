# Rust-only release and CI results

## Scope

Baseline: `61bf722dc`. Branch: `ci/faster-pipelines`. All commits are local.
No push, SSH, host access, service activation or database adoption was performed.
Rust is the only application backend. Retained Python is host/operator tooling,
monitor code, stdlib disposable database fixtures and their tests.

## Source removal

Removed the application tree, migration implementation/configuration, API
container/compose assets, generated FastAPI schema, template, backend-only data
tools, backend test servers and test suites, and the Python comparison stack.
Moved retained tooling tests and fixtures to `tests/tooling/`; monitor tests stay
in `tests/monitor/`. Removed both Python backend service source files.
Final path and executed-test counts will be recorded after gates complete.
The gateway interrupted three queued gate drivers; only their completed exit
records are used. Remaining runners and whole-suite gates are being repeated.

Fixed expected Rust assertions remain. App-backed comparisons without fixed
expected values are removed. Native runtime preference and quota inputs now
reside in Rust, with their values preserved. Disposable PostgreSQL setup uses the
frozen native baseline and SQL fixtures; retained historical audit metadata
allows the unchanged role-policy validator to run.

## CI

The required contexts read from staging protection were `test` and
`postgres-analytics` (strict mode). `test` now runs retained tooling/monitor
checks in the Rust workflow. `postgres-analytics` always reports and requires all
four PostgreSQL cells to succeed. Sixteen shell runners appear exactly once.
No path filter can leave a required context absent. Publication waits for the
whole offline workspace, both lints, native checks, PostgreSQL, tooling and
artifact gates. Binary/archive subjects and digest checks are preserved.

Cargo cache keys bind the pinned toolchain, Cargo.lock, target and cell, not the
source SHA. `Swatinem/rust-cache` is pinned to executable commit
`d1527c8f67ef9af357290475d4af4b0fab8f12e0`. Release compilation runs parallel to
offline tests. Duplicate offline work was removed from the compliance database
runner, not from publication dependencies.

Expected warm hosted critical path: **25–35 minutes, provisional**. The measured
23m14s PostgreSQL step ran `run_postgres.sh` alone and failed; the later ten steps
were skipped. It is not evidence of a balanced 11-script serial workload. New
matrix timings must be collected before claiming a measured speedup. The
previous release build plus installer was about 13 minutes and is now parallel
with roughly 14 minutes of offline work.

## Exact compatibility exceptions

These are deliberately retained under the PM's release decision:

- `requirements.txt` and `constraints.txt` remain byte-identical. The installed
  M helper rejects their change against the next release. Remove them only in a
  later separately approved release.
- `scripts/deploy.sh` remains byte-identical, including its dependency-change
  check, old-service stop/fence and native publication protocol.
- `deploy/scripts/pre-m-bundle.sh` remains byte-identical. It restores captured
  pre-M source and restarts that release's Python API.
- `scripts/prepare_rust_budget.py:70` retains its Settings import for legacy
  operator/rehearsal compatibility. Normal native budget tests now use fixed
  expected fields; Python tooling tests stage approved historical Settings in a
  temporary fixture rather than restore the application to this checkout.
- `scripts/rust_cache_authority.py:318` retains its legacy activation dependency
  on `scripts/apply_migration_checkpoint.py`, whose migration-engine imports
  remain unchanged. Native deploy authority commands do not use that path.
- `deploy/scripts/generate-sqlx-baseline.sh` is a historical baseline reproduction
  tool and needs approved old source. It is not the native release migrator.
- `deploy/tests/rehearsal-ci-setup.sh:85–88`,
  `deploy/tests/rehearsal/stack.py:375` and
  `deploy/tests/rehearsal/companion.py:520` keep historical setup behavior.
  This deleted-source checkout cannot supply their old application. Retaining
  the manual workflow does not establish a passing rehearsal.
- `deploy/tests/phase7-acceptance.sh`, `deletion-contract.sh` and
  `final-measure.sh` retain historical deleted-source/comparison dependencies.
  They are not normal CI jobs and were not represented as working installed
  acceptance tools.
- Migration audit table names in native adoption, ACL policy and tests refer to
  inert retained database history, not an executable Python backend.

Therefore the raw import/history grep still has intentional compatibility and
inert-data hits. A zero-hit grep is not claimed. The application trees and all
live Python API units are absent from source.

## Gate record

Logs and exact exit files are in ignored `work/ci-faster/gates/` and
`work/ci-faster/final/`. Pending gates will be replaced with observed results;
no pending gate is counted as a pass.

Observed passing gates so far:
- PostgreSQL authority, reminders, abilities, inspection, feedback and schema runners:
  exit 0 each.
- Native SQLx baseline migration and metadata prepare check: exit 0.
- Retained tooling and monitor suite: exit 0, 2,409 passed, 7 skipped and
  32 subtests passed on the deleted-source tree.
- Retained reviewed shell syntax and ShellCheck: exit 0.
- Workflow YAML parse, full action pins, dependency graph, inline Bash/Python and
  sixteen-runner uniqueness: exit 0. `actionlint` is not installed.
- Disposable Linux artifact contract: exit 0, 16 cases plus workflow assertions.
- Disposable Linux host-inventory contract: exit 0.
- Disposable Linux deploy transaction: exit 0.
- Disposable Linux static unit installer: exit 0, 32 cases; two full-mode cases
  are explicitly not run by static mode.
- Native real-Caddy route test: exit 0, one test passed.
- Stdlib notification and preference PostgreSQL runners: exit 0 each.

Observed blockers/failures, not hidden:
- Linux release build: exit 101, `E0463: can't find crate for core`; the local
  macOS toolchain has no `x86_64-unknown-linux-gnu` target. Full installer: exit 1,
  `full mode requires VERDAXIS_READ_BIN to name an executable Linux ... binary`.
  Linux release/glibc/package/full-installer execution still needs its hosted
  runner; no substitute binary was used.
- Advisory gate: exit 1, two findings in unchanged locked transitive dependencies:
  RUSTSEC-2026-0387 (`crypto-bigint` 0.5.5) and RUSTSEC-2023-0071 (`rsa` 0.9.10),
  both reached through `jsonwebtoken`. No advisory suppression was added.
- Manual rehearsal: exit 2, `require root on a GitHub-hosted Linux runner`.
- Initial integrated PostgreSQL run: exit 101,
  `switch_follows_the_deployed_environment`: `ModuleNotFoundError: dotenv`.
  CI now installs the three pinned retained helper packages with system Python;
  final local reruns use the existing tooling interpreter.
- Initial feature lint: exit 101, missing `Value` type in the fixed budget test;
  the qualified type is committed and final gates are queued.
- Second PostgreSQL run: exit 101, `python_serves_no_offboarding_route` read the
  removed application directory. That Python-only source scan is now removed;
  Rust disabled-command, quota and Caddy assertions remain unchanged.

## Host actions

Before promoting this source release, the PM must complete the approved M
installation/adoption on both hosts, transfer and drain timer ownership to Rust,
stop/fence the old Python services, and obtain the runbook's exact readiness and
signed acceptance. Keep the captured pre-M package for its rollback procedure.
Do not run retained legacy helper paths against this deleted-source checkout.
No host action is inferred from a passing source fixture.
