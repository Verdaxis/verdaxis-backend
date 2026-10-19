# Production path source acceptance

19 October 2026. Plan: `docs/plans/2026-10-19-rust-phase-1-production.md`.

## Built boundary

- Production bootstrap policy remains false. New unit is `verdaxis-rust.service`, loopback 8010, app-only projected runtime credentials, existing start/revision/budget prestarts, 192/256 MiB memory limits, one-CPU quota and lower CPU weight.
- Production configuration requires `verdaxis_app`, `127.0.0.1:5432/verdaxis_prod`, union checkpoint and prepared budget. Production CORS uses the current Python production origin set. MCP remains non-production.
- Shared deploy selects the environment policy and binds production artifact provenance to `refs/heads/staging` (decision PD-P1, below). Unit absence and wrong provenance refuse before source mutation. Both readiness documents are required before state clears.
- Production Caddy candidates now edit the confirmed single EU Caddyfile: P0 redacted API access logging, then one exact Task 13 matcher/handle block after analytics and before Python fallback. Rust proxies retain explicit client_ip forwarding, flush -1 and keepalive off. Exact prior full-file bytes are the rollback. Staging retains import-argument mode; production MCP changes refuse.
- No changes under `app/` or `alembic/`, no staging policy/comparator edits. Staging unit changes came only from PM-authorized merge of PR #94 (`origin/staging` at `b068f89e`); this lane applies its env-launch rule to production. No live host install/deploy/Caddy action, signing, database access or measurements were performed.

## Test-first evidence (isolated staging test account only)

Each command used `verdaxis-staging-test` and `/home/verdaxis-test/bin/heavy run 3600`. Local logs are untracked under `work/`.

| Gate | Exit | Result |
| --- | --- | --- |
| Initial production Python authority red | 1 | Missing policy/unit and environment selector |
| Initial Rust authority red | 101 | Production unsupported |
| Production deploy red | 1 | Existing production path ignores Rust activation and refusal inputs |
| CORS red | 101 | Production origin policy absent |
| Caddy candidate red | 1 | Generator absent |
| Production prepared-budget red | 101 | Missing budget accepted before guard |
| Deployment/runtime/unit/budget/Caddy/health regressions | 0 | 216 passed, one existing warning |
| Final Rust runtime + HTTP | 0 | 33 passed |
| Final production/Caddy/monitor source ownership | 0 | 16 passed |
| Pinned Rust formatting | 0 | Passed after retrieving remote formatting |
| Workspace all-target clippy, warnings denied | 0 | Passed |

Intermediate runs found obsolete staging-only assertions, a fixture import error (collection exit 2), fixture production database mismatch, and local formatting drift; all were corrected and rerun. One staging fixture readiness attempt failed once and passed in the complete rerun. One wrapper transport call returned 255; the isolated unit was confirmed absent before rerun. These were not suppressed or reported as PASS. All test calls finished.

GitHub PR CI is the full build/offline/contract/PostgreSQL authority and is recorded in the delivery report. A PR does not publish a deployable production artifact; actual `prod` push offline, contract and publish success remains a later operator gate.

`refresh-codesight` completed locally as documentation generation, not a build/test. Its broad generated changes (including unrelated schema/wiki rewrites) were retained in `work/` and excluded from the production patch; `ARCHITECTURE.md` carries the scoped relationship update.

## Operator gates still open

Private runbooks are in the workspace-level `outputs/runbooks/`, not this public repository:
- `2026-10-19-rust-staging-caddy-rehearsal.md`
- `2026-10-19-rust-production-phase1.md`

Live acceptance requires staging 14b PASS, checklist D rehearsal, reviewed full promotion payload, production B' deploy/install, separate prod-push S' artifact and activation, then simple/orderbook/prices/curves windows. Production MCP stays Python. R12–R14 quotas are process-local; the PM must accept the bounded one-minute quota reset or stop prices for a separately approved shared limiter.

## PM host answers and amendment acceptance

PM supplied read-only facts at ~04:20 SGT: EU systemd 255/four CPUs/7.9 GB RAM (6.7 available)/45 GB root free, clean baseline `56d75770`, union schema, PostgreSQL maximum 100/UTC/11 idle app plus one postgres. Python MemoryMax 1 GiB and analytics/isolation drop-ins remain unchanged. Rust unit/listener absent. Only unit Restart recovery exists. Analytics relay 8700, test guard and the named operational timers are not owned by this lane.

Caddy 2.11.2 has one `/etc/caddy/Caddyfile`, no imports and initially no access log. Strict trusted proxy is shared host /32; API DNS is direct EU. Shared host's obsolete production relay/log remains untouched and cannot supply EU baseline. Fixture models the supplied structure (including analytics Origin control, headers/body limits and fallback); it is not a verbatim host-file capture or permission to overwrite unreviewed host bytes.

Production unit `/usr/bin/env` launch is verified with `.runtime-rust/` absent. Three real caller shapes (old production helper at `56d75770`, installer, next helper) each returned 1 before the fix and 0 after on both systemd 249 and 255. PID/environment check passed on both. Test-host suite exits: 249 red 1, 255 red 1, 249 green 0, 255 green 0. No service starts or fake future binary were used by analyzer fixtures.

P0 logging red test exited 1 before generator support. Real staging Caddy validate/adapt now returns 0 for P0 and each of four cumulative group candidates (10 individual checks). Initial fixture syntax failed validation (exit 1); its multi-origin matcher was corrected to an equivalent valid expression before acceptance. API log rolls at 20MiB/3 and the adapted filter deletes Authorization/Cookie/X-Monitor-Token request headers and Set-Cookie response headers. Original routes/other sites stay byte-identical; group rollback removes only its exact generated block.

