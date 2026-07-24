# Market Support Organization Context

## Purpose

An authorized Verdaxis administrator may enter an approved real customer
organization and use the normal customer-facing platform to create and cancel
BID and ASK listings on its behalf. The administrator remains the authenticated
and accountable actor. The customer organization remains the economic party.
Verdaxis never requests customer credentials, mints a customer session, selects
a customer user as a proxy actor, or attributes the administrator's action to
the customer.

## Product Contract

Assisted Order Entry is not a separate trading workspace. Entry begins from admin
organization management and activates an opaque, server-owned context. The
existing buyer and supplier routes, navigation, marketplace, order form, and own-order
views then render with a persistent, non-dismissible banner:

`Acting for <organization>`

The banner identifies the signed-in administrator, support reference, expiry,
and an explicit Exit action. The real admin
identity remains visible in the profile menu. The context never appears in the
URL and the browser stores only its opaque ID in `sessionStorage`.

## Phase 1 Boundary

- Approved `REAL` organizations only.
- One active, absolute-expiry context per administrator.
- Organization-scoped reads needed to render the buyer and supplier platform.
- BID and ASK creation through the normal order form.
- GTC and explicitly dated orders.
- Cancellation only for orders created through Assisted Order Entry.
- No listing edits; changed terms require cancellation and replacement.
- No hit/lift, negotiation, RFQ mutation, inventory mutation, trade lifecycle,
  watchlist mutation, notification mutation, or customer settings mutation.
- Every unallowlisted mutation carrying a Market Support context fails closed.
- `MARKET_SUPPORT_ENABLED=true` and both durable Market Support capabilities
  are required; `ADMIN` role alone is insufficient.

## Request Identity

Every context-aware request resolves an immutable request party:

- `actor`: the authenticated administrator;
- `effective_organization`: the selected customer organization;
- no proxy customer principal;
- `effective_role`: the buyer or supplier side selected for the order;
- `mode`: `MARKET_SUPPORT`;
- `support_context_id` and support reference;
- `creation_method`: `MARKET_SUPPORT`.

Ordinary requests resolve the authenticated user as actor, organization, and
principal with `SELF_SERVICE` creation. Assisted requests intentionally have no
customer principal. Code must never mutate `current_user`, construct a fake
customer user, or accept an organization/principal header on normal market
endpoints.

The frontend sends:

`X-Verdaxis-Market-Support-Context: <opaque UUID>`

The header is only a locator. The server reloads the context and revalidates
actor ownership, expiry, feature flag, capabilities, organization approval and
`REAL` provenance. Unknown or foreign context IDs are non-enumerating.

The backend stores one active context row per administrator using a partial
unique index. The context uses the fixed `ASSISTED_ORDER_ENTRY` scope and configured short
TTL; expiry is persisted as `EXPIRED` before the request is rejected. Both
durable listing and authorization capabilities are required for context entry
and request-party resolution.

## Context Lifecycle API

All routes use the existing `/api` prefix:

- `GET /admin/market-support/capabilities`
- `POST /admin/market-support/capability-assignments`
- `POST /admin/market-support/capability-assignments/{id}/revoke`
- `GET /admin/market-support/organizations`
- `GET /admin/market-support/organizations/{org_id}/entry`
- `POST /admin/market-support/contexts`
- `GET /admin/market-support/contexts/active`
- `GET /admin/market-support/contexts/{context_id}`
- `POST /admin/market-support/contexts/{context_id}/exit`

Context creation is the only route that accepts an organization ID. It is a
lookup input, never an authorization claim. Context exit is idempotent.
Starting a different context requires an explicit replacement confirmation.

## Order Creation

The existing `POST /orderbook` contract remains the customer-facing entry
point. In Market Support mode it additionally requires:

- an idempotency key;
- `side=BID` or `side=ASK`;
- GTC or an explicit order expiry;
- the existing supplier metadata for ASKs;
- a final support confirmation containing an external instruction reference,
  instruction time and exact-terms/standing-order acknowledgements.

No evidence excerpt is required or retained. The server obtains consent
version/reference from server configuration. In one
transaction it locks and revalidates the context and market slice, creates the
exact one-use authorization, rejects any crossing order, inserts one post-only
resting order with immutable dual attribution, consumes the authorization,
records audit and notification rows, and commits.

Assisted orders use only the canonical delivery point (never a port or vessel).
There is no artificial maximum order lifetime or instruction-age cutoff:
context expiry limits when the admin may publish, not how long the resulting
standing order may remain live. The authorization stores the instruction
timestamp and acknowledgement booleans as forensic facts. Legacy admin
authorization/listing mutation routes
are retired while context mode is enabled; their read routes remain available
for compatibility.

Public orderbook serializers never expose context, authorization, support
reference, or administrator details.

## Cancellation And Concurrency

The normal own-order interface uses `POST /orderbook/{id}/cancel` with a
mandatory reason and `If-Match`. Market Support may cancel only an order whose
organization matches the effective organization and whose creation method is
`MARKET_SUPPORT`. Missing, malformed, and stale preconditions return `428`,
`400`, and `412`. Both sides are supported, but only assisted orders belonging
to the active organization may be cancelled. Every actor, context,
organization, reason, and order version is audited.

## Session Behavior

- Refresh blocks customer rendering until the stored context ID is revalidated.
- Access-token refresh continues to authenticate only the real admin.
- Explicit exit ends the server context, clears session storage, broadcasts an
  invalidation to attached tabs, and returns to admin organization management.
- Context expiry or revocation fails closed and removes the local attachment.
- New tabs and bookmarked URLs do not infer a context.
- A separate tab may explicitly resume the administrator's active context.
- Logout best-effort ends the context and always clears local context state.

## Activation Gates

The migration and ACL changes must pass disposable PostgreSQL tests and the
reviewed literal migration-checkpoint process. Staging activation requires
approved customer-authority wording, evidence handling, consent
version/reference, context TTL, notifications, erroneous-fill procedure,
tenant-isolation tests, route-enumeration denial tests, normal-user regression
tests, and browser dogfooding. Production requires a separate reviewed rollout.
