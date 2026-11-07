# Disposable full rehearsal

## Source release status

This is a historical pre-M rehearsal driver. Its retained setup still builds a
legacy database through the old application and migration sources. Those sources
are absent from the Rust-only release. The manual workflow and restore protocol
remain unchanged for release compatibility, but this checkout is not a working
input for the old before/after rehearsal. Do not report its retention as a
passing rehearsal. Rebinding that driver to an approved historical source package
is separate work; no live host procedure is changed here. Normal Rust, tooling
and PostgreSQL CI checks do not install the old backend environment.


This lane runs on GitHub-hosted VMs only. It has no host credentials and does
not authorize a live cutover. Normal pull-request and push jobs are unchanged.

## Dispatch

After the branch is pushed by the operator:

```bash
gh workflow run "Rust read CI" --ref feat/rust-phase-10-rh -f rehearsal=true
```

The boolean defaults to false. The job waits for the same run's `offline`
release artifact. Its two independent legs are Ubuntu 22.04 / systemd 249 /
staging and Ubuntu 24.04 / systemd 255 / production. No GitHub deployment
environment or release private key is used.

Allow approximately 2–6 hours after the offline build for the full before/after
publication matrix. The matrix job has a six-hour limit. The complete run also
includes the workflow's existing contract jobs. The first run can take longer
to download dependencies. This is a source-based estimate, not a measured
result; the first hosted run must establish whether the complete matrix fits.

## Real checks and local contracts

`deploy/tests/full-rehearsal.sh --disposable` requires a disposable hosted Linux
VM with root and systemd. It refuses a Mac before creating files. The rehearsal
must use actual PostgreSQL, Caddy, users, ACLs, unit invocations and previous
release startup; fixture command adapters do not count as installed evidence.
The previous release is the signed staging push artifact from run
`42034562253`, release `0920be378ebbc42e88afba455d9521d60d49845a`.
GitHub provenance is verified before CI produces disposable-key records.
A separate manual-only provenance job uses GitHub OIDC to attest the current
artifact; it does not use the release private key. The actual feature-branch
workflow ref stays in the record and is admitted only behind the hosted
rehearsal marker, never by the normal serving-host gate.

W2 installation stops, drains and fences old producers before M. Activation
and per-group acceptance follow M's exact-ready restart because the M binary
admits only `sqlx_1`. The old package remains installed through rollback tests;
W5 deletion runs only after the forward and restore checks.

The ten Task 31 names each print one `PASS` or `FAIL` line. A failed prerequisite
makes dependent checks fail, not skip or pass. The evidence validator requires
all ten names, successful results and matching file digests. Source manifest
rows are evidence requirements, not accepted results.

Local `test_phase10_*.py` and `verify-host-inventory.sh --fixture` test contracts
and refusal cases only. They do not collect or sign a real host inventory.
The disposable signed inventory describes the VM, not either live host.
Frontend pages and the preserved 03:00/04:00 scheduler baseline are explicitly
modeled disposable fixtures. Their local HTTP responses, actual dump/copy work
and unchanged scheduler bytes test monitor and preservation behavior. They are
not proof of live frontend content or live scheduler bytes. API readiness and
rollback use the actual release executables, not fixture HTTP servers.

## Evidence

Each matrix leg uploads `rust-rehearsal-<runner-image>-<environment>` with
**7-day retention**. Only the sanitized `public/` evidence directory is uploaded.
It contains `results.json` and digest-bound observations. Runtime environment
files, credentials, disposable private keys, protected pre-M bundles and raw
logs must remain outside that directory. No live secrets are supplied.

## Current M integration blockers

The first hosted run is not expected to pass until these interfaces are resolved:

- Adoption compares one compiled catalog fingerprint that includes the literal
  production schema owner. The correctly seeded staging schema has the staging
  migrator owner and therefore cannot match that fingerprint.
- ACL convergence removes the independent bundle reader's direct read grant on
  `alembic_version`. The protected restore then cannot verify the retained audit
  head. Reader authority must be explicitly designed and tested; the rehearsal
  does not silently restore that grant.

These are source-review findings, not observed hosted-run failures. Do not alter
fixture ownership or bypass the read-only checker to obtain a passing result.

A failed or missing cell blocks disposable acceptance. A missing or expired
previous artifact, unsupported installed package closure, or changed pinned
package fails the run; it must not be replaced with a fixture binary. Both CI
legs must pass before this lane is described as rehearsed. Separate authorized
host inventory, reviews, soak and cutover acceptance remain required.
