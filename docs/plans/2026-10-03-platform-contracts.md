# Verdaxis trading reliability and market reads — implementation plan

**Goal:** Implement the source-confirmed trade, recovery, freshness, priority and analytics improvements, then release reviewed changes to staging and production.
**Architecture:** Retain FastAPI, PostgreSQL, advisory market locks, transactional audit/outbox, and the existing React cache. Reuse existing disposable database fixtures, CI, release scripts and browser smoke tools. Production and staging are separate branches and catalogs.
**Tech stack:** Python/FastAPI/SQLAlchemy/PostgreSQL; TypeScript/React/Vite/Vitest; guarded systemd and Vercel releases.
**Execution:** This session, with GPT-6.1 Sol workers and independent spec and code-quality review. User has authorized planning and execution; no execution-choice question is needed.

## Workflow and limits

1. Read source, project instructions and relevant lessons under `verdaxis-prod`; freeze branch/runtime identities and clean statuses.
2. Create isolated integration worktrees from current `origin/prod` and `origin/staging`. Preserve canonical checkouts and unrelated work. Give each worker exact file ownership; use separate feature worktrees for overlapping backend/frontend files.
3. Each worker implements the named behavior and adds only regressions that can catch a real economic, recovery or UI-state failure. Run the affected existing unit tests once after implementation. Do not create a framework, generalized harness, alternate release tool or new monitoring service.
4. Root integrates one finished change at a time. Spec review checks this plan first. Code-quality review then checks safety, readability and scope. Reopen only concrete defects; do not chase optional abstractions.
5. Run one integrated required check pass per environment. Serialize PostgreSQL suites and frontend builds on this shared VPS. A failed check gets one diagnosis and the smallest repair, followed by affected checks. Repeat a full suite only when the repair affects its validity.
6. Use one existing disposable PostgreSQL parity/profile pass for the exact book/port reads. Compare identical fixture/filter sets. Keep an optimization only with correct results and a measured reduction in work or latency. At most two query candidates; if neither helps, retain the safe contract improvements and record the measured limit.
7. Release staging first using existing guards. Verify exact release/schema and a bounded rendered/read-only smoke. Take and validate a production archive before schema changes. Release the exact reviewed production backend and frontend through existing gates. No real customer order, payment, email or analytics event is created by a manual smoke.
8. Stop after required CI, migration/ACL, release readiness and bounded smoke checks pass. Record exact commits, runtime identities, checks, measured limits and deferred policy work. No second load-test ramp or extra harness after success.

## Scope and acceptance

### A. Bind a direct hit to reviewed terms

Backend: `app/schemas/orderbook.py`, `app/routers/trades.py`, the existing order response assembler and a small shared executable-terms helper if needed. Frontend: `src/types.ts`, `src/services/api.ts`, `src/components/Marketplace.tsx` and existing trade-flow tests.

- Publish a canonical economic terms digest on selectable orders. Include mutable execution-relevant product/port/window, price, quantity/expiry, qualifications and certificate terms. Normalize exact decimals and UTC times. It must cover ordinary and assisted orders and preserve staging FAME-specific fields.
- Require the reviewed digest for a new direct hit; compare it after market and row locks, before reservation. A changed or missing review fails closed with a stable, useful conflict. Existing successful same-key replay returns the original response before mutable-current-term checks.
- Include the digest in the idempotency request hash. Keep bilateral `PENDING_CONFIRMATION` semantics.
- The UI freezes the digest with reviewed terms, returns to review on a conflict, and displays the returned trade price/quantity/status on success. It must not auto-submit changed terms.
- Regression: price/qualification change before hit creates no trade/reservation; same-key replay after later amendment returns the original trade; success uses server response values. Extend existing tests, not a new browser harness.

### B. Repair stream catch-up and overflow

Backend: `app/services/market_event_dispatch.py`, `app/services/event_bus.py`, `app/routers/stream.py`, `app/services/market_events.py`; frontend: `src/hooks/useSSE.ts` and its existing consumers/tests.

- Capture replay high-water, page all authorized rows to it, then merge queued live events in increasing cursor order. Permit global sequence holes. Preserve tenant checks, auth revocation and token expiry during long catch-up.
- Overflow makes the connection explicitly terminal/resync-required; do not leave an orphan queue sending keepalives.
- Add a compatible payload schema version. Do not enable outbox pruning in this release.
- A public reconnect must invalidate the relevant cached snapshot. Reuse shared committed events for bounded sanitized public invalidation where feasible; do not build full L2 deltas/checksums.
- Regression: >500 old authorized rows plus a newer queued row deliver the exact expected event set; overlap deduplicates; holes and tenant isolation remain valid; overflow closes/resets; reconnect refreshes current state.

### C. One coherent, bounded order-book read

