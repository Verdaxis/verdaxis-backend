# In-app feedback contract

## Purpose

A lightweight first-party channel for signed-in users to tell the team what is
broken, confusing, or missing — the qualitative counterpart to the product
analytics workspace. At pilot scale this is the primary "why did they stall"
instrument; the analytics tabs only show *where*.

## Privacy stance (deliberately different from product analytics)

Feedback is voluntary, user-authored, and identified. Admins see the author's
email and organization so they can reply — this is an operational support
surface like the admin Users tab, not an aggregate analytics surface, so the
product-analytics suppression and no-identity rules do not apply. Nothing is
forwarded to Umami or any external collector. The optional `page` field is a
path only — never a full URL, query string, or fragment (mirroring the
reliability telemetry rule).

## Endpoints

- `POST /api/feedback` — any authenticated user (including PENDING accounts:
  users stuck in onboarding are exactly who we need to hear from).
  Body: `{message: 1..2000 chars (trimmed, non-blank), page?: "/path"}`.
  An invalid `page` (non-path, query string, fragment, overlong) is dropped to
  null rather than failing the submission — the message is the payload.
  Rate limited 5/minute per client. Returns 201 `{id, created_at}`.
- `GET /api/admin/feedback?limit&offset` — ADMIN role, newest first, returns
  `{items: [{id, created_at, message, page, user_email, user_name,
  org_name}], total}`. Rate limited 60/minute.

## Storage

`feedback_entries`: id (uuid pk), user_id (FK users, ON DELETE CASCADE,
indexed), message (varchar 2000), page (varchar 200, nullable), created_at
(timestamptz). Append-only; no retention pruning at pilot volume. Migration
`fb_20260804_feedback_entries` (checkpoint pair allowlisted in
`deploy/migration-checkpoints.tsv`).

## Frontend

A floating "Feedback" control in the app shell opens a dialog with a single
textarea and submits with the current path. Admin review lives at
`/app/admin/feedback` (third admin tab).

## Related: onboarding attention endpoint

`GET /api/admin/onboarding-attention` (same release) exposes the
`onboarding_attention` service classification — per-user stall stage, email,
and stalled-since timestamp — so admins can reach out from the Users tab
("Needs outreach" panel). It reuses the exact stage rules the external alert
timer uses; the endpoint adds no new classification logic.
