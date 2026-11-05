# Admin

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Admin subsystem handles **19 routes** and touches: db, queue, cache, auth.

## Routes

- `GET` `/api/admin/onboarding-attention` → out: AttentionResponse
  `app/routers/admin_attention.py`
- `GET` `/admin/audit-logs` → in: Annotated, out: list [db]
  `app/routers/audit.py`
- `GET` `/api/admin/invitations/organizations` → in: Annotated, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `POST` `/api/admin/invitations` → in: _Request, out: LoginResponse [db]
  `app/routers/auth_simple.py`
- `GET` `/api/admin/review-queue` → in: Annotated, out: LoginResponse [db, queue]
  `app/routers/auth_simple.py`
- `GET` `/api/admin/review-queue/{user_id}` params(user_id) → in: Annotated, out: LoginResponse [queue]
  `app/routers/auth_simple.py`
- `GET` `/api/admin/feedback` → out: FeedbackCreated [db]
  `app/routers/feedback.py`
- `GET` `/admin/{user_id}/documents` params(user_id) → in: Annotated [db]
  `app/routers/kyc.py`
- `GET` `/admin/documents/{document_id}` params(document_id) → in: Annotated [cache]
  `app/routers/kyc.py`
- `PUT` `/admin/{user_id}/approve` params(user_id) → in: uuid [db]
  `app/routers/kyc.py`
- `PUT` `/admin/{user_id}/reject` params(user_id) → in: uuid [db]
  `app/routers/kyc.py`
- `GET` `/admin/commissions` → in: Use, out: list [db]
  `app/routers/orders.py`
- `GET` `/admin/commissions/summary` → in: Use, out: list [db]
  `app/routers/orders.py`
- `PUT` `/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [db]
  `app/routers/orders.py`
- `GET` `/admin/subscriptions` → in: Annotated, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `GET` `/admin/subscriptions/{org_id}` params(org_id) → in: Annotated, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `PUT` `/admin/subscriptions/{org_id}` params(org_id) → in: uuid, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `GET` `/api/admin/analytics/overview`
  `tests/unit/test_preauth_rate_limit.py`
- `GET` `/api/admin/analytics` [auth]
  `rs/read/tests/phase2_http.rs`

## Related Models

- **app__routers__admin_analytics__OverviewResponse** (10 fields) → [database.md](./database.md)

## High-Impact Files

- `app/routers/auth_simple.py` — imported by 61 files

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/admin_attention.py`
- `app/routers/audit.py`
- `app/routers/auth_simple.py`
- `app/routers/feedback.py`
- `app/routers/kyc.py`
- `app/routers/orders.py`
- `app/routers/subscriptions.py`
- `tests/unit/test_preauth_rate_limit.py`
- `rs/read/tests/phase2_http.rs`

---
_Back to [overview.md](./overview.md)_