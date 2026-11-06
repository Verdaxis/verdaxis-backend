# M release and window tooling — 5 November 2026

## Scope

Local implementation from `b36279c328a6a193093965f71e8e1a9876618d71` in
`feat/rust-phase-10-m`. No push, SSH, host inspection or live deployment. No
application schema change or Python deletion. Staging combines W2/W3;
production combines W2/W4. Soak and W5 remain separate.

The first commit is `8476cadd5`, `docs/phase10/window-gaps.md`. Its baseline
findings remain source evidence, not installed acceptance. Recovery is retired;
the final monitor group verifies its mask/absence and the restart/watchdog
replacement, not an unavailable recovery CLI.

## Scriptable interfaces

### M identity and deployment

The M startup lists admit exactly `sqlx_1`, with compiled/file drift checks.
The immutable checkpoint policy adds exactly the final offboarding adoption and
`sqlx_1` no-change transition. Native resume revalidates baseline/checksum,
retained Alembic audit head and catalog without rewriting the adoption row.
Historical schema capabilities remain separate from startup admission.

From the separately authorized fixed, clean environment checkout:

```bash
MIGRATION_EXPECTED_CURRENT_REVISION=ob_20261031_offboarding \
MIGRATION_TARGET_REVISION=sqlx_1 ./scripts/deploy.sh --dry-run
```

Use the returned full SHA as both `APPROVED_RELEASE_SHA` and
`MIGRATION_APPROVED_SOURCE_SHA` for the real run. Pass the same exact
current/target, signed artifact/archive inputs, and private
`DEPLOY_ACCEPTANCE_RECORD` / `DEPLOY_ACCEPTANCE_SIGNATURE` output paths.
A repeat approved release uses the explicit `sqlx_1 -> sqlx_1` pair. No alias or
unlisted transition is accepted.

The transaction retains canonical source/branch, immutable dry-run, artifact
provenance, app/migrator identity, budget, unit, sudo and both-lock/state gates.
It stops/disables Python, revokes/drains native cache authority, runs adoption
and ACL convergence, publishes matching identity, binds fresh cache authority,
installs verified bytes and restarts only Rust. It does not install packages,
run Alembic or start Python. Failures keep startup state blocked.

Task 31's trusted external producer signs one JSON object with Ed25519 using the
reviewed provenance key. The record has exactly these fields:

```json
{
  "auth": true,
  "binary_sha256": "<verified executable SHA-256>",
  "deploy_nonce": "<fresh transaction nonce>",
  "environment": "staging",
  "expected_current": "ob_20261031_offboarding",
  "monitor": true,
  "passive_harm": true,
  "release_sha": "<full approved SHA>",
  "session": true,
  "target": "sqlx_1"
}
```

The transaction prints non-secret `DEPLOY_ACCEPTANCE_NONCE` and
`DEPLOY_ACCEPTANCE_BINARY_SHA256` after exact loopback/public readiness, then
waits within its configured health retry bound. The signer performs the checks;
setting booleans locally is not evidence. A missing, stale, altered or mismatched
record refuses state release. Private signing keys never belong on the host.

Disposable-only `DEPLOY_KILL_POINT=before:NAME|after:NAME` requires
`VERDAXIS_DISPOSABLE_DEPLOY=1` and a noncanonical checkout. Names are
`cache-revoke`, `source`, `budget`, `release`, `monitor`, `policy`, `revision`,
`runtime`, `cache-bind`, `binary`, `authorization`, `clear`.
`DEPLOY_PUBLICATION_LOG` contains only boundary/environment/SHA records.

### Caddy preparation

```bash
python -m scripts.rust_phase10_window_caddy \
  --site "$SAVED_SITE" --environment "$ENVIRONMENT" \
  --output-dir "$NEW_PRIVATE_DIRECTORY_OUTSIDE_SOURCE" \
  --caddy-bin "$TRUSTED_ABSOLUTE_CADDY" --caddy-sha256 "$CADDY_DIGEST"
```

This off-host tool reuses the reviewed candidate helpers and route snippets.
It preserves live abilities/offboarding, includes all CRUD groups in production,
routes the catch-all to Rust and leaves no active Python dial. It emits
`candidate.caddy`, byte-exact `rollback.caddy`, `probes.json`, and a digest-bound
validation manifest. Unknown route rules refuse instead of being discarded.
The exact candidate is adapted; typed JSON file writers are remapped privately
for validation. This is not installed log-permission proof, a filesystem sandbox,
a reload or live ownership acceptance.

### W2 ordered handover

```bash
sudo -n ./scripts/rust_w2_handover.sh \
  --action install --environment "$ENVIRONMENT" --source-ref "$SHA" \
  --binary-sha256 "$VERIFIED_BINARY_DIGEST" --evidence "$PRIVATE_EVIDENCE" \
  --host-scope environment
```

Actions are `install`, `advance`, `accept`, `rollback`, `status`. Group actions
also take `--group`, and post-install actions require `--timer-inventory`.
Use explicit `--host-scope host` only with independent bound ownership evidence
for the host-global monitor binary, identities, ACLs and units.

