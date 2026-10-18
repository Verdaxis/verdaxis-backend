# Phase 2 source checkpoint and progress

## Scope and source

- Authorized scope: Tasks 1 and 2 only. Task 3 and all later tasks remain unstarted.
- Worktree: `work/rust-phase2`, branch `feat/rust-phase-2`.
- Python oracle: `573c962de8bbac857b339da49b61f4de6fff4e10`.
- Plan source: `a62b6b7179d4763d6c9346d0aa24faf0e4a5a577`.
- The six router files are unchanged between those two commits.
- `tests/rust_contract/phase2/routes.json` pins source SHA-256 digests, full HTTP OpenAPI operations, referenced component schemas, preference namespace schemas, and manual response shapes for untyped handlers.
- Regenerate only after reviewing an oracle change: on the locked test host run `venv/bin/python -m tests.rust_contract.phase2.export_inventory --write`. Copy the generated fixtures back without running project code on the Mac.
- This is source evidence, not a live release observation. No staging or production app/database was accessed.
- Schema/checkpoint source is pinned by the digest of `deploy/migration-checkpoints.tsv`; no database revision or ACL convergence result is claimed. Those identities must come from later disposable evidence.

## Pinned route inventory

| Group | Routes | Reads | Mutations |
| --- | ---: | ---: | ---: |
| Notifications | 4 | 2 | 2 |
| Preferences | 2 | 1 | 1 |
| Feedback | 2 | 1 | 1 |
| Watchlists | 11 | 3 | 8 |
| Alerts | 3 | 1 | 2 |
| Referrals | 5 | 3 | 2 |
| Total | 27 | 11 | 16 |

No method/path or count difference from section 4. Public R05 and administrator F02 are included. Automatic redirects, OPTIONS, HEAD failures and rejected state-changing GETs are scenario coverage, not extra declared routes. Auth issuance, SSE/dispatcher ownership, alert triggering and referral progression remain excluded.

`effects.notifications=unchanged` means no new notification rows; N03/N04 may update only existing rows' read state, as recorded separately. The inventory does not establish SQL equivalence or full middleware outcomes for each CRUD request. It checks the full HTTP OpenAPI export without lifespan/startup and without a database connection.

## Known source dispositions

Keep watchlist visibility and malformed stored PIN-window acceptance blocked until the separate Python repair lane is accepted and the oracle is regenerated. Do not repair them in Rust alone. The alert-cap race remains for Task 16 to characterize. Decision 78 permits exact shared per-IP limits instead of Python's accidental per-worker amplification. No source repair is part of these tasks.

## Task 1 — inventory

- RED: missing pinned inventory/manifest, exit 1. Current test file: 5 failed, including the absent exporter. Log: `work/rust-phase2/task1-red-current.log`.
- GREEN: exporter plus exact inventory command, exit 0; 5 passed. Log: `work/rust-phase2/task1-green.log`. Eleven existing Pydantic class-config deprecation warnings occur when importing the application.
- Review: module ownership selects every operation mounted from the six routers, so an added/renamed operation cannot escape a path-prefix filter. Exact OpenAPI equality checks query/body/response export drift. Source digests also catch validators, helpers, model and middleware changes that OpenAPI does not describe.
- The evidence manifest pins required cases and separate disposable-artifact/production-cutover scopes. It is a requirements template with `results=not-run`, not a candidate verdict. Task 3 owns validation and evaluation.
- Precommit Rust format: exit 0. Literal plan Clippy invocation: exit 101 (`build requires RELEASE_SHA: NotPresent`). Corrected invocation with `RELEASE_SHA=$(git rev-parse HEAD) SQLX_OFFLINE=true`: exit 0. Logs: `work/rust-phase2/task1-rust-checks.log`, `work/rust-phase2/task1-clippy-identity.log`.
- Inventory SHA-256: `7bbdf11714b8eac3d598b2ae3ad5d00fb81e598e50df9423020ae762d1148b1a`.
- Manifest SHA-256: `2cfb207e7049e8d872cd7361d304aca5d80dd21f72fb1d37c0b6a9619442574d`.
- Additional files beyond the task list: `phase2/export_inventory.py` is the reproducible HTTP/schema export helper; `ARCHITECTURE.md` records its relationship as required by project instructions. No application behavior changed.

