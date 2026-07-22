# Market Support Assisted Listings

## Purpose

Verdaxis administrators may set up a supplier ASK listing for an approved real
customer organization without impersonating a customer or obtaining customer
credentials. The organization remains the economic party, an approved supplier
user in that organization remains the accountable principal, and the
authenticated administrator is retained as the action actor.

## Phase 1 Boundary

- Approved `REAL` supplier organizations only.
- ASK listings only.
- One exact authorization creates at most one listing.
- Administrators may create and cancel assisted listings. They may not edit,
  hit, lift, negotiate, confirm, deliver, or pay for customer trades.
- Changed terms require cancellation and a new authorization.
- The feature is hidden unless `MARKET_SUPPORT_ENABLED=true`.
- Ordinary `ADMIN` role membership is insufficient. The actor also needs the
  relevant durable capability assignment.

## Authorization Contract

An authorization records the exact normalized listing terms, selected
accountable supplier, customer evidence reference and SHA-256 digest,
commercial-consent version/reference, fixed expiry, and a canonical terms
digest. Its lifecycle is `ACTIVE -> CONSUMED` or `ACTIVE/CONSUMED -> REVOKED`.

Publication atomically:

1. verifies the listing capability and idempotency key;
2. locks the authorization namespace and market slice;
3. locks and validates the authorization, candidate market rows, users, and
   organizations in deterministic order;
4. refuses if any executable opposing order would cross, or if the complete
   crossing set cannot be assessed within the operational bound;
5. inserts one resting ASK with immutable support attribution;
6. marks the authorization consumed, records audit and notification rows,
   updates market projections, and commits once.

Revocation synchronously cancels the remaining quantity of the linked open
listing. Completed trades stand. A consumed authorization remains valid for
the uniquely linked order and later or partial fills until revoked or the order
expires.

## Concurrency

Support-created orders have a monotonically increasing `version`. Support and
customer cancellation require `If-Match` with the order ETag. Missing,
malformed, and stale preconditions return `428`, `400`, and `412` respectively.
Ordinary customer edits of a support-created order are rejected; the customer
must cancel and create a replacement.

## API

All routes use the existing `/api` prefix:

- `GET /admin/market-support/capabilities`
- `POST /admin/market-support/capability-assignments`
- `POST /admin/market-support/capability-assignments/{id}/revoke`
- `GET /admin/market-support/organizations`
- `GET /admin/market-support/organizations/{org_id}/context`
- `POST|GET /admin/market-support/organizations/{org_id}/authorizations`
- `POST /admin/market-support/organizations/{org_id}/authorizations/{id}/revoke`
- `POST|GET /admin/market-support/organizations/{org_id}/listings`
- `POST /admin/market-support/organizations/{org_id}/listings/{id}/cancel`

Organization paths are authoritative. Public orderbook serializers never
expose support authorization, evidence, actor, or customer-contact data.

## Activation Gates

Before enabling the feature, Verdaxis must approve the customer-authority
wording, acceptable evidence, commercial-consent version, maximum listing
lifetime, notification recipients, and erroneous-fill procedure. Migration and
ACL changes must pass the disposable PostgreSQL suite and the reviewed
migration-checkpoint process before staging activation.