Installation snapshots old unit/drop-in/launcher bytes, stops timers, drains
work and verifies direct-launcher fences before immutable unit replacement.
Activation remains separate. Groups are news; auth/prune; reminders; onboarding;
demo; local-monitor; external-monitor/recovery-retirement. Each run records a
new successful systemd InvocationID; fresh private acceptance must bind those
runs and no-duplicate/market checks. Alert rollback stops native work and
reconciles both journals before old ABI and permissions can return. Shared
claims and journals are not rewound. Timer-start failures can resume without
rerunning the accepted job or repeating a completed alert ownership release.

**Timer count remains unresolved:** the active source job allowlist is
production **6**, staging **4**. The requested **7/5** is not the active manifest.
The retained staging reminder files are marked gone; staging has no onboarding
timer. Global monitor timers have separate host ownership. Status reports the
mismatch. No timer was invented or silently enabled to satisfy a count.

### Protected pre-M bundle

Before capture, install the reviewed `deploy/scripts/provision-bundle-reader.sh`
as root-owned mode 0755 at
`/usr/local/libexec/verdaxis/provision-bundle-reader.sh`, under root-owned parents
that are not writable by group or other. It requires `/usr/bin/python3` (standard
library only), `psql`, `runuser` and local PostgreSQL peer administration as
`postgres`. Do not run a checkout-writable helper through sudo. Authorize only
the installed helper's exact `--environment production|staging --check` command
for the deploy operator; provisioning itself stays a separate root operation.

Run for each independently approved environment before pre-M capture:

```bash
sudo -n /usr/local/libexec/verdaxis/provision-bundle-reader.sh --environment "$ENVIRONMENT"
sudo -n /usr/local/libexec/verdaxis/provision-bundle-reader.sh --environment "$ENVIRONMENT" --check
PRIVATE_READER_SERVICE_FILE="/etc/verdaxis-bundle-readers/$ENVIRONMENT-bundle-reader.pg_service.conf"
```

The helper creates only `verdaxis_bundle_reader[_staging]` as a plain LOGIN
with NOINHERIT and no memberships or grants. A host-generated password is
published atomically in that root-owned 0600 `[pre_m_reader]` service file;
only a client-generated SCRAM verifier reaches PostgreSQL. No password is
printed or supplied in arguments or environment. Reruns preserve the service
bytes and role password. A retry after file publication but before role creation
uses the saved password. A role without its service, a wrong endpoint, unsafe
role attributes, memberships or a nonprivate path refuses; investigate rather
than rotate or broaden authority automatically. Keep the service separate from
runtime credentials and evidence. `--check` authenticates the exact role on
`127.0.0.1:5432` and the canonical database without creating a role or granting.
The deploy runs that installed check before state/source mutation or any stop.

Before capture, also run the independently approved existing ACL convergence
with `bundle_reader_role=verdaxis_bundle_reader[_staging]` for this environment.
Provisioning intentionally grants nothing; convergence owns the retained
`public.alembic_version` SELECT, schema USAGE and database CONNECT policy.
Verify the capture with this private service; do not use runtime or migrator
credentials as the capture reader.

Run a trusted root-owned copy of `deploy/scripts/pre-m-bundle.sh` outside the
checkout, with its reviewed `verify-artifact.sh` and catalog SQL siblings.
Commands are `capture`, `verify`, `restore`, `accept`:

```bash
pre-m-bundle.sh capture --environment "$ENVIRONMENT" --release-sha "$OLD_SHA" \
  --bundle "$PRIVATE_PARENT/pre-M-$ENVIRONMENT-$OLD_SHA" \
  --inventory "$APPROVED_PRIVATE_INVENTORY" --manifest-sha256 "$INVENTORY_DIGEST" \
  --reader-service-file "$PRIVATE_READER_SERVICE_FILE" \
  --acceptance "$SIGNED_CLOSURE" --acceptance-signature "$CLOSURE_SIGNATURE" \
  --acceptance-public-key "$APPROVED_PUBLIC_KEY"
```

For later commands, `--manifest-sha256` is the independently approved emitted
bundle manifest digest, not the inventory digest. The inventory binds the exact
checkout and branch, old full SHA, workflow ref, canonical database/roles, actual
Rust unit, unit property digests, every runtime/credential/package input,
Caddy/guard/monitor bytes and native dependency closure. Capture retains the
original signed artifact, Git source objects, file digests/sizes/owners/modes and
protected payload copies. Public evidence never includes secret contents.

The key digest is pinned in the independently approved inventory. Signed
closure evidence binds environment, old SHA and the recomputed concrete closure
digest, with `runtime_dependencies`, `credential_sources`, `unit_inputs` equal
to `PASS`. Restore rechecks the read-only catalog and old compiled pin, acquires
both stable locks with blocked state before the restart lock, and restores
matching source/runtime/units/guards, monitor copy/identity, Caddy and the actual
old executable. Every private credential input, including migration URLs needed
by the previous release, returns with its captured bytes, owner and mode before
either backend starts. The forward M deploy still projects a Rust runtime with
no migration authority. Restore enables the retained Python backend, restarts
both backends, checks their exact readiness, then reloads saved Caddy bytes and
checks public readiness and the Python route. It leaves state present for acceptance.

