# Rust Phase 1 Production Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use executing-plans to implement this plan task-by-task. Work sequentially; no subagents.

**Goal:** Prepare the approved production Rust read path, with a rehearsed staging route rollback and separate operator gates for each production route group.

**Architecture:** Extend the existing source-attested dual-service deployment transaction. Production has its own policy, unit, exact database identity and `prod` provenance ref; keep the first release policy-off. Import the Task 13 exact routing snippet in the existing Caddy route block and change one upstream argument per window. Do not replace a site's TLS, logging, proxy trust or Python fallback.

**Tech Stack:** Rust 1.95, Python deployment helpers, PostgreSQL 17, systemd, Caddy, GitHub Actions attestations.

---

## Authority and boundaries

Standing GO is decisions 101–104; the Phase 1 gate is `docs/rust-phase1/go-no-go.md`. No founder plan review is requested. Each live action still needs the PM's operator approval. This lane never connects to a live host. Host facts below are questions, not inferred facts. No measurements, no load test (decision 96), no edits under `app/` or `alembic/` (93/94), no changes to staging policy, staging unit or comparator. The signing key is passed by path to the verifier only. No key inspection, copy, listing or hash.

Initial source: clean `feat/rust-phase1-production`, `13f89a01`. Read `CLAUDE.md`, `ARCHITECTURE.md`, the Task 13 snippet `deploy/caddy/rust-public-staging.caddy`, and private `outputs/runbooks/2026-10-19-rust-staging-pass-14b.md` before resuming.

## Review before implementation

- Architecture **PASS**: reuse one transaction, budget helper, artifact verifier and exact routing snippet; no parallel deploy authority.
- Code quality **WARN**: shared helper extensions must retain staging defaults and signatures. Production selection must not read staging policy. Explicitly bind verifier to `refs/heads/prod`.
- Test coverage **PASS**: real subprocess deployment fixtures cover policy-off, activated restart, provenance refusal, missing units and failed readiness; Rust configuration tests cover exact production DB authority and checkpoint.
- Performance **WARN**: limits are safety caps, not new measurements. Keep Python four workers. Rust has 16 read slots plus one readiness connection. With defaults, conservative budget is `2 services * 4 workers * (2 pool + 1 overflow + 1 readiness) + 20 reserve + 17 Rust = 69 <= 100`; 31 connections remain beyond the reserve. Although staging is on another host, retain the existing conservative two-service accounting. Confirm observed PostgreSQL maximum and active connection owners before approval. Production Rust unit: `MemoryHigh=192M`, `MemoryMax=256M`, `CPUQuota=100%`, `CPUWeight=50` on the four-core EU host. One CPU is a cap, not a throughput claim. Do not cap or change Python/PostgreSQL here.

## Promotion payload: not Rust alone

Production supplied baseline is full SHA `56d7577085af99411a6f9c3e34e64ad929d09954`, checkpoint `p0_20261016_union`. Staging supplied baseline is `f4663bf4091ea4261875414f017cd27cd5a2ceb1`. Verify both at window entry.

Normal PR path: this branch -> PM integration branch -> staging PR -> staging push CI -> staging B/S and 14b -> staging rehearsal -> reviewed staging-to-prod PR B' -> prod push CI -> B' deploy -> root install -> later staging-to-prod PR S' enabling **production policy only** -> prod push offline/contract/publish -> signed prod artifact -> S' deploy -> public groups individually. Do not merge S' before B' is installed; deploy pins the remote branch tip. Do not reuse staging binary/SHA/signature. Do not push `staging` or `prod` from this lane.

The promotion also carries exactly these pre-existing changes relative to production:

1. PR #84 (`98ab7358`): Phase 0 git-owner assertion fix; KYC systemd credential placeholder and installer preflight; associated tests, operator/runbook corrections and cutover record. Both backend unit files and the installer differ. Root installation is therefore a reviewed whole production manifest installation, not just copying the Rust unit.
2. PR #85 (`f4663bf4`): drift-checked `openapi.json`, `scripts/export_openapi.py`, snapshot test, README and architecture notes. No deployment automation.
3. Integrated Phase 1 through `13f89a01`: Rust workspace/routes/shared market visibility/MCP/calculations, policy exports and compiled revisions, disposable contract/corpus/simulation/gate tools and evidence, CI build/contract/artifact publication, public provenance key/verifier, guarded staging deploy and bootstrap support, budget/runtime helpers, staging unit/manifest, exact Caddy snippet, shadow comparison and documentation. The exact Python promotion delta against `56d75770` is `app/routers/orderbook.py` (deterministic listings pagination, 2 additions/2 removals) and `app/seeds/market_seed.py` (partial-fill seed repair, 20 additions/10 removals); there is no Alembic delta. Phase 0 supplier metadata alignment belongs to the Rust oracle work, not an additional Python promotion change; do not edit or silently exclude them in this lane. The production PR must review `git diff --name-status 56d75770..B_PRIME`, particularly `app/`, `alembic/`, backend units and the installer, and reject unlisted later work.
4. This plan's production-only additions and environment-aware helper extensions. No new schema change: exact self-transition `p0_20261016_union -> p0_20261016_union` only.

