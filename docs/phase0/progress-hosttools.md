# Phase 0 host-tool sign-off fixes — partial checkpoint

## Current checkpoint 2 — authority primitives, report writes and flags

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
