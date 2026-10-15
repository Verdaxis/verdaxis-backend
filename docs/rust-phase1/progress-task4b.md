# Task 4b — accepted non-MCP corpus

## Current disposition (decisions 18–20)

Accepted golden source: `a127672ee4c82fafbb4c633eb84f66bbdc676e75`. Nine groups, 82 cases per KYC state, 164 golden cases total. Both states completed two fixed-clock passes with `TOTAL_UNSTABLE=0; RECORD_FAILURES=0`. MCP is explicitly deferred to Task 11. The old golden was replaced; policy, MCP catalog artifact, manifest and catalog/terms vectors were regenerated from this source. The historical blocked-run notes below are retained as evidence, not current status.

News compares only the reviewed `/api/news` root array tie groups as multisets of exact item tokens, with an identical published-time key sequence. Changed/missing items, byte differences within items and wrong key order fail. Other paths remain exact bytes. The Rust SQL still orders `published_at DESC`.

Positive FAME/LNG declarations are Python-validated stored dumps. The malformed holder row is cancelled for positive collections and temporarily opened by the attested writer only for `T4B-negative-fame-holder`; its Python 500 is recorded separately, then the writer restores cancelled state in `finally`. All positive list cases now return 200 (invalid query cases retain 422). The Task 6 matrix still passes.

Implementation commits: `42af364` tie rule and decisions; `41e60d7` stored fixtures/MCP exclusion; `c6ca90c`, `b93d9a0` writer-environment/driver repairs; `a127672` negative projection scope. Publication: `ff760b2`. Test-only count alignment: `27b91b2` (five active products/seven active points in the populated catalog). No route code changed.

Health differences remain assigned to Task 13: HEAD `/health/live` status/headers and POST/OPTIONS 405 Allow values (four comparison paths). Corpus echo matches both cases; corpus simple_reads matches all 29 cases. These are the only implemented groups compared; the other six groups are Python goldens, not claimed Rust passes.

Final checks after publication: fmt/clippy pass; workspace 74 pass; PostgreSQL 81 pass; market matrix 2 pass; Python contract suite 59 pass. Initial regenerated-vector tests failed only because old hard-coded active-catalog counts were 6/8; test assertions were corrected to the fixture's 5/7 and both Rust suites rerun successfully. The regenerated projection itself already matched Python. No route behavior was changed to make these tests pass.

Logs: `work/rust-phase1/task4b-accepted-{enabled,disabled}.txt` and `task4b-final-*`. Each state's metadata records source SHA, KYC switch, fixed instant, schema and fixture counts. The rejected first attempts encountered missing writer environment/undeclared synchronous driver before the successful clean-source run; these were corrected using the existing async fixture writer.

## Historical first attempt (superseded)

## Scope and commits

- `3d81917`: fixture writer, parent-schema history and ten regression tests.
- `ac81bd87a157ef453b6ea1d21bccd87c1ebc0a8a`: corpus runner, bounded two-pass recording, explicit KYC setting and separate `T4B-` case entries.
- No route code, live host, live database, shared lane file or application policy was changed. No push.

## Construction

TDD: all ten `test_seed_corpus.py` tests failed before implementation; all ten passed afterward. The writer refuses non-test environments, non-numeric/non-loopback hosts, mismatched/non-test database names, superusers and any role other than the disposable `rust_owner` fixture writer. Missing or wrong counts fail closed. The runner uses Colima and the pinned PostGIS image, and removes its container and volume.

Corpus mode deliberately replaces the ordinary seed, rather than layering random ordinary rows onto exact fixture counts. Ordinary mode is retained separately. The current schema is upgraded through real Alembic migrations. No trigger, guard, constraint or session replication setting is disabled.

| Fixture class | Rows |
| --- | ---: |
| Ports (bare, one intelligence row, two intelligence rows) | 3 |
| Intelligence (duplicate rows flushed in fixed insertion order) | 3 |
| News (all six categories, tied dates, nullable summary) | 6 |
| Task 6 market matrix orders | 46 |
| Additional stored scientific-decimal orders | 4 |
| Historical order | 1 |
| Current market matrix trades | 6 |
| Historical trades | 2 |
| Signals (trusted and untrusted indication) | 2 |
| Verified ingestion run | 1 |
| Benchmarks | 0 |

Total orders: 51. Total trades: 8. Port coordinates retain `103.123456789123, 1.234567891234`. FAME CFPP and LNG methane-number fields contain Python-dumped `1E-29` and `0E-100`. Public region cases use Singapore/Asia and Rotterdam/Europe. The reused matrix includes REAL owners, ownerless expiring DEMO, registered TEST, UNKNOWN, rejected/moved/unverified users, grace boundaries, both KYC switch behaviors, assisted customers, inactive catalog rows, private typed terms and wrong-lane guard refusals. No demo KYC or LNG benchmark/demo quote is fabricated.

CANARY is not a legal fresh market fixture: the actual guard refusal is recorded. The historical mixed snapshot uses UNKNOWN/DEMO provenance through a deterministic demo organization present before integrity migration.

## Historical-version result

