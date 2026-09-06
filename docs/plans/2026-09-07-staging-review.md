# Verdaxis Staging Reliability and Product Review Plan

**Goal:** Improve transaction safety, market clarity, operational usefulness, and visual quality on staging, based on the 2026-09-07 source and browser review.

**Authorization:** The user explicitly authorizes frontend/backend/product/UI improvements and staging deployment without further approval. Production must remain unchanged. Luna agents implement bounded tasks; the parent reviews and integrates.

**Architecture:** Retain React/TypeScript/Vite and FastAPI/SQLAlchemy/PostgreSQL. Reuse existing idempotency records, authentication, shared UI controls, and query patterns. No new framework or speculative platform layer.

## Product and design decisions
- Preserve Forward Curve's intentional dark market-console layout (explicit user correction). Improve readability within that layout, never convert it to light mode for consistency.
- Preserve the off-platform handoff at trade confirmation; do not reinstate delivery/payment tasks.
- Keep monitoring and execution separate; demo rows must offer inspection, not misleading execution labels.
- Make the dashboard an accurate work queue: awaiting your confirmation, awaiting counterparties, and confirmed trades, counted across the organization rather than one page.
- Preserve existing approved product/port taxonomy, anonymous trading, and real/demo isolation.

## 1. Safe marketplace requests — frontend
Files: src/components/OrderPlaceModal.tsx, src/components/Marketplace.tsx, src/services/api.ts, relevant EN/ZH trading strings and tests.
- Give every order/trade submission a stable Idempotency-Key per exact draft. Reuse after an uncertain network result; reset only for a new or explicitly edited draft. Preserve assisted confirmation payload locking.
- Send sort_by=price_asc|price_desc|quantity_desc|newest to paged asks/bids; stop sorting only eight server-selected rows.
- Use View demo for demo row actions; retain clear demo explanation and disabled execution in details.
- Improve order modal keyboard access, labels, and actionable error/retry feedback using existing controls.
- Prove timeout-then-retry uses the same key, changed intent does not replay old payload, and sort parameters reach the API.

## 2. Restore private event streaming — frontend
Files: src/hooks/useSSE.ts, src/components/TradeNotifier.tsx if required, focused tests.
- Obtain an authenticated short-lived stream token from GET /auth/stream-token for private trade connections; do not put access tokens in URLs.
- Refresh the stream token on reconnect, preserve the last durable event id, handle server reset/auth signals, and cancel pending connection work on logout/unmount.
- Public price/orderbook behavior remains intact. Prove successful auth, replay, and cleanup with a fake EventSource.

## 3. Correct marketplace and operational queries — backend
Files: app/routers/orderbook.py, app/routers/trades.py, app/schemas/orderbook.py as needed, focused unit tests.
- Validate sort_by and apply stable SQL ordering before pagination for both bids and asks; preserve existing default for compatibility.
- Add GET /trades/summary returning total_count, action_required_count, awaiting_counterparty_count, confirmed_count for the effective organization. Confirmed includes historical DELIVERED/PAID states, but these are not new workflow tasks.
- Add optional action_required=true to GET /trades/my. Use the same organization-scoped, non-initiating-party predicate for counts and list; derive organization identity safely from existing request-party resolution.
- No schema migration expected. Test tenant isolation, mixed initiator roles, more than one page, and sorting ties.

## 4. Accurate, focused command center — frontend
Files: src/components/CommandCenter.tsx, NeedsAttentionFeed.tsx, relevant locale entries, api.ts small additions, tests.
- Consume summary + bounded actionable list rather than deriving totals from first 20 trades.
- Remove permanently zero match metric; show clear request-failure/retry states rather than All caught up after a fetch error.
- Make actions primary and keep market watch secondary. Replace oversized duplicate CTA tiles with restrained operational hierarchy.
- Preserve buyer/supplier and assisted-context behavior. Complete English/Chinese parity.

## 5. Intentional dark market console — visual design
Files: ForwardCurveWorkspace.tsx and its scoped styles; shared shell styles only where justified.
- Preserve dark theme and functional matrix/curve/inspector.
- Improve muted-text contrast, typography minimums, selected-state visibility, price hierarchy, and layout at 1024/1440/1920 widths.
- Preserve data provenance labels, chart gaps, dates, and Marketplace handoff. No invented data or decorative chart.
- Validate browser screenshots and keyboard controls. Respect mobile desktop gate as existing product policy.

## 6. PostgreSQL market verification — backend CI
Files: .github/workflows/backend-ci.yml, existing disposable PostgreSQL test scripts/fixtures only as needed.
- Provision a dedicated *_market_integrity_test database and required roles; configure MARKET_INTEGRITY_TEST_DATABASE_URL.
- Ensure market locking, idempotency, schema/ACL tests actually run; fail required CI checks on an unconfigured suite rather than silently skipping.
- Run the smallest reproducible disposable PostgreSQL check locally. Never point mutating tests at staging/production.

## 7. Integration and release
- Parent reviews each diff for requirement fit and code quality, runs focused regressions then required FE/BE checks.
- Keep existing work in canonical staging checkouts (currently tasks/lessons.md edits) intact; isolate all implementation in worktrees.
- Release backend with exact SHA and existing migration checkpoint through its guarded deploy helper; release frontend with a validated staging artifact.
- Browser check buyer/supplier home, marketplace sorting, order draft and demo details, private SSE, Forward Curve and settings at relevant viewports. Do not send third-party messages or place real customer trades.
- Save before/after evidence and a final concise report explaining changes, reasons, checks, and remaining limitations.
