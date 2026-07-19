# be — Overview

> **Navigation aid.** This article shows WHERE things live (routes, models, files). Read actual source files before implementing new features or making changes.

**be** is a python project built with fastapi, using sqlalchemy for data persistence.

## Scale

129 API routes · 38 database models · 66 library files · 7 middleware layers · 43 environment variables

## Subsystems

- **[Auth](./auth.md)** — 8 routes — touches: auth, db, cache, email, upload
- **[Activity](./activity.md)** — 1 routes — touches: auth, db, cache, queue
- **[Admin](./admin.md)** — 10 routes — touches: auth, db, upload
- **[Admin_analytics](./admin_analytics.md)** — 12 routes — touches: auth, db
- **[Ai](./ai.md)** — 1 routes — touches: auth
- **[Auth_simple](./auth_simple.md)** — 12 routes — touches: auth, db, cache, email
- **[Availability](./availability.md)** — 1 routes — touches: auth, db
- **[Catalog](./catalog.md)** — 2 routes — touches: auth, db
- **[Compliance](./compliance.md)** — 1 routes — touches: auth, db, upload
- **[Compliance_api](./compliance_api.md)** — 5 routes — touches: auth, db
- **[Curves](./curves.md)** — 5 routes — touches: auth, db
- **[Dashboard](./dashboard.md)** — 1 routes
- **[Inventory](./inventory.md)** — 7 routes — touches: auth, db
- **[Kyc](./kyc.md)** — 1 routes — touches: auth, db, upload
- **[Matchmaking](./matchmaking.md)** — 2 routes — touches: auth, db
- **[Monitor](./monitor.md)** — 1 routes — touches: auth, db
- **[Negotiations](./negotiations.md)** — 5 routes — touches: auth, db
- **[Notifications](./notifications.md)** — 3 routes — touches: auth, db
- **[Orderbook](./orderbook.md)** — 10 routes — touches: auth, db, cache
- **[Ports](./ports.md)** — 2 routes — touches: auth, db
- **[Preferences](./preferences.md)** — 2 routes — touches: auth, db
- **[Price_discovery](./price_discovery.md)** — 2 routes — touches: auth, db
- **[Rbac](./rbac.md)** — 1 routes — touches: auth
- **[Referrals](./referrals.md)** — 5 routes — touches: auth, db
- **[Rfq](./rfq.md)** — 4 routes — touches: auth, db
- **[Stream](./stream.md)** — 3 routes — touches: auth, cache, queue
- **[Subscription](./subscription.md)** — 1 routes — touches: auth, db
- **[Subscriptions](./subscriptions.md)** — 1 routes — touches: auth, db
- **[Trades](./trades.md)** — 3 routes — touches: auth, db
- **[Vessels](./vessels.md)** — 2 routes — touches: auth, db
- **[Watchlists](./watchlists.md)** — 9 routes — touches: auth, db
- **[Infra](./infra.md)** — 6 routes — touches: auth, db, cache, upload

**Database:** sqlalchemy, 38 models — see [database.md](./database.md)

**Libraries:** 66 files — see [libraries.md](./libraries.md)

## Required Environment Variables

- `GEMINI_API_KEY` — `.env.example`
- `ITEST_PASSWORD` — `scripts/benchmark_product_analytics.py`
- `MIGRATOR_DATABASE_URL` — `.env.example`
- `PATH` — `tests/unit/test_runtime_hardening.py`
- `UMAMI_API_PASSWORD` — `.env.example`
- `UMAMI_API_USERNAME` — `.env.example`
- `UMAMI_WEBSITE_ID` — `.env.example`

---
_Back to [index.md](./index.md) · Generated 2026-07-19_