# verdaxis-backend — Overview

> **Navigation aid.** This article shows WHERE things live (routes, models, files). Read actual source files before implementing new features or making changes.

**verdaxis-backend** is a python project built with fastapi, axum, using sqlalchemy for data persistence.

## Scale

332 API routes · 331 database models · 228 library files · 47 middleware layers · 182 environment variables

## Subsystems

- **[Auth](./auth.md)** — 42 routes — touches: auth, db, email, queue, upload
- **[Activity](./activity.md)** — 1 routes — touches: auth, cache
- **[Admin](./admin.md)** — 53 routes — touches: db, queue, cache, auth, upload
- **[Admin_analytics](./admin_analytics.md)** — 12 routes — touches: auth, db
- **[Admission_postgres.rs](./admission_postgres.rs.md)** — 1 routes — touches: db
- **[Ai](./ai.md)** — 1 routes
- **[Alerts.rs](./alerts.rs.md)** — 3 routes — touches: auth, db
- **[Auth_http.rs](./auth_http.rs.md)** — 1 routes — touches: auth, db, email
- **[Auth_simple](./auth_simple.md)** — 18 routes — touches: auth, db, email
- **[Auth_tokens.rs](./auth_tokens.rs.md)** — 1 routes — touches: auth, db
- **[Availability](./availability.md)** — 1 routes — touches: db
- **[Catalog](./catalog.md)** — 2 routes — touches: db
- **[Catalog.rs](./catalog.rs.md)** — 2 routes — touches: db, cache
- **[Compliance_api](./compliance_api.md)** — 5 routes — touches: auth, db
- **[Compliance_scores.rs](./compliance_scores.rs.md)** — 1 routes — touches: auth, db
- **[Contract_server.rs](./contract_server.rs.md)** — 1 routes — touches: auth, cache
- **[Crud_referral_invite.rs](./crud_referral_invite.rs.md)** — 1 routes — touches: auth, email
- **[Curves](./curves.md)** — 5 routes
- **[Curves_canonical.rs](./curves_canonical.rs.md)** — 3 routes — touches: db
- **[Curves_legacy.rs](./curves_legacy.rs.md)** — 2 routes — touches: db
- **[Export_alerts](./export_alerts.md)** — 2 routes
- **[Feedback](./feedback.md)** — 1 routes
- **[Http.rs](./http.rs.md)** — 9 routes — touches: auth, db
- **[Http_contract.rs](./http_contract.rs.md)** — 4 routes — touches: auth, db
- **[Inventory](./inventory.md)** — 7 routes — touches: db
- **[Kyc](./kyc.md)** — 1 routes — touches: auth, db, upload
- **[Market_support](./market_support.md)** — 16 routes — touches: db, auth
- **[Market_write](./market_write.md)** — 19 routes — touches: auth, db
- **[Market_write_pg_create.rs](./market_write_pg_create.rs.md)** — 1 routes — touches: auth, db
- **[Market_write_pg_races.rs](./market_write_pg_races.rs.md)** — 1 routes — touches: auth, db
- **[Matchmaking](./matchmaking.md)** — 2 routes — touches: db
- **[Mod.rs](./mod.rs.md)** — 8 routes — touches: auth
- **[Monitor](./monitor.md)** — 1 routes — touches: db
- **[Negotiations](./negotiations.md)** — 5 routes — touches: db
- **[News.rs](./news.rs.md)** — 1 routes — touches: db, cache
- **[Notifications](./notifications.md)** — 4 routes — touches: db
- **[Orderbook](./orderbook.md)** — 15 routes — touches: db, queue, auth
- **[Orderbook_lists.rs](./orderbook_lists.rs.md)** — 3 routes — touches: auth, db, cache
- **[Orderbook_map.rs](./orderbook_map.rs.md)** — 7 routes — touches: db
- **[Phase2_order_write_oracle](./phase2_order_write_oracle.md)** — 2 routes — touches: auth, db
- **[Phase2_transport.rs](./phase2_transport.rs.md)** — 1 routes — touches: auth, db
- **[Phase3_auth.rs](./phase3_auth.rs.md)** — 1 routes — touches: auth
- **[Phase4.rs](./phase4.rs.md)** — 2 routes — touches: auth, db, upload
- **[Phase4_kyc_review.rs](./phase4_kyc_review.rs.md)** — 1 routes — touches: auth, db
- **[Ports](./ports.md)** — 2 routes — touches: db
- **[Ports.rs](./ports.rs.md)** — 2 routes — touches: db, cache
- **[Preferences](./preferences.md)** — 1 routes — touches: db
- **[Preferences.rs](./preferences.rs.md)** — 2 routes — touches: auth, db
- **[Price_discovery](./price_discovery.md)** — 2 routes — touches: auth
- **[Price_headers.rs](./price_headers.rs.md)** — 3 routes
- **[Prices.rs](./prices.rs.md)** — 2 routes — touches: db
- **[Rbac](./rbac.md)** — 1 routes
- **[Referrals](./referrals.md)** — 5 routes — touches: db
- **[Rfq](./rfq.md)** — 6 routes — touches: db
- **[Routes.rs](./routes.rs.md)** — 2 routes — touches: auth, db
- **[Stream](./stream.md)** — 3 routes — touches: auth, cache, db
- **[Subscription](./subscription.md)** — 1 routes — touches: db
- **[Subscriptions](./subscriptions.md)** — 2 routes
- **[Supplier_offers](./supplier_offers.md)** — 3 routes — touches: db
- **[Test_account_rejection_sessions](./test_account_rejection_sessions.md)** — 1 routes
- **[Test_kyc_upload_limits](./test_kyc_upload_limits.md)** — 1 routes — touches: auth, upload
- **[Test_phase3_cross_stack_auth](./test_phase3_cross_stack_auth.md)** — 1 routes
- **[Trade_tape.rs](./trade_tape.rs.md)** — 1 routes — touches: db
- **[Trades](./trades.md)** — 4 routes — touches: db, auth
- **[Vessels](./vessels.md)** — 2 routes — touches: db
- **[Watchlists](./watchlists.md)** — 9 routes — touches: db
- **[Infra](./infra.md)** — 6 routes — touches: db