Freeze the exact promotion file list privately before merging B'. Recompute it if staging advances; these categories are not approval for unrelated future commits.

## Task 1 — Production runtime authority (failing tests first)

Files: modify `rs/read/tests/runtime.rs`, `rs/read/src/runtime/config.rs`; create `tests/unit/test_rust_production.py`; modify `scripts/prepare_rust_runtime.py`; create `deploy/rust/production-policy.json`. Existing `deploy/rust/allowed-revisions.json` already includes `p0_20261016_union`; keep it, and require that exact checkpoint for production configuration.

1. Add tests for valid production `127.0.0.1:5432/verdaxis_prod` as `verdaxis_app`, loopback HTTP 8010, full compiled SHA and union checkpoint. Reject staging role/database, wrong port, non-union revision and non-loopback transport. Replace the old test assertion that production is unsupported with the new exact contract.
2. Add helper tests projecting production identity, app-only credentials and the existing KYC switch. Reject cross-environment policy/credentials. Keep existing staging helper API defaults intact.
3. Run new tests through the staging wrapper and record each red exit code in `work/`. No tests or builds on Mac.
4. Add the production policy (false, `verdaxis-rust.service`, `127.0.0.1:8010`, UTC, `.runtime-rust/releases`). Extend helper identity selection from its policy, with exact fixed unit/bind/readiness per environment. Production runtime must not accept staging's checkpoint merely because it is in the shared compiled allowlist.
5. Run Rust runtime and Python helper regression gates. Commit source plus tests.

## Task 2 — Production unit and bootstrap manifest (failing tests first)

Files: `tests/unit/test_rust_production.py`, new `deploy/systemd/verdaxis-rust.service`, modify `deploy/systemd/runtime-units.manifest`.

1. Test production manifest selection, production paths, start guard environment, exact 8010 bind, projected runtime environment (never `.env` in the long-running process), revision/budget prestarts and memory/CPU limits.
2. Observe failure, then derive the production unit from staging without changing staging bytes. Use `/home/verdaxis-prod/verdaxis/prod/be`, existing sandbox, four-worker Python untouched, 256 MiB hard Rust memory cap and 100% CPU cap.
3. Run tests and systemd syntax checks on the isolated test account where available. Missing fixed host paths are not live install evidence. Commit.

## Task 3 — Production guarded deployment (failing tests first)

Files: modify `tests/unit/test_rust_release.py` (parameterize disposable fixture environment), `tests/unit/test_rust_production.py`, `scripts/deploy.sh`, `scripts/prepare_rust_runtime.py`; update any source-attestation fixture pin that refers to the changed deploy blob, after inspecting its owner.

1. Add production subprocess cases: policy off keeps Python-only ledger; policy on requires installed unit before mutation; staging-signed record is refused; prod-signed record starts both services with one SHA; failed Rust readiness retains blocked state and stops attempted services. Reuse fixture keypair only, never the workstation key.
2. Observe failure. Select policy/unit by `DEPLOY_ENVIRONMENT`; archive that environment's policy; pass explicit `--workflow-ref refs/heads/prod` for production (staging retains `refs/heads/staging`); prepare runtime based on the selected environment. Use environment-specific policy filename in `.runtime-rust`. Preserve locks, restart authorization, no identity-only rollback, immutable source and exact readiness.
3. Confirm workflow already triggers `prod` pushes and publishes only after offline + contract success. Add regression assertion rather than a second workflow. No PR artifact is deployable.
4. Run production + staging release/budget/unit tests and deploy health/runtime regressions. Commit.

## Task 4 — Exact per-group Caddy release assets (failing tests first)

Files: new `scripts/rust_caddy_candidate.py`, new `tests/unit/test_rust_caddy_candidate.py`; reuse `deploy/caddy/rust-public-staging.caddy` unchanged. Private candidate/rollback files are generated from the host's reviewed site file, not a fabricated complete EU config.

