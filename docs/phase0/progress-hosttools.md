# Phase 0 host-tool sign-off fixes — partial checkpoint

## Interface changes from review fixes

Flags, file names, manifest fields and secret formats are **unchanged** by the
R1–R3 fixes. The M2 appendix commands already listed below remain exactly valid.
Three behavior details changed:

1. Tool selection: passing another tool's manifest now exits 1 with
   `PHASE0_TOOL_MISMATCH` before any validation-only return or connection.
2. Validation-only output: bootstrap/copy without `--apply` now print
   `authority validated only; no connection opened`, never "completed".
3. Failure output: registered `PHASE0_*` reason codes on stderr replace the
   former generic one-line message. Detail stays in the private on-host report.
4. Verify never reports validation-only success: it publishes its report or
   exits nonzero.

## Current production interface — review fixes R1–R3

**Interface changes for the M2 host appendix are listed under "Interface changes
from review fixes" below. Do not use the older wording for tool selection or
validation-only output.**

R1 (blocker, fixed): each CLI passes its own literal tool name into the shared
runner; a manifest with a different `tool` value is refused before any
validation-only return, receipt lookup or connection. Cross-tool manifests now
exit 1 with `PHASE0_TOOL_MISMATCH`. Validation-only success remains allowed only
for the matching bootstrap or copy CLI. Verify has no validation-only success:
it either publishes its new report or returns nonzero.

R3 (minor, fixed): validation-only runs now print `... authority validated only;
no connection opened ...` instead of "completed". `--apply` success prints the
completed/published message.

R2 (major): refusal reason codes. Every production gate raises a registered code
from `scripts/phase0/refusal.py`; CLIs print only the code on stderr:

| Code | Gate |
|---|---|
| `PHASE0_TOOL_MISMATCH` | CLI tool does not match the manifest `tool` |
| `PHASE0_ENVIRONMENT` | not root, or `ENVIRONMENT` is not `production` |
| `PHASE0_MANIFEST` | manifest field set or per-tool field is wrong |
| `PHASE0_AUTHORITY` | manifest differs from authority; host, release, revision, approval, or frozen source catalog |
| `PHASE0_INSTALLED_BYTES` | release tree ownership/digest differs |
| `PHASE0_DB_PAIR` | approved source/target pair differs |
| `PHASE0_RECEIPT` | bootstrap receipt path, digest or authority binding differs |
| `PHASE0_REPORT_PATH` | evidence path is not private, or already exists |
| `PHASE0_SECRET` | secret file format or endpoint differs |
| `PHASE0_DB_IDENTITY` | connected database/role/OID/endpoint/cluster differs |
| `PHASE0_SOURCE_SESSIONS` | another source session exists |
| `PHASE0_TARGET_FENCE` | target ACL, owner, CONNECT grantees or extra target session differs |
| `PHASE0_SOURCE_STATE` | frozen source data, catalog or sequence state changed |
| `PHASE0_REPORT` | publication failed |
| `PHASE0_COPY` | unclassified copy failure (detail stays in the private report) |

Codes carry no credentials, SQL values, paths or host names. Copy writes its
detailed private failure evidence separately; that file must stay on-host.

## Earlier end-to-end synthetic Linux proof (superseded counts)

**The production bootstrap → copy → verify CLI chain passes. Independent security
review and operator approval remain required before any host activation.**

Implementation: `f1ee45c2` (shared production authority and fences), `52b16f71`
(temporary synthetic diagnostic), `78f8343b` (root-controlled test installation),
`16a0555c` (copy boundary import), `35e19f6d` (expanded refusals/recovery; removes
bootstrap diagnostic retry). Real CLI chain: **15 passed**, 198.59 seconds,
including four fixture tests and ten bootstrap refusal cases. The success case
also checks copy/verify wrong host/release/bytes/receipt, both sides' wrong
OID/role/name/host/port/cluster, extra target session, changed ACL, copy replay,
and independent verification after deletion of the copy receipt. Refusals are
exit 1, not argparse exit 2. No authority check is mocked. Root-owned evidence
and distinct production-mode credentials are real inside the disposable runtime.

Completed regression gates at `35e19f6d`:

- Explicit production Linux acceptance: **15 passed**, 1 warning, 198.59 seconds.
- Full remote Phase 0 PostgreSQL suite: **177 passed**, 62 warnings, 713.31
  seconds; no failed/skipped cases. This standard suite includes synthetic
  bootstrap/copy/verify and the seven database-fence cases; the production CLI
  acceptance is the separate explicit Linux invocation above.
- Full unit suite: **2211 passed, 19 failed**, 514 warnings, 124.64 seconds;
  all failures are the known macOS `test_deploy_health.py` cases.
- Remote logs: `work/hosttools-production-chain-final.log`,
  `work/hosttools-production-postgres.log`; local log:
  `work/hosttools-production-unit.log`. Both remote runners completed and
  released the lane lock; no pytest/runner processes remained. Their stuck local
  return wrappers were stopped only after completed results were retrieved.
  All test runs finished; owned containers were removed with volumes.

### Operator authority and installation

The operator must install `/root/verdaxis-phase0/authority.json`, root-owned 0600
inside a root-owned 0700 directory with root-controlled ancestors. This is the
independent approval anchor; commands cannot select or replace its path.
It must come from reviewed release archive bytes and trusted operator database
observations, **not a blind recapture from arbitrary target data**. The tools
compare the installed source tree digest with this authority, not ambient Git.
Install source directories/files root-owned and not group/world-writable.
Interpreter/package installation remains an operator-attested prerequisite.

The exact authority JSON fields are:

- `mode`: `production`; `approved`: JSON `true`; `hostname`: exact OS nodename.
- `release_sha`: approved full 40-lowercase-hex release SHA, also required as
  `RELEASE_SHA`; `tool_tree_sha256`: `host_authority.tree_digest` of that reviewed
  installed archive (scripts/app/alembic/deploy plus enumerated root inputs).
