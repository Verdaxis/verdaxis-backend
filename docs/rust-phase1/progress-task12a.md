# Task 12a — verify artifacts and gate CI publication

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`.

## Execution log (17 Oct)

The PM lifted the first pause at 00:21 SGT and asked for the full brief. Three
gates passed; the PM then paused the host again (a third rehearsal, "Phase 0
final rehearsal at 56d75770 on the prod host, 17 Oct from about 01:15 SGT,
about 45 minutes"), so the last three gates were not run.

| # | Gate | Command | Result |
| --- | --- | --- | --- |
| 1 | Artifact fixture tests | `venv/bin/python -m pytest tests/unit/test_rust_artifact.py -q` | **34 passed** (first run 21 failed on a test-helper defect, fixed) |
| 2 | Offline Rust workspace | `RELEASE_SHA=<HEAD> SQLX_OFFLINE=true cargo test --workspace` | **153 passed**, 0 failed, 0 warnings |
| 3 | Release build | `cargo build --locked --release -p verdaxis-read` | **built** in 6m07s; 12,136,104 bytes; sha256 `9918660b9df992f8feabf945e209ca2cdea1972aac6dbbe9e7a5ee6d329cf34d` |
| 4 | glibc smoke check | `objdump -T … \| verify_rust_artifact.py --glibc-check` | **not run — PAUSED** |
| 5 | fmt / clippy | `cargo fmt --check`; `cargo clippy … -D warnings` | **not run — PAUSED** |
| 6 | Full Python unit suite | documented `ENVIRONMENT=test … pytest tests/unit -q` | **not run — PAUSED** |

All six Rust artifact unit tests ran inside gate 2:
`runtime::artifact::tests::{wrong_binary_refuses_new_release,
matching_identity_is_accepted, malformed_identity_is_refused,
version_document_reports_the_compiled_identity,
enumerated_allowlist_matches_the_reviewed_file, compiled_identity_matches_this_build}`.

The plan's Step 2 "expected red" was not captured: the code existed before the
brief, so capturing a red would have meant reverting working code. Said plainly:
**no red was captured.**

Logs: `work/rust-phase1/t12a-artifact-tests{,2}.log`, `t12a-workspace.log`,
`t12a-release-build.log`, `t12a-glibc.log` (holds the pause refusal).

### First failure and its fix

Gate 1's first run failed 21 of 34 cases on two test-only defects: the fixture
helpers passed raw `bytes` to `tarfile.addfile` (which wants a file-like object),
and `Fixture.assert_refused` called `self.assertFalse` on a non-TestCase helper.
Fixed in commit `3a6f5d18`; no verifier behavior changed. Re-run: 34 passed.

### Resume: the three gates still owed, then the rest

The host is paused. When it unpauses, run gates 4-6 above in that order, then the
Linux round trip (step 6 below). One call at a time, each logged with a
`CALL_EXIT=` marker:

1. `verdaxis-prod-test "$PWD" 'cargo fmt --manifest-path rs/Cargo.toml --all -- --check'`
2. `verdaxis-prod-test "$PWD" 'export RELEASE_SHA=$(git rev-parse HEAD); cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings'`
3. `verdaxis-prod-test "$PWD" 'ldd --version | head -1; objdump -T rs/target/release/verdaxis-read > /tmp/objdump.txt; python3 scripts/verify_rust_artifact.py --glibc-check --objdump /tmp/objdump.txt --max 2.35 --source-sha "$(git rev-parse HEAD)"'`
4. `verdaxis-prod-test "$PWD" 'ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars DATABASE_URL="sqlite+aiosqlite:///:memory:" venv/bin/python -m pytest tests/unit -q'`
5. The Linux round trip: package the release binary, sign a record on the
   workstation, verify on the host (the exact commands are in the "Attestation
   split" section below).

fmt and clippy were not run at all, so a formatting or lint repair may follow.
The release build and the workspace compiled with zero warnings, which does not
by itself prove `cargo fmt --check` or `-D warnings` clippy.

## Signing-key lifecycle for the attestation split

The verifier's `--attest` mode signs a provenance record with an Ed25519 key; the
deploy host's default mode verifies that signature with the matching public key.
**No real key exists yet.** The fixture tests generate an ephemeral keypair in a
temporary directory with `cryptography` and never touch a real key, and this task
created none.

**Who creates it.** The PM (the release owner) generates the keypair once, on the
PM workstation, with a tool that writes standard PEM:

```
openssl genpkey -algorithm ED25519 -out release-provenance.pem      # PKCS#8 PEM
openssl pkey -in release-provenance.pem -pubout -out release-provenance.pub.pem  # SPKI PEM
```

`verify_rust_artifact.py` loads exactly those two encodings
(`load_pem_private_key`, `load_pem_public_key`) and refuses a non-Ed25519 key.

**Where the private key lives.** On the PM workstation only, outside the
repository, owned by the PM's user, mode `0600`. Suggested path:
`~/.config/verdaxis/artifact-provenance/release-provenance.pem`, with the public
key beside it. The repository never carries private-key material. `--attest`
runs only on the workstation; the private key never leaves it, and the deploy
host never receives it.

**How the host pins the public key.** The public key is not a secret, but which
key is accepted must be pinned to exact bytes. Two supported channels, usable
together:

- **Repository bytes (preferred).** Commit the public key at a reviewed path,
  `deploy/rust/artifact-provenance.pub.pem`. The deploy transaction already
  attests the approved release tree, so the pinned key travels with the reviewed
  commit and its digest is recorded in the artifact metadata (`revision_allowlist`
  is the pattern; a sibling `provenance_key` block can hold `path` + `sha256`).
- **Operator install.** The host may keep `/etc/verdaxis/artifact-provenance.pub.pem`
  (root-owned, `0644`) provisioned out of band, for a key the operator rotates
  without a release.

Pinning both closes the substitution gap: the transaction should require the host
file's SHA-256 to equal the reviewed digest from the approved tree, then pass
that path to `--provenance-public-key`. Today the CLI verifies the signature with
the path it is given; **the digest pin of that path is Task 12b's integration
work**, not built here.

**On key loss.** Existing records and their detached signatures stay verifiable
with the public key, so already-published artifacts and already-carried records
remain trustworthy. Loss only prevents signing a *new* release. Recovery:
generate a new keypair, distribute and pin the new public key, and re-run
`--attest` for the release that needs a record. Because each record binds the
GitHub attestation and the archive/binary digests, a lost private key does not
weaken past provenance.

**Rotation.** Rotate on compromise, on staff change, or on a fixed schedule
(suggested annually). Procedure:

1. Generate the new keypair on the workstation (commands above).
2. Add the new public key at the reviewed path and/or install it on the host,
   keeping the previous public key(s) in a short allowlist so a release signed
   before the switch still verifies during the window.
3. Switch the pinned key at one reviewed commit; record the change in the plan's
   resume point and the progress file.
4. After the window, retire the old public key and destroy the old private key.

On suspected compromise, skip the overlap: stop pinning the old public key,
re-sign any pending release with the new key, and treat any record signed by the
old key after the compromise time as untrusted.

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

Gates 1-3 (artifact fixture tests, offline workspace, release build) ran and
passed. Gates 4-6 (glibc smoke check, fmt/clippy, full unit suite) and the Linux
package/verify round trip were **not run, host paused**. Nothing there is claimed
as passed.
