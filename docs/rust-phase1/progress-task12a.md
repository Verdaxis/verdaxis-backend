# Task 12a — verify artifacts and gate CI publication

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`.

## Gate status: the test host is PAUSED — no gate is checked

`verdaxis-prod-test` refuses before it contacts the host: the PM pause file
(`~/.local/state/verdaxis-prod-test/pause`) reads "Phase 0 rehearsal attempt 2 on
the prod host (17 Oct, from about 00:15 SGT, 1-1.5 hours)". The wrapper's message
is "do not retry". No test, build, Docker command or Cargo command was run for
this task, locally or on the host.

Consequently **no plan checkbox for Task 12a is checked** and every gate below is
recorded as unrun. Nothing in this document claims a passing test.

### Resume: run these, in this order, when the host unpauses

One call at a time, each with its output redirected to `work/rust-phase1/` and a
`CALL_EXIT=` marker, then wait in bounded steps for the marker.

1. Artifact fixture tests — 34 cases, expected green:
   `verdaxis-prod-test "$PWD" 'venv/bin/python -m pytest tests/unit/test_rust_artifact.py -q'`
   The plan's Step 2 expected red (verifier absent) was not captured: the host was
   already paused when the verifier was written, so the first run will be green or
   will fail honestly. It is not recorded as a captured red.
2. Pure Rust identity tests and the whole workspace, with the compiled identity
   set:
   `verdaxis-prod-test "$PWD" 'export RELEASE_SHA=$(git rev-parse HEAD) SQLX_OFFLINE=true; cargo test --manifest-path rs/Cargo.toml --locked --workspace'`
3. Format and lints:
   `verdaxis-prod-test "$PWD" 'cargo fmt --manifest-path rs/Cargo.toml --all -- --check'`
   `verdaxis-prod-test "$PWD" 'export RELEASE_SHA=$(git rev-parse HEAD); cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings'`
4. Release build and the glibc smoke test (the VPS runs glibc 2.39, so this is
   only a smoke test — see "What only the first real CI run can prove"):
   `verdaxis-prod-test "$PWD" 'export RELEASE_SHA=$(git rev-parse HEAD); cargo build --manifest-path rs/Cargo.toml --locked --release -p verdaxis-read'`
   then `objdump -T rs/target/release/verdaxis-read | python3 scripts/verify_rust_artifact.py --glibc-check --max 2.35 --source-sha "$(git rev-parse HEAD)"`
5. Whole Python unit suite with the documented environment (the 19 known macOS
   `mapfile` failures in `tests/unit/test_deploy_health.py` do not reproduce here):
   `verdaxis-prod-test "$PWD" 'ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars DATABASE_URL="sqlite+aiosqlite:///:memory:" venv/bin/python -m pytest tests/unit -q'`
6. A real round trip on Linux: build the release binary (step 4), package it,
   sign a record on the workstation, then verify on the host.
   `python3 scripts/verify_rust_artifact.py --package --binary rs/target/release/verdaxis-read --data-dir deploy/rust --lockfile rs/Cargo.lock --toolchain "$(grep -oP 'channel = "\K[^"]+' rs/rust-toolchain.toml)" --source-sha <sha> --target x86_64-unknown-linux-gnu --output work/rust-phase1/release.tar --metadata-out work/rust-phase1/artifact-metadata.json`

## What was built

Files created (the plan's list):

- `rs/read/src/runtime/artifact.rs` — the plan's interface
  `pub fn verify_artifact_identity(compiled: &str, runtime: &str) -> Result<(), String>`,
  plus `verify_compiled_identity`, the `LIBC_MIN = "2.35"` floor, the compiled
  revision allowlist, and `version_json()`.
- `deploy/rust/artifact.schema.json` — the release metadata contract. Its
  `required` set, consts and bounds are asserted against the verifier's executable
  validator by a test, so the two cannot drift.
- `scripts/verify_rust_artifact.py` — operator CLI, four modes: default host
  verify, `--attest` (workstation), `--package` (CI), `--glibc-check`.
- `tests/unit/test_rust_artifact.py` — 34 fixture tests.
- `.github/workflows/rust-read-ci.yml` — three jobs.

Files changed outside the plan's list, each minimal and named here for review:

- `rs/read/src/main.rs` — added `--version-json`, which the plan requires ("The
  binary exposes `--version-json` before serving"). It prints the version document
  and returns before any config, pool or policy work.
- `rs/read/src/lib.rs`, `rs/read/src/runtime/mod.rs` — register and re-export the
  new module.
- the plan's Task 12a **Resume point** — records the attestation split, as the PM
  instructed.

No deploy transaction, systemd unit, ACL policy or `.gitignore` change.

## Archive contract

The archive is an uncompressed POSIX/USTAR tar, read whole into memory so the
bytes that are hashed are the bytes that are parsed. Members:

- `verdaxis-read` (mode 0755) — the release binary;
- `artifact-metadata.json` — release metadata;
- `SHA256SUMS` — one line per other member;
- `data/*.json` — every `deploy/rust/*.json` except `artifact.schema.json`.

The member set must equal `{verdaxis-read, artifact-metadata.json, SHA256SUMS}`
plus exactly the paths the metadata lists. Every refusal happens before any file
is written to disk: the tests assert the output directory is never created and a
marker file is never produced by a fixture "binary" that would create it if
executed.

Refused: any non-regular member (directory, symlink, hardlink, char/block/fifo),
absolute paths, `.`/`..`/empty path segments, backslashes, NUL bytes, duplicate
members, unexpected members, absent binary, a member above the per-member bound,
an archive above the total bound, more than the maximum member count, a
compressed archive, a `SHA256SUMS` that does not exactly cover the members, and
any digest mismatch between the metadata, `SHA256SUMS`, the policy manifest and
the actual bytes.

Provenance refusals: a wrong archive digest, wrong source SHA, wrong target,
wrong workflow path, wrong workflow ref, wrong repository, an unverified
attestation, a record attesting only one subject, an unsigned record, a record
signed by another key, and a record that names a different archive or binary
digest. Nothing is extracted on any of these.

## CI shape (PM decision 1 and 2)

Triggers: `pull_request` to `staging`/`prod`, `push` to `staging`/`prod`, and
`workflow_dispatch`. No job runs on a feature-branch push.

| Job | When | Needs |
| --- | --- | --- |
| `offline` | PRs, pushes, dispatch | — |
| `contract` | PRs, pushes, dispatch | `offline` |
| `publish` | push to `staging`/`prod` only | `offline` **and** `contract` both exactly `success` |

The publish condition also names `github.event_name == 'push'` and the two branch
refs, so both `skipped` and `failure` block it. It carries
`permissions: {contents: read, id-token: write, attestations: write}` only on that
job. No secret is referenced anywhere in the workflow.

- `offline` runs the artifact fixture tests, the offline Rust workspace tests,
  `cargo fmt --check`, strict `clippy`, the release build for
  `x86_64-unknown-linux-gnu`, the glibc floor check from `objdump -T`, and the
  packaging step; it uploads the binary, archive, metadata and glibc report.
- `contract` (same job only) creates a venv, installs `sqlx-cli 0.9.0`, installs
  the pinned Caddy, downloads the release artifact, **runs it** as
  `--version-json` and requires its compiled identity to equal `GITHUB_SHA`,
  then runs the combined disposable gate
  (`--group all --rust-target all --mixed-caddy --require-complete-ledger --report docs/rust-phase1/ci-contract.json`)
  and `cargo sqlx prepare --check` against the same job's migrated container. It
  uploads the gate report and `tested-binary.json`.
- `publish` requires the contract job's recorded digest to equal the digest of
  the downloaded binary, requires the report to name the same source SHA and to
  have no failed groups, attests both subjects with
  `actions/attest-build-provenance`, and uploads the release bundle.

`.sqlx` metadata: the project calls `sqlx::query(...)` at runtime only
(`grep` finds no `query!`/`query_as!`/`query_scalar!` macros), so no `.sqlx`
directory exists and there is nothing to commit. The contract job's
`cargo sqlx prepare --check` is the guard: if a future change adds a compile-time
query macro, that step fails until the metadata is committed.

## Pinned Action SHAs

Resolved from each release tag with read-only `gh api` calls (`gh` 2.96.0,
authenticated as `jonathanjie`), dereferencing annotated tags to commits.

| Action | Tag | Commit SHA |
| --- | --- | --- |
| `actions/checkout` | v5 | `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` |
| `actions/setup-python` | v6 | `ece7cb06caefa5fff74198d8649806c4678c61a1` |
| `actions/cache` | v4 | `0057852bfaa89a56745cba8c7296529d2fc39830` |
| `actions/upload-artifact` | v4 | `ea165f8d65b6e75b540449e92b4886f43607fa02` |
| `actions/download-artifact` | v4 | `d3f86a106a0bac45b974a628896c90dbdf5c8093` |
| `actions/attest-build-provenance` | v2 | `e8998f949152b193b063cb0ec769d69d929409be` |

Toolchain and Caddy are pinned without a third-party Action: the Rust channel is
read from `rs/rust-toolchain.toml` and installed with the runner's `rustup`;
Caddy 2.11.7 `linux_amd64` is downloaded and checked against
`a7a433a1b133efc3c8d10eb0b99d52a24b5ef5c322dc77f5282182b1c0402139ab83f3a99f0c52409df77d20123fb0b523edad8a66d8f5e49136197bf61ef0e7`
from the release's own `caddy_2.11.7_checksums.txt`.

## Attestation split

See the plan's Task 12a Resume point. In one line: **the PM workstation holds
`gh` and the Ed25519 signing key and produces a signed provenance record; the
deploy host verifies that signature and the record's bindings with the standard
library and `cryptography` only, then extracts.** The host installs nothing.

The workstation check was not run for this task: it needs a published GitHub
attestation, which needs a real push and a real CI run.

## One documented deviation from the plan's literal CLI

The plan's Interface line gives the host invocation as
`python scripts/verify_rust_artifact.py --archive PATH --source-sha SHA --target x86_64-unknown-linux-gnu --output-dir PATH`.
That form still works, but it now also requires
`--provenance-record`, `--provenance-signature` and `--provenance-public-key`,
because PM decision 3 requires the attestation check to pass before anything is
extracted, and the host must not hold `gh`. The extra flags carry the
workstation's signed result. Nothing else about the four documented flags
changed.

## What only the first real CI run can prove

- That the release binary built on `ubuntu-22.04` needs no glibc symbol newer
  than 2.35. The VPS smoke test runs glibc 2.39, so it cannot prove this; only
  the `ubuntu-22.04` runner can.
- That the two Action SHAs and the `attest-build-provenance` v2 interface behave
  as expected on a real run (`id-token`/`attestations` permissions, subject-path
  glob expansion for two subjects).
- That the combined contract gate passes end to end in CI, including the
  disposable Caddy and the unchanged integration suite inside that one job.
- That `cargo sqlx prepare --check` and `cargo install sqlx-cli 0.9.0` behave as
  they did on the test host.
- That `gh attestation verify` accepts the published subjects on the PM
  workstation, and therefore that the split is workable in practice.
- That `actions/cache` keys and `rustup default` produce a usable toolchain on
  `ubuntu-22.04`.

## Unrun gates

Tests, builds, Cargo commands, Docker: **not run, host paused**. Nothing here is
claimed as passed.