- `source_sha`: `c08266bd6969143812dec7c5033338428cd5f159`.
- `source`, `target`: each has exactly `host`, `port`, `database`, `oid`, `role`,
  `server_host`, `server_port`, `cluster_system_identifier`. Client host is
  `127.0.0.1`; server host is the exact loopback address/CIDR reported by PostgreSQL.
  Source is `verdaxis` / `verdaxis_backup`; target is `verdaxis_prod` /
  `verdaxis_migrator`. Distinct positive OIDs, same independently attested cluster
  and endpoint; cluster identifier is a positive decimal string. No new roles.
- `source_catalog_sha256`: `manifest.digest(load_manifest()['source_catalog'])`.
- `frozen_source`: complete `fingerprint_database` result from the frozen source
  (table/column hashes and sequence states), obtained under the post-stop gate.
- `target_datacl`: exact non-null `pg_database.datacl::text`, independently
  recorded after PUBLIC CONNECT is revoked. CONNECT may name only the migrator;
  no grant option. Target owner must be the migrator. No HBA changes.
- `bootstrap_receipt`: absolute new private report path for this target build.

Each per-tool manifest repeats all authority fields exactly and adds exactly:
`tool` (`bootstrap`, `copy`, or `verify`), `report` (absolute new output path),
and `bootstrap_receipt_sha256` (JSON null for bootstrap; exact SHA-256 of the
successful bootstrap report bytes for copy/verify). Bootstrap `report` must equal
`bootstrap_receipt`. Do not reuse one manifest for different tools.

### Exact production commands and secret formats

Run from the approved root-controlled installation, with production Settings
and the approved full release SHA. Distinct app/migrator production URLs must
validate; source routing is always its separate backup secret, not Settings.
Each tool gets its own separate source and target root-owned 0600 secret files
inside private root-owned directories. Each contains exactly one line:

```text
DATABASE_URL=postgresql+asyncpg://<exact-role>:<password>@127.0.0.1:<port>/<exact-database>
```

No URL query parameters; explicit nonempty passwords; source and target passwords
must differ. URLs never go in argv or public error output. All three tools use
the same `--source-environment-file` / `--target-environment-file` spelling.
Bootstrap's historical synthetic path retains its old one-line
`MIGRATOR_DATABASE_URL` format; do not use that format for production.

```sh
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.bootstrap \
  --manifest /root/verdaxis-phase0/bootstrap.json \
  --source-environment-file /root/verdaxis-phase0/bootstrap-source.env \
  --target-environment-file /root/verdaxis-phase0/bootstrap-target.env --apply
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.copy \
  --manifest /root/verdaxis-phase0/copy.json \
  --source-environment-file /root/verdaxis-phase0/copy-source.env \
  --target-environment-file /root/verdaxis-phase0/copy-target.env \
  --source-sha "$OLD_SHA" --target-sha "$RELEASE_SHA" --apply
/opt/verdaxis-phase0/venv/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/verify.json \
  --source-environment-file /root/verdaxis-phase0/verify-source.env \
  --target-environment-file /root/verdaxis-phase0/verify-target.env \
  --report /root/verdaxis-phase0/verified.json
```

`OLD_SHA` is the exact pinned source SHA above. Verify's `--report` must equal
its manifest's report path. Bootstrap/copy without `--apply` validate authority
only; they do not open database connections or authorize changes. Verify is
always read-only. Unknown/abbreviated arguments refuse.

Outputs, always new/private:

- Bootstrap: authority digest, target identity, exact migration-created reference
  rows and sequence baseline. Captured within successful DDL transaction;
  published only after commit and final source/fence checks.
- Copy: existing copy receipt (manifest digest, source snapshot, target OID,
  table/column/reference verification, numeric totals, sequence values), plus
  durable `.attempt.json` and failure evidence when applicable. No replay.
- Verify: manifest digest, frozen source fingerprint, verification results,
  sequences and both identities. Requires bootstrap receipt, not copy receipt.
  Receipt-loss recovery invokes verify with a new report, never copy again.

Entry/precommit/end observations check target ACL and every other session;
source remains read-only repeatable-read, with fresh post-operation transaction
checks before publishing. PostgreSQL CONNECT ACLs do not evict prior sessions
or constrain superusers: the operator post-stop gate remains mandatory. Tools
never change grants/settings or kill sessions. After runtime CONNECT is granted,
these pre-acceptance tools deliberately refuse; stop and restore the approved
fenced operating boundary before any later independent verification.

Boundary coverage. Boundaries are: entry = first observation before any
mutation; precommit = last in-transaction fence observation before commit or
publish; final = fresh post-operation observation before publication. Every new
test asserts the named refusal code, the committed or rolled-back database state
and the absence of the success report. No authority gate was weakened.

| Fault class | Tool | entry | precommit | final |
| --- | --- | --- | --- | --- |
| source change | bootstrap | `test_bootstrap_late_source_change_refuses_at_entry` | `test_bootstrap_in_flight_source_sequence_change_refuses_before_commit` | `test_bootstrap_late_source_row_change_refuses_at_final_boundary` |
| source change | copy | `test_late_source_change_refuses[copy]` | `test_copy_in_flight_source_sequence_change_refuses_before_commit` | `test_copy_late_source_row_change_refuses_at_final_boundary` |
| source change | verify | `test_late_source_change_refuses[verify]` | n/a: the verifier's own comparison raises a plain `ValueError` before the coded fence | `test_verify_late_source_row_change_refuses_at_final_boundary` |
| late target session | bootstrap | `test_late_target_session_refuses[bootstrap]` | `test_bootstrap_in_flight_target_session_refuses_before_commit` | n/a: a live session is refused at the in-transaction fence |
| late target session | copy | `test_late_target_session_refuses[copy]` | `test_copy_in_flight_target_session_refuses_before_commit` | n/a: a live session is refused at the in-transaction fence |
| late target session | verify | `test_late_target_session_refuses[verify]` | `test_verify_in_flight_target_session_refuses` | n/a: a live session is refused at the in-transaction fence |
| late target ACL change | bootstrap | `test_late_target_acl_change_refuses[bootstrap]` | `test_bootstrap_in_flight_target_acl_change_refuses_before_commit` | n/a: the READ COMMITTED target sees a committed ACL change at the precommit fence |
| late target ACL change | copy | `test_late_target_acl_change_refuses[copy]` | `test_copy_in_flight_target_acl_change_refuses_before_commit` | n/a: the READ COMMITTED target sees a committed ACL change at the precommit fence |
| late target ACL change | verify | `test_late_target_acl_change_refuses[verify]` | n/a: the repeatable-read snapshot hides a committed ACL change from the in-transaction fence | `test_verify_late_target_acl_change_refuses_at_final_boundary` |
| receipt publication failure | bootstrap | `test_pre_existing_report_destination_refuses[bootstrap]` | n/a: publication follows the precommit fence | `tests/unit/test_phase0_report_publication.py` |
| receipt publication failure | copy | `test_pre_existing_report_destination_refuses[copy]` | n/a: publication follows the precommit fence | `tests/unit/test_phase0_report_publication.py` |
| receipt publication failure | verify | `test_pre_existing_report_destination_refuses[verify]` | n/a: publication follows the precommit fence | `tests/unit/test_phase0_report_publication.py` |

