# Task 1 — frozen source and synthetic fixtures

Status: passed Task 1 PostgreSQL gate. Later Phase 0 gates remain open.
Implementation commit: `f421366` (`test: add Phase 0 source schema and copy inventory fixtures`).
No live host/database, real data, provider, deployment or push was used.

## Frozen inputs and outputs

- Production source/fixture SHA: `c08266bd6969143812dec7c5033338428cd5f159`.
- Production revision: `kycg_20261014_prod`.
- Staging source SHA: `b218ab264b92f99007bacc05d4871413e024b72a`.
- Staging revision: `kycd_20261015_staging`.
- `scripts/phase0/manifest.py`: `inspect_catalog(connection) -> CatalogManifest`,
  explicit inventory comparison, JSON digest and frozen pins. The connection
  is SQLAlchemy synchronous; async callers use `connection.run_sync`.
- `deploy/phase0/copy-manifest.json`: 61 source tables (55 copy, 2 reference
  regeneration, 4 skip), explicit source column names/types and 15 target
  additions. Includes both catalogs, constraints, indexes, user triggers with
  function bodies, sequence definitions/ownership, extension membership and
  complete reference values captured from the pinned builds.
- `deploy/phase0/provisional-union.json`: eight distinct products, production
  delivery identities, UTC sessions, Singapore certificate dates, KYC off,
  no grace email or LNG/UCOME seed prices. This is not a deploy approval.
- `tests/phase0/conftest.py`, `test_inventory.py`, `synthetic.py`,
  `build_fixtures.py`, `runner.py`, `network_guard.py`: synthetic history,
  migration build, test connections and fixture-only Python network guard.
- `scripts/run_phase0_postgres_tests.sh`: disposable runner; accepts test paths,
  not database URLs. It removes its container and volumes on exit.
- `docs/phase0/progress.md`: downstream facts and resume ownership.

## Digests

Canonical JSON SHA-256 uses sorted keys and compact separators. Structural
hashes exclude reference values because migration `created_at` values vary.
Every run writes separate exact reference-value hashes; these values must
still be preserved by Task 5.

| Contract | SHA-256 |
|---|---|
| Production schema | `e86d6e9d2b879153553b365590f112b6975724890ebe38742d3b8baeb295a9f5` |
| Staging schema | `6fd8ce03449cb2b98a20bfcd286fa9a1e46f8fe64cb357192a2fb7742b7e7b78` |
| Provisional union | `849ca8f45a3bacd908be4173d28024006213c1eb2f370b8ce7d30ee8980547e6` |

## Build proof and checks

1. Wrote inventory tests first. The required initial command failed with exit
   127 because `scripts/run_phase0_postgres_tests.sh` did not exist.
2. Archived each full SHA into a unique directory below
   `work/phase0-fixtures/`. Each tree ran its own imports and unchanged Alembic
   files, using its `venv` link to the supplied shared CPython 3.12 environment.
   No packages were installed or changed.
3. Built PostGIS in the disposable template, created a least-privilege owner,
   and ran production migrations to `miq_20260720_market_quarantine`. Inserted
   six historical trade states there, then ran the real chain to production
   head. Their snapshots remain SQL NULL. No stamp or migration edits occurred.
4. Independently built the pinned staging head. It has 62 classified tables.
   The future union schema is deliberately not implemented in Task 1.
5. Populated every copy table and exercised version-1 frozen trade identity
   after party rejection, pending membership, rejected/moved KYC, expired
   grace, sent notice audit, private reminder payload, RFQ cycle, consumed
   support authorization, command receipt, legacy dependencies, duplicate port
   intelligence, inactive reference rows and sparse global sequences.
6. Tested extra tables, partitions, columns and changed column types; all
   refuse. Tested PostgreSQL 17/PostGIS 3.6, external DNS/socket denial, exact
   revisions, schema definitions, JSON precision, SQL NULL versus JSON null,
   private notification fields and numeric snapshot-zero refusal.
7. Final PostgreSQL command: **16 passed, 0 failed, 0 skipped** (4.15 seconds
   for pytest; container and migration time excluded). A skipped/empty suite
   fails the runner.
8. Required full unit command: **2020 passed, 19 failed, 0 skipped**, 504 warnings.
   All failures are in `test_deploy_health.py`, the known macOS Bash 3.2
   `mapfile` failure and dependent assertions. No other unit failure occurred.
9. `git diff --check` passed. No owned Phase 0 container remains.

Final local run evidence is in
`work/phase0-fixtures/3578d2c8daa94fb5aef38772b9280de9/` (not committed).
Logs: `work/phase0-final-verified.log` and `work/phase0-unit.log`.

## Resume and limits

```bash
bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_inventory.py
```

This creates new isolated databases on a Docker-assigned loopback port using
`postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272`.
Per-run `prod-catalog.json` and `staging-catalog.json` capture migration-only
baselines; `prod-populated-catalog.json` includes synthetic reference rows.
`prod-receipt.json` and `staging-receipt.json` bind source SHA, revision, table
count, schema hash and exact reference hash. Connections are closed before
container deletion; no database survives the run.

No required Task 1 test was omitted. No later-task PostgreSQL suite, live
rehearsal, import, ACL convergence or deployment was run. The only adaptations
are the supplied shared venv link instead of a new package installation and
small test-only helper modules. No application behavior changed. The source
support graph has no reverse authorization-to-order FK; its actual links are
covered rather than fabricating a cycle. See `docs/phase0/progress.md` before
implementing Task 5's cycle handling or legacy commission import.
