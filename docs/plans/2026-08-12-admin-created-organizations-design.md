# Admin-Created Organizations in Pre-Approved Invitations

**Date:** 2026-08-12
**Status:** Approved by implementation request

## Goal

Allow an administrator to prepare a new live-market organization and its users in
the existing invitation flow. The recipient receives the same single-use link,
accepts the platform terms, creates a password, and enters Verdaxis without
organization creation, email verification, or account approval steps.

## Contract

- `POST /api/auth/admin/invitations` accepts exactly one organization source:
  an existing `organization_id`, or a nested `new_organization` containing
  name, type, country code, and optional tax ID.
- Only authenticated `ADMIN` users may use either path. The new organization
  path creates `verification_status = APPROVED` and `provenance = REAL`; the
  audited administrator action is the trust decision for a new live-market
  organization. Ordinary registration continues to use the database-owned
  `UNKNOWN` default.
- The requested account role must match the organization type. A supplier may
  create only `FUEL_SUPPLIER`; a buyer may use the supported buy-side types.
- Organization creation, user creation, referral attribution, audit evidence,
  and invitation issuance are one database transaction. Any failure rolls back
  the entire operation.
- The runtime role inserts the organization with database-owned provenance and
  verification defaults, then uses its narrowly granted verification-status
  update in the same transaction; protected columns are never inserted.
- The normalized business email domain is attached only when it is eligible as
  a tenant boundary and is not already owned. A domain already owned by another
  organization is a conflict; public mailbox domains remain unclaimed.
- Existing-organization invitations accept approved `REAL` and `UNKNOWN`
  organizations. Reissue behavior, invitation acceptance, KYC state, and
  ordinary self-registration remain unchanged.
- Audit evidence for a newly created organization records its ID, type, and
  country, but does not copy tax identifiers or invitation secrets.

## Interface

The Admin Users invitation dialog offers an explicit segmented choice between
`Existing organization` and `New organization`. Existing mode shows approved
`REAL` and `UNKNOWN` organizations with `Verified for live market` and `Market
verification pending` labels. New mode shows organization name,
role-compatible type, country, and optional tax ID. No values silently carry
across modes.

The success state names the created organization and returns the existing
copyable, seven-day acceptance link. The acceptance page does not change.

## Verification

- Backend tests cover admin authorization, mutually exclusive organization
  sources, role/type boundaries, live-market creation, repeated invitations,
  domain conflicts, transaction rollback, audit evidence, and existing
  invitation regression.
- Frontend tests cover both modes, role-compatible types, request shape, mode
  resets, and translated labels.
- Staging browser dogfood creates a disposable organization and two users before
  either link is accepted, verifies the organization status and selector labels,
  and removes the disposable records before production promotion.