A row change is invisible to the source repeatable-read snapshot at an
in-transaction fence, so the bootstrap/copy precommit source cells use a
sequence change (sequence state is not MVCC-snapshot bound); the row-change
proof is the final cell. Entry-boundary claim faults are covered by
`test_bootstrap_claim_fault_refuses_before_ddl` (all `CLAIM_FAULTS`, including
`PHASE0_SOURCE_STATE`) and
`test_endpoint_claim_fault_refuses_before_connection_for_copy_and_verify`
(`PHASE0_DB_PAIR` for copy and verify server-address/port).

Residual scope, not needed for B1/B2: a verify in-flight source change is
structurally uncoded, because the verifier's own comparison raises a plain
`ValueError` first, so the coded `PHASE0_SOURCE_STATE` for verify appears only at
the final fresh observation. A late CLI-level publication failure, after the
final observation, is covered only at unit level by
`tests/unit/test_phase0_report_publication.py`; `PHASE0_REPORT` is registered but
never raised. Operator installation/provenance review remains a separate
signature gate. Do not infer live permission from tests.

## Historical checkpoint 8 — role decision resolved; production CLI red test committed

**B1/B2 remain incomplete. The integrated production chain is not implemented.**

PM confirmed: `verdaxis_migrator` is the target tool role. Create no additional
production roles. Before ACL acceptance only that role has target CONNECT;
source reads use existing read-only `verdaxis_backup` authority, never a writer.
This resolves checkpoint 7's question. No further role decision is required.
No HBA changes, source grant/settings changes or session termination are allowed.

`434f34ac` adds the first actual production CLI chain acceptance test and shared
private-input builder. It runs only inside the pinned root-owned Linux runtime,
against the production-named synthetic fixture. Commands invoke the real
`python -m scripts.phase0.bootstrap`, `copy`, and `verify` subprocesses. The
runtime receives the archived candidate's full SHA in `/release.json`; that file
is test input, not yet an implemented production release attestation mechanism.

Completed remote red command:

```sh
verdaxis-prod-test "$PWD" 'venv/bin/python -m tests.phase0.linux_database'
```

Result: **1 failed, 14 passed**, 61.75 seconds. Bootstrap rejected the proposed
`--source-environment-file` argument; it must read and bind the frozen source
before target DDL, not rely on an unobserved manifest claim. Ten wrong-authority
cases currently pass through this same parser refusal. **They do not yet prove
specific authority enforcement.** Four existing scratch tests remain green.
Remote log: `work/hosttools-production-chain-red.log`. Both owned containers
were removed with `-v`; no test remains running. No full-suite rerun was made
at this red-test checkpoint; previous green counts are historical, not a claim
that the new explicit Linux acceptance path passes.

### Exact resume point after checkpoint 8

Implement against `tests/phase0/production_cli_acceptance.py` and
`production_cli_case.py` (explicitly selected by `linux_database_inside.py`).
The input fields and `DATABASE_URL` secret format there are proposed test
contracts, **not final operator interfaces**. No production bypass is enabled.

1. Add the production authority loader: root-private manifest/secret reads,
   exact per-tool fields, approved installed archive/release bytes, host and
   independently trusted cluster/endpoint/OIDs, literal revisions, frozen source
   catalog/fingerprint, exact target ACL. Preserve synthetic behavior. Do not
   treat `/release.json` or a caller digest as self-authenticating authority.
2. Extend bootstrap's strict parser with source secret input for production;
   retain existing synthetic command compatibility. Bind its supplied Alembic
   connection to validated production identity, run source read-only checks,
   and verify source quiescence/target ACL at entry, precommit and exit. Capture
   baseline references/sequences inside successful bootstrap, publish only after
   commit to a new root-private report. No arbitrary baseline recapture.
3. Bind copy to the exact trusted bootstrap receipt; retain all import gates,
   source read-only snapshot, role split and durable attempt/no-replay behavior.
   Integrate the fence observations at all required boundaries. Bind verify to
   the same source/bootstrap authority, independently read-only, including
   post-commit receipt-loss recovery.
4. Make the successful chain pass first, then require wrong-identity tests to
   reach the intended authority gate (not argument parsing). Extend wrong
   identities to all three tools, ACL/session changes at boundaries, altered
   bootstrap evidence, secret mismatch, source/catalog drift and report failures.
5. Run the explicit Linux acceptance and both required full suites through the
   approved environment rules. Record final interfaces only after the chain
   passes; update architecture. No further environment build or permission ask
   is needed; no pushes or host setup changes.

## Checkpoint 7 — database fence decision and passing primitive tests

**B1/B2 remain incomplete; production CLIs are still closed.**

