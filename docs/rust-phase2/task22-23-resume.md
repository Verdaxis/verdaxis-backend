# Tasks 22 and 23 — checkpoint, 23 October 2026

## Status

The Task 22 review fixes, the Task 23 runtime projection and the two PM items
for this round (watchlist settings from release metadata; one payload order
rule) are committed. The final gate batch records its results in the section
below. The final review's F1/F2 gate repairs are committed and qualified in
the "Review fix round" section. No staging or production deployment was
attempted. Task 24 has not started. No measurement was run (decision 110).

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

## PD2-L1 follow-up and final qualification

The earlier failures above are superseded by these results. Implementation
revision: `f770aa3dc3e744f841584031ba727cd2aa39e77e`. Documentation-only commits
after that revision do not change the qualified code.

- `27ae9778`: separate Codesight refresh, committed before gate calls.
- PostgreSQL ordering test `5c9ad00d`, SQL fix `7ef9d6ee`, formatting
  `9e82f4d8`: Rust orders by count descending, then referrer UUID ascending,
  before taking ten. The migrated disposable test includes ties inside the
  list and at the cut, and asserts every returned entry in exact order.
- Comparator tests `b67df0a7`, `4cdee293`, change `39bd0e44`: only successful
  GET R03 `/api/referrals/leaderboard` permits undefined Python tie order.
  Counts match rank by rank; complete groups retain their member sets; the
  final group in a full ten retains its count and distinct members. Status,
  headers, fields, types and consecutive ranks remain checked. No Python
  application code changed.
- Race barrier tests `bac6f9ff`, fix `f770aa3d`: allocation previously watched
  hard-coded scenario-zero databases even when selected ninth. The runner now
  supplies the current scenario's database identities. This was a harness
  failure, not a runtime allocation failure.

| Evidence | Revision | Host / UTC on Oct 23 | Exit |
| --- | --- | --- | --- |
| Comparator RED / GREEN | `4cdee293` / `9e82f4d8` | M1 00:44:23–25 | 1 (two intended failures) / 0 (40 passed) |
| PostgreSQL ordering RED / GREEN | `5c9ad00d` / `9e82f4d8` | staging 03:09:17–03:16:24 | 1 (exact order assertion) / 0 |
| Race-cell RED / GREEN | `bac6f9ff` / `f770aa3d` | staging 06:04:19–33 | 1 (two intended failures) / 0 (28 passed) |
| fmt, both Clippy modes, offline workspace | `f770aa3d` | staging 06:53:58–07:06:29 | 0 each; 400 passed, one ignored |
| `cd rs && cargo audit` | `f770aa3d` | staging 07:06:29–34 | 0 |
| `bash rs/read/tests/run_postgres.sh` | `f770aa3d` | staging 07:06:34–07:17:20 | 0 |
| All Phase 2 scenarios | `f770aa3d` | staging 07:17:20–07:58:52 | 0 |
| Standalone races, `heavy run 7200` | `f770aa3d` | staging 06:05–06:40:29 | 0; nine scenarios, 27 races |
| Phase 1 meaning, `heavy run 7200` | `f770aa3d` | staging 08:00–08:52:05 | 0; no failed groups |

Local logs: `work/pd2-l1-m1.log`, `work/pd2-l1-staging.log`,
`work/pd2-race-red-green.log`, `work/pd2-races-final.log`,
`work/pd2-final-batch.log`, `work/pd2-meaning-final.log`.
The M1 audit at `9e82f4d8` failed to fetch the advisory database; both subsequent
staging audits passed. The prices group, including `R13-invalid-visibility`,
passed in both meaning runs; no price fix was required. Full Python unit and
monitor tests were not repeated in this follow-up: no application Python or
shared Python inputs changed (PM instruction). Their preceding green result
is retained above.

## Review fix round (F1/F2) — 23 October 2026

The final review's two remaining gate defects are closed at
`9ccb9328fd1c86ee5592d283d4fa4ab0a1ec3796`:

- **F1 (major), order-write cache evidence.** Tests `42f11ca0`, fixes
  `533d625c` and `9ccb9328`. `phase2_order_write.py` deep-copies `before`,
  applies `expect_cache_invalidations(expected, int(changed))`, then compares
  both complete cache table objects with `after` — the referral
  implementation's exact pattern. Outbox and receipt checks are retained.
  `tests/rust_contract/test_order_write_cache_state.py` rejects a wrong new
  event key or value, deletion or mutation of an old event, a different
  domain, column damage, extra generation rows, and replay metadata mutation
  for both replay actions.
- **F2 (minor), leaderboard rank type.** Tests `42f11ca0`, fix `533d625c`.
  `phase2_scenarios.py` requires `type(entry["rank"]) is int` before the
  consecutive-value check. `tests/rust_contract/test_leaderboard_ties.py`
  refuses Boolean, float and string ranks. The PD2-L1 exception stays limited
  to successful GET R03 at `/api/referrals/leaderboard`.

Decision 126 order: RED tests committed first at `42f11ca0` (12 intended
assertion failures against the prior gate code in
`work/pd2-review-fix-batch.log`), then the fixes.

Rust source is unchanged since `f770aa3d` (`git diff f770aa3d..9ccb9328 -- rs/`
is empty), so the recorded fmt, clippy, offline, Rust PostgreSQL and Phase 1
meaning results at `f770aa3d` carry forward. `cargo audit` and PR #88's Rust CI
still report RUSTSEC-2026-0385 (`ctutils` 0.4.3), inherited; `Cargo.lock` is
unchanged.

Gate evidence at `9ccb9328` (staging test host; UTC on Oct 23):

| Gate | Evidence | UTC | Exit |
| --- | --- | --- | --- |
| RED tests | `work/pd2-review-fix-batch.log` | 14:09:42–46 | 1; 12 intended failures, 11 passed |
| Harness GREEN (76 tests) | `work/pd2-review-fix-batch3.log` | 16:49:41–53 | 0 |
| All Phase 2 scenarios | `work/pd2-review-fix-batch5.log` | 18:06:33–18:50:30 | 0; 21 scenarios, 367 actions |
| Standalone races | `work/pd2-review-fix-batch3.log` | 16:56:29–17:27:08 | 0; 9 scenarios, 27 races |

Two intermediate scenario attempts failed on host events, not assertions: the
first (`pd2-review-fix-batch3.log`, 16:49Z) on a transient harness startup
error (`nothing accepted a connection on 127.0.0.1:56183`) under heavy
concurrent load, the second (`pd2-review-fix-batch4.log`, 17:34Z) when the
test host rebooted mid-run (exit 143). The rerun after recovery passed all 21
scenarios.

## Remaining

- F1/F2 are closed at `9ccb9328` and the review-fix gates are green (see the
  "Review fix round" section). Pushed `feat/rust-phase-2` at `9ccb9328` on
  Oct 23 after those gates. PR #88 CI finished: backend `test`,
  `postgres-analytics`, and `postgres-phase0` passed (run `39932402006`).
  Rust `offline` failed in the dependency advisory step before its test/build
  gates; `contract` and `publish` were skipped (run `39932402025`). The new
  Oct 23 advisory is RUSTSEC-2026-0385 in `ctutils` 0.4.3; the audit
  recommends >=0.4.4. Earlier test-host audits passed before this advisory
  appeared. No advisory suppression or dependency update was made in this
  round. PM follow-up: coordinate the dependency update, then rerun Rust CI.
  Local failed-step log: `work/pd2-ci-failed.log`.
- Task 24 (staging deploy and journeys) and Task 25 (production) remain with
  the PM. Neither task started; no live deployment or measurement ran. The PM
  first reconciles Task 24's scope with decisions 110, 112 and 113 before it
  starts.

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
