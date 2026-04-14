# be — Overview

> **Navigation aid.** This article shows WHERE things live (routes, models, files). Read actual source files before implementing new features or making changes.

**be** is a python project built with fastapi, using sqlalchemy for data persistence.

## Scale

109 API routes · 30 database models · 56 library files · 4 middleware layers · 18 environment variables

## Subsystems

- **[Auth](./auth.md)** — 8 routes — touches: auth, db, email, upload
- **[Activity](./activity.md)** — 1 routes — touches: auth, cache, queue
- **[Admin](./admin.md)** — 9 routes — touches: auth, db, upload
- **[Admin_analytics](./admin_analytics.md)** — 2 routes — touches: auth, db
- **[Ai](./ai.md)** — 1 routes — touches: auth
- **[Auth_simple](./auth_simple.md)** — 10 routes — touches: auth, db, email
- **[Availability](./availability.md)** — 1 routes — touches: auth, db
- **[Catalog](./catalog.md)** — 2 routes — touches: auth, db
- **[Compliance](./compliance.md)** — 1 routes — touches: auth, db, upload
- **[Compliance_api](./compliance_api.md)** — 4 routes — touches: auth, db
- **[Curves](./curves.md)** — 2 routes — touches: auth, db
- **[Dashboard](./dashboard.md)** — 1 routes
- **[Inventory](./inventory.md)** — 7 routes — touches: auth, db
- **[Kyc](./kyc.md)** — 1 routes — touches: auth, db, upload
- **[Matchmaking](./matchmaking.md)** — 2 routes — touches: auth, db
- **[Negotiations](./negotiations.md)** — 5 routes — touches: auth, db
- **[Notifications](./notifications.md)** — 3 routes — touches: auth, db
- **[Orderbook](./orderbook.md)** — 10 routes — touches: auth, db
- **[Ports](./ports.md)** — 2 routes — touches: auth, db
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

**Database:** sqlalchemy, 30 models — see [database.md](./database.md)

**Libraries:** 56 files — see [libraries.md](./libraries.md)

## Required Environment Variables

- `ENVIRONMENT` — `app/config.py`
- `GEMINI_API_KEY` — `.env.example`
- `TEST_API_URL` — `tests/conftest.py`

---
_Back to [index.md](./index.md) · Generated 2026-04-14_