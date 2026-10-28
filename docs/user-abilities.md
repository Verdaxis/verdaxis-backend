# User abilities contract

`GET /api/auth/me/abilities` tells the frontend what the signed-in user may do
and why not, so the UI does not infer permissions from `user.role` strings.

Abilities are **UI hints**. Every endpoint still enforces its own rules and is
authoritative. `allowed: true` means the user-level gate passes; per-subject
rules (order ownership, trade party, order state, counterparty provenance,
idempotency) are not part of this flat payload and can still reject a request.

## Why a separate endpoint

`/auth/me` is the hot path. It takes a shared user-row lock and is called on
every page load. Abilities need the organization row and, for administrators,
the staff capability rows. A separate route keeps `/auth/me` unchanged and
lets the frontend fetch abilities lazily or after login. The route is read-only
and is registered in `AUTHENTICATED_SHARED_LOCK_PATHS`, so it shares the user
row lock like `/auth/me`. It requires an approved, current account (the same
`get_current_user` dependency): it answers 401/403 exactly as `/auth/me` does,
including `Password change required.` for a user who must change a password.

## Response

```json
{
  "schema_version": 1,
  "abilities": [
    {"action": "place_bid_order", "allowed": true, "reason_code": null, "reason": null},
    {"action": "place_ask_order", "allowed": false,
     "reason_code": "SUPPLIER_ROLE_REQUIRED",
     "reason": "Only supplier accounts can do this."}
  ]
}
```

- `action`: stable snake_case id (below). Every id is always present, in a fixed order.
- `allowed`: boolean.
- `reason_code`: stable UPPER_SNAKE code when denied, else `null`. Build UI logic on this.
- `reason`: short English text for display. Wording may change; never match on it.
- The response carries `Cache-Control: no-store`. Refetch after any account,
  organization, KYC or capability change.
- New action ids and reason codes may be added; ids and codes are never renamed or removed.
  Treat an unknown `action` as ignorable and an unknown `reason_code` as a generic denial.

## Action ids

| action | meaning | gate (first denial wins) |
| --- | --- | --- |
| `place_bid_order` | `POST /orderbook` with side BID | execution eligibility, BUYER |
| `place_ask_order` | `POST /orderbook` with side ASK | execution eligibility, SUPPLIER |
| `amend_order` | `PUT /orderbook/{id}` | legacy admin path retired, execution eligibility |
| `cancel_order` | `POST/DELETE /orderbook/{id}` | legacy admin path retired (no admission needed; owner cleanup) |
| `hit_ask_order` | `POST /trades/` against an ASK | execution eligibility, BUYER, provenance can execute |
| `hit_bid_order` | `POST /trades/` against a BID | execution eligibility, SUPPLIER, provenance can execute |
| `confirm_trade` | `PUT /trades/{id}/confirm` | execution eligibility |
| `deliver_trade` | `PUT /trades/{id}/deliver` | execution eligibility |
| `pay_trade` | `POST /trades/{id}/pay` | execution eligibility, SUPPLIER |
| `manage_inventory` | add/update/delete inventory | execution eligibility, SUPPLIER |
| `publish_inventory` | `POST /inventory/{id}/publish` | execution eligibility, SUPPLIER |
| `create_rfq` | `POST /rfq` | execution eligibility, BUYER, organization provenance REAL (or allowlisted demo) |
| `quote_rfq` | `POST /rfq/{id}/quote` | execution eligibility, SUPPLIER, provenance can execute |
| `view_admin_analytics` | `/admin/analytics/*` dashboards | ADMIN |
| `view_audit_log` | audit log pages | ADMIN |
| `view_user_activity` | per-user activity timeline | ADMIN |
| `view_feedback` | feedback inbox | ADMIN |
| `manage_subscriptions` | `/admin/subscriptions*` | ADMIN |
| `manage_accounts` | approve/reject accounts and organizations, invitations, admin user list, onboarding attention | ADMIN |
| `review_kyc` | `PUT /kyc/admin/{id}/approve|reject` | ADMIN |
| `manage_commissions` | `/orders/admin/commissions*` | ADMIN |
| `market_support_access` | Market Support organization list | feature enabled, approved ADMIN, any active staff capability |
| `market_support_assisted_orders` | start an assisted-order context and publish for a customer | feature enabled, approved ADMIN, both `MARKET_SUPPORT_LISTINGS` and `MARKET_SUPPORT_AUTHORIZATIONS` |

"Execution eligibility" is the account gate for market actions. It is the single
predicate `execution_party_denial` (`app/services/execution_policy.py`), also used
by `require_execution_eligible_user`.

## Reason codes

