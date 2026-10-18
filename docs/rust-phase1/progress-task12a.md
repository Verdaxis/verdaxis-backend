# Task 12a — verify artifacts and gate CI publication

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`. **All Task 12a gates pass.**

## Execution log (17 Oct)

The host paused twice (Phase 0 rehearsals). The six gates below all ran once the
host was free between 01:20 and 02:05 SGT, each a single `verdaxis-prod-test`
call logged with a `CALL_EXIT=` marker.

| # | Gate | Command | Result |
| --- | --- | --- | --- |
| 1 | Artifact fixture tests | `venv/bin/python -m pytest tests/unit/test_rust_artifact.py -q` | **34 passed** (first run 21 failed on a test-helper defect, fixed) |
| 2 | Offline Rust workspace | `RELEASE_SHA=<HEAD> SQLX_OFFLINE=true cargo test --workspace` | **153 passed**, 0 failed, 0 warnings |
| 3 | `cargo fmt --check` | `cargo fmt --manifest-path rs/Cargo.toml --all -- --check` | **pass** (one formatting fix in `artifact.rs`) |
| 4 | Strict `clippy` | `cargo clippy … --all-targets -- -D warnings` | **pass**, finished clean |
| 5 | Release build | `cargo build --locked --release -p verdaxis-read` | **built** in 3m25s; sha256 `3ae71e3e7649236d0f7bedaaad4c10ade4e5df79d1a5906e6fdf2e77b5cf32bb` |
| 6 | glibc smoke check | `objdump -T … \| verify_rust_artifact.py --glibc-check --max 2.35` | **pass**: `glibc_max=2.34` ≤ 2.35 |
| 7 | Full Python unit suite | documented `ENVIRONMENT=test … pytest tests/unit -q` | **2080 passed**, 0 failed, 504 warnings |
| 8 | Linux package/verify round trip | see below | **pass** |

Every binary digest printed above embeds the release SHA via `build.rs`, so it
differs between gates that ran at different HEADs; that is expected, not drift.

All six Rust artifact unit tests ran inside gate 2:
`runtime::artifact::tests::{wrong_binary_refuses_new_release,
matching_identity_is_accepted, malformed_identity_is_refused,
version_document_reports_the_compiled_identity,
enumerated_allowlist_matches_the_reviewed_file, compiled_identity_matches_this_build}`.

The plan's Step 2 "expected red" was not captured: the code existed before the
brief, so capturing a red would have meant reverting working code. Said plainly:
**no red was captured.**

Logs: `work/rust-phase1/t12a-{artifact-tests,artifact-tests2,workspace,fmt,fmt2,clippy,glibc2,unit,roundtrip}.log`.

### The two failures and their fixes

1. **Gate 1, first run: 21 of 34 failed on two test-only defects.** The fixture
   helpers passed raw `bytes` to `tarfile.addfile` (which wants a file-like
   object), and `Fixture.assert_refused` called `self.assertFalse` on a
   non-TestCase helper. Fixed in `3a6f5d18`; no verifier behavior changed.
2. **Gate 3, first run: one formatting diff** in `artifact.rs`'s drift test.
   Fixed in the same commit as this document's update.

### Gate 8 — the Linux package/verify round trip

A temporary untracked runner (`t12a_roundtrip.py`, deleted after the run and
never committed) ran on the host against the real release binary:

- `--package` built `release.tar`, sha256 `ab76dcadf1c58419bb49efcb192824e24b7a310e4563bc144684fd3a364694ac`.
- Host-mode verify with a **throwaway** Ed25519 key (no real key exists)
  returned 0 and extracted `SHA256SUMS`, `artifact-metadata.json`, `data/`
  (14 files) and `verdaxis-read`.
- The extracted binary digest matched the metadata, and its mode was `0755`.
- A record with a tampered `archive_sha256` was refused (exit 1) and extracted
  nothing.

This proves the packaging, provenance-verification and extraction paths on Linux.
It is a **mechanics** test: the attestation step is simulated because a real
GitHub attestation needs a real CI push, so the workstation split itself is not
exercised here — only the host half is.

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
Caddy 2.11.7 `linux_amd64` is downloaded and checked with `sha256sum` against
`727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b`, the SHA-256 GitHub
records for that release asset. (The first pin was the SHA-512 from the release's
`caddy_2.11.7_checksums.txt`, which `sha256sum -c` cannot check; the first GitHub run
of the contract job, 17 Oct, failed on it.)

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

## Gates still owed

None of the brief's gates. Every gate listed in the execution log passed. What
remains is outside this task: the real **CI** run (first push to `staging`/`prod`)
and the **workstation** half of the attestation split, both of which need a real
GitHub attestation. See "What only the first real CI run can prove".