PM decision: no HBA fence and no `pg_hba.conf` changes. The operator creates the
new target with CONNECT revoked from PUBLIC, retaining only approved tool and
migrator connections until ACL acceptance. Tools must check exact target
`pg_database.datacl` and no other target sessions at entry, before commit and
at exit. Source reads remain repeatable-read/read-only plus Task 7's post-stop
writer gate (every other source session blocks). Tools never change source
settings/grants or terminate sessions.

Implemented only the observation primitives so far:

- `a5cd5bdb`: seven failing database-fence tests (completed remote red run).
- `189bde99`: read-only `database_fence.py` and synthetic target creation with
  PUBLIC CONNECT revoked. Seven remote tests passed, 16.85 seconds. They prove
  runtime connection denial, extra-target-session refusal, exact changed ACL
  refusal, unsafe ACL refusal even when supplied as expected, wrong OID refusal,
  and every-other-source-session refusal. `pg_stat_clear_snapshot()` prevents a
  cached statistics snapshot from hiding a later session. No HBA change.
- Logs on test host: `work/hosttools-db-fence-{red,green}.log`. Both test runs
  finished; runner-owned containers/volumes were removed. No tests remain running.
- These helpers are not yet integrated at CLI entry/precommit/exit, nor do they
  replace the full identity/bootstrap evidence chain. No end-to-end pass claimed.

### Decision needed before execution integration

The current approved identity model uses `verdaxis_migrator` as the target
owner/bootstrap/import role and `verdaxis_backup` as the read-only source role.
The new PM wording names both a dedicated tool role and the migrator. Confirm
whether the existing migrator is the dedicated target tool identity, or provide
the exact additional role and privileges. Do not invent an import role or grant
membership. The current primitive accepts only migrator CONNECT; if a separate
role is intended, revise it with tests before connecting it to production CLIs.

**Exact resume:** resolve that role identity, add complete production CLI chain
and wrong-identity tests on the Linux fixture, then integrate entry/precommit/exit
fence checks, frozen-source and trusted bootstrap receipts, and immutable release
attestation. Final production interfaces remain pending. Full suites were not
rerun at this decision stop; latest full results remain checkpoint 6.

## Checkpoint 6 — all 12 remote regressions classified

**Regression gate recovered; B1/B2 integrated production path still pending.**

- Eleven failures: **test assumptions about umask**, not product defects. Remote
  umask is `0002`; fixture-created deployment state and some lock files inherited
  group-write permission. The unchanged deploy guard correctly refused them.
  `ed69e9cd` explicitly chmods the fixture state and locks to 0600. Initial remote
  reproduction: **11 failed, 4 passed**; corrected focused run: **15 passed**.
- `dd360b43` forces umask 0002 in the tuple suite and restores it afterward, so
  future tests retain the remote regression on other platforms. **14 passed**
  through the remote wrapper after adding this coverage.
- Twelfth failure: missing generated `union-catalog.json`. **Test-evidence/run
  assumption, not a demonstrated product defect**. It did not recur in the
  clean serialized full run. The earlier overlapping wrapper invocation was a
  possible cause, not conclusively proved. The wrapper now isolates incoming
  files and locks the lane. No application/migration change was made for it.
- None of these twelve failures involved a disposable-port check. No port guard
  or target attestation was weakened. If port work becomes necessary, use the
  PM rule: one shared kernel-range helper, Linux procfs/macOS sysctl, fail closed
  on unreadable/invalid bounds; never assume the IANA range is the kernel range.
- Clean complete remote PostgreSQL run at `ed69e9cd`: **170 passed**, 62 warnings,
  719.83 seconds. Log: remote `work/hosttools-regression-clean.log`.
- Full local unit run: **2211 passed, 19 known macOS deploy failures**, 514
  warnings, 50.53 seconds. Log: `work/hosttools-regression-classified-unit.log`.
- The original local return wrapper remained waiting after remote completion.
  A subsequent serialized call retrieved the complete result and confirmed no
  pytest/runner processes remained. Only then was the stuck local wrapper stopped;
  its two local wait tasks were also stopped. The final focused run returned
  normally. No tests remain running; this is not an interrupted pytest result.

### Exact resume point after checkpoint 6

Continue checkpoint 5's execution-authority integration; do not repeat image
preparation or regression classification. Use the pinned amd64 Linux runner via
`verdaxis-prod-test`. First write failing end-to-end authority tests using the
production-named fixture, then implement the narrow production bootstrap → copy
→ independent verify path and all wrong-identity refusals. Root-owned manifests,
exact archive/release bytes, independently observed cluster/OIDs, frozen source,
continuous isolation and trusted bootstrap baseline receipts remain required.
The tools are still synthetic-only; final production manifest/secret formats
are not ready for the runbook author. No real product defect was found in the
classified failures. No new host setup or dependency permission is needed.

## Checkpoint 5 — amd64 runtime and Linux database fixture passed

**B1/B2 production execution remains incomplete. No runtime blocker remains.**
The production bootstrap → copy → verify integration is not implemented. Do not
interpret the fixture results as acceptance of that chain.

PM authorized image pull/build through `verdaxis-prod-test` without changing host
setup. Local Colima remains off; all Docker/Linux/PostgreSQL commands below ran
through that wrapper. No SSH, image push or production service access occurred.

### Recorded amd64 runtime

- Base child manifest (verified as linux/amd64 by manifest inspection):
  `docker.io/library/python@sha256:2b4f19dae3a777dfc3b76730bda1e82e1f66ab2a2686fa93ca78edbfb4f04ffe`.
- Built image ID:
  `sha256:e0605c4e3b06d7bb3ac182d9d6dbb184647625c5e21b88f3eaca5230d64f1efb`.
- Recipe: `tests/phase0/Dockerfile.hosttools`, repository-Dockerfile-derived;
  unchanged requirements plus repository constraints, including existing test
  requirements. No new Python dependency. Package record: `/phase0-packages.txt`.
- `tests/phase0/runtime_images.py` records immutable IDs for amd64 and historical
  arm64. Runners explicitly select amd64; never use the build tag for execution.
  Historical arm64 image/base records below are not test-host execution authority.

Rebuild only through the approved wrapper:

