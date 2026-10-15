# Phase 0 Task 4b — union revision and final schema policy

## Scope

Work started at `cab1e0a` on the integration branch. No host, deployed database,
provider, real data, package installation, push, skill or subagent was used.
All PostgreSQL tests use the pinned disposable Colima image and loopback ports.

## Schema contract

- Revision: `p0_20261016_union`; parent: `kycd_20261015_staging`.
- Earlier migrations and parents are unchanged. Normal checkpoint policy adds
  only the predecessor-to-union and union-to-union rows. No stamp or base alias.
- Frozen migration literals retain eight distinct product identities and eight
  delivery identities. Missing references are inserted; existing reference values,
  activity flags, timestamps and geographical timezones are not overwritten.
- Four product checks add B30/B100. The trade function retains the complete
  FAME/LNG immutable snapshot, provenance and canonical identity checks.
- The final schema has 62 classified tables and 15 added shared-table columns.
  Grace RESTRICT FKs, target-only tables, sequence definitions, cyclic FK names,
  import order and all unchanged trigger definitions remain as reviewed in Task 5.
  No activity tracker is added. Fresh bootstrap creates no customers or grace.
- `app/market_constraints.py` already derives all eight products from Task 2's
  catalog. No redundant edit was needed. `alembic check` passes on the final schema.
- `copy-manifest.json` pins complete final checks, functions, extension membership,
  sequence definitions and reference baseline. The production source pin is unchanged.

### Frozen digests

- Migration graph: `748dbcdfba75e5b9b2b563c814a5d42808c05a7c4139218fee23402cc7742df9`.
- Structural schema: `54a40050c8af02f0132fe3c8516089212cf0e7494c9b1e24eee6b42885d85ffa`.
- Canonical reference values: `5b522ebaed9732c8576f00bebf9100d7c36c7f67079add01f0234e3445bf1c58`.
- Complete copy contract: `8afaf39315bd6787ac4bc4ee658d5392a5b6831917911ec52fb57ab40b60e722`.

## Bootstrap and authority

The checked-connection path now targets union and requires an exact full local
source SHA. It checks graph, structural schema and canonical reference-value
hashes. Independent runs generate their own reference creation timestamps;
copy verification still compares every source-derived timestamp exactly.
The manifest's `source_sha` is the full commit being tested, not the staging base.
The copy CLI likewise requires the full current target SHA rather than the old
staging ancestor. These tools remain synthetic-only.

The policy records the permanent production contract: `EMPTY` to union,
`verdaxis_prod`, `verdaxis_app`, `verdaxis_migrator`, `verdaxis_backup`.
**Host apply remains disabled.** Task 7's production Settings identity is not
integrated here. Root-owned host manifests, trusted host/cluster attestation,
source freeze/outbox drain, isolation/egress and production-Settings scratch-cluster
proof still require integration and separate operator approval. This is not a
claim that the host bootstrap or live copy path is ready.

## Task 5 re-check

All copy tests now clone the independently built union, not pinned staging.
Synthetic source history adds B30/B100 orders, trades and receipts. The extended
cycle test crosses the 256-row batch boundary with 258 RFQ/quote pairs.
Formatting, RLS, empty trade history, SQL/JSON null, exact Decimal/JSON, EWKB,
sequence-state and all six interruption cases run against union. No newly copied
bytea or native-array type was introduced. Vault bytea columns remain target-only.
The reference regression now uses the real migration-created Dalian baseline.
The copied-union ACL test applies and validates the unchanged canonical policy,
then checks raw app/backup denials for grace, receipts, audit, provenance, vault,
control tables and schema creation. It grants no runtime import authority.

## Test record

- Initial required red run: 6 failed, 29 passed. Union was absent and bootstrap
  still selected staging. Tests committed first as `1f8470b`.
- Fresh/upgraded schema proof: 2 passed; Alembic reported no new operations.
- First full union PostgreSQL run: 86 passed, 46 warnings, 397.28 seconds.
- Expanded focused run: 36 passed, 1 failed due to a missing product ID in the
  new multi-batch test fixture. The fixture was corrected.
- ACL/lane run: 2 passed, 1 failed in test teardown because the non-superuser
  owner cannot DROP OWNED BY an unrelated role. Cleanup now uses the disposable
  administrator, without granting membership to the owner.
- Required unit suite: 2132 passed, 19 failed, 513 warnings, 127.91 seconds.
  All failures are the known macOS Bash 3.2 `test_deploy_health.py` failures.
  There are no other unit failures.
- Required final bootstrap/union command: 36 passed, 46 warnings, 149.87 seconds.
- Checkpoint/KYC graph unit contracts: 28 passed, 1 warning.
- The multi-batch fixture also needed the RFQ delivery point to match its accepted
  trade. The source INSERT/acceptance guard refused it before copy; the fixture
  now supplies both required identities. The CLI SHA regression passed.
- Expanded full run before the delivery-point fixture fix: 88 passed, 1 failed.
  The sole failure was that same multi-batch fixture, not the copy implementation.
- Corrected multi-batch/new-INSERT/CLI-SHA and copied-union ACL checks: 3 passed,
  39.16 seconds. The 258 cyclic pairs cross the COPY batch boundary successfully.
- Final repeated unit suite: 2132 passed, the same 19 known deploy failures,
  513 warnings, 110.72 seconds. No other failure.
- Final `bash scripts/run_phase0_postgres_tests.sh`: **90 passed**, 46 warnings,
  no skips or failures, 317.73 seconds. `alembic check` reports no new operations.
  Evidence: `work/phase0-fixtures/cf91490562d844aeb73609479d03b80d`.
  The runner removed each owned container with its volumes; other lanes' containers
  were left untouched. `git diff --check` and Python syntax checks passed.
- Not run: host bootstrap/copy, production-Settings separate-cluster acceptance,
  live rehearsal, Rust goldens and release-oracle checks. Host work is not authorized;
  exact production identity and final oracle integration belong to Tasks 7/9/10.

Implementation commits: `a588e19c` (union/policy) and `a272fcab` (copy integration).
The full tested code commit is `a272fcaba2b6b20666aef0d793fbd5cef934795b`.
This is a local integration checkpoint, not an approved production release.
Logs: `work/task4b-focused-final.log`, `work/task4b-copy-final.log`,
`work/task4b-postgres-acceptance.log`, `work/task4b-unit-final.log` and
`work/task4b-checkpoints.log`. Temporary evidence is not committed.

## Ownership deviations and resume

Minimal integration edits were needed in Task 4a's bootstrap/test, Task 1's fixture
builder/inventory/synthetic rows, Task 5's target manifest/copy fixture/regressions
and CLI SHA check. These implement the requested final-schema re-check, not a new
copy mechanism. The KYC head assertion now names union while retaining the old
checkpoint assertions. ARCHITECTURE has one focused contract update.
No deploy helper, identity settings, runtime grants, timers or other lane feature
was changed. KYC gate settings remain off in the disposable runner.

Task 7 must consume the union literal and policy hashes without enabling host
apply by editing JSON. Task 9 owns the final release oracle and affected Rust
goldens. Freeze the eventual integrated release SHA after all lanes merge; the
staging base SHA is ancestry evidence, never that release's approval.
