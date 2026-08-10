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
- Commit the approval, status-transition fact, and audit record before calling
  the email provider. A provider failure must never undo an approved account.
- Use the environment-specific `FRONTEND_URL` for the sign-in call to action.
- Escape all user-provided names before inserting them into HTML.
- Keep organization and membership approvals independent. The message states
  that the account is approved and does not claim that KYC or trading access is
  complete.

## Delivery Model

The first release uses the existing synchronous Resend client after the
database commit, matching Verdaxis's established KYC notification behavior.
The provider result is logged without exposing the recipient address. A
durable email outbox is intentionally deferred until email volume or observed
delivery failures justify another table and worker.

## Verification

- The email template has the correct subject, environment-specific sign-in
  link, and escaped greeting.
- A real account-status transition sends exactly once.
- Re-approving an already-approved account sends nothing.
- A failed provider call leaves the account approved and the endpoint
  successful.
- The missed production recipient is selected from the authoritative
  `admin.user_approved` audit event and sent one email after production deploy.