```sh
verdaxis-prod-test "$PWD" 'set -eu
mkdir -p work/hosttools-amd64-build
cp requirements.txt constraints.txt work/hosttools-amd64-build/
cp tests/phase0/Dockerfile.hosttools work/hosttools-amd64-build/Dockerfile
docker build --platform linux/amd64 \
  --iidfile work/hosttools-amd64-build/image.id \
  -t localhost/verdaxis-phase0-hosttools:amd64 work/hosttools-amd64-build
docker image inspect localhost/verdaxis-phase0-hosttools:amd64 --format "{{.Id}} {{.Architecture}}"'
```

A rebuild can produce another ID due to apt/build metadata. Inspect and record
it before changing the explicit image table. No floating-tag fallback.

### Verified runner progress

- `9f12556d`: amd64 image/base and architecture records.
- `753158da`: fix missing PLATFORM import in the database runner.
- `6fd857ad`: use `/root/phase0-work` for private writable fixture evidence.
  Rootless image `/` is mode 0555; no permissions/capabilities were widened.
- Offline Linux authority/report/CLI primitives: **51 passed** (17.30 seconds).
- Linux production-named database fixture: **4 passed** (8.47 seconds), following
  two disclosed setup failures above. All pinned historical/union migrations
  built in Linux; drift check passed. Real production Settings, separate scratch
  credentials, collision and wrong-cluster refusal passed.
- Database container has `--network none`, no published ports; runtime shares
  only its network namespace. Database listens on `127.0.0.1:55432`. Exact source
  archives are copied, no bind mounts or host secrets. Containers removed with
  `docker rm -f -v` on both failed and successful attempts.
- Full remote PostgreSQL regression: **158 passed, 12 failed**, 62 warnings,
  782.41 seconds. Eleven failures are cutover/two-lock/tuple rollback assertions;
  the union comparison failed because its generated `union-catalog.json` was
  absent. These are unresolved, not accepted platform exceptions. A second
  wrapper invocation was used to inspect progress while the suite ran; because
  the wrapper re-ships the tree, that may have removed generated work evidence.
  Do not invoke this wrapper concurrently for the same worktree. Repeat the full
  suite without a concurrent wrapper call before diagnosing the missing file.
  The guard failures also need investigation; do not weaken runtime guards.
  Remote process inspection after the summary found no test/runner processes and
  no owned Phase 0 containers. The original local wrapper remained stuck after
  remote completion and was stopped; its local wait task then exited. No tests
  remain running. The pytest result above is from the completed remote log, not
  a successful wrapper exit.
- Unit suite: **2211 passed, 19 known macOS deploy failures**, 514 warnings.
- Remote log: `work/hosttools-linux-database-amd64.log`; local unit log:
  `work/hosttools-unit-amd64-runner.log`.

### Exact resume point after checkpoint 5

The remote image and database fixture are working. Do not repeat environment
preparation or request more runtime permission. Run through the wrapper:
`venv/bin/python -m tests.phase0.linux_database`.

Next add failing **complete execution-authority** tests in that Linux fixture:
root-owned exact tool/release manifests, independently observed cluster/OIDs,
frozen source catalog/data/sequences, continuous source/target isolation, and
bootstrap-generated baseline receipts. Then implement and invoke the actual
production bootstrap → copy → verify CLIs, testing every wrong-identity refusal.
Current tools remain synthetic-only. The test runtime has no Git binary/repo;
release-byte attestation must be designed for the approved immutable archive,
not silently bypassed or patched. Do not grant new dependencies without approval.

Keep the common target flag and separate per-tool manifests/secrets. Publish
final host formats only after the integrated chain passes, update architecture,
then rerun both required suites. No host setup changes, push or live operation.

## Historical environment stop — remote-only testing, 16 October

The founder has stopped local Colima. Do not start Colima or Docker locally.
All PostgreSQL/Docker/Linux checks now run only through
`verdaxis-prod-test "$PWD" 'COMMAND'`. Do not SSH or change test-host setup.
This supersedes the local execution authorization below.

Tracked files were clean at resume; untracked `.claude/` and `work/` were left
untouched. Current implementation is `3919071d` (offline Linux database runner),
following `972ce799` (two runner isolation tests). Those two unit tests passed.
The database runner has not yet been executed successfully.

The authorized wrapper preflight completed with exit 125:

- Remote architecture: `x86_64`; harness Python: `3.12.3`.
- Docker wrapper exists at `/home/verdaxis-test/bin/docker`.
- Required image `sha256:5165bc002d5ec7a9fe44d6e560c0a9aea36e5bbcea7aa48f8cd3a072f59fcf02`
  returned **image not known**. That image was built locally for arm64; the
  current `linux_database.runtime_command` explicitly selects `linux/arm64`.

No image was pulled, built, transferred or substituted. No host configuration
was changed. No test run was started or left running. Per the founder's explicit
instruction to stop if the environment lacks something, execution stopped here.
**B1/B2 remain open; no new acceptance counts are claimed.**

### Exact resume point after remote environment stop

Obtain an operator-provided approved amd64 runtime image containing the pinned
repository dependencies, or explicit approval to adapt these test runners to
use the provided Linux harness without weakening root-owned authority checks.
Do not implicitly reuse the old local image-build permission to change this host.
Then pin the approved runtime identity/platform in test tooling, execute the
new loopback-only database runner through `verdaxis-prod-test`, and continue
checkpoint 4 steps 2–4: complete execution-authority tests, integrated production
bootstrap → copy → verify, wrong-identity refusals, and final interfaces.
The existing runner uses committed archives: commit implementation/test changes
before running that path; the outer wrapper's edit transfer alone does not put
uncommitted files into its inner `git archive HEAD`.

## Checkpoint 4 — Linux runtime available; execution integration pending

**The runtime limitation is resolved. B1/B2 production execution remains open.**
No further permission is needed to continue the authorized local synthetic work.
The production bootstrap → copy → verify chain has NOT been implemented or
accepted. The successful Linux tests below cover primitives, not that chain.

### Local image and rebuild

