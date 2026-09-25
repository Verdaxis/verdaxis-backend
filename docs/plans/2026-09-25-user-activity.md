# User Activity Implementation Plan

**Goal:** Let administrators inspect actions by signed-in user, including browsing and market interest, with 90-day browsing retention.

**Architecture:** Reuse login-day facts and durable business audit records. Store browsing separately with authenticated server identity, bounded event fields and server receipt timestamps. Privacy terms are maintained by a separate policy system. Extend the existing admin Users view with a filtered, paginated timeline; keep Umami anonymous.

**Tech Stack:** Existing FastAPI, SQLAlchemy, PostgreSQL/Alembic, React, TypeScript, Vitest and pytest.

## Approved design
User approved on 2026-09-25 after reviewing scope and storage estimates.
- Show recorded logins, pages/market views, market filters, watchlist/pin changes, orders, RFQs and trades.
- The separate privacy policy covers account-linked browsing collection; this application does not issue or validate an analytics consent prompt.
- Collect canonical page/product/port/period fields only; no free text, tokens, query strings or session replay.
- UI shows source and limits: login facts are daily aggregates; browsing is client-reported, not proof of a completed transaction. No inferred active-time metric.
- Keep detailed browsing for 90 days. Existing security/business retention stays unchanged.
- No retroactive reconstruction of anonymous visits.

## Task 1: Backend storage, intake and timeline
Files: app/models/user_activity.py, app/schemas/user_activity.py, app/services/user_activity.py, app/routers/user_activity.py, app/main.py, app/models/__init__.py; exact names may follow adjacent conventions.
Add a single indexed table with per-user event UUID deduplication; forbid caller-supplied identity and time. Accept an optional legacy version-2 marker for rolling compatibility, allowlisted actions and canonical fields, max 50 per batch, authenticated rate limit.
POST /api/activity/events. GET /api/admin/users/{user_id}/activity with days=7|30|90, kind=all|browsing|business|login, limit <=100, offset. Return items, has_more and last_activity_at.
Combine safe audit projections, login-day aggregates, and retained browsing in consistent time order. Add missing watchlist and market-watch preference audit records at successful mutations.
Run focused tests for authentication, admin-only access, spoofing, duplicates, filters and page boundaries.

## Task 2: Schema and retention
Files: one Alembic migration after fee_20260912_seller_per_mt, deploy/migration-checkpoints.tsv, deploy/postgres/app_acl_policy.sql, scripts/prune_product_analytics.py.
Grant only required existing-role permissions and extend the existing prune job. Do not add a timer/service.
Prove upgrade/model parity, app-role INSERT/read privileges, denied updates and retention in a disposable PostgreSQL database. Never test migrations on live data.

## Task 3: Frontend collection
Files: new activity adapter/provider, src/App.tsx, src/services/cookiePreferences.ts, market components, docs/behavioral-analytics.md.
Bound queue and batch requests; clear on logout and account changes. Quiet failure must not log out users, show telemetry errors, or disrupt app operations.
Instrument normalized route views and deliberate canonical market views/filter selections. Exclude hidden retained-map effects. Reuse existing API/session architecture.
Test identity changes, invalid fields and market events.

## Task 4: Admin timeline
Files: src/components/admin/AdminDashboard.tsx, new UserActivity component, src/services/api.ts and activity types.
Add View activity per user. Show date/source filters, readable action details, timestamps, last activity, pagination and loading/error/empty states using existing admin components.
Test filtering/pagination, stale-user response isolation, and safe rendering.

## Task 5: Integration and review
Run backend unit tests and disposable PostgreSQL migration checks; frontend tests, typecheck, translations and build artifact validation. Run browser verification against a disposable/local target. Obtain independent review for auth boundaries, shared types and real lifecycle coverage. Address findings before integration.
Record exact commits and check results. Production release must follow existing guarded migration/backend and immutable frontend release procedures; no direct source swaps or migration aliases.
