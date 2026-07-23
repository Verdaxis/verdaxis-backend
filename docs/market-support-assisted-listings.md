# Market Support Organization Context

## Purpose

An authorized Verdaxis administrator may enter an approved real customer
organization and use the normal customer-facing platform to create and cancel
supplier ASK listings on its behalf. The administrator remains the
authenticated actor. The customer organization remains the economic party, and
an approved supplier in that organization remains the accountable principal.
Verdaxis never requests customer credentials, mints a customer session, or
attributes the administrator's action to the customer.

## Product Contract

Market Support is not a separate trading workspace. Entry begins from admin
organization management and activates an opaque, server-owned context. The
existing supplier routes, navigation, marketplace, order form, and own-order
views then render with a persistent, non-dismissible banner:

`Market Support - Acting for <organization> as Supplier`

The banner identifies the accountable supplier, the signed-in administrator,
the support reference, expiry, and an explicit Exit action. The real admin
identity remains visible in the profile menu. The context never appears in the
URL and the browser stores only its opaque ID in `sessionStorage`.

## Phase 1 Boundary

- Approved `REAL` supplier organizations only.
- One approved, email-verified, execution-eligible supplier principal is
  selected when the context starts.
- One active, absolute-expiry context per administrator.
- Organization-scoped reads needed to render the supplier platform.
- ASK creation through the normal order form.
- Cancellation only for ASK listings created through Market Support.
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
- `accountable_principal`: the selected eligible supplier;
- `effective_role`: `SUPPLIER`;
- `mode`: `MARKET_SUPPORT`;
- `support_context_id` and support reference;
- `creation_method`: `MARKET_SUPPORT`.

Ordinary requests resolve the authenticated user as actor, organization, and
principal with `SELF_SERVICE` creation. Code must never mutate `current_user`,
construct a fake customer user, or accept an organization/principal header on
normal market endpoints.

The frontend sends:

`X-Verdaxis-Market-Support-Context: <opaque UUID>`

The header is only a locator. The server reloads the context and revalidates
actor ownership, expiry, feature flag, capabilities, organization approval and
`REAL` provenance, principal membership, role, and operation-specific
eligibility. Unknown or foreign context IDs are non-enumerating.

The backend stores one active context row per administrator using a partial
unique index. Phase 1 uses the fixed `ASK_LISTINGS` scope and configured short
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

Context creation is the only route that accepts organization and principal IDs.
They are lookup inputs, never authorization claims. Context exit is idempotent.
Starting a different context requires an explicit replacement confirmation.

## ASK Creation

The existing `POST /orderbook` contract remains the customer-facing entry
point. In Market Support mode it additionally requires:

- an idempotency key;
- `side=ASK`;
- explicit order expiry;
- the existing supplier metadata;
- a final support confirmation containing an external instruction reference,
  instruction time, transient evidence excerpt, and exact-terms/standing-order
  acknowledgements.

The server hashes the evidence excerpt and never stores or logs its plaintext.
It obtains consent version/reference from server configuration. In one
transaction it locks and revalidates the context and market slice, creates the
exact one-use authorization, rejects any crossing ASK, inserts one post-only
resting order with immutable dual attribution, consumes the authorization,
records audit and notification rows, and commits.

Support ASK requests use only the canonical delivery point (never a port or
vessel) and remain within the configured listing TTL. A recent customer
instruction may predate context entry; the context limits when the admin may
publish, not how long the resulting standing ASK may remain live.
The authorization stores only the instruction timestamp, acknowledgement
booleans, and evidence digest as forensic facts; the evidence excerpt is
discarded after hashing. Legacy admin authorization/listing mutation routes
are retired while context mode is enabled; their read routes remain available
for compatibility.

Public orderbook serializers never expose context, authorization, evidence,
support reference, administrator, or accountable-principal details.

## Cancellation And Concurrency

The normal own-order interface uses `POST /orderbook/{id}/cancel` with a
mandatory reason and `If-Match`. Market Support may cancel only an order whose
organization matches the effective organization and whose creation method is
`MARKET_SUPPORT`. Missing, malformed, and stale preconditions return `428`,
`400`, and `412`. Cancellation remains available as a cleanup action when the
principal later becomes ineligible, but every actor, context, organization,
original principal, cancellation principal, reason, and order version is
audited.

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