## Task 2 — explicit comparator profile

- Entry point: `scripts.rust_contract.compare.compare_responses`. Callers supply raw status, headers and body bytes, explicit profile `phase2-meaning-v1`, stable route ID, and the reviewed rule-ledger digest. No profile keeps the Phase 1 byte gate. Existing recording behavior is unchanged.
- Private implementation: `scripts/rust_contract/phase2_meaning.py`. This new file keeps most code out of the shared comparator. `tests/rust_contract/phase2/README.md` documents the interface and rule boundary.
- Normalization ledger SHA-256: `08e9165617b804704a35fa46685f90ca310bd3f0985c61797ddad20c706dfee4`. A caller's matching digest alone cannot authorize an unreviewed ledger: the implementation pins it too.
- RED: exit 1; 40 failed for missing explicit profile dispatch, 17 existing tests passed. Log: `work/rust-phase2/task2-red.log`.
- Initial GREEN attempt: exit 1; 3 failed, 54 passed. Two test inputs lacked required timestamp/JSON content type; one exposed the need to report cross-key movement as a contract difference rather than invalid evidence. These were corrected without weakening the required assertions. Log: `work/rust-phase2/task2-green.log`.
- GREEN: exit 0; 57 passed. Log: `work/rust-phase2/task2-green-fixed.log`.
- Final expanded regression gate: exit 0; 114 passed, 11 existing Pydantic warnings. Includes inventory, Phase 2 comparison, default Phase 1 comparison, and existing news/trade-tape/price-summary tie tests. Rust format exit 0; identity-pinned Clippy exit 0. Log: `work/rust-phase2/task2-final.log`.
- Every host call used the measurement wait, shared lock (`-w 7200`) and `timeout --kill-after=60 3600`. No project code ran on the Mac. No call was interrupted, and all calls finished.

### Cases that prevent hidden differences

Exact numeric values preserve bool/number separation and large-number precision. Decimal normalization is restricted to the two alert response paths, with string wire types retained. Typed datetime paths compare exact microseconds; arbitrary preference/notification/event JSON and opaque cursors remain exact. Null versus absence, private `_email_` fields, error messages/locations/input values, array order, status, header values/absence, content length and supplied request-ID round trips are tested.

Tie tests retain every item field, rank, multiplicity and page member. They fail dropped, duplicated, changed, cross-key or substituted members; even matching incorrectly sorted arrays fail. Only W01 entries are fully unordered. Nested slice/PIN ties normalize only at their exact paths. Invalid UTF-8, duplicate keys, nonfinite JSON, invalid typed strings, unknown profiles/routes, wrong digests and widened paths are invalid evidence.

### Review and limits

One comparator authority, one opt-in profile, one pinned ledger. No new harness lifecycle, runtime service, schema, migration, database role or application feature. Shared edits are the small `compare.py` dispatch, the Phase 1 separation regression and required architecture documentation. The gate lane's report/measure/Rust files and the speed lane's SQL/runtime files were not changed.

This is comparator and inventory unit evidence only. No twin-database state, token acceptance, full CRUD middleware matrix, cursor execution, performance, signed artifact, staging or cutover result is claimed. The evidence manifest is still `not-run`. Task 3 and all later tasks remain unstarted.

## Resume point

Task 1 commit: `95d31f6f` (`test: pin Phase 2 CRUD contract inventory`). Task 2 is committed separately as `feat: add explicit Phase 2 meaning comparator`. Commit attribution was updated to the user's requested trailer; tested file bytes did not change. Stop here. The PM must reconcile the Phase 1 gate lane before authorizing Task 3. After the approved Python watchlist fixes merge, regenerate the pinned oracle and review changed digests before accepting the Rust port.