The legal parent revision is `miq_20260720_market_quarantine`. It has no market snapshot version column. Two confirmed trades are inserted there: an unlinked legacy trade and a linked quarantined mixed-provenance trade. The real integrity migration leaves their `market_snapshot_version` as SQL NULL. Literal version 0 is not the schema's legacy representation. Both rows survive the complete chain and are asserted absent from Python formal trade evidence. The current guard also refuses a new literal-version-0 trade. No invented version-0 row is inserted after migration.

This closes the legal-history construction gap, but does not manufacture version-1 UNKNOWN evidence that the migration does not produce. Task 9 must use these actual stored semantics when reviewing quarantine metadata.

## Recording gate

Candidate oracle SHA: `ac81bd87a157ef453b6ea1d21bccd87c1ebc0a8a`. Each run pins `KYC_EXECUTION_GATE_ENABLED` explicitly; recording metadata includes that state, fixed UTC instant, Singapore timezone, schema revision and fixture counts. Disabled-state golden case IDs would carry `--kyc-off`; normal case entries remain unchanged for lane merges.

**No new accepted golden is published.** The enabled trial found 100 unstable comparison paths in four news cases with tied dates. The Python route has no tie-breaker; changing its order, hiding ties or weakening exact-byte comparison is outside this task. MCP recording also fails because the stream does not finish within the bounded recording deadline. The runner now records diagnostic first/second passes under `work/`, continues other groups, and refuses golden publication unless every group is complete and stable.

The old `775a75c` golden, policy/MCP artifacts and manifest are retained together. They must not be relabelled with the candidate SHA. Artifact regeneration is blocked by the failed complete/stable gate, not silently skipped as success.

The reused shallow-valid matrix also exposes Python serializer failures: FAME ASK `nomination_status` is absent in some SQL-only fixtures; the intentionally typed-invalid holder case fails public projection. Those 500 responses are recorded as observed, not repaired in routes. This is not a claim that populated orderbook routes pass.

## Final verification on `ac81bd8`

All requested commands ran with a fresh `RELEASE_SHA`:

- fmt and clippy (`--workspace --all-targets -- -D warnings`): passed.
- Offline workspace: 74 passed.
- Disposable PostgreSQL runner: 81 passed, market matrix tests explicitly filtered for their dedicated runner.
- Python contract tests: 52 passed (including ten new fixture tests).
- Corpus `health`: four differences — `H04-live-head` headers/status, and `Allow` header values on `H05-live-post-405` and `H06-live-options-405`.
- Corpus `echo`: two cases matched, one integration test passed.
- Corpus `simple_reads`: 29 cases matched, one integration test passed. This one pair does not cure Python's repeated-recording instability.
- Both enabled and disabled recordings: health 8, echo 2, snapshot 3, lists 14, map 6, prices 9, legacy curves 4, canonical curves 6 had zero within-run unstable paths. Simple reads 29 had 100 unstable paths; MCP timed out. Thus `TOTAL_UNSTABLE=100; RECORD_FAILURES=1` in each state.
- Unported groups were recorded only, not claimed as Rust comparisons. Six list cases return Python 500: `R17-asks`, `R18-with-ci`, `R26-orderbook`, `R27-listings`, `T4B-region-Singapore`, `T4B-region-Asia`.

No accepted new golden SHA exists. Artifact regeneration was intentionally not performed because the required stable complete oracle does not exist. The candidate SHA and both-state diagnostic recordings remain available for the next owner. Final documentation/ledger changes do not modify tested source.

## Resume

1. Review the news tie-order contract with Task 8 and the PM; do not normalize away the difference in the comparator.
2. Supply bounded protocol-aware MCP recording before claiming every group was recorded.
3. Review SQL-only malformed declarations with orderbook consumer owners; keep deliberate negative cases separate from positive stored-form terms.
4. Record all groups twice for both KYC states from one clean source SHA. Require zero unstable paths and no recording failures.
5. Only then replace the old golden directory and regenerate policy, MCP artifact, manifest and vectors from that source.

Raw logs and recordings: `work/rust-phase1/task4b-*`, `work/rust-phase1/record-enabled/`, `work/rust-phase1/record-disabled/`. A partial early trial golden was moved to `work/rust-phase1/task4b-trial-partial-golden`; it is not accepted or committed as golden data.

## Open findings for Task 7b (recorded, not changed)

The 2026-10-16 recheck of Task 4b (`outputs/research/rust-task4b-recheck-2026-10-16.md`) found no BLOCKER and no MAJOR. Its other findings were fixed in the comparator and the runner. These two stay with Task 7b, so its list-route work owns the decision and the fix.

1. **Whitespace stripping between news items.** `_news_item_bytes` (`scripts/rust_contract/compare.py`) skips whitespace before each item, so `[A,\n B]` and `[A,B]` compare equal inside one tie group. Item-internal bytes and key order stay exact. Task 7b must decide whether inter-item whitespace is contract-visible before it relies on the news tie relaxation for any further case.

2. **Full ties in order-list fixtures.** The corpus sets every order's `created_at` to one instant (`scripts/rust_contract/seed_corpus.py`) and most prices and quantities are equal. With `sort_by=newest` and `price_asc`, list responses are therefore full ties, and decision 14 forces an exact order for them. This is a false-failure risk for Rust (a different Postgres plan order fails the comparison), not a false pass — both recorded passes were stable. Task 7b must give the list fixtures a deterministic tie-breaker, or record a reviewed ordered-object exception, before it claims those cases.