| reason_code | meaning |
| --- | --- |
| `ADMIN_CANNOT_EXECUTE` | Administrators never place or execute market actions. |
| `ROLE_NOT_ASSIGNED` | The account has no BUYER/SUPPLIER role yet. |
| `ORGANIZATION_REQUIRED` | No organization, or no matching organization record. |
| `ACCOUNT_NOT_APPROVED` | Account status is not APPROVED. |
| `EMAIL_NOT_VERIFIED` | Email address not verified. |
| `PASSWORD_CHANGE_REQUIRED` | Account is flagged to change its password. |
| `KYC_REJECTED` | A human reviewer rejected the user's KYC. |
| `KYC_ORGANIZATION_MISMATCH` | KYC state belongs to a different organization. |
| `ORGANIZATION_NOT_APPROVED` | The organization's verification status is not APPROVED. |
| `BUYER_ROLE_REQUIRED` | Action is for buyer accounts. |
| `SUPPLIER_ROLE_REQUIRED` | Action is for supplier accounts. |
| `ADMIN_ROLE_REQUIRED` | Action is for administrator accounts. |
| `ORGANIZATION_PROVENANCE_UNVERIFIED` | The organization's market provenance (for example UNKNOWN) cannot execute. |
| `LEGACY_ADMIN_MUTATION_RETIRED` | With Market Support on, administrators must use an assisted context to amend/cancel. |
| `MARKET_SUPPORT_DISABLED` | Market Support feature flag is off. |
| `MARKET_SUPPORT_CAPABILITY_REQUIRED` | The administrator lacks the needed active staff capability. |

Notes for UI authors:

- An organization with UNKNOWN provenance may still place orders (they are
  quarantined from the public market) but cannot execute: `hit_*`, `quote_rfq`
  and `create_rfq` report `ORGANIZATION_PROVENANCE_UNVERIFIED`. DEMO provenance
  may execute only against its paired demo organization; that pairing is per
  counterparty, so the endpoint decides.
- `cancel_order` and `amend_order` are baseline gates. Order ownership and state
  are per order and not reported.
- Deny order mirrors the endpoint's check order, so the first reason shown is the
  one the endpoint would raise.

## How it stays in sync with enforcement

`app/services/abilities.py` calls the same predicates the endpoints call; it
re-implements none of them. Role rules and the vocabulary live in
`app/services/action_policy.py`.

| rule | shared predicate | enforced in |
| --- | --- | --- |
| execution eligibility | `execution_party_denial` / `execution_user_denial` / `execution_party_is_eligible` | `middleware/execution.py`, trades, inventory, matching, market admission |
| place BID/ASK role | `place_order_role_denial` | `routers/orderbook.py` create_order |
| hit ASK/BID role | `hit_order_role_denial` | `routers/trades.py` create_trade |
| mark paid role | `mark_trade_paid_role_denial` | `routers/trades.py` pay_trade |
| inventory role | `inventory_role_denial` | `routers/inventory.py` |
| RFQ roles | `create_rfq_role_denial`, `quote_rfq_role_denial` | `routers/rfq.py` |
| admin pages | `allowed_roles(ability)` / `role_may(ability, role)` / `role_permitted` | `middleware/rbac.py`, `require_role(*allowed_roles(...))` in admin routers, inline checks in `auth_simple.py`, `kyc.py`, `orders.py` |
| legacy admin amend/cancel | `legacy_admin_workspace_mutation_denial` | `routers/orderbook.py` |
| provenance can execute | `provenance_may_execute` | `execution_provenance_compatible`, trades, matching, RFQ pairs |
| RFQ participant provenance | `participant_provenance_eligible` | `assert_market_participant_provenance` |
| Market Support staff gate | `market_support_staff_denial` | `require_market_support_admin` |
| capability active | `capability_assignment_is_active`, `active_capabilities`, `ASSISTED_ORDER_CAPABILITIES` | Market Support routes, `request_party.py` |

Change a rule in its predicate and both the endpoint and this payload change.
`tests/unit/test_user_abilities.py` drives each real endpoint or dependency and
the payload for the same user profiles and asserts the same verdict.

## Not covered

- `decline_trade`: the route has no user-level gate (rejected users may withdraw
  their own state); only the per-trade party check applies. Nothing to hint.
- Subscription tiers: `require_tier` exists but no endpoint uses it, so no
  premium-analytics ability exists. The only tier-like rule is the hard-coded
  five-active-alert quota in `routers/alerts.py`, a per-organization count.
- Negotiations, matchmaking generation, vessels, compliance, AI chat: not in
  schema version 1. They would follow the same pattern.
