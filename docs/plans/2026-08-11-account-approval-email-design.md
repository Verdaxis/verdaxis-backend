# Account Approval Email Design

## Purpose

Notify a verified Verdaxis user when an administrator changes their account
status to `APPROVED`, so the user knows that they can sign in without relying
on manual follow-up from the Verdaxis team.

## Behavior

- Send only after a genuine transition from `PENDING` or `REJECTED` to
  `APPROVED` through the canonical admin account-approval endpoint.
- Do not send when an already-approved account is submitted to the endpoint
  again.
- Store the exact pending status-transition UUID, immutable provider payload,
  and initial retry time in the same transaction as the approval,
  status-transition fact, and audit record.
- Commit before calling the email provider. A provider failure must never undo
  an approved account or clear its pending notification marker.
- Use the environment-specific `FRONTEND_URL` for the sign-in call to action.
- Escape all user-provided names before inserting them into HTML.
- Keep organization and membership approvals independent. The message states
  that the account is approved and does not claim that KYC or trading access is
  complete.

## Delivery Model

The request attempts delivery immediately after commit. Delivery locks and
re-reads the user before sending, so a concurrent rejection either waits for
an in-progress send or invalidates the pending notification before it starts.
The immutable payload captured at approval time is replayed with the
status-transition UUID as Resend's `Idempotency-Key`; mutable profile fields or
later deployments cannot change a retried request. The pending state is
cleared only after Resend accepts the request.

The existing hourly authentication-maintenance job retries a bounded batch of
due markers independently from the API process. A failed attempt moves its
retry time forward, so permanently failing recipients cannot starve newer
approvals. Invalidated notifications are discarded. Resend retains
idempotency keys for 24 hours, making ordinary timeout and acknowledgement
retries duplicate-safe. This is durable at-least-once application delivery
with provider-level duplicate suppression, not an impossible cross-system
exactly-once claim. A generic email outbox remains deferred until Verdaxis has
more than one transactional email workflow requiring durable retry.

## Verification

- The email template has the correct subject, environment-specific sign-in
  link, escaped greeting, and transition-scoped idempotency key.
- A real account-status transition persists one pending marker and attempts
  delivery after commit.
- Re-approving an already-approved account sends nothing.
- A failed provider call leaves the account approved, the endpoint successful,
  and an immutable payload available for retry.
- A rejection committed before delivery prevents the approval email; delivery
  and rejection are serialized on the user row.
- Authentication maintenance retries a bounded, fair set of due markers,
  including legacy users whose role is null, and clears only the exact
  transition it delivered.
- The missed production recipient is selected from the authoritative
  `admin.user_approved` audit event and sent one email after production deploy.
