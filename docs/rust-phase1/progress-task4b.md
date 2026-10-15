# Task 4b — corpus construction and blocked recording

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
