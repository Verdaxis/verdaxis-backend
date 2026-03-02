# Verdaxis Backend Changelog

## 2026-03-02 — Phase 1 Exchange Infrastructure (7 commits)

### Enterprise Hardening (180077d)
- PyJWT + bcrypt auth, dual-token JWT, rate limiting, RBAC, audit logging, structlog

### Exchange Core (2daea86)
- SSE event bus + 3 stream endpoints (200/channel cap)
- Match-on-insert engine (price-time priority, partial fills, auto-confirmed)
- Compliance scoring (FuelEU/ETS/CII, 9 fuels, scenario engine)
- Password change endpoint (returns fresh tokens)

### Security Fixes (745a52f)
- 9 fixes from adversarial code review (subscriber cap, IDOR, rate limits)

### Data Products (5b1065b)
- Reference Price API: daily VWAP (GET /prices/reference)

### Quality Gate Fixes (c683589)
- OR-join doubling fix, rate limiting on public endpoints, datetime.now(UTC)

### Feature Branch: feature/oauth-integration
- Google + Microsoft SSO via Authlib, 169 tests on branch

## Stats: 155 tests on main, 60+ API endpoints, 15 routers
