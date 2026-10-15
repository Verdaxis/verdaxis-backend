# Phase 0 Task 2 — production products on staging

Implementation commit: `486379c`.

## Scope and resume point

This lane preserves production scalar B30/B100 on the staging runtime. It does
not add the union migration, copy data, change KYC, activate jobs, or contact a
live host. Task 4b must add the reviewed catalog baseline, product-domain checks
and trade function before this source can be deployed.

The product/delivery policy SHA-256 is
`fbe6fb695a983267d0f7db973e3aac8b8b0ba803543ef7327756bde7e953bb20`.
Compute it from a JSON object with `products` and `delivery_points` as ordered
`dataclasses.asdict` lists from `app.market_catalog`, plus
`biofuel_specification_standards` with string UUID keys. Serialize with
`default=str`, `sort_keys=True`, and `separators=(',', ':')` before hashing.

- B30: `c4cecebc-3d2d-5840-8021-57a9a11bc673`, Biofuel/B30, 200 MT,
  ISO 8217:2024 RF 380. All eight canonical delivery points.
- B100: `74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5`, Biofuel/B100, 200 MT,
  ISO 8217:2024 DFA, no HVO. All eight canonical delivery points.
- UCOME_B100: `e561e43f-d9b2-598e-981c-f1d28d515ddc`, unchanged typed FAME,
  1 MT, Singapore only. Its curve midpoint remains non-executable reference data.
- BIO_LNG: unchanged typed declarations, 1 MT, Rotterdam only; no seed price.
- The four alcohol identities and all production delivery tuples are unchanged.

Task 8 must use these IDs, not labels. Task 4b can freeze literals from
`app.market_catalog` and `app.models.forward_monitoring._MARKET_PRODUCT_CHECK`.
`app.market_constraints` already derives its product domain from the catalog;
no edit was needed. Model/schema Biofuel enums, catalog response fields, live
benchmark qualification and curve grouping already compose with this union.

## Test-first evidence

Before implementation:
- `tests/unit/test_phase0_catalog.py`: 19 failed, 8 passed.
- Disposable `tests/postgres/test_phase0_market_contract.py`: 20 failed on the
  absent B30/B100 IDs. No collection skips.

After implementation:
- Focused catalog, FAME/LNG, seeds, public market, preferences, inventory,
  demo and compliance-pricing suite: 178 passed.
- Task-specific unit file: 50 passed.
- Disposable SQL/Python qualification parity: 20 passed.
- Required `bash scripts/run_phase0_postgres_tests.sh`: 16 passed.
- Full unit suite: 2074 passed, 19 failed. Every failure is in
  `test_deploy_health.py`, with the known macOS Bash 3.2 `mapfile` limitation.
- Affected existing market PostgreSQL suites: 61 passed, no skips. Suites:
  `test_market_routes`, `test_market_support_routes`, `test_b100_orderbook`,
  `test_lng_reseller_admission`, `test_orderbook_map_summary` and
  `test_market_integrity_concurrency`.

API fixtures are in `test_phase0_catalog.py`, `test_inventory_publish.py`,
`test_orderbook_marketplace_filters.py` and `test_preferences.py`. They cover
create/amend refusals, direct takes, automatic-match and assisted post-only
qualification, inventory publication, public asks/facets/counts/map/curves/demand,
benchmark exclusion, delivery tuples, quantity minimum and all-eight preference
round trips. Production direct-take and public-read tests were reused against
staging fixtures. Zero lifecycle CI remains valid; NULL CI does not. The
PostgreSQL test evaluates the same metadata predicate on real PostgreSQL.
The unchanged compliance suite proves no inferred cash savings are restored.

## Minimal scope deviations and integration notes

- `execution_policy.py`: only imports, two scalar biofuel helpers and the ASK
  qualification block changed. No KYC, party-admission, FAME or LNG helper was
  replaced. The integration owner must retain these small hunks when merging
  Task 3; there is no Task 3 implementation in this lane.
- Additional market files: `routers/trades.py` restores the production refusal
  of an undeclared supplier direct take of a B30/B100 BID; `routers/availability.py`
  adds the shared scalar predicate; `routers/demand.py` restores product-name
  filtering. These are in Task 2's market-router lane but absent from its file list.
- `services/demo_activity.py` needs seven production lines so added biofuel demo
  coverage carries the fixed specification and whole-fuel metadata. No live
  seed runner was invoked and no LNG/UCOME seed evidence was added.
- Existing catalog/count/depth tests now expect eight products and six seeded
  products. The bounded opposing-slice SQL budget increases from 10 to 12 KiB
  for the two catalog cases and fixed qualification predicate; duplicate-input
  SQL and bind-parameter equality remain required.
- `ARCHITECTURE.md` has only product-policy documentation changes. Keep them
  when merging other lanes' architecture notes.
- Existing PostgreSQL tests use a temporary copy of the existing runner under
  `work/`, with pinned image, loopback ports and `docker rm -f -v` cleanup.
  Its automatic `alembic check` is omitted because union DDL is Task 4b's work;
  real migrations still run. No checked-in runner, migration or checkpoint
  policy was changed. Full union migration/drift acceptance remains Task 4b.

No package was installed in the shared venv. No push, live access, real data,
Skill tool, Agent tool or subagent was used. Temporary logs remain in `work/`.
