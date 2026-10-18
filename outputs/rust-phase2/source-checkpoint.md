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

## Resume point

Finish Task 1 precommit checks and commit only its inventory/exporter/test/checkpoint files. Then run Task 2's comparator RED/GREEN cycle and commit it separately. Do not begin the evidence gate, runtime ports, or Task 3.
