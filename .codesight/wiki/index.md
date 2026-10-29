# verdaxis-backend — Wiki

_Generated 2026-10-22 — re-run `npx codesight --wiki` if the codebase has changed._

Structural map compiled from source code via AST. No LLM — deterministic, 200ms.

> **How to use safely:** These articles tell you WHERE things live and WHAT exists. They do not show full implementation logic. Always read the actual source files before implementing new features or making changes. Never infer how a function works from the wiki alone.

## Articles

- [Overview](./overview.md)
- [Database](./database.md)
- [Auth](./auth.md)
- [Activity](./activity.md)
- [Admin](./admin.md)
- [Admin_analytics](./admin_analytics.md)
- [Ai](./ai.md)
- [Alerts.rs](./alerts.rs.md)
- [Auth_simple](./auth_simple.md)
- [Auth_tokens.rs](./auth_tokens.rs.md)
- [Availability](./availability.md)
- [Catalog](./catalog.md)
- [Catalog.rs](./catalog.rs.md)
- [Compliance_api](./compliance_api.md)
- [Contract_server.rs](./contract_server.rs.md)
- [Crud_referral_invite.rs](./crud_referral_invite.rs.md)
- [Curves](./curves.md)
- [Curves_canonical.rs](./curves_canonical.rs.md)
- [Curves_legacy.rs](./curves_legacy.rs.md)
- [Export_alerts](./export_alerts.md)
- [Feedback](./feedback.md)
- [Http.rs](./http.rs.md)
- [Http_contract.rs](./http_contract.rs.md)
- [Inventory](./inventory.md)
- [Kyc](./kyc.md)
- [Market_support](./market_support.md)
- [Matchmaking](./matchmaking.md)
- [Mod.rs](./mod.rs.md)
- [Monitor](./monitor.md)
- [Negotiations](./negotiations.md)
- [News.rs](./news.rs.md)
- [Notifications](./notifications.md)
- [Orderbook](./orderbook.md)
- [Orderbook_lists.rs](./orderbook_lists.rs.md)
- [Orderbook_map.rs](./orderbook_map.rs.md)
- [Phase2_order_write_oracle](./phase2_order_write_oracle.md)
- [Phase2_transport.rs](./phase2_transport.rs.md)
- [Ports](./ports.md)
- [Ports.rs](./ports.rs.md)
- [Preferences](./preferences.md)
- [Preferences.rs](./preferences.rs.md)
- [Price_discovery](./price_discovery.md)
- [Price_headers.rs](./price_headers.rs.md)
- [Prices.rs](./prices.rs.md)
- [Rbac](./rbac.md)
- [Referrals](./referrals.md)
- [Rfq](./rfq.md)
- [Stream](./stream.md)
- [Subscription](./subscription.md)
- [Subscriptions](./subscriptions.md)
- [Supplier_offers](./supplier_offers.md)
- [Test_account_rejection_sessions](./test_account_rejection_sessions.md)
- [Test_kyc_upload_limits](./test_kyc_upload_limits.md)
- [Trade_tape.rs](./trade_tape.rs.md)
- [Trades](./trades.md)
- [Vessels](./vessels.md)
- [Watchlists](./watchlists.md)
- [Infra](./infra.md)
- [Libraries](./libraries.md)

## Quick Stats

- Routes: **230**
- Models: **330**
- Components: **0**
- Env vars: **52** required, **106** with defaults

## How to Use

- **New session:** read `index.md` (this file) for orientation — WHERE things are
- **Architecture question:** read `overview.md` (~500 tokens)
- **Domain question:** read the relevant article, then **read those source files**
- **Database question:** read `database.md`, then read the actual schema files
- **Library question:** read `libraries.md`, then read the listed source files
- **Before implementing anything:** read the source files listed in the article
- **Full source context:** read `.codesight/CODESIGHT.md`

## What the Wiki Does Not Cover

These exist in your codebase but are **not** reflected in wiki articles:
- Routes registered dynamically at runtime (loops, plugin factories, `app.use(dynamicRouter)`)
- Internal routes from npm packages (e.g. Better Auth's built-in `/api/auth/*` endpoints)
- WebSocket and SSE handlers
- Raw SQL tables not declared through an ORM
- Computed or virtual fields absent from schema declarations
- TypeScript types that are not actual database columns
- Routes marked `[inferred]` were detected via regex and may have lower precision
- gRPC, tRPC, and GraphQL resolvers may be partially captured

When in doubt, search the source. The wiki is a starting point, not a complete inventory.

---
_Last compiled: 2026-10-22 · 60 articles · [codesight](https://github.com/Houseofmvps/codesight)_