# Verdaxis reliability follow-ups — 2026-10-03

**Goal:** Finish six named follow-ups with the current architecture, then measure the released system once.

**Architecture:** PostgreSQL remains authoritative. Store successful named-command responses in the existing mutation transaction. Derive activity-report identity from authentication. Use the committed market outbox and existing per-worker SSE hubs for a sanitized refresh signal. Keep REST recovery and polling. Use existing CI, migration guards, atomic staging publication and protected Vercel release.

**Tech stack:** FastAPI, SQLAlchemy async, PostgreSQL, Alembic, React/TypeScript, Vitest, GitHub Actions and existing systemd/Vercel release tools.

## Fixed scope and stop rules

Implement confirm, decline, deliver, pay, amend and cancel result replay; repair supported dependency advisories; report bounded browser losses in the existing admin activity drawer; protect both release branches in both repositories; deliver committed public refresh signals; perform one bounded read-only latency/capacity run. Larger matching, database, analytics and trading-policy changes are assessment only. No new framework, broker, dashboard, consent flow, load harness or commercial rule.

Use isolated owner worktrees and serialize integration commits. Preserve staging FAME and production's six-fuel catalog. Do not restore delivery/payment UI. Do not promote the identified activity pipeline into staging. One affected local check per implementation and one complete integrated CI pass per branch; repeat only for actual failures or new changes. Review specification first, then correctness/security. Stop the load ramp at the existing guard or saturation; no crash target.

## Task 1 — Durable successful command replay

Paths: app/models/command_result.py, app/models/__init__.py, app/services/command_results.py, app/routers/trades.py, app/routers/orderbook.py, app/schemas/orderbook.py; existing market/ACL/migration tests. Frontend: src/services/api.ts, MyTrades.tsx, CommandCenter.tsx, Marketplace.tsx and their existing tests/translations.

1. Create append-only market_command_results scoped by actor + operation + key. Bind exact effective organization, support context, target, request hash and original status/body.
2. Authenticate and authorize current actor/context before replay. Replay before mutable eligibility, status, expiry, ETag and economic admission. On a miss retain current locks and rules.
3. Insert the original response in the same transaction as mutation/audit/outbox/economic effects. Same key with changed intention/context returns 409. External clients may omit keys for compatibility.
4. Move amendment future-time admission after replay while preserving fresh-request rejection. Freeze maintained client key/body/preconditions/auth generation/context across unknown outcomes. Add no inactive workflow UI.
5. Migration rcp_20261003_command_results follows obp_20261003_acceptance_priority in both branches. Exact runtime grants SELECT, INSERT; no UPDATE/DELETE. Retain receipts indefinitely.
6. Verify replay snapshots after later state changes, no repeated effects, context/auth revocation, stale amendment bodies and one representative concurrency case in existing PostgreSQL suites. Extend existing frontend mutation tests.

## Task 2 — Supported dependency patch

Paths: each branch's package.json and package-lock.json; staging .github/workflows/frontend-ci.yml.

Pin Vitest 4.1.11 and regenerate each branch's own lock. Use Node 24 in staging CI, matching production. Verify with a fresh dependency installation or fresh existing CI; never mix shared node_modules.

Current audit has two root advisories: patched Vitest (two moderate graph entries) and unpatched braces through Tailwind 3 (five high graph entries). Runtime omit-dev audit is zero. Do not use an unsupported override or silently conceal advisories. A Tailwind 3→4 visual/browser compatibility migration exceeds this bounded dependency patch; document the unpatched build-only finding and supported migration option. Validate audit totals and full existing CI.

## Task 3 — Browser-reported delivery loss (production only)

Paths: app/models/user_activity.py, app/schemas/user_activity.py, app/services/user_activity.py, scripts/prune_product_analytics.py; frontend activityTracking.ts, types/activity.ts, admin/UserActivityDrawer.tsx, admin locales, existing analytics docs/tests.

1. Add at most one UUID + bounded dropped/rejected count report to a normal existing activity batch. No report-only requests, raw events, URLs, headers or client identity.
2. Freeze report ID/counts with a batch retry. Discard on principal/auth generation changes. Exclude intentional clear() losses. Avoid recursive loss reporting and background work solely for metrics.
3. Server derives actor, deduplicates user + report ID, inserts with the batch even when events are duplicates, and retains reports for the existing 90-day period.
4. Aggregate received reports by the selected admin cutoff. Label coverage as partial; closed/offline browsers and failed reports remain unknown. No delivery percentage or audit-ledger claim.
5. Production-only migration uadl_20261003_delivery_reports follows rcp_20261003_command_results. Runtime grants SELECT, INSERT, DELETE only. Update model/checkpoint coverage and existing prune/ACL tests.

## Task 4 — Branch protection

Apply the reviewed classic protection payload to prod and staging in Verdaxis/verdaxis-backend and Verdaxis/verdaxis-frontend. Require PRs, zero external approvals, strict current-base CI from GitHub Actions app 15368, admin enforcement, resolved conversations, no force pushes or deletion. Backend checks: test and postgres-analytics. Frontend check: test.

Immediately record exact SHA and absent protection before each mutation. Stop on changed state. Read back full policy and protected=true after each change. Existing deployment paths remain unchanged. Record rollback to the pre-state; do not invoke it without cause.

## Task 5 — Committed public refresh

Paths: app/services/market_events.py and existing committed dispatcher/SSE hub; src/services/sse.ts and existing cache consumers/tests, with minimal producer changes if required.

Reuse committed outbox data and current fanout. Deliver only schema_version=1/resync_required=true for public prices/orderbook refresh; no identities, slice, prices, quantities, private cursor or event reason. Strip internal markers from private payloads. Coalesce each drain and preserve queue bounds, reconnect/reset REST recovery and polling. Cover committed book-changing routes, expiry/demo writers and transaction rollback in existing suites. Assign sequencing only in the existing post-commit dispatcher. No new table/broker/L2 protocol.

## Task 6 — Existing release and one measurement

1. Integrate reviewed commits sequentially. Check clean source, exact migration ancestry and role coverage. Update relevant architecture/release notes before the final CI freeze.
2. Push feature branches, open PRs to the correct environment, wait for exact required checks, merge and verify exact push CI. Branch protection must permit this recorded path.
3. Take fresh existing staging and EU production backups. Dry-run then run backend guards with exact source SHA and explicit current/target revisions.
4. Publish staging frontend through the existing atomic publisher. Publish production frontend through release-vercel.yml (candidate_only=false, notify_release=false). Verify readiness/source SHA/catalog/rendered artifacts using existing smoke tools.
5. Run one existing read-only API/browser profile with at most four phases, numeric concurrency, CPU/readiness abort guards and full final health/cleanup checks. No real customer mutations, notification emails or identified test analytics. Report actual p50/p95/error/throughput/CPU evidence; do not equate request workers with users.
6. Mark each item complete only on evidence. Report the unpatched build advisory separately. Close owned browser/load sessions and update the operator handoff with current release identities.

## Release targets

Staging: schema rcp_20261003_command_results. Production: schema uadl_20261003_delivery_reports. Current schema in both: obp_20261003_acceptance_priority. Production backend stays on EU host 169.58.37.164; retired local production remains disabled. Preserve authenticated session cutoff and acceptance ordinal migrations on any forward corrective release.
