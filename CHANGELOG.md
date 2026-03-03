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

## 2026-03-03 — Email Verification + Gemini KYC (STORY-010a/b)

### Email Confirmation (STORY-010a)
- `email_verified` + `email_verification_token` columns added to `users` table (migration h7i8j9k0l1m2)
- Login now gates on `email_verified=True` (403 with clear message if not verified)
- Registration sends verification email via Resend REST API (httpx, no extra dependency)
- New endpoints:
  - `GET /auth/verify-email?token=<token>` — marks email verified, clears token
  - `POST /auth/resend-verification` — re-sends verification link
- Email service (`app/services/email.py`): gracefully no-ops if `RESEND_API_KEY` unset

### Self-Built KYC via Gemini Flash Lite (STORY-010b)
- `kyc_status` + `kyc_rejection_reason` columns added to `users` table
- KYC service (`app/services/kyc.py`): submits documents to `gemini-2.0-flash-lite` vision model
  - Checks: valid document, readable, no tampering, confidence score
  - Falls back to `gemini-1.5-flash-8b` if lite model unavailable
  - Auto-approves if `GEMINI_API_KEY` not set (safe for dev)
- New KYC router (`app/routers/kyc.py`):
  - `POST /kyc/submit` — multipart upload (passport + company_doc), triggers Gemini, auto-approves/rejects
  - `GET /kyc/status` — returns `{email_verified, kyc_status, kyc_rejection_reason}`
  - `PUT /kyc/admin/{user_id}/approve|reject` — admin override
- Approval/rejection emails sent via Resend

### Required `.env` additions to activate
```
RESEND_API_KEY=re_...
FRONTEND_URL=https://app.verdaxis.exchange
```
`GEMINI_API_KEY` likely already present. Without these, service runs normally — emails skipped, KYC auto-approves.
