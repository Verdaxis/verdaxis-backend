# Tasks 22 and 23 — checkpoint, 23 October 2026

## Status

The Task 22 review fixes, the Task 23 runtime projection, the two PM items
for that round (watchlist settings from release metadata; one payload order
rule) and PD2-SEC-1 are committed. The final gate batch records its results in the section
below. The final review's F1/F2 gate repairs are committed and qualified in
the "Review fix round" section. No staging or production deployment was
attempted. Task 24 has not started. No measurement was run (decision 110).

## PD2-M3 — staging merge, gates and the meaning entry condition, 24 October 2026

Staging `b1106280` (PR #124 audit policy, PR #125 meaning-oracle retry) merged
with `--no-ff` into `1b3f983df`; the merge commit is `5d52c6b6b`. The only
conflict was `rs/read/src/lib.rs`, where both sides added fields and
re-exports; both were kept and no assertion was dropped.

The merge exposed three real defects, each fixed on its own commit:

- `1b3f983df`: the alert contract adapter built `AppState` by literal and
  missed the staging cache fields; it now uses `AppState::new`.
- `8d0b82271`: four merged test `Config` literals lacked the Task 23 `phase2`
  field. Both are compile errors, not behaviour changes.
- `2d6faf3fa`: staging's `trading_cache_postgres` integration test updates
  order-book rows to prove cache invalidation, which the Task 20 boundary scan
  forbids. The scan now names that one integration test as its only DML
  exception, and a new test asserts the exception is under `rs/read/tests/`.
  No production source and no other assertion changed.

Gates at `2d6faf3fa` (`fd659a5` records them):

| Gate | Host / UTC | Exit |
| --- | --- | --- |
| fmt; both Clippy; `cargo check` | M1, 13:13–13:20 | 0 each |
| Offline workspace | M1 | 0; 440 passed, one ignored |
| `cd rs && cargo audit` | M1 | 0 |
| Rust PostgreSQL workspace | prod test host, 13:21–13:32 | 0; 450 passed, one ignored |
| All Phase 2 scenarios | prod test host, 13:32–14:04 | 0; 21 scenarios, 367 actions |
| Standalone races | prod test host, 14:04–14:20 | 0; 9 scenarios, 27 races |
| Python unit, monitor, rust_contract | prod test host, 16:49–16:58 | 0; 4774 passed, two skipped |

The first Python run failed one case, `test_main_mount_and_lifespan`, because
that gate invocation omitted `DATABASE_URL`, so the lifespan dialed a real
PostgreSQL. The documented environment (`DATABASE_URL=sqlite+aiosqlite:///:memory:`)
passes the same tree: the first combined run at staging's own parent also
passed 3731 tests with that variable. This was a harness invocation error, not
a code defect. `work/pd2-m3-python-bad-env.log` retains the failed run.

Meaning gate (PD2-M3), once, on the same `2d6faf3fa`, on the prod test host:
**MEANING_EXIT=0** at 17:19:16Z. All ten groups passed with a complete
assertion ledger. **Zero `ORACLE_RETRY` and zero `ORACLE_UNAVAILABLE`** lines,
so the PR #125 retry path was not exercised and no oracle was ever unavailable.
The host was checked first for the harness: the disposable Podman docker shim,
PostgreSQL through it, Caddy 2.11.2, and the project venv with `alembic`.

Evidence: `work/pd2-m3-m1.log`, `work/pd2-m3-prod.log`,
`work/pd2-m3-python.log`, `work/pd2-m3-python-bad-env.log`,
`work/pd2-m3-meaning.log`.

Pushed as a fast-forward to `6af28cc22`. PR #88 CI then passed: `offline`,
`contract`, `test`, `postgres-analytics` and `postgres-phase0`; `publish` skips
on a feature branch, as before. The first `postgres-phase0` run failed one case,
`test_host_assertions_postgres.py::test_assertion_cli_runs_all_database_checks`
(`rollback-safe: refused`); neither merge parent changed that test or
`scripts/phase0/assert_host.py`, and staging's own CI passed at `b1106280`. The
failed job passed on rerun, so the first failure was a runner fault, not a code
defect, and no source changed for it.

## PD2-SEC-1 — pre-auth quota cannot lock out new clients, 24 October 2026

The automated review of `373dc817` found that one 50,000-bucket map, keyed by
client address, refused **every** new identity once full. An address flood
therefore locked out legitimate clients, administrators included. Fixed at
`9a8639576da12194bbd6e9bd1e26f93f9cb7bcb5`; not pushed, pending independent
security review.

- **Authenticated quotas** (feedback, invite, preferences, admin feedback) are
  keyed by verified user ID, in their own store. Every limited endpoint
  requires a verified user; none is reachable unauthenticated, so no limited
  route needed address keying.
- **Pre-auth `/api/admin/` quota** keeps its own store, keyed by client
  address; IPv4 per address, IPv6 grouped to its /64 (IPv4-mapped spellings
  fold to the IPv4 bucket). A full table **evicts the soonest-expiring**
  entry, which is the oldest accounting because both windows are uniform, and
  counts it in a number-only warn log. The user store never evicts.
- **Client derivation** is `runtime/http.rs::client_ip`: the forwarded hop is
  read only when the socket peer is loopback, and then only its last element;
  a direct caller cannot choose its key. `rate_limit_is_per_client_and_route`
  (`rs/read/tests/http_contract.rs`) pins that a non-loopback peer's header is
  ignored. Caddy sets `X-Forwarded-For {client_ip}` on every group proxy.

Tests first: `10df338d3` is tests-only and fails on **assertion**, not compile,
against `373dc817` — 4 of 8 cases red (`preauth_flood_admits_…`,
`preauth_flood_does_not_consume_user_quota`, `ipv6_clients_share_one_prefix_bucket`,
`ipv4_mapped_ipv6_shares_the_ipv4_bucket`); evidence `work/pd2-sec1-red.log`.
Green at the fix: `work/pd2-sec1-green.log`.

Two adapters needed a change, found by running the scenarios rather than by
inspection: user keying made one scenario user exceed 5/min (feedback) and
30/min (preferences) where rotating addresses had hidden it. Proven first:
`work/pd2-sec1-scenario-red.log` shows `schema-page-url: unexpected HTTP
status 429`. `feedback_contract` and `preferences_server` now build their
quota authority per request, so the scenarios stay route oracles; quota is
owned by the quota contract file and the PostgreSQL route tests.

| Gate at `9a8639576` | Host / UTC | Exit |
| --- | --- | --- |
| fmt; Clippy plain; Clippy contract-tests; `cargo check` | M1, 11:59:47–12:02:32 | 0 each |
| Offline workspace | M1 | 0; 407 passed, one ignored |
| `cd rs && cargo audit` | M1 | 0; `rs/.cargo/audit.toml` now staging's (PD-SEC-2), so both inherited IDs are suppressed |
| Rust PostgreSQL workspace | prod test host, 12:03:48–12:13:16 | 0; 417 passed, one ignored |
| All Phase 2 scenarios | prod test host, 12:13:16–12:38:31 | 0; 21 scenarios, 367 actions |
| Standalone races | prod test host, 12:38:31–12:51:55 | 0; 9 scenarios, 27 races |

Evidence: `work/pd2-sec1-m1.log`, `work/pd2-sec1-prod.log`. The meaning gate
was not required for this round (PM). No Mac build or test was run.

## PD2-M2 — stricter load wait and one unchanged rerun

On Oct 24 the PM classified the earlier failures as Python-oracle timeouts
under host load and authorized one more unchanged run after a stricter load
wait. Exact source tested: `2b719ad5c38529c0a776bb08cdee80958775c079`.
No code, test, deadline, tolerance or retry policy changed.

Five-minute polling began at 03:37:39Z and qualified at 07:20:13Z (45 readings,
about 3h43m, within the six-hour cap). Every reading is retained in
`work/pd2-m2-load-wait.log`; final load was **7.51 / 9.52 / 12.08**. The
wrapper/heavy startup took about one minute. At actual gate start 07:21:13Z,
load had risen to **15.88 / 11.21 / 12.48**, above the strict limits again.
This is an execution limitation: the load was not checked again as an
admission condition inside the heavy slot, so this is not quiet-host evidence.
No second run was started.

The unchanged `bash opus-r2-meaning.sh`, under `heavy run 7200`, ended at
07:52:14Z with **MEANING_EXIT=1** and **CALL_EXIT=1**. After-run load was
**37.94 / 29.14 / 21.01**. Exact final failure lines:

```text
contract_server ready group=orderbook_map addr=127.0.0.1:57295
disposable run failed: nothing accepted a connection on 127.0.0.1:50347
MEANING_EXIT=1
```

The Rust listener was ready. `run_disposable.py` starts both adapters, waits
for Python first (`wait_for_port(python_port)`), then Rust. The failure here
is the Python-oracle startup wait: no request/response was captured for that
group, no Rust response differed in meaning, and no assertion established a
runtime defect. The log does not establish why the Python listener did not
start; elevated load is observed context, not a proven process-level cause.
This is a Python-side harness startup timeout, not the previous statement
cancellation or 503. The run reached this group after the earlier groups;
there is no complete meaning report and no full-pass claim.

Evidence: `work/pd2-m2-meaning-rerun.log`, recovered full log
`work/pd2-m2-meaning-detail-recovered.log`, and the load-wait log above.
No push, CI dispatch or further rerun. Stop for PM harness disposition.
Local `rs/target` is absent; Task 24 remains unstarted.

## PD2-M1 — single unchanged rerun, 24 October

PM classified the preceding cancelled-order setup failure as Python database
contention, not a Rust meaning regression, and authorized one unchanged rerun.
No source, test, deadline, tolerance or retry policy was changed.

- Load-wait check at 02:39:03Z: `load average: 15.60, 33.82, 57.54`.
  The first reading met the below-16 condition, so no five-minute poll was
  needed. Immediately before execution at 02:39:44Z it had risen to
  `16.00, 31.67, 55.81`; this boundary reading is disclosed, not rounded down.
- Executed unchanged `bash opus-r2-meaning.sh` at exact `2b719ad5`, under
  `heavy run 7200`, 02:39:44–03:33:38Z. Gate and wrapper exit **1**.
- After execution: `load average: 17.61, 19.22, 19.23` at 03:33:38Z.
- Before/after `lslocks --notruncate --output PID,COMMAND,PATH` filtered for
  `runs/.lock-` returned no matching rows. This is the observed output, not
  proof that no other session held a lock (lock visibility may be limited).

The failed group was `live_benchmarks`; all ten other groups passed. The
cancelled-order integration failure did not recur. Exact retained failure
excerpts (full private log: `work/pd2-m1-meaning-detail-recovered.log`):

```text
contract_server ready group=live_benchmarks addr=127.0.0.1:59637
{"event": "KYC execution gate: enabled", "level": "info", "timestamp": "2026-10-24T02:59:24.015394Z"}
ERROR:    Exception in ASGI application
asyncpg.exceptions.QueryCanceledError: canceling statement due to statement timeout
```

The Python traceback is in `tests/rust_contract/live_benchmarks.py:50`
(`project`) → `app/services/live_benchmarks.py:144`
(`_calculate_live_slice_benchmarks`). Its SQL is a SELECT over eligible
orderbook rows joined to products/delivery points, not a write or a recorded
lock-holder statement. The subsequent capture and Rust test lines are:

```text
contract group live_benchmarks OK: 4 case(s)
test live_benchmarks_match_python ... FAILED
thread 'live_benchmarks_match_python' (888231) panicked at contract/tests/live_benchmarks.rs:14:5:
live benchmark matrix differs
TimeoutError: timed out
scripts.rust_contract.compare.ContractError: case acceptance-true-base could not be sent
error: test failed, to rerun pass `-p verdaxis-contract --test live_benchmarks`
contract target live_benchmarks: FAILED
```

**Failing response:** none completed for `acceptance-true-base`; the Python
HTTP read timed out. No exact status, body or headers for that failed capture
exist in the retained log. There is no matching Rust HTTP response line for
that case; the Rust contract-test failure above is the matching retained
consumer evidence. No contention-503 response or message was observed in this
failure sequence. The logs do not establish a blocking session, and no new
instrumentation was added. Do not infer a lock holder or claim a matching
response that was not captured.

Logs: `work/pd2-m1-load-wait.log`, `work/pd2-m1-meaning-rerun.log`, and the
recovered detail log above. No further rerun or push is authorized after this
failure; stop for PM disposition. Task 24 remains unstarted; local `rs/target`
remains absent.

## Staging merge qualification — 24 October

Merged exact staging `539fe2a2e7735a301955e71d6797e3c565a11112` with
`--no-ff` in `2b719ad5c38529c0a776bb08cdee80958775c079`. No conflicts,
assertion removal, test-attribute removal, or branch-specific re-pinning was
needed. PR #122's inventory/digest updates were retained unchanged. The merge
includes ctutils 0.4.4 and PR #120's meaning-runner changes.

| Gate at `2b719ad5` | Host / UTC | Exit |
| --- | --- | --- |
| fmt; Clippy plain; Clippy contract-tests | this Mac, Oct 23 20:16:04–20:17:57, decision 134/shared lock | 0 each |
| Offline workspace | staging, Oct 23 20:41:34–21:02:06 | 0 |
| `cd rs && cargo audit` | staging, Oct 23 20:33:17–24 | 0 |
| Rust PostgreSQL workspace | staging, Oct 23 21:02:06–21:20:49 | 0 |
| Python unit, monitor, rust_contract | staging, Oct 23 21:20:49–21:46:46 | 0; 4739 passed, two skipped |
| All Phase 2 scenarios | staging, Oct 23 22:58:30–Oct 24 00:10:18 | 0; fresh report `work/merge-scenarios-resume2.json` |
| Standalone races | staging, Oct 23 21:52:36–22:38:29 | 0; nine scenarios, 27 races |
| Changed Phase 1 meaning gate, once | staging, Oct 24 00:10:18–00:17:32 | 1; disposable integration stage, cancelled-order trade case |

Meaning failed at
`tests/integration/test_trades.py::TestCreateTrade::test_cannot_hit_cancelled_order`
(`tests/integration/test_trades.py:84`), with 78 passed and one failed. No
passing meaning report is claimed and no rerun or tolerance change was made.
This blocks the conditional push: the merge remains local pending PM follow-up.
Current PR #88 checks at the older `6a619a1a` are green for offline, contract,
backend and both PostgreSQL jobs; publish is skipped. They do not qualify this
merge revision.

Logs: `work/pd2-merge-mac.log`, `work/pd2-merge-staging.log`,
`work/pd2-merge-staging-resume.log`, `work/pd2-merge-final-resume2.log`.
The first local offline attempt failed because the Python oracle interpreter
was absent; Linux offline passed. Interrupted calls are not successful gates.
An earlier scenario attempt failed adapter startup; its printed report named
an older SHA and was not used. The fresh scenario invocation above passed.
Local `rs/target` (3.0 GB) was inspected and removed at round end as requested.
No local venv was created. Task 24 has not started.

### Signed candidate production (read-only investigation)

`.github/workflows/rust-read-ci.yml` runs offline and contract for PRs targeting
staging/prod and for manual `workflow_dispatch`. Neither trigger can run
`publish`: its condition requires a **push** to staging/prod and both prerequisite
jobs exactly successful. A feature-branch PR therefore produces test/build
artifacts but no published attested release bundle. Manual dispatch alone also
cannot satisfy entry condition 3. A separately authorized promotion to a live
branch, or a separately reviewed workflow change for candidate publication,
is needed; neither was performed here.

On the permitted push, `publish` checks the tested binary digest against the
release binary, packages `rust-read-release`, and uses GitHub's
`attest-build-provenance` with OIDC (`id-token: write`, `attestations: write`).
It uses no repository production private signing key. The deployment provenance
signature is a second, operator-owned step: `scripts/verify_rust_artifact.py
--attest` on the PM workstation verifies both GitHub subjects and signs the
bound record with the workstation-held Ed25519 private key. The deploy host
holds only its public key. No signing, dispatch, release, key access, or live
operation was performed in this round.

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
