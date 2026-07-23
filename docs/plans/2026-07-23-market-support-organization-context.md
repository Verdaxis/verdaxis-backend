# Market Support Organization Context Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let an authorized administrator enter a real supplier organization and use the normal supplier platform to create and cancel assisted ASK listings without impersonating a customer.

**Architecture:** A short-lived database row binds the real admin actor to one effective organization, accountable supplier, support reference, and narrow scope. Normal customer endpoints resolve an explicit request party from the real bearer token plus an opaque context header. The frontend stores only that opaque ID in session storage, reuses the normal route tree, and adds persistent context chrome and a thin final ASK confirmation.

**Tech Stack:** FastAPI, SQLAlchemy async, PostgreSQL/Alembic, React 19, TypeScript, React Router, Vitest, Pytest, Playwright/agent-browser.

---

### Task 1: Context persistence and contracts

**Files:**
- Modify: `app/models/market_support.py`
- Modify: `app/models/__init__.py`
- Modify: `app/schemas/market_support.py`
- Modify: `app/config.py`
- Create: `alembic/versions/ms_20260723_organization_context.py`
- Modify: `deploy/migration-checkpoints.tsv`
- Test: `tests/unit/test_market_support_contract.py`

**Steps:**
1. Add failing contract tests for context lifecycle enums, constraints, settings, and authorization-to-context linkage.
2. Add `MarketSupportContext` with actor, organization, accountable principal, support reference, scope, expiry, end state, and version.
3. Add nullable `market_support_context_id` to existing authorizations for compatibility and require it for new context-mode operations.
4. Add the explicit migration checkpoint after `ms_20260723_assisted_listings`.
5. Run the focused unit and migration-model tests.
6. Commit the schema slice.

### Task 2: Context lifecycle API

**Files:**
- Modify: `app/routers/market_support.py`
- Modify: `app/schemas/market_support.py`
- Modify: `app/services/market_support.py`
- Modify: `app/services/audit_actions.py`
- Test: `tests/unit/test_market_support_routes.py`
- Test: `tests/integration/test_market_support_context.py`

**Steps:**
1. Write failing tests for entry eligibility, start, resume, foreign-context denial, expiry, replacement, and idempotent exit.
2. Implement organization entry details without exposing KYC evidence.
3. Implement one-active-context lifecycle with row locks and complete audit events.
4. Revalidate feature flag, both capabilities, approved `REAL` organization, membership, supplier role, and eligibility.
5. Run unit tests and disposable PostgreSQL integration tests.
6. Commit the lifecycle slice.

### Task 3: Explicit request party and mutation firewall

**Files:**
- Create: `app/services/request_party.py`
- Create: `app/middleware/market_support_scope.py`
- Modify: `app/main.py`
- Test: `tests/unit/test_market_support_request_party.py`
- Test: `tests/security/test_market_support_mutation_allowlist.py`

**Steps:**
1. Write failing tests proving unscoped admins remain unable to execute, context IDs cannot cross actors, and organization/principal headers are rejected.
2. Implement immutable self-service and Market Support request parties.
3. Resolve the opaque context from `X-Verdaxis-Market-Support-Context`.
4. Add a route-method allowlist that rejects every other support-mode mutation.
5. Add route enumeration so a new mutation fails tests until classified.
6. Run focused security tests and commit.

### Task 4: Normal orderbook ASK creation

**Files:**
- Modify: `app/schemas/orderbook.py`
- Modify: `app/routers/orderbook.py`
- Modify: `app/services/market_support.py`
- Test: `tests/unit/test_orderbook_market_support.py`
- Test: `tests/integration/test_market_support_orderbook.py`

**Steps:**
1. Write failing tests for support confirmation, idempotency, exact dual attribution, evidence disposal, post-only crossing rejection, and atomic authorization consumption.
2. Extend the normal order request with an optional support confirmation accepted only in context mode.
3. Reuse the current canonical term digest, slice locks, crossing checks, authorization model, audit, notifications, and market-event pipeline.
4. Persist no evidence plaintext and expose no support internals publicly.
5. Run unit and concurrency integration tests and commit.

### Task 5: Normal own-order reads and cancellation

**Files:**
- Modify: `app/schemas/orderbook.py`
- Modify: `app/routers/orderbook.py`
- Create: `app/services/order_cancellation.py`
- Test: `tests/integration/test_market_support_orderbook.py`

**Steps:**
1. Write failing tests for effective-organization own orders, assisted-only cancellation, reason, ETag errors, race outcomes, and cleanup after eligibility loss.
2. Add canonical `POST /orderbook/{id}/cancel`; retain the legacy delete wrapper for ordinary-client compatibility.
3. Share one locked cancellation transaction between both routes.
4. Record original and cancellation principals plus actor/context identity.
5. Run focused tests and commit.

### Task 6: Required read routes