PM authorized the local Linux runtime build. `tests/phase0/Dockerfile.hosttools`
is a test-only derivative of the repository Dockerfile: same Python 3.12 slim
base and `gcc libpq-dev` installation, but an immutable base digest and the
repository's `constraints.txt` applied to `requirements.txt`. Existing pytest
and pytest-asyncio requirements are included; no extra Python packages were
requested. Source, credentials and `work/` are not baked into the image.

- Platform: `linux/arm64` on Colima.
- Base: `python:3.12-slim@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1`.
- Built local image ID (content digest, not a pushed registry manifest):
  `sha256:5165bc002d5ec7a9fe44d6e560c0a9aea36e5bbcea7aa48f8cd3a072f59fcf02`.
- Local tag: `verdaxis-phase0-hosttools:local`. Never pushed.
- Image contains `/phase0-packages.txt` (`pip freeze --all`); `pip check` passed.
- Runtime smoke: Linux, Python 3.12.15, asyncpg 0.31.0, pytest 9.1.1,
  SQLAlchemy 2.0.51.

Rebuild from this worktree with only these three build-context files:

```sh
mkdir -p work/hosttools-linux-build
cp requirements.txt constraints.txt work/hosttools-linux-build/
cp tests/phase0/Dockerfile.hosttools work/hosttools-linux-build/Dockerfile
docker build --platform linux/arm64 \
  --iidfile work/hosttools-linux-build/image.id \
  -t verdaxis-phase0-hosttools:local work/hosttools-linux-build
docker image inspect verdaxis-phase0-hosttools:local --format '{{.Id}}'
```

The base and Python versions are pinned. Apt repository metadata/build output
can change the resulting image ID. Review any rebuilt ID before updating the
exact `IMAGE` constant in `tests/phase0/linux_hosttools.py`; do not silently
substitute the mutable local tag in test execution.

Initial build failed because adding `--no-install-recommends` omitted libc
headers needed by pinned psutil on arm64. Restoring the repository Dockerfile's
original apt installation flags fixed it without changing dependency versions.
The failed intermediate container was identified in the build log and removed
with `docker rm -v`; unrelated containers were left alone.

### Completed checks

- `f7b7708c`: pinned runtime recipe; build and import smoke passed.
- `ad812ffd`: `venv/bin/python -m tests.phase0.linux_hosttools` runs an exact
  committed archive in the pinned image with `--network none`, no capabilities,
  no-new-privileges, no bind mount, and no host secrets. It proves a real root-owned
  0600 authority file in a root-owned 0700 directory is readable, then runs the
  authority/report/CLI primitive suites: **51 passed**. Container removed with `-v`.
- Full PostgreSQL runner: **170 passed**, 62 warnings, 450.62 seconds; owned
  container/volumes removed. Full macOS unit suite: **2209 passed, 19 failed**,
  514 warnings, 55.70 seconds. Failures are only known `test_deploy_health.py` cases.
- Logs: `work/hosttools-linux-build/{build.log,build-final.log}`,
  `work/hosttools-linux-smoke.log`, `work/hosttools-postgres-checkpoint4.log`,
  `work/hosttools-unit-checkpoint4.log`. All runs finished.

### Exact resume point after checkpoint 4

Use the now-built Linux image. Do not repeat the resolved runtime permission ask.

1. Extend the disposable runner to execute the production-named synthetic
   fixture and CLI acceptance in Linux. Use a private internal network boundary
   and shared PostgreSQL network namespace so the tested tool endpoint is actual
   numeric loopback; do not weaken host endpoint checks for Docker NAT. Keep
   root-owned evidence on container-local storage and source from exact archives.
2. Add failing complete execution-authority tests: exact installed/release bytes,
   trusted independent cluster/endpoint/OID and source freeze, continuous writer
   and target isolation, trusted bootstrap receipts, distinct scratch credentials,
   every wrong identity and no source mutations. The existing fixture/primitive
   tests do not replace these tests.
3. Implement the narrow production path across bootstrap/copy/verify, including
   tool-specific manifests (bootstrap cannot require its own future receipt),
   bootstrap-generated baseline publication, copy binding and independent verify.
   Preserve every synthetic gate; no stamp, generic override or authority mocks.
4. Run exact CLI commands end to end, then both required full suites. Publish
   final production interfaces and update architecture before sign-off. Do not
   push the image or Git, access deployed systems, or use real data.

## Checkpoint 3 — production-named synthetic database fixture

**B1 and production B2 remain open. The production CLI chain is not implemented.**

- `d9140536`: fixture contract tests; initial run had four missing-module errors.
- `f0ffe501`: test-only `tests/phase0/production_scratch.py`. Creates synthetic
  `verdaxis` from the runner's pinned production template and empty
  `verdaxis_prod` from its PostGIS template. Uses exact `verdaxis_app`,
  `verdaxis_migrator`, `verdaxis_backup` roles with three independent generated
  passwords, distinct from the runner administrator password. All storage is
  in the runner-owned disposable cluster, with its independently observed system
  identifier and ephemeral loopback endpoint. No live cluster is contacted.
- Collision and wrong-cluster checks precede mutation. Cleanup drops only the
  databases/roles created by this fixture. The source backup role has SELECT
  only; an UPDATE probe refuses. Source ownership changes apply only to the new
  synthetic clone, not its template or any existing database.
- Production Settings validates the exact field names and effective app/migrator
  URLs. The first green attempt had one failure because the test omitted the
  explicit `DATABASE_NAME`/`DATABASE_USER` fields; the fixture test now supplies
  both, without relaxing Settings. Corrected focused run: **4 passed**.

### Environment limitation and exact resume point

The complete root-owned execution-authority test needs a Linux Python runtime
with the required dependencies. Inspection found only the pinned PostGIS image
in the local Docker cache. That image has no `python` or `python3`. The supplied
`venv/bin/python` is a Darwin interpreter, not usable inside that Linux image.
No package was installed and no ownership check was patched to bypass this.
A cached compatible runtime/dependency bundle, or explicit permission to prepare
one, is needed for the planned Linux CLI acceptance test.

