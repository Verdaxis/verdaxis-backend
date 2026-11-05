# Admin

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Admin subsystem handles **53 routes** and touches: db, queue, cache, auth.

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
- `GET` `/api/admin/commissions` → in: Use, out: list [db]
  `app/routers/orders.py`
- `GET` `/api/admin/commissions/summary` → in: Use, out: list [db]
  `app/routers/orders.py`
- `PUT` `/api/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [db]
  `app/routers/orders.py`
- `GET` `/admin/subscriptions` → in: Annotated, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `GET` `/admin/subscriptions/{org_id}` params(org_id) → in: Annotated, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `PUT` `/admin/subscriptions/{org_id}` params(org_id) → in: uuid, out: FeeScheduleResponse [db]
  `app/routers/subscriptions.py`
- `GET` `/api/admin/analytics/overview`
  `tests/unit/test_preauth_rate_limit.py`
- `PUT` `/api/admin/analytics/users/{user_id}/reject` params(user_id) [auth, db, queue]
  `rs/read/src/business/admission.rs`
- `PUT` `/api/kyc/admin/{user_id}/approve` params(user_id) [auth, db]
  `rs/read/src/kyc/review.rs`
- `PUT` `/api/kyc/admin/{user_id}/reject` params(user_id) [auth, db]
  `rs/read/src/kyc/review.rs`
- `GET` `/api/orders/admin/commissions` [auth]
  `rs/read/src/routes/market_write/commission.rs`
- `GET` `/api/orders/admin/commissions/summary` [auth]
  `rs/read/src/routes/market_write/commission.rs`
- `PUT` `/api/orders/admin/commissions/{commission_id}` params(commission_id) [auth]
  `rs/read/src/routes/market_write/commission.rs`
- `GET` `/api/admin/trades/frozen` [auth]
  `rs/read/src/routes/market_write/freeze.rs`
- `POST` `/api/admin/trades/{trade}/freeze` params(trade) [auth]
  `rs/read/src/routes/market_write/freeze.rs`
- `POST` `/api/admin/trades/{trade}/unfreeze` params(trade) [auth]
  `rs/read/src/routes/market_write/freeze.rs`
- `GET` `/api/admin/market-support/capabilities` [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/capability-assignments` [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/capability-assignments/{id}/revoke` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/organizations` [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/organizations/{id}/authorizations` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/organizations/{id}/authorizations` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/organizations/{id}/listings` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/organizations/{id}/listings` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/organizations/{id}/listings/{order}/cancel` params(id, order) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/organizations/{id}/authorizations/{authorization}/revoke` params(id, authorization) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/organizations/{id}/entry` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/contexts` [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/contexts/active` [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/contexts/{id}` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `POST` `/api/admin/market-support/contexts/{id}/exit` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/admin/market-support/organizations/{id}/context` params(id) [auth]
  `rs/read/src/routes/market_write/support.rs`
- `GET` `/api/kyc/admin/{user_id}/documents` params(user_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `GET` `/api/kyc/admin/documents/{document_id}` params(document_id) [auth, db, upload]
  `rs/read/src/runtime/phase4.rs`
- `GET` `/api/admin/analytics/product-analytics/overview` [auth, db]
  `rs/read/src/runtime/phase4_analytics.rs`
- `GET` `/api/admin/analytics/product-analytics/activation` [auth, db]
  `rs/read/src/runtime/phase4_analytics.rs`
- `GET` `/api/admin/analytics/product-analytics/engagement` [auth, db]
  `rs/read/src/runtime/phase4_analytics.rs`
- `GET` `/api/admin/analytics/product-analytics/retention` [auth, db]
  `rs/read/src/runtime/phase4_analytics.rs`
- `GET` `/api/admin/subscriptions` [auth, db]
  `rs/read/src/subscriptions/routes.rs`
- `GET` `/api/admin/subscriptions/{org_id}` params(org_id) [auth, db]
  `rs/read/src/subscriptions/routes.rs`
- `PUT` `/api/admin/subscriptions/{org_id}` params(org_id) [auth, db]
  `rs/read/src/subscriptions/routes.rs`
- `GET` `/api/admin/analytics` [auth]
  `rs/read/tests/phase2_http.rs`

## Related Models

- **app__routers__admin_analytics__OverviewResponse** (10 fields) → [database.md](./database.md)

## High-Impact Files

- `app/routers/auth_simple.py` — imported by 65 files

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
- `rs/read/src/business/admission.rs`
- `rs/read/src/kyc/review.rs`
- `rs/read/src/routes/market_write/commission.rs`
- `rs/read/src/routes/market_write/freeze.rs`
- `rs/read/src/routes/market_write/support.rs`
- `rs/read/src/runtime/phase4.rs`
- `rs/read/src/runtime/phase4_analytics.rs`
- `rs/read/src/subscriptions/routes.rs`
- `rs/read/tests/phase2_http.rs`

---
_Back to [overview.md](./overview.md)_