# Tasks 22 and 23 — checkpoint, 22 October 2026

## Status

The Task 22 review fixes, the Task 23 runtime projection and the two PM items
for this round (watchlist settings from release metadata; one payload order
rule) are committed. The final gate batch records its results in the section
below. No staging or production deployment was attempted. Task 24 has not
started. No measurement was run (decision 110).

## Commits in the review fix round

| Item | Tests first | Change |
| --- | --- | --- |
| Trusted client address through CRUD proxies | `366138b2`, `32681e55`, `30aae50b` | `0f6f5925` |
| Python-owned groups survive a Python restart | `5aec05a9` | `fb5cf024` |
| Bounded quota map, pruned once a minute | `86d77ab5`, `3335129f` | `eb303728`, `a586341a` |
| `RATE_LIMITED` refusals with `Retry-After` | `1451af71` | `4f4d776e` |
| Owner reached only after a marked 204 | `1dd6edec`, `03cfd23d` | `fe77f392` |
| Gate removal independent of snippet bytes | `01270e5b` | `dfaee46b` |
| CI: routing, PostgreSQL, scenarios and races | — | `8310d3ec`, `1ae790b9`, `54cbae9b` |
| Raw projection of the Phase 2 settings | `ac8b7933` | `4ed2e514` |
| Disposable route fixture under migrator authority | `1ae790b9` | `c58c8de6`, `1a0ac624` |
| Real Rust authority stops the 301st admin request | `c7c1f643` | (coverage) |
| One payload order rule for feeds and counts | `0f6cc48a` | `0a244ca8` |
| Watchlist activation only from release metadata | `a43a4405` | `86bfe072` |

Formatting commits: `75446614`, `22b2c2c6`, `e61ba52a`. Records: the plan
record, `ARCHITECTURE.md`, `docs/runtime-hardening.md` and the new
`docs/rust-phase2/release-runbook.md`.

## Gate results

Pending in this lane's final batch; see the lane report until this table is
filled.

## Remaining

- Push `feat/rust-phase-2` when every gate is green, then read PR #88 CI.
- `.codesight` refresh: not run in this lane (it needs the refresh skill).
- Task 24 (staging deploy and journeys) and Task 25 (production) run with the
  PM.

## Earlier steps

- `2be99321`: exact Caddy ownership for 25 operations in six groups, independent
  rollback, Python fallback, pre-auth ingress, and real-Caddy tests.
- `0fe0fb02`: compose the Phase 2 routers, the shared quota authority and the
  private runtime configuration. W09/W10 stay on Python.
- `dda3030a`: merge `origin/staging` at `274bd3ab` without conflict (PR #115's
  certifi change; this lane did not edit the constraint).
- `108480a4` merged PR #111; `293af2c3` records PD6-2 (producer in Phase 8, the
  PD4-G guard unchanged in Phase 2).
- The Phase 1 meaning gate passed with `heavy run 7200` at `dcc6eb9d`
  (`MEANING_EXIT=0`).

## Plan corrections

See "Tasks 22 and 23 — review fixes and plan corrections" in
`docs/rust-phase2-plan-record.md`: 25 operations instead of 27; the
`disposable-artifact` report and the Phase 2 `--mixed-caddy` runner mode are
replaced by the Rust-native evidence (decisions 112 and 113); the runbook lives
under `docs/`; no measurements.