Resume with checkpoint 2's **execution-authority integration steps 1–6**, using
`production_scratch()` as the production-named synthetic storage fixture. The
fixture alone does not provide root-owned manifests, a trusted installed tool
release, continuous isolation, or bootstrap receipts. Its observed Docker
server address is not the final host-loopback authority contract. Do not expand
production host endpoint rules merely to accommodate Docker port translation.
The production entry points remain closed until the complete authority and
bootstrap → copy → verify tests exist and pass. All current command interfaces
remain those recorded in checkpoint 2; no final production format is claimed.

Completed checks at `f0ffe501`:

- Full PostgreSQL runner: **170 passed**, 62 warnings, 639.07 seconds; exit 0.
  The owned container and volumes were removed.
- Full unit suite: **2209 passed, 19 failed**, 518 warnings, 111.50 seconds.
  Every failure is in the known macOS `test_deploy_health.py` group.
- Logs: `work/hosttools-scratch-{red,green,green2}.log`,
  `work/hosttools-postgres-checkpoint3.log`, `work/hosttools-unit-checkpoint3.log`.
- All test runs finished. No packages, host access, push, Skill or Agent calls.

## Checkpoint 2 — authority primitives, report writes and flags

**B1 remains open. B2 production authority remains open. No host use approved.**
The production contract validator is deliberately not wired to execution. It
cannot turn a mode field into permission. Do not regard passing input tests as
proof of continuous isolation or trusted bootstrap evidence.

New commits:

- `c348be6c` / `a2a4cd41`: 37 initially failing B1 input-contract tests, then
  strict contract fields/types, exact source/revision and production names/roles,
  numeric loopback endpoint/OID/cluster validation, root-controlled no-follow
  private file traversal, duplicate/nonfinite JSON refusal, and source-tree
  digest primitives. These are prerequisites, not the complete authority chain.
- `42938cdd` / `eeb30e2c`: 8 initially failing report-publication tests, then
  exclusive publication from a fully written/fsynced private temporary file.
  Serialization, write and file/directory fsync failures leave no report;
  pre-existing or racing destinations are not overwritten or removed. A crash
  during directory fsync remains uncertain: verify again to a new path, never
  replay import. Parent directory must be owned and private.
- `99ba8056` / `e632a3bd`: exact common target flag and no abbreviated arguments.
  Interface red: 2 failed, 4 passed. Combined focused final: **51 passed**.

Current exact CLI interfaces (the flag spelling is final; production formats
are NOT final and execution remains synthetic-only):

```sh
venv/bin/python -m scripts.phase0.bootstrap \
  --manifest <bootstrap.json> --target-environment-file <bootstrap-target.env> --apply
venv/bin/python -m scripts.phase0.copy \
  --manifest <copy.json> --source-environment-file <copy-source.env> \
  --target-environment-file <copy-target.env> \
  --source-sha <production-source-40hex> --target-sha <current-release-40hex> --apply
venv/bin/python -m scripts.phase0.verify \
  --manifest <verify.json> --source-environment-file <verify-source.env> \
  --target-environment-file <verify-target.env> --report <new-private-report.json>
```

Use separate manifest and secret files for all three tools. Existing synthetic
formats remain as described below. Bootstrap's old `--environment-file` is
removed, not an alias. No production-format promise is made yet.

Completed full checks at code commit `e632a3bd`:

- PostgreSQL: **166 passed**, 62 warnings, no failures/skips, 575.70 seconds.
  The runner completed, including its container/volume cleanup.
- Unit suite: **2209 passed, 19 failed**, 514 warnings, 108.31 seconds. All
  failures are the known macOS `test_deploy_health.py` cases.
- Logs: `work/hosttools-{authority,publication,interface}-red.log`,
  `work/hosttools-postgres-checkpoint2.log`, `work/hosttools-unit-checkpoint2.log`.
- No test remains running. No packages, push, agents, skills or host connections.

### Exact resume point after checkpoint 2

Continue with **production execution authority integration tests**, not more
standalone validation tests. The Decision problems section has now been read.

1. Build a local synthetic production-named scratch fixture with distinct exact
   app/migrator/backup credentials and production Settings. Test the complete
   CLI chain with root-owned authority inside a disposable Linux boundary; do
   not monkeypatch ownership or identity checks in the acceptance path.
2. Define and test independently observed cluster/endpoint/OID evidence and a
   continuous writer/target isolation boundary through bootstrap, import and
   ACL acceptance. A root manifest digest, one session count or a cooperative
   advisory lock alone is insufficient. No source ACL/settings changes.
3. Wire exact installed/release bytes and the frozen source revision/catalog/
   fingerprint to that authority. Validate actual host and root-owned installed
   inputs, not only `validate_contract`. The current tree digest primitive does
   not yet prove a clean commit or package/interpreter provenance.
4. Add bootstrap receipt generation within the verified migration transaction,
   publish only after commit, and bind copy/verify to those exact reference and
   sequence baseline bytes. Do not recapture an arbitrary existing target.
   The provisional `bootstrap_receipt_sha256` field in `host_authority` must be
   made tool-specific: bootstrap cannot depend on its own future receipt.
5. Integrate bootstrap/copy/verify production entry points only after these gates
   pass; preserve every synthetic/refusal test. Add fresh-source/late sequence
   writer, missing-input-file and explicitly read-only verifier tests. Strengthen
   report parent traversal for production using root-controlled descriptors.
6. Replace this provisional contract with final separate per-tool manifests and
   secrets, update architecture, run both full suites, and commit. B1/B2 cannot
   be signed off until this chain is proved. No push or live operation.

---

The sections below describe checkpoint 1 and are historical where superseded.

## Status: NOT ready for host use

This checkpoint addresses the false-success verifier in B2 only. **B1 is not
implemented. B2 is not closed for production.** Bootstrap, copy and the new
verifier retain the synthetic-only gate. Do not run these commands on a host
rehearsal or live database. No staging/production connection was made.

Commits:

