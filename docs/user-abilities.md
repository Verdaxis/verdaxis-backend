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

The `policy` column is the market-party policy the real endpoint applies to the
account gate (`app/services/execution_policy.py`, `MarketPartyPolicy`):

- `EXECUTION`: a new economic commitment. Account admission plus the KYC
  execution rule: KYC approved for the current organization, or a valid grace
  row. With `KYC_EXECUTION_GATE_ENABLED=false` (Private Beta) it falls back to
  the advisory rule: only a rejected review, or a review bound to another
  organization, blocks.
- `CLEANUP`: settling or withdrawing an existing commitment. Account admission
  only; KYC never blocks it.
- `INDICATIVE`: non-executable surfaces (RFQ, inventory records). Account
  admission plus the advisory KYC rule.

| action | meaning | policy | gate (first denial wins) |
| --- | --- | --- | --- |
| `place_bid_order` | `POST /orderbook` with side BID | EXECUTION | party gate, BUYER |
| `place_ask_order` | `POST /orderbook` with side ASK | EXECUTION | party gate, SUPPLIER |
| `amend_order` | `PUT /orderbook/{id}` | EXECUTION | legacy admin path retired, party gate |
| `cancel_order` | `POST/DELETE /orderbook/{id}` | CLEANUP | legacy admin path retired, party gate (rechecked under row locks) |
| `hit_ask_order` | `POST /trades/` against an ASK | EXECUTION | party gate, BUYER, provenance can execute |
| `hit_bid_order` | `POST /trades/` against a BID | EXECUTION | party gate, SUPPLIER, provenance can execute |
| `confirm_trade` | `PUT /trades/{id}/confirm` | EXECUTION | party gate |
| `deliver_trade` | `PUT /trades/{id}/deliver` | CLEANUP | party gate |
| `pay_trade` | `POST /trades/{id}/pay` | CLEANUP | party gate, SUPPLIER |
| `manage_inventory` | add/update/delete inventory | INDICATIVE | party gate, SUPPLIER |
| `publish_inventory` | `POST /inventory/{id}/publish` | EXECUTION | party gate, SUPPLIER |
| `create_rfq` | `POST /rfq` | INDICATIVE | party gate, BUYER, organization provenance REAL (or allowlisted demo) |
| `quote_rfq` | `POST /rfq/{id}/quote` | INDICATIVE | party gate, SUPPLIER, provenance can execute |
| `view_admin_analytics` | `/admin/analytics/*` dashboards | none | ADMIN |
| `view_audit_log` | audit log pages | none | ADMIN |
| `view_user_activity` | per-user activity timeline | none | ADMIN |
| `view_feedback` | feedback inbox | none | ADMIN |
| `manage_subscriptions` | `/admin/subscriptions*` | none | ADMIN |
| `manage_accounts` | approve/reject accounts and organizations, invitations, admin user list, onboarding attention | none | ADMIN |
| `review_kyc` | `PUT /kyc/admin/{id}/approve|reject` | none | ADMIN |
| `manage_commissions` | `/orders/admin/commissions*` | none | ADMIN |
| `market_support_access` | Market Support organization list | none | feature enabled, approved ADMIN, any active staff capability |
| `market_support_assisted_orders` | start an assisted-order context and publish for a customer | none | feature enabled, approved ADMIN, both `MARKET_SUPPORT_LISTINGS` and `MARKET_SUPPORT_AUTHORIZATIONS` |

"Party gate" is the single predicate `market_party_denial` for the policy in
the row. `market_party_is_eligible` and the route dependencies
(`require_execution_eligible_user`, `require_market_cleanup_user`,
`require_market_admitted_user`) are exactly `denial is None`, so the endpoint
and this payload cannot disagree.

KYC details for the UI:

- A user inside the one-time KYC grace window (`kyc_execution_grace`, expires
  28 Oct 2026) is **allowed** with `reason_code: null`. The payload does not
  carry the deadline; `GET /api/auth/me` reports
  `execution_eligibility {eligible, reason, grace_expires_at}`.
- Once grace has expired (gate on), every `EXECUTION` ability reports
  `KYC_REQUIRED` for a user without approved KYC; `CLEANUP` abilities stay
  allowed, so a user can still cancel, deliver and pay.
- With the gate off, a user whose KYC is PENDING or SUBMITTED is allowed to
  execute; turning the gate on later flips those abilities without a code change.
- Per-order rules are not reported: Bio-LNG orders need `lng_terms` and the
  Rotterdam lane, a Bio-LNG ASK cannot be published from inventory, and a
  Bio-LNG ASK can only be taken with compatible buyer terms
  (`LNG_TERMS_INCOMPATIBLE`). `FUEL_TRADER` organizations may hold the
  SUPPLIER role (invitation only), so they get the supplier abilities.

## Reason codes

| reason_code | meaning |
| --- | --- |
| `ADMIN_CANNOT_EXECUTE` | Administrators never place or execute market actions. |
| `ROLE_NOT_ASSIGNED` | The account has no BUYER/SUPPLIER role yet. |
| `ORGANIZATION_REQUIRED` | No organization, or no matching organization record. |
| `ACCOUNT_NOT_APPROVED` | Account status is not APPROVED. |
| `EMAIL_NOT_VERIFIED` | Email address not verified. |
| `PASSWORD_CHANGE_REQUIRED` | Account is flagged to change its password. |
| `KYC_REQUIRED` | KYC is not approved for the current organization and no valid grace applies. Same code as the 403 `KYC_REQUIRED` the execution gate returns. |
| `KYC_REJECTED` | A human reviewer rejected the user's KYC. |
| `KYC_ORGANIZATION_MISMATCH` | The KYC review is bound to a different organization (advisory rule, `INDICATIVE`). Under `EXECUTION` this case reports `KYC_REQUIRED`. |
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
| market-party eligibility (all three policies) | `market_party_denial`, with `market_party_is_eligible` = `denial is None`; built from `user_account_denial`, `organization_denial`, `advisory_kyc_denial` and `evaluate_kyc_execution` (`services/kyc_execution.py`) | `middleware/execution.py`, trades, inventory, orderbook, matching, `market_admission.lock_and_load_market_organizations` |
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

`POLICY_BY_ABILITY` (`services/abilities.py`) records which policy each endpoint
applies. A unit test reads the real endpoints and fails if the table drifts.

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