**Files:**
- Modify only the organization-scoped GET routers required by the rendered supplier shell.
- Test: focused router tests for each changed read.

**Steps:**
1. Inventory network calls made by the normal supplier route tree.
2. Make only approved organization-owned reads request-party aware.
3. Keep principal-private pages unavailable unless explicitly approved.
4. Leave all mutations on ordinary authorization and the support firewall.
5. Run focused tests and commit.

### Task 7: Frontend context transport and lifecycle

**Files in `verdaxis-frontend`:**
- Create: `src/context/MarketSupportContext.tsx`
- Create: `src/services/marketSupportContextStore.ts`
- Modify: `src/services/api.ts`
- Modify: `src/context/AuthContext.tsx`
- Test: `src/tests/market-support-context.test.tsx`
- Test: `src/tests/api-market-support-context.test.ts`

**Steps:**
1. Write failing tests for opaque session storage, request-header allowlist, refresh retry, structured expiry, logout, and broadcast exit.
2. Implement bootstrap, enter, explicit resume, exit, invalidation, and no-identity storage.
3. Inject the context header only into approved customer requests, never auth requests.
4. Handle `204` without JSON parsing.
5. Run Vitest and commit.

### Task 8: Full customer shell with context chrome

**Files in `verdaxis-frontend`:**
- Modify: `src/App.tsx`
- Modify: `src/components/Layout.tsx`
- Modify: `src/components/layout/Header.tsx`
- Modify: `src/components/layout/Sidebar.tsx`
- Create: `src/components/market-support/ActingOrganizationBanner.tsx`
- Test: `src/tests/app-routing.test.tsx`
- Test: `src/tests/market-support-banner.test.tsx`

**Steps:**
1. Write failing route and identity tests.
2. Derive effective supplier mode without overwriting the admin's base mode.
3. Block rendering until context rehydration resolves.
4. Render the banner on every customer route and preserve the real admin profile.
5. Hide role switching and admin navigation while attached; redirect direct admin paths.
6. Run tests and commit.

### Task 9: Entry from organization management

**Files in `verdaxis-frontend`:**
- Modify: `src/components/admin/AdminDashboard.tsx`
- Create: `src/components/admin/market-support/MarketSupportEntryDialog.tsx`
- Modify the existing Users/Organizations component used by `AdminDashboard`.
- Test: admin organization-entry component tests.

**Steps:**
1. Write failing tests for eligible/ineligible states and principal selection.
2. Add `Enter supplier platform` to organization details.
3. Require support reference and explicit scope confirmation.
4. Start context and navigate to the normal supplier home.
5. Remove the standalone Market Support tab from navigation.
6. Run tests and commit.

### Task 10: Reuse normal ASK form and cancellation

**Files in `verdaxis-frontend`:**
- Modify: `src/components/OrderPlaceModal.tsx`
- Create: `src/components/market-support/MarketSupportFinalConfirmation.tsx`
- Modify the normal own-order/listing management component.
- Modify: `src/services/api.ts`
- Test: `src/tests/order-place-modal.test.tsx`
- Test: marketplace/listing cancellation tests.

**Steps:**
1. Write failing tests for final confirmation, exact draft invalidation, evidence cleanup, stable retry key, support badge, and ETag cancellation.
2. Insert the support confirmation after normal form validation without duplicating fields.
3. Submit the normal order request with the transient confirmation.
4. Reuse the normal own-order UI for support-created listings and canonical cancellation.
5. Run tests and commit.

### Task 11: Deny unsupported frontend actions

**Files in `verdaxis-frontend`:**
- Modify only components that expose mutations while the supplier shell is rendered.
- Add shared support-scope capability helpers.
- Test each hidden/disabled mutation group.

**Steps:**
1. Inventory all mutation controls and background writes in the supplier shell.
2. Disable or hide trade, RFQ, inventory, settings, watchlist, alert, notification, and tutorial mutations.
3. Preserve approved read-only content.
4. Surface server denial consistently if a request is forced.
5. Run focused and route tests and commit.

### Task 12: Remove the parallel workspace and verify

**Files:**
- Delete frontend `src/components/admin/market-support/MarketSupportWorkspace.tsx`.
- Remove obsolete frontend API methods and types.
- Retire backend workspace routes after the coordinated staging build.
- Modify both `ARCHITECTURE.md` files and translation resources.

**Steps:**
1. Prove no current frontend call references the old workspace endpoints.
2. Delete the workspace component/tab and old client contracts.
3. Run backend unit tests, disposable PostgreSQL integration tests, frontend Vitest, i18n, typecheck, staging build, and build artifact checks.
4. Dogfood admin entry, refresh, ASK creation preview, cancellation preview, exit, direct links, duplicate tabs, expiry, and normal buyer/supplier flows.
5. Deploy only to staging through the reviewed migration and build procedures.
6. Verify public staging readiness and leave production untouched.