- `45671e42`: failing executable verifier tests (13 failures before implementation).
- `8b039cac`: independent read-only verifier and corrected test pool teardown.

## Current command interfaces (not the final host contract)

All commands use `venv/bin/python -m scripts.phase0.<tool>` from this worktree.
Use distinct private mode-0600 regular files for each tool. None of these
examples grants production permission.

```sh
venv/bin/python -m scripts.phase0.bootstrap \
  --manifest <bootstrap.json> --environment-file <bootstrap-target.env> --apply
venv/bin/python -m scripts.phase0.copy \
  --manifest <copy.json> \
  --source-environment-file <copy-source.env> \
  --target-environment-file <copy-target.env> \
  --source-sha <production-source-40hex> --target-sha <current-release-40hex> --apply
venv/bin/python -m scripts.phase0.verify \
  --manifest <verify.json> \
  --source-environment-file <verify-source.env> \
  --target-environment-file <verify-target.env> --report <new-private-report.json>
```

**The bootstrap flag mismatch remains open:** it still accepts
`--environment-file`, not the required common `--target-environment-file`.
The runbook author must not treat these as final approved host interfaces.

Current formats:

- Bootstrap JSON: exactly `mode`, `approved`, `target_revision`, `identity`, as
  defined in `bootstrap.validate_manifest`. Its secret file contains only
  `MIGRATOR_DATABASE_URL=<explicit asyncpg URL>` and a newline.
- Copy JSON: the complete committed `deploy/phase0/copy-manifest.json` plus
  `execution`, as defined in `copy.validate_manifest`. Each copy secret file
  contains only `DATABASE_URL=<explicit URL>` and a newline.
- Verify JSON: the complete copy contract plus its per-run `execution`, including
  the original baseline reference rows and sequences. It additionally requires
  `execution.frozen_source` (the complete `fingerprint_database` result captured
  before copy) and `execution.source_catalog_sha256` (`manifest.digest` of the
  committed source catalog). Each verify secret file contains only
  `DATABASE_URL=<explicit URL>` and a newline. The current synthetic target
  verifier uses the owner role, inside a read-only transaction.
- Reports contain identities, manifest digest, source table/column fingerprints,
  verified table/column/reference hashes, numeric totals and sequence state/next
  values. A report destination must not already exist. No import receipt is
  needed; no import replay is attempted.

These current formats are not root-owned production authority. In particular,
caller-provided baseline and freeze values are not independently trusted host
proof. B1 must supply that trust before production mode can be accepted.

## Verification implemented

The CLI rejects missing/unknown arguments, missing files and existing report
paths. It opens independent connections with read-only session defaults and
repeatable-read/read-only transactions. It attests endpoint/database/OID/role
and connected cluster identity, compares the full existing catalog inspector's
schema/constraint/index/trigger/function/extension/sequence definitions, checks
frozen source hashes, runs the shared data verifier, and checks exact target
sequence state and high-water calculations without calling `nextval` or
`setval`. It rechecks source sequence state before completing.

Tests execute the documented flag combination as a subprocess, including
receipt loss after commit. They cover changed data/reference rows, disabled
triggers, advanced sequences, changed source rows, wrong OID/cluster, changed
freeze/catalog binding, existing report destination and unknown/missing args.
The initial implementation test had 1 failure because the fixture returned its
closed connections to an idle pool. The verifier correctly refused the other
target session. Disposing both fixture pools fixed the test; no isolation gate
was weakened.

## Completed checks

- Focused verifier red: **13 failed**.
- Focused verifier final: **13 passed**.
- `bash scripts/run_phase0_postgres_tests.sh`: **166 passed**, no failures/skips;
  union drift check passed; owned Colima container removed with volumes.
- Full unit suite: **2158 passed, 19 failed**. All failures are the known macOS
  `tests/unit/test_deploy_health.py` cases. No other unit failures.
- Logs: `work/hosttools-verify-{red,green,green2}.log`,
  `work/hosttools-postgres-checkpoint.log`, `work/hosttools-unit-checkpoint.log`.
- No packages installed, pushes, Skill calls, agents or host operations.

## Exact resume point

Continue on `feat/phase0-hosttools` after this documentation checkpoint.
Preserve untracked `work/` and `.claude/`; neither belongs to these commits.

1. Finish the review/plan context and relevant lessons; the supplied review,
   architecture, Tasks 4a/4b/5/10, task progress and runbook were read. The plan's
   Decision problems section still needs a dedicated read.
2. Write failing B1 tests first: trusted root-owned private authority; exact tool
   and release bytes; host; old/new names and role split; endpoints/OIDs; trusted
   cluster binding; frozen source revision/catalog/data/sequences; trusted
   bootstrap receipt; production Settings with distinct scratch credentials;
   wrong identity before DDL/import; no source changes; continuous target
   isolation through import and ACL checks. Keep synthetic tests unchanged.
3. Implement the narrow production authority and bootstrap receipt path. Do not
   enable host mode by changing policy JSON, use generic overrides, recapture an
   arbitrary target baseline, change source grants/settings, or add stamp.
4. Integrate that authority with verify. Tighten manifest field validation, bind
   exact tool/release and frozen source evidence, and prove the production-mode
   verifier under distinct scratch credentials. Source repeatable-read alone
   does not prove external writer quiescence; B1 must supply continuous isolation.
5. Harden report publication: the reused `private_json` writer uses exclusive
   creation and fsync, but a write/fsync failure can leave a partial destination.
   Add fault-injection tests and success-only publication/cleanup. Current tests
   cover an existing destination, not all write/fsync failures. Also explicitly
   prove read-only statements and missing-input files on the executable path.
6. Unify bootstrap on `--target-environment-file` with strict parsing, test the
   exact commands, and replace this provisional interface section with final
   separate manifest/secret formats for the runbook author. Do not claim the
   broader M2 host appendix is completed in this lane.
7. Update architecture and this status, rerun both required full suites, and
   commit in small steps with the requested attribution. Independently review
   production authority before declaring B1/B2 complete. Do not push.
