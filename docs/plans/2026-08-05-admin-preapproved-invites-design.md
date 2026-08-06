# Admin Pre-Approved Invitations

**Date:** 2026-08-05
**Status:** Approved by implementation request

## Goal

Let an administrator prepare an account inside an existing approved Verdaxis
organization and give the recipient one link to accept the invitation, agree to
the platform terms, set a password, and enter the platform. The recipient does
not repeat signup or wait for a second administrator approval.

## Boundaries

- Only authenticated administrators may create or reissue these invitations.
- The target organization must already be approved and classified as real.
- The administrator may assign only `BUYER` or `SUPPLIER`; this path cannot
  create another administrator.
- The assigned role must match the organization's trading side: `SUPPLIER`
  only for `FUEL_SUPPLIER`, and `BUYER` only for the supported buy-side
  organization types. The backend enforces this independently of the dialog
  and rejects previously issued mismatched invitations during resolution.
- The recipient remains unable to authenticate until the claim secret is
  accepted. Because an administrator copies and delivers the link, acceptance
  proves possession of that secret rather than independently proving mailbox
  ownership; administrators must send it only to the intended recipient.
- Account admission is pre-approved; this does not assert that formal KYC has
  been completed.
- Ordinary self-registration, referrals, verification, and password reset keep
  their current behavior.

## Existing Primitives Reused

No schema change is required. An unclaimed invitation is represented by the
existing user fields:

- `status = APPROVED`
- `email_verified = false`
- `must_change_password = true`
- an approved organization membership and a valid, hashed one-time token in
  the existing password-reset token slot

The existing referral relationship records which administrator invited the
recipient. The existing append-only audit log records both issuance and
acceptance, including the terms/privacy URLs accepted by the recipient.

## API Contract

- `GET /api/auth/admin/invitations/organizations` lists eligible organizations.
- `POST /api/auth/admin/invitations` creates an unclaimed account and returns a
  seven-day acceptance URL. Repeating the same request for the same unclaimed
  account rotates the token rather than creating a duplicate user.
- `POST /api/auth/invitations/resolve` validates a token and returns only the
  recipient email, role, organization, inviter, and expiry needed by the page.
- `POST /api/auth/invitations/accept` requires explicit terms acceptance and a
  valid password, consumes the token under a row lock, activates the prepared
  identity, progresses referral state, issues the normal device-bound session,
  and returns an access token.

Invitation tokens are generated with `secrets`, stored only as SHA-256 hashes,
never logged, and placed in the URL fragment so they are not sent to the
frontend host in HTTP requests. The browser removes the fragment immediately
and keeps the token only in the current history entry so a reload can recover
without placing the secret in a request URL. Query-string tokens are rejected.
Invalid, expired, used, or ineligible tokens share one public error response.

## Frontend Flow

The admin Users tab gets an `Invite user` command. Its dialog captures name,
email, buyer/supplier role, and one eligible organization. Role and organization
have no implicit defaults, eligible options are filtered by trading side, and
the organization domain is shown so the administrator can verify the tenant.
It then presents a copyable acceptance link. The public `/accept-invite` page
shows the prepared account details, password controls, and an explicit
Terms/Privacy checkbox.
Success signs the recipient in and opens `/app`; an unavailable link offers
Sign In and Forgot Password routes.

## Verification

- Backend checks cover authorization, eligible organization binding, duplicate
  protection/reissue, single-use acceptance, session issuance, referral state,
  audit evidence, and the unverified-account password-reset boundary.
- Frontend checks cover admin link generation and recipient acceptance.
- Browser dogfood covers the complete staging flow at desktop and compact
  viewport sizes before production promotion.
