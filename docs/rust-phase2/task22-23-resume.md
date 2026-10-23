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

Continuation evidence at `a8b8f77de0f679d74b8f3656e6b519ebe89618a8`
(staging test host; UTC dates below). No push is authorized by these results:
not all gates are green.

| Gate | Command / evidence | UTC | Exit |
| --- | --- | --- | --- |
| Referral cache regression RED | Tests-only `2d530e8c`; preload `phase2_scenarios`, then pytest `tests/rust_contract/test_referral_cache_state.py` | Oct 22 21:15:31–35 | 1; four intended assertion failures, 16 passed |
| Referral cache regression GREEN | `venv/bin/python -m pytest tests/rust_contract/test_referral_cache_state.py -q` | Oct 22 21:15:35–38 | 0; 20 passed |
| fmt, Clippy (plain and contract-tests), offline workspace | `cargo fmt --all -- --check`; Clippy with `-D warnings`; `cargo test --locked --workspace`, all with `--manifest-path rs/Cargo.toml` | Oct 22 21:15:38–21:50:39 | 0 each; 400 passed, one ignored |
| Full Python unit and monitor | `venv/bin/python -m pytest tests/unit tests/monitor -q -p no:warnings` | Oct 22 21:50:39–22:03:30 | 0; 2839 passed, one skipped |
| Runner and regression tests | pytest `test_phase2_scenarios.py`, `test_phase2_runner.py`, `test_referral_cache_state.py` under `tests/rust_contract/` | Oct 22 22:03:30–35 | 0; 51 passed |
| Audit | `cd rs && cargo audit` | Oct 22 22:03:35–43 | 0 |
| Rust PostgreSQL | `bash rs/read/tests/run_postgres.sh` | Oct 22 22:34:33–22:59:13 | 0 |
| Caddy and routing/runner tests | pytest `test_routing.py`, `test_phase2_caddy.py`, `test_phase2_report.py`, `test_phase2_scenarios.py`, `test_phase2_runner.py`, `test_route_jit_policy.py`, `test_referral_cache_state.py`, and `tests/unit/test_rust_caddy_candidate.py` | Oct 22 22:59:13–23:04:34 | 0 |
| All Phase 2 scenarios | `venv/bin/python -m scripts.rust_contract.run_disposable --phase2 --phase2-group all --report /tmp/opus-r2-phase2-scenarios.json` | Oct 22 23:04:34–Oct 23 00:04:21 | 1; populated leaderboard tie case differs |
| Phase 2 races | Registered route-qualified race scenarios, `run_disposable --phase2 --phase2-races` | Oct 23 00:04:21–00:34:40 | Incomplete; enclosing batch exit 124 |

Local logs: `work/sol-p2-unit-batch-3-recovered.log` and
`work/sol-p2-linux-batch.log`. Remote detailed logs remain in
`/home/verdaxis-test/runs/rust-phase2/work/sol-*.log` and
`/tmp/opus-r2-phase2-{scenarios,races}.log`. The combined Linux call used
`heavy run 7200`; it exhausted that total budget during races. This is not a
race assertion failure. No Phase 2 unit or child process remained at the
post-run check.

The previous referral allocation failure was fixed by accounting for the exact
transaction-owned cache generation and event row, not by excluding metadata.
Tests: `2d530e8c`; implementation: `3d220846`, `2181e4a1`; corrected test loading:
`fee69c5c`, `9dd9cee3`, `a8b8f77d`. Earlier collection-error runs are superseded,
not RED evidence. Staging `fbe0b2c1` was merged without conflict in `9afea6dc`.
The M1 full-Python run is also superseded by the staging result above.

## Remaining

- Push `feat/rust-phase-2` when every gate is green, then read PR #88 CI.
- Diagnose `referral-reads-populated` / `top-ten-with-ties-across-the-boundary`:
  the harness still requires Python's arbitrary membership at the tenth-place
  tie. Check both payloads and client/business assertions before changing the
  harness; do not change Python or treat byte equality as acceptance.
- Rerun the incomplete races separately within a bounded call; do not repeat
  passed gates unless a fix touches them. The last race log was still in
  preferences schedules when the combined budget expired.
- Run the final-SHA Phase 1 meaning gate once with `heavy run 7200`; it has not
  run on this continuation's SHA. Python PostgreSQL and Phase 0 last passed at
  `dabf9799`, not the current SHA.
- `.codesight` refresh completed via the skill. Generated changes are preserved
  but not yet committed or reviewed for publication.
- PR #88 still points to `c078f1e7`; no push was made in this continuation.
  Existing CI is not evidence for the local commits.
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