1. Staging tests retain full-line import argument updates. Production tests use `tests/fixtures/caddy/production.Caddyfile`: separate P0 access-log candidate, then exact group insertion after two analytics handles and before Python fallback. Rollback restores exact original complete-file bytes. Refuse unknown/duplicate groups, imports, unexpected fallback or ambiguous API/route anchors. Check all non-target bytes remain unchanged; validate/adapt each complete candidate with real Caddy 2.11.2.
2. Implement bounded offline generation of candidate + rollback from a copied site file; no live writes, reloads or automatic fallback. Private files must be inspected and `caddy validate`d with the complete host configuration before install.
3. Run tests; commit. Keep public HEAD/non-GET requests, private orderbook paths, writes, `/health*`, docs and fallback on Python. Production `/mcp` remains Python (disabled there); do not activate MCP production.

### Route order and exact matchers

Source of truth: Task 13 snippet (all method restrictions and slash forms remain byte-for-byte unchanged):

1. **simple** (`args[0]`): GET regexp `^/api/(ports(/[^/]+)?|catalog/(products|delivery-points)|news)/?$`.
2. **orderbook** (`args[1]`): GET exact paths `/api/orderbook`, `/snapshot`, `/bids`, `/asks`, `/with-ci`, `/aggregated`, `/product-counts`, `/map-summary`, `/map-summary/compact`, `/products`, `/regions`, `/fuel-types` (suffixes relative to `/api/orderbook`), and `/api/listings`, each with and without trailing slash. `/my`, UUID and other private paths do not match.
3. **prices** (`args[2]`): GET `/api/prices`, `/api/prices/reference`, `/api/prices/reference/export`, `/api/trade-tape`, each with and without trailing slash.
4. **curves** (`args[3]`): GET `/api/curves/forward`, `/export`, `/table`, `/slice`, `/board` (suffixes relative to `/api/curves/forward`), each with and without trailing slash.
5. **mcp** (`args[4]`): exact `/mcp`, GET POST DELETE, **staging only** and last. Production argument stays `127.0.0.1:8000`; its existing disabled boundary remains intact.

EU has no imports: after P0 redacted access logging, each candidate inserts one group's exact matcher/handle blocks after analytics and before the final Python handle in the complete `/etc/caddy/Caddyfile`. Rust uses numeric 8010 with the existing forwarding/flush/keepalive options. Rollback is that window's exact prior complete file, so earlier accepted groups remain Rust. The `args` indexes above identify staging snippet groups only; no production import is created. Never use weighted pools, retries to Python on failure or dual/shadow public delivery. Staging retains 8001/8011 import mode.

After each change: exact Python/Rust four-key readiness for the approved shared SHA, bounded GET probes for every route family in that group, trailing slash and HEAD, plus private `/api/orderbook/my` and write boundary probes without credentials (no mutations), existing public health and other sites. Probe bodies stay private or are discarded. Prices probes are serial and spaced, not a limit-exhaustion test. MCP staging uses one approved read-only discovery call, not a tool mutation.

PM's concrete rule: compare equal adjacent ordinary-traffic windows (30 minutes each) on exact group GET paths. Roll back on failed exact Rust readiness; new 5xx when before=0 and after>=1; 5xx rate > before +0.5 percentage points; p95 >2x before with >=50 requests in both windows; or new Rust/Python error class. Also retain rollback for unexpected 429/client-IP collapse, OOM/restart or private/write misrouting. Sparse or incomplete evidence HOLDs the next group; no load test fills the window. P0 logging must precede baseline capture because no EU access log/threshold existed. `scripts/rust_passive_no_harm.py` and the private runbooks implement exact passive commands. Restore rollback bytes, validate full config, reload and repeat readiness/probes; never roll back source or change release identity.

### Rate-limit boundary R12–R14

Rust `runtime/http.rs` owns 60/30/10 per minute per client/path in one process; typed validation precedes consumption. Python retains SlowAPI in its four workers for requests still routed there. Caddy owns exact routing and trusted forwarded-client transport, not a new rate bucket. Splitting **groups** does not duplicate a request: prices moves as one group to one backend; slash redirect lands on the same owner. Do not split R12/R13/R14 by clients or put them in a mixed upstream pool. Existing buckets are not shared across Python workers or between Python and Rust. A cutover, restart or rollback can therefore under-count aggregate use across the boundary for up to one minute; there is no exactly-once cross-process quota claim. Existing in-flight requests can finish on the old backend while new ones use the new bucket. Wait a quiet minute before the prices window when practical; otherwise record this bounded reset and obtain PM acceptance before moving prices. If strict continuous global quotas are required, STOP that group for a separately approved shared-limiter design; do not modify Python here. A trusted-proxy/client-IP mismatch is a rollback signal.