**Database:** sqlalchemy, unknown, 331 models — see [database.md](./database.md)

**Libraries:** 228 files — see [libraries.md](./libraries.md)

## High-Impact Files

Changes to these files have the widest blast radius across the codebase:

- `app/models/user.py` — imported by **174** files
- `app/database.py` — imported by **129** files
- `app/models/orderbook.py` — imported by **113** files
- `app/market_catalog.py` — imported by **93** files
- `app/config.py` — imported by **72** files
- `app/models/catalog.py` — imported by **69** files

## Required Environment Variables

- `AUTH_INTEROP_ADMIN_URL` — `tests/rust_contract/phase2/auth_writer.py`
- `AUTH_INTEROP_EXPECT_BLOCKED` — `tests/rust_contract/phase2/auth_writer.py`
- `AUTH_INTEROP_SUBJECT` — `tests/rust_contract/phase2/auth_writer.py`
- `BACKUP_DATABASE_URL` — `tests/postgres/test_runtime_role_policy.py`
- `CADDY_BIN` — `scripts/rust_contract/measure.py`
- `CHROMEDRIVER_BIN` — `deploy/external_monitor/verdaxis_monitor.py`
- `CREDENTIALS_DIRECTORY` — `app/services/kyc_document_vault.py`
- `DATABASE_URL` — `rs/read/tests/fixtures/offboarding_python_harness.py`
- `DEMO_OUTPUT` — `tests/monitor/test_runtime_identity.py`
- `DISPOSABLE_ITEST_PASSWORD` — `scripts/rust_contract/seed_integration.py`
- `DISPOSABLE_TEST_TOKEN` — `rs/read/tests/curves_canonical_oracle.py`
- `GEMINI_API_KEY` — `.env.example`
- _...63 more_

---
_Back to [index.md](./index.md) · Generated 2026-11-05_