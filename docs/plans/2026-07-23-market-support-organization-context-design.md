# Market Support Organization Context Design

## Decision

Verdaxis will use an opaque, short-lived, database-backed Market Support
context. The real admin bearer token remains the only authentication
credential. The server resolves the context into the effective organization,
accountable supplier, support reference, expiry, and narrow phase-1 scope.

## Alternatives

1. A raw organization header is rejected because it lets the browser select
   tenancy on every request.
2. A signed acting JWT is viable but rejected because it behaves like a second
   customer session and still needs revocation and database revalidation.
3. The opaque server context is selected because it provides explicit dual
   identity, immediate revocation, expiry, audit continuity, and safe multi-tab
   behavior without customer impersonation.

## UX

Admin Users/Organizations exposes `Enter supplier platform`. Entry selects an
eligible supplier and requires a support reference. The normal supplier
platform opens with a persistent acting-organization banner and Exit control.
The normal ASK form is reused; after ordinary validation, an admin-only final
confirmation captures instruction evidence and acknowledgements. Other
customer mutations are disabled in the UI and denied by the backend.

## Security Boundary

The context header is an opaque locator and is meaningful only with the owning
admin token. Every context-aware endpoint revalidates the context and target.
An explicit request-party abstraction keeps actor, economic party, and
accountable principal separate. A deny-by-default mutation allowlist prevents
new or forgotten customer mutations from becoming available in support mode.

## Data And Audit

The context records actor, organization, principal, support reference, scope,
start, absolute expiry, end state, and lifecycle version. ASK creation retains
the current exact one-use authorization, digest, post-only crossing check,
idempotency, immutable order attribution, ETag, audit, notification, and market
event behavior. Evidence plaintext is transient; only its digest and external
reference persist.

## Rollout

Backend compatibility lands first with the feature disabled by default. The
frontend then moves entry into admin organization management, installs context
state and banner chrome, and reuses normal supplier routes. Staging is migrated
and dogfooded before the obsolete workspace routes and component are removed.
Production is not changed by this implementation cycle.