## Task 5 — Private operator runbooks and acceptance

Create outside Git under `/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/runbooks/`:
- `2026-10-19-rust-staging-caddy-rehearsal.md`: after 14b PASS only, snapshot mapping/DNS/unit/readiness/site and imports; initialize all-Python exact snippet as a separate reviewed no-traffic-change step; simple candidate -> validate -> reload -> probes/passive observations -> exact rollback -> probes -> optional reapply only with approval. This completes checklist D only after the PM executes and records it.
- `2026-10-19-rust-production-phase1.md`: entry questions; promotion inventory; B' dry-run and approved self-transition deploy, timer drain/restore, root manifest install and byte checks; separate S' prod-push verification/signing/transfer, preflight, shared identity dual readiness; staged four public group windows with exact candidates/rollback, no-harm signals and stop conditions. No production MCP activation. Explicitly handle failed deploy state by stopping and using the existing forward-revert/recovery procedure, never deleting state or pairing old identity with new bytes.

Commands must check every critical return code (`set -euo pipefail`), use exact approved 40-hex SHAs, preserve active timer lists, check symlinks, and never inspect credentials. Host commands are instructions for the PM, not actions by this lane. Keep the earlier 14b runbook unchanged.

## Verification and delivery

For each test-first cycle, use one wrapper call at a time:

```bash
(verdaxis-staging-test "$PWD" '/home/verdaxis-test/bin/heavy run 3600 -- bash -lc "ENVIRONMENT=test RELEASE_SHA=test JWT_SECRET=test-secret-key-for-testing-minimum-32-chars DATABASE_URL=sqlite+aiosqlite:///:memory: venv/bin/python -m pytest tests/unit/test_rust_production.py -q"'; echo "CALL_EXIT=$?") > work/production-red.log 2>&1
```

Background long calls and wait with `until grep -q CALL_EXIT= work/production-red.log; do sleep 30; done`. Record each gate's own exit code, not only a shell pipeline's status. Expand to release/unit/budget/deploy suites for green. Rust runtime tests use compiled full `RELEASE_SHA`, `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test runtime --locked`. No builds/tests on Mac. GitHub CI supplies full Rust/offline/contract/PostgreSQL gates; do not duplicate all those host runs. Wait for every call and CI watch to finish.

Commit in small logical steps with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Push only `HEAD:refs/heads/feat/rust-phase1-production`. Open draft staging PR titled `[CI only] Rust Phase 1 production path`, body `CI only. Do not merge. The PM merges this through the integration branch.` plus required attribution. Do not edit/comment/merge other PRs. Check all CI results and fix source failures on this branch.

Update `ARCHITECTURE.md` for production relationships. Before final delivery use demand-elegance and verification-before-completion. Report plan/commit/runbook paths, each gate exit code, CI and unresolved host questions. Do not claim live staging rehearsal or production readiness acceptance.

## PM amendment — 19 October, ~04:20 SGT (supersedes earlier Caddy assumptions)

Architecture **PASS**: keep the single EU `/etc/caddy/Caddyfile`, no imports. Code quality **PASS**: scoped structural edits, unchanged analytics/fallback and exact rollback bytes. Tests **PASS**: real analyzer on systemd 249 and 255, real Caddy validate/adapt, synthetic passive-log edge cases. Performance **PASS**: passive existing traffic only, no workload generator or measurement.

PM facts: EU `vmi3623757`, systemd 255, four CPUs, 7.9 GB RAM (~6.7 available), 96 GB root/45 GB free. Clean production remains `56d75770`, database `verdaxis_prod`, union checkpoint, max_connections 100, UTC, 11 idle app connections plus one postgres at observation time. Python MemoryMax 1 GiB, analytics EnvironmentFile drop-in and production-isolation drop-in (CPUWeight 500, SSH inaccessible); no Rust unit/listener. No recovery beyond unit Restart. Timers include onboarding-attention, order-expiry-reminders, demo-activity, auth-maintenance, analytics-prune, local-db-backup and news-refresh. Analytics relay 8700 and test guard must remain untouched.

EU Caddy 2.11.2 has one file, no access log or passive thresholds. Global trusted proxy is 194.233.68.86/32 with strict parsing. API route contains `/script.js` and Origin-guarded `/api/send` analytics handles, then Python fallback with explicit X-Forwarded-For client_ip, flush_interval -1 and keepalive off. Preserve all those bytes and security/body/encoding directives. Public DNS is direct EU. Shared-host stale production relay/log remains untouched; it is not the EU baseline.