`accept` requires signed environment/SHA/manifest/restore-nonce binding,
current Rust InvocationID, exact-ready identity, actual `/proc/PID/exe` digest,
exact installed bytes/metadata, and health/backup monitor invocation results.
Auth/session/monitor/passive/old-pin/freeze evidence must pass. A self-contained
accepted journal makes previously active timer restoration resumable after
state release; missing auxiliary files do not erase the saved timer list.
The adoption row and Alembic audit row are retained. No migration writer runs.

**Supported subset:** all publication targets must share the deploy-state
filesystem for atomic rename. Canonical literal quoted/unquoted runtime URL
forms are supported; escaped/dynamic credentials, Caddy imports, package
symlinks and unresolved native dependencies refuse. Linked Git worktrees,
redirected metadata and local Git configuration outside the inert recovery
allowlist also refuse. The trusted operator must quiesce other source writers;
root recovery does not grant an untrusted deploy account concurrent mutation
authority. Non-serving consumers must already be inactive/drained. The captured
Python unit/package and credential inputs must support startup. Task 31 must
provide approved closure evidence and
prove actual old-release startup on the intended installed layout. These
restrictions can require a pre-M compatibility release; local fixture success
is not full host compatibility or installed recovery acceptance.

## Verification record

Verified locally so far:

- Rust formatting and both all-target clippy modes passed under Rust 1.95.0,
  `SQLX_OFFLINE=true`, the shared target flock and `CARGO_INCREMENTAL=0`.
- Actual disposable PostGIS migration target: **14 passed**, no ignored cases;
  includes checksums, history-table parity, adoption/resume, both-lock ownership,
  catalog drift refusal and exact target policy. The container and volume were
  removed. Alembic was used only to build that disposable pre-M test template.
- Targeted native runtime: **32 passed**; environment projection: **13 passed**;
  producer: **21 passed**; stream runtime: **8 passed, 2 database cases ignored**;
  artifact identity/drift: **6 passed**.
- Real local Caddy contracts: **23 passed**, both environments.
- Retained monitor regressions: **291 passed, 1 skipped**.
- W2 portable contracts: **6 passed**. Static unit installer: **32 passed,
  2 disposable identity cases skipped**.
- Rust-only deploy Linux shell fixture: **88 passed**, including all **65**
  mapped standing gates, 24 before/after publication cells, genuine Ed25519
  signing/refusals, staged digest changes, cache fences and exact dry-run pairs.
  Git/filesystem operations are real; database, systemd, curl and artifact
  verification are fixture adapters. This is not real systemd acceptance.
- Protected bundle Linux file-transaction fixture: capture/verify/tamper,
  restore/freeze and missing acceptance passed; **54 killed restore attempts**
  covered before/after all 27 observed publications with successful replay.
  Signed source/mode refusals, accepted-journal interruption, timer-start
  interruption with missing auxiliary files, real Ed25519 acceptance and actual
  native fixture executable PID digest passed. This is a tiny native fixture,
  not the actual previous signed Verdaxis release or a real systemd service.

Initial broad feature-enabled library tests included database-only cases without
their required variables and failed; they are not claimed as a passing suite.
Targeted non-database tests and the actual migration database target were then
run explicitly. Native startup fixtures with previous release pins were updated
to the selected M pin after their expected failures; historical capability
refusals remain tested.

The full Phase 10 Python gate covers 1,469 collected cases. Its first final
run had one expected stale shell-fixture ledger digest and 1,468 passing cases;
the final refreshed-corpus rerun passed **all 1,469 cases**, with no skips.
ShellCheck 0.11.0 passed
for every path in the active list, including all new shell entry points.
The CI-pinned ShellCheck 0.10.0 binary was not run locally. RED/GREEN component runs are recorded in the lane history;
a passing source-string contract is not a substitute for native transaction or
real systemd evidence. Initial Linux transaction runs exposed temporary-file
resume and enable-order failures. After the changes, the full native Linux
transaction rerun passed all 88 cases; earlier failed runs are not counted as
successes. Local logs are retained under `work/phase10-m/`.

Local structure refresh regenerated summaries but bootstrap failed with
`ModuleNotFoundError: render_bootstrap`. Generated bytes were archived privately
under `work/phase10-m/codesight-refresh/` and excluded from commits.

## Task 31 resume boundary

Task 31 must compose these entry points in a disposable Linux/systemd target,
provide signed old/new release artifacts and approved inventory, run actual
old-release startup, auth/session/passive probes, every unit/timer and alert
identity/ACL case, route ownership probes and every interruption boundary.
It must resolve the active timer scope and prove previously active schedules
resume only after acceptance. Fixture success does not grant host authority.
W5 deletion and installed-host acceptance remain separate.