Backend: `app/routers/orderbook.py`, orderbook schemas and existing read helpers/tests. Frontend: `src/components/OrderBook.tsx`, orderbook API adapter/cache and related tests.

- Add one selected-market snapshot read for both sides and metadata from one consistent database view. Return at most the current 15 rows per side; preserve public qualification, rejected-owner, expiration and demo filters.
- Avoid two listing counts and duplicate quote-reference work when depth does not consume them. Do not add a warehouse or speculative persistent summary.
- Include canonical market identity and generation time. Use the existing shared cache with a validity no longer than the visible refresh cadence; forced reconnect/manual refresh bypasses it.
- Show actual snapshot age, refreshing, stale and unavailable states. Keep prior data only with a visible stale/failure indication. Label individual order depth accurately; do not aggregate non-fungible certificates or mix demo with real executable claims.
- Regression: both sides and metadata agree, old-filter responses cannot replace a new selection, expiry without a write removes rows, and failed refresh is visibly stale. One bounded existing profile records SQL count/time and response parity.

### D. Deterministic priority and amendment rules

Backend: order model, matching engine, create/update route, one additive migration if required, exact app-column grants/checkpoint policy and existing PostgreSQL matching tests. Preserve audit creation time.

- Use a persisted acceptance ordinal allocated under the existing market lock; price then ordinal then stable ID selects eligible candidates.
- Price change, quantity increase or broadened execution terms receive fresh priority. Quantity reduction retains priority. Fill and unrelated metadata changes do not reset it.
- Backfill legacy priority deterministically from creation time and ID. Preserve certificates, catalog minima, smaller partial fills, STP and the 100-candidate atomic rollback cap.
- Document the cap as an explicit bounded rejection, not partial success. Do not introduce a continuation worker without evidence that current commercial sizes require one.
- Regression: same-time orders, price away/back, increase/reduction, partial fill and concurrent writes follow the declared rule. Migration and raw runtime-role ACL checks are required if persistence changes.

### E. Safe retry outcome states

Frontend: existing API error type, order review/direct-hit state and existing tests. Backend durable-command replay extension is limited to operations that can reuse an existing authoritative operation record without a new generic command subsystem.

- Classify transport timeout/network loss as outcome unknown; retain the frozen request/key and provide a deliberate safe retry. An HTTP rejection remains a rejection.
- Preserve user/auth/organization context checks and never change a key for the same intention. Render the authoritative replay response.
- Review amendment/cancel/confirm/decline/deliver/pay retry behavior. Make a bounded state-aware replay improvement where an existing immutable transition supports it; otherwise document the exact gap for a separately scoped durable-command table. Do not promise general idempotency without atomic storage.
- Regression: committed create with lost response resolves under the same key; changed context prevents retry; definitive HTTP errors do not show unknown success.

### F. Honest market and activity analytics

Frontend: `DataAnalytics.tsx`, `trading/BenchmarkPriceBlock.tsx`, the existing activity service/API adapter, relevant existing tests and locale entries. Backend: existing activity response/status and domain event schemas only if needed.

- Show source/time for quote references; call a resting quote average a quote reference, not a confirmed-trade VWAP. Preserve the forward curve's existing disclosures.
- Show static demand/fallback as sample data, and distinguish unavailable API data from a real zero. Render available source/update metadata.
- Inspect HTTP response status for activity delivery; bounded retry keeps event UUIDs and context. Bound buffer size, attempts and age; count dropped/rejected attempts without treating telemetry as the trade ledger. Reuse the existing admin coverage contract where suitable.
- Do not add consent, duplicate admin dashboards, or copy production identified analytics wholesale into the staging catalog experiment.
- Regression: failed analytics load is labelled sample/unavailable; HTTP failure is not accepted delivery; retries reuse event IDs and stop at the declared limit.

## Explicitly outside this release

New matcher/warehouse/Supabase migration, Kafka, microservices, full L2 feed, generalized benchmark harness, fresh high-concurrency capacity campaign, new collateral/credit rules, related-owner STP, product tick policy, outbox-prune activation, broad dependency upgrades and staging catalog promotion. These require measured need or a commercial policy decision and are not needed to close the source-confirmed first-batch defects.

## Completion record

The 2026-10-03 read-only production ASK profile matched all 16 serialized responses and reduced the warm service median from 114.199 ms to 74.320 ms by narrowing the eligible ID page before ORM hydration. The implemented ASK-only query carries the exact filtered total in that page, hydrates only the requested rows in the existing sort order, and runs the prior filtered count only for an out-of-range nonzero offset; BID reads and response enrichment remain unchanged.

Update this plan with task status and exact check commands. Maintain one concise release record, not a chain of overlapping reports. Source pass, runtime release, smoke scope and performance evidence must be stated separately. Once the gates pass, close test/build/profile sessions and report the remaining limits.