### Amendment tasks (test first, sequential)

1. After PR #94 is merged, fetch and merge `origin/staging`. Read `docs/rust-phase1/bootstrap-unit-verify.md`. Extend its real three-caller analyzer regressions to the production unit with `.runtime-rust/` absent. Run red then `/usr/bin/env` launch green via staging wrapper (249) and prod-test wrapper (255), each `heavy run 3600`. Preserve environment/PID and every start/sandbox gate; no placeholder binary.
2. Replace production import-argument generation in `scripts/rust_caddy_candidate.py` with scoped group-block insertion into the actual API route, after analytics and before bare fallback. Keep staging import mode. Fixture `tests/fixtures/caddy/production.Caddyfile` models the PM-supplied one-file structure, including another site to prove isolation. Reuse Task 13 matcher bytes, add identical explicit proxy options, refuse absent/ambiguous anchors, imports and duplicate group. Rollback is the exact original full file; earlier groups remain.
3. Add separate `access-log` candidate operation for P0: API site only, `/var/log/caddy/api.verdaxis.exchange.log`, roll 20MiB/3, JSON filter deleting request Authorization/Cookie/X-Monitor-Token and response Set-Cookie; remove only the stale temporary-log comment. Real Caddy validate/adapt on staging test host checks the complete fixture, redaction configuration and unchanged analytics/fallback. No route moves in P0.
4. Add `scripts/rust_passive_no_harm.py` with synthetic tests in `tests/unit/test_rust_passive_no_harm.py`. Read ordinary access/journal logs only. Equal adjacent pre/post windows (default 30 min); exact group GET paths; fail on Rust readiness failure, new 5xx from zero or increase >0.5 percentage points, p95 >2x before only with >=50 requests in both windows, or new Rust/Python error class. Empty traffic or incomplete inputs are inconclusive, never PASS. Exact passive-log commands go in both private runbooks. No active traffic generator.
5. Update both private runbooks, this plan, `docs/rust-phase1/production-path.md` and architecture relationships. P0 logging precedes baseline and route windows; never use the shared relay log as EU evidence. Commit logical steps, push only this branch, wait for draft #95 CI.

Resume: amendment tests first, then unit, generator/logging, passive checks and runbooks. Source acceptance does not mark any live window executed.

## Historical open host questions (answered above; recheck at window entry)

1. Correct host/mapping and DNS? On each authorized host: `hostname -s; getent ahostsv4 api.verdaxis.exchange; getent ahostsv4 api-staging.verdaxis.exchange`; read the two map files identified in CLAUDE.md.
2. Actual units/paths/drop-ins/limits? `systemctl show verdaxis-backend.service verdaxis-rust.service caddy.service -p User -p WorkingDirectory -p FragmentPath -p DropInPaths -p ExecStart -p MemoryMax -p CPUQuotaPerSecUSec`; staging names separately.
3. EU layout answered: one `/etc/caddy/Caddyfile`, no imports, Caddy 2.11.2 and strict shared-host trusted proxy. Recheck with `sudo caddy version; sudo cat /etc/caddy/Caddyfile`; never use staging's `050-verdaxis.caddy` paths on EU. P0 adds redacted access logging before group windows.
4. PostgreSQL maximum/timezone/activity? `sudo -u postgres psql -X -d verdaxis_prod -c 'SHOW max_connections' -c 'SHOW TimeZone' -c 'SELECT usename,datname,state,count(*) FROM pg_stat_activity GROUP BY 1,2,3 ORDER BY 1,2,3'`. No query text or credentials. Budget/startup also validates observed limits.
5. Exact live source and checkpoint? `sudo -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/prod/be status --porcelain; sudo -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/prod/be rev-parse HEAD; curl -fsS --max-time 2 http://127.0.0.1:8000/health/ready`; `sudo -u postgres psql -X -d verdaxis_prod -c 'SELECT version_num FROM alembic_version'`.
6. Existing passive alert thresholds/log fields, usable pre-window latency baseline and observation interval? `sudo journalctl -u caddy --since '-15 minutes' --no-pager -n 100`; operator identifies the existing access-log path and monitor configuration without dumping secrets. No active speed test.

## Resume point

Plan written before source changes. Next: Task 1 failing production tests on the staging test account, then runtime authority, unit, guarded deploy, Caddy generator, private runbooks and CI. Live gates remain PM-owned: staging 14b PASS -> checklist D rehearsal -> production payload approval -> B' -> unit install -> S' -> one group at a time. An unconfirmed host fact blocks its host window, not source implementation.