Passive gate red exited 1 before helper implementation; synthetic threshold tests passed (exit 0). Helper reads existing logs only: equal adjacent 30-minute windows, new 5xx from zero or rate increase >0.5 percentage points, p95 >2x with >=50 rows in each window, new journal error class, or failed exact Rust readiness. Sparse/invalid evidence is HOLD, not PASS. Runbooks contain complete commands for rotated log snapshots, journal windows, readiness input, checker invocation and rollback. No active measurement or traffic generator is used.

Final staging amendment suite: exit 0, 20 passed, including CLI return codes and adapted redaction fields. A late test run returned 1 for a missing cache import; it was corrected and the complete suite rerun. The existing unit-policy/production regression selection also passed (31 tests, exit 0). All ten individual Caddy validate/adapt invocations returned 0. Historical verifier command text is pinned in the regression so shallow PR checkouts need no commit-history fetch.

Both private runbooks now distinguish EU one-file P0/block rollout from staging imports, retain all seven PM-named timer/deploy controls (exact names rechecked on entry), and state the concrete passive thresholds. Host acceptance still requires operator execution and fresh entry drift checks; supplied point-in-time facts do not prove later state.

## Staging S2 rules in the production path (round 3)

19 October 2026. The branch merged `origin/staging` at `cd964294` (PRs #98, #105 and #107) and applies the same deployment rules to production:

- The shared deploy helper checks every reviewed backend and Rust restart/stop/disable command with `sudo -n -l` before the first source or state change, then runs each with `sudo -n`. The reviewed production sudoers file now lists the same exact Rust commands as staging, and the unit tests pin the list.
- The Rust unit start check is the stdlib-only `verify_rust_start.py`; it reads `revision.json` and the budget file without `.env` or application imports. The prepared budget is replaced atomically and a symlink at its path is refused, so a second release and a retry both work.
- Python restarts first and must report exact readiness before Rust starts. A Rust-only failure records `rust-failed`, stops and disables only Rust, and leaves Python serving; tests cover the retry and the next release after each failure.
- Canonical production deploys follow the tip of `origin/staging` (`*/prod/be` maps to `DEFAULT_BRANCH=staging`), as Phase 0 set and founder decisions 89 and 99 record. This round leaves that mapping exactly as on `origin/staging`.
- `deploy/caddy/rust-public-staging.caddy`: every group proxy now carries `flush_interval -1` and `transport http { keepalive off }`, the same options as the live staging Python fallback (Uvicorn closes idle keep-alives after five seconds, and the MCP group carries POST). A unit test pins the options in each group handle, and a real-Caddy adapt test checks the prepared site and the first single-group change. The staging candidate generator still edits only the import line. Production generation reuses the same group blocks.
- `rust_passive_no_harm.py` reads the staging API access-log shape (Caddy JSON with nested `request`, `tls`, headers and `resp_headers`); a synthetic fixture test covers it. Its volume rule is unchanged: fewer than 50 requests in a window is HOLD. Any staging exemption for sparse traffic is operator policy in the private runbook, not checker behavior.

Gate results for this round (isolated staging test account, `heavy run`):

| Gate | Exit | Result |
| --- | --- | --- |
| Transport test against the old snippet | 1 | Missing `flush_interval` on the first group proxy |
| Caddy candidate suite after the snippet change | 0 | 10 passed, including the real-Caddy transport test |
| Focused production, budget, bootstrap, units, release, Caddy, passive and artifact suites with source ownership | 0 | 146 passed |
| Deploy health, runtime hardening, migration checkpoint, real-Caddy routing and shadow suites | 0 | 550 passed, 1 skipped |
| Staging prep and validation commands on a synthetic tree with Caddy 2.11.2 | 0 | Prep diff as expected, validate and adapt passed, host set unchanged |

The complete unit suite runs in GitHub CI (first run on this branch: 2755 passed, one failure, below). Two tests in `tests/unit/test_assert_host_git_owner.py` fail on the staging test host with "Invalid artifact" from `scripts/phase0/assert_host.py`; they pass in CI, and this lane changed neither file, so treat them as a property of the test host's shared checkout.

An earlier commit in this round changed production's default branch to `prod`, and the first CI run failed `test_canonical_branch_and_canary_origin`, the guard for the Phase 0 rule. That was a regression and is reverted: production follows `staging` (founder decisions 89 and 99), the guard test is byte-identical to `origin/staging`, and the production release fixtures use a `staging` source branch.

### Decision PD-P1: production artifact provenance binds to `refs/heads/staging`

Production's Rust artifact provenance binds to `refs/heads/staging`, not `refs/heads/prod`. Production deploys the `staging` tip, CI signs on the `staging` push, and `prod` has diverged (258 commits not on `staging`), so a promotion push is not possible without a force-push. This does not widen trust: whoever can push to `staging` already decides production's Python code.

Every other binding stays exact: repository, workflow file path, the exact commit being deployed and the attestation checks. `RUST_WORKFLOW_REF` in `scripts/deploy.sh` is one constant, `refs/heads/staging`, for both environments; the staging binding is unchanged.

Tests (`tests/unit/test_rust_production.py`): production refuses an artifact attested from `refs/heads/prod`, a `refs/pull/...` ref, a feature branch, a tag or a look-alike branch, and refuses the right ref for a different commit (a record that names another commit, and a whole artifact built for another commit). Each refusal happens before any source, state or service change. Earlier text in this directory and in the plan that names `refs/heads/prod` as the production provenance ref is superseded by this decision.
