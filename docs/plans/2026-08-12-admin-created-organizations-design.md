# Admin-Created Organizations in Pre-Approved Invitations

**Date:** 2026-08-12
**Status:** Approved by implementation request

## Goal

Allow an administrator to prepare a new onboarding-approved organization and its first user in
the existing invitation flow. The recipient receives the same single-use link,
accepts the platform terms, creates a password, and enters Verdaxis without
organization creation, email verification, or account approval steps.

## Contract

- `POST /api/auth/admin/invitations` accepts exactly one organization source:
  an existing `organization_id`, or a nested `new_organization` containing
  name, type, country code, and optional tax ID.
- Only authenticated `ADMIN` users may use either path. The new organization
  path creates `verification_status = APPROVED` and retains the database-owned
  `provenance = UNKNOWN` default. Market provenance remains subject to the
  separate trusted operator approval after the invited user accepts.
- The requested account role must match the organization type. A supplier may
  create only `FUEL_SUPPLIER`; a buyer may use the supported buy-side types.
- Organization creation, user creation, referral attribution, audit evidence,
  and invitation issuance are one database transaction. Any failure rolls back
  the entire operation.
- The normalized business email domain is attached only when it is eligible as
  a tenant boundary and is not already owned. A domain already owned by another
  organization is a conflict; public mailbox domains remain unclaimed.
- Existing-organization invitation behavior, reissue behavior, invitation
  acceptance, KYC state, and ordinary self-registration remain unchanged.
- Audit evidence for a newly created organization records its ID, type, and
  country, but does not copy tax identifiers or invitation secrets.

## Interface

The Admin Users invitation dialog offers an explicit segmented choice between
`Existing organization` and `New organization`. Existing mode preserves the
current filtered selector. New mode shows organization name, role-compatible
type, country, and optional tax ID. No values silently carry across modes.

The success state names the created organization and returns the existing
copyable, seven-day acceptance link. The acceptance page does not change.

## Verification

- Backend tests cover admin authorization, mutually exclusive organization
  sources, role/type boundaries, onboarding-approved creation, domain conflicts,
  transaction rollback, audit evidence, and existing invitation regression.
- Frontend tests cover both modes, role-compatible types, request shape, mode
  resets, and translated labels.
- Staging browser dogfood creates a disposable organization and user, accepts
  the link, verifies the resulting session and organization, and removes the
  disposable records before production promotion.
