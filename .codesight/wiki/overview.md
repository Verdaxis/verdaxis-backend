# be — Overview

> **Navigation aid.** This article shows WHERE things live (routes, models, files). Read actual source files before implementing new features or making changes.

**be** is a python project built with fastapi, using sqlalchemy for data persistence.

## Scale

143 API routes · 47 database models · 88 library files · 13 middleware layers · 63 environment variables

## Subsystems

- **[Auth](./auth.md)** — 6 routes — touches: auth, db, cache, queue, email
- **[Activity](./activity.md)** — 1 routes — touches: auth, db, cache, queue
- **[Admin](./admin.md)** — 12 routes — touches: auth, db, cache, queue, email
- **[Admin_analytics](./admin_analytics.md)** — 12 routes — touches: auth, db
- **[Ai](./ai.md)** — 1 routes — touches: auth
- **[Auth_simple](./auth_simple.md)** — 16 routes — touches: auth, db, cache, queue, email
- **[Availability](./availability.md)** — 1 routes — touches: auth, db
- **[Catalog](./catalog.md)** — 2 routes — touches: auth, db
- **[Compliance_api](./compliance_api.md)** — 5 routes — touches: auth, db
- **[Curves](./curves.md)** — 5 routes — touches: auth, db
- **[Inventory](./inventory.md)** — 7 routes — touches: auth, db, queue
- **[Kyc](./kyc.md)** — 1 routes — touches: auth, db, upload
- **[Market_support](./market_support.md)** — 11 routes — touches: auth, db, queue
- **[Matchmaking](./matchmaking.md)** — 2 routes — touches: auth, db
- **[Monitor](./monitor.md)** — 1 routes — touches: auth, db
- **[Negotiations](./negotiations.md)** — 5 routes — touches: auth, db, queue
- **[Notifications](./notifications.md)** — 3 routes — touches: auth, db
- **[Orderbook](./orderbook.md)** — 10 routes — touches: auth, db, cache, queue
- **[Ports](./ports.md)** — 2 routes — touches: auth, db
- **[Preferences](./preferences.md)** — 2 routes — touches: auth, db
- **[Price_discovery](./price_discovery.md)** — 2 routes — touches: auth, db
- **[Rbac](./rbac.md)** — 1 routes — touches: auth
- **[Referrals](./referrals.md)** — 5 routes — touches: auth, db
- **[Rfq](./rfq.md)** — 4 routes — touches: auth, db, queue
- **[Stream](./stream.md)** — 3 routes — touches: auth, db, cache, queue
- **[Subscription](./subscription.md)** — 1 routes — touches: auth, db
- **[Subscriptions](./subscriptions.md)** — 1 routes — touches: auth, db
- **[Test_kyc_upload_limits](./test_kyc_upload_limits.md)** — 1 routes — touches: auth, db, upload
- **[Trades](./trades.md)** — 3 routes — touches: auth, db, queue
- **[Vessels](./vessels.md)** — 2 routes — touches: auth, db
- **[Watchlists](./watchlists.md)** — 9 routes — touches: auth, db
- **[Infra](./infra.md)** — 6 routes — touches: auth, db, cache, upload, queue

**Database:** sqlalchemy, 47 models — see [database.md](./database.md)

**Libraries:** 88 files — see [libraries.md](./libraries.md)

## Required Environment Variables

- `BACKUP_DATABASE_URL` — `tests/postgres/test_runtime_role_policy.py`
- `DATABASE_URL` — `tests/postgres/test_runtime_role_policy.py`
- `DEMO_OUTPUT` — `tests/monitor/test_runtime_identity.py`
- `DISPOSABLE_ITEST_PASSWORD` — `tests/conftest.py`
- `DISPOSABLE_TEST_TOKEN` — `tests/disposable_server.py`
- `GEMINI_API_KEY` — `.env.example`
- `GIT_CONFIG_GLOBAL` — `tests/unit/test_migration_checkpoint.py`
- `ITEST_PASSWORD` — `scripts/benchmark_product_analytics.py`
- `JWT_SECRET_PREVIOUS` — `.env.example`
- `MARKET_INTEGRITY_TEST_DATABASE_URL` — `tests/postgres/test_market_integrity_concurrency.py`
- `MARKET_REMEDIATION_DATABASE_URL` — `scripts/remediate_market_data.py`
- `MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS` — `.env.example`
- _...12 more_

---
_Back to [index.md](./index.md) · Generated 2026-07-22_