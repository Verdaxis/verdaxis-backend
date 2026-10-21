# Rust staging unit bootstrap verification

Date: 19 October 2026. Branch: `fix/rust-unit-verify-bootstrap`.
Base: `aea484d7f611e82eff8f523685566f4b8b7bfc75` (policy-off release B).

## Change and scope

The Rust unit now starts `/usr/bin/env` with the absolute installed binary path
and its unchanged arguments. No `-i`, shell, optional environment file, condition,
or placeholder binary is added. `env` preserves the systemd environment and execs
the binary in the same PID. The sandbox, prestart checks, account, restart policy,
resource limits and artifact installation remain unchanged. A real invocation
checks environment, arguments and PID preservation.

Current unit SHA-256:
`e18cae0829bd0424536c1640b9b3ff63f2618f6848dd7bbd86c1b57e4f5b4666`.
The old `6662a389…9453` evidence in Task 12c materialized the binary path and did
not prove first-install acceptance. It is now marked historical.

## Reproduction and gates

All test calls use `verdaxis-staging-test` and `heavy run 3600`. No Mac builds,
live deploys, unit installations, service actions, database changes or
measurements were run. Test host: systemd `249.11-0ubuntu3.23`, Python 3.12.13.

`test_bootstrap_verifies_without_a_rust_artifact` runs the real analyzer for
three caller shapes. The old helper invocation is pinned to the source at
`f4663bf4091ea4261875414f017cd27cd5a2ceb1`; installer and current helper
invocations are read from current source. Only the absolute checkout prefix is
relocated into a private test directory. Existing Python/OS executables and
required dependency targets are supplied, but `.runtime-rust/` is absent.
The test does not execute the full deployment or root installer.

| Gate | Before fix | After fix |
| --- | --- | --- |
| Old helper dry-run verification of corrected B | 1, missing Rust executable | 0 |
| B unit installer verification | 1, missing Rust executable | 0 |
| B helper dry-run verification of S | 1, missing Rust executable | 0 |
| Affected Python suites | not run against old unit | 0: 162 passed |

The initial fixture attempt returned 1 because systemd 249 does not support
`verify --root`; it was corrected before the actual red run. An initial affected
suite selection returned 4 for a nonexistent test filename; the corrected
selection above completed successfully. Raw logs remain in ignored `work/`.

Affected suites: `test_rust_units`, `test_rust_release`, `test_rust_budget`,
`test_rust_artifact`, `test_deploy_health`, and monitor source-ownership tests.
Full Rust workspace: `SQLX_OFFLINE=true cargo test --manifest-path rs/Cargo.toml
--locked --workspace`, with the base full SHA as `RELEASE_SHA`: exit 0. All test
binaries and doc tests finished with zero failures.

PR: https://github.com/Verdaxis/verdaxis-backend/pull/94 (normal, target `staging`).
Implementation commit: `ab34808ba041877d29a246fa65e3bac588f6a7d0`.
CI at that commit: Rust `offline` and `contract` passed (run `39151838054`);
backend `test`, `postgres-analytics` and `postgres-phase0` passed
(run `39151838058`). `publish` was skipped as required for a PR.
`gh pr checks --watch` exited 0. Merge remains with the PM. A later documentation
commit records these results; its CI must also finish before the handoff.

## Source consumer audit

- `tests/unit/test_rust_units.py`: exact launch assertion now requires the env
  prefix; three real-analyzer regressions and a real exec/PID check cover it.
- `tests/unit/test_rust_release.py`: release archives now carry the shipped Rust
  unit instead of replacing it with `/bin/true`. Service actions remain fake.
- `docs/rust-phase1/progress-task12c.md` and the Phase 1 plan: current digest and
  correction added; old verification claims explicitly remain historical.
- `tests/monitor/test_source_ownership.py:28–34`: pins app/deploy sources, not the
  Rust unit. No pin change is needed; deploy and installer bytes are unchanged.
- Manifest selection and provenance checks compute unit bytes from the approved
  commit. No static unit digest is embedded in shipped executable code.
- Source search found no Rust main-PID executable inspection. Runtime readiness
  checks exact JSON identity, not an `ExecStart` executable name.

## Remaining bootstrap order audit

Line references below refer to the base source, which this fix does not change.

| Step / source | Dependency and finding |
| --- | --- |
| Old helper `scripts/deploy.sh:560` at f4663bf4 | Verifies every staging manifest unit before an artifact exists. Fixed in shipped unit; old helper need not change. |
| Installer `scripts/install_systemd_units.sh:228` | Same missing-executable dependency. Fixed in shipped unit. Installer does not start Rust. |
| Current helper `scripts/deploy.sh:729` | Same dependency during S dry-run. Fixed in shipped unit. Dry-run does not execute artifact or prestart helpers. |
| Old helper `scripts/deploy.sh:663–704` at f4663bf4 | Publishes B identity and deploys Python only. No Rust runtime input is consumed. |
| Current helper `scripts/deploy.sh:820–846` | Requires loaded unit before mutation, then verifies/extracts artifact. Unit is installed during B; verifier creates extraction parents (`verify_rust_artifact.py:559–572`). No active binary is needed. |
| Current helper `scripts/deploy.sh:919–987` | Budget and runtime environment are prepared before active binary publication; all precede joint restart at 994. First activation has no prior budget file. |
| `prepare_rust_runtime.py:146–150` | Requires `.env` identity S before generating runtime.env. This is an explicit operator prerequisite in PM runbook section 4, not an output that the deploy later creates. Preserve the approved private backup/config procedure. |
| Rust unit prestart / environment inputs | Revision helper and Python venv already exist from B. Release identity, runtime.env, budget and active binary exist before S restarts Rust. Missing files still fail a real start; verification does not waive startup gates. |
| Installer `scripts/install_systemd_units.sh:230–288` | Existing KYC credential source preflight, not a Rust-generated input. No change to its authority or placeholder policy. |

No further first-bootstrap dependency defect was found in these paths. This is a
source audit plus isolated correctness tests, not live staging acceptance.

## Promotion and production rule

PM must merge the corrected policy-off B, deploy it with the existing helper,
install its immutable unit bundle, and only then promote policy-on S. B and S
must retain identical unit/manifest bytes. This branch does not merge or deploy.

The parallel production unit must also verify before its first artifact exists.
Use an existing OS executable such as `/usr/bin/env` followed by the absolute
verified-artifact path, without `-i`, while retaining sandbox and startup checks.
Do not require a placeholder at the future binary path, skip unit verification,
or use a shell wrapper that changes PID/signal behavior.
