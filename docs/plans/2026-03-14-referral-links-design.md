# Referral Links — Design Document

**Date:** 2026-03-14
**Goal:** Let any verified Verdaxis user create and share referral links, track who they refer, and compete on a platform-wide leaderboard.
**Scope:** Network-focused referrals — anyone can refer anyone (buyers, suppliers, brokers, sales). Tracking only, no rewards yet (data model supports future incentives).

---

## Data Model

### User Model Changes

Add to existing `users` table:

- `referral_code` — String, unique, nullable. Auto-generated on email verification as `VDX-{6 alphanumeric}` (e.g. `VDX-7K3MX9`). 2.1B possible combinations; retry on collision.
- `referred_by_id` — FK to `users.id`, nullable. Set at registration if a valid referral code was used.

### New `referrals` Table

| Column | Type | Notes |
|--------|------|-------|
| `id` | UUID | PK |
| `referrer_id` | FK → users | Who shared the link |
| `referred_user_id` | FK → users, unique | Who signed up (one referrer per user) |
| `referral_code_used` | String | Snapshot of code used at signup |
| `status` | Enum | `SIGNED_UP` → `VERIFIED` → `ACTIVE` |
| `created_at` | DateTime | When signup happened |
| `verified_at` | DateTime, nullable | When email was verified |
| `activated_at` | DateTime, nullable | When first trade occurred |

**Status progression:**
- `SIGNED_UP`: referral row created at registration
- `VERIFIED`: referred user verifies email (hook into existing verify-email endpoint)
- `ACTIVE`: referred user's first trade is confirmed (hook into trade confirmation flow)

---

## API Endpoints

### Referral Router (`app/routers/referrals.py`)

| Method | Path | Auth | Purpose |
|--------|------|------|---------|
| `GET /api/referrals/my-code` | Authenticated | Returns user's referral code + shareable link |
| `GET /api/referrals/my-referrals` | Authenticated | List of referred users (org name, status, date) |
| `GET /api/referrals/leaderboard` | Authenticated | Top referrers platform-wide (name, org, count) |
| `POST /api/referrals/invite` | Authenticated | Send referral email (rate-limited 10/hour) |
| `GET /api/referrals/resolve/{code}` | Public | Validate code, return referrer org name + type |

### Registration Change

`POST /auth/register` accepts optional `referral_code` param:
- Valid code: set `referred_by_id` on new user, create `referrals` row with `SIGNED_UP` status
- Invalid code: silently ignore (don't block registration)

---

## Frontend

### 1. Invite Landing Page (`/invite/:code`)

- Public route, no auth
- Calls `GET /api/referrals/resolve/{code}`
- Hero: "You've been invited by **[Org Name]** to join Verdaxis Exchange"
- CTA → `/register?ref={code}`
- Invalid code: falls back to generic "Join Verdaxis Exchange" → `/register`

### 2. Referrals Tab in Settings

New tab in `Settings.tsx`:
- **Top:** referral link with copy button + "Invite by Email" form (email input + send)
- **Stats cards:** Total Referrals | Verified | Active
- **Table:** each referral — org name, role (buyer/supplier), status badge, signup date
- No PII (no email) — org-level info only

### 3. Leaderboard

Below the referrals table:
- Top 10 referrers: rank, name, org, referral count
- Current user highlighted if on the board

---

## Email

**Referral invite email:**
- Resend API, follows existing template pattern (emerald/dark slate)
- Subject: "[User Name] invited you to Verdaxis Exchange"
- CTA → `/invite/{code}`
- Rate-limited: 10/hour per user
- Blocked if recipient email is already registered (friendly error)

---

## Edge Cases

- **Self-referral:** blocked — can't use your own code
- **Duplicate referral:** `referred_user_id` unique constraint — one referrer per user, first code wins
- **Deleted referrer:** referral rows persist, landing page falls back to generic
- **Code collision:** retry on collision (6 alphanum = 2.1B combinations)
- **Generation timing:** code created on email verification, not registration

## Intentionally Excluded (YAGNI)

- No rewards/credits/commissions (data model supports future addition)
- No campaign-specific links (single code per user)
- No admin referral dashboard
- No CSV bulk invite
- No referral expiry
