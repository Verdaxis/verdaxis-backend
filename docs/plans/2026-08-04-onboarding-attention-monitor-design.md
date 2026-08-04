# Onboarding Attention Monitor Design

## Purpose

Notify Verdaxis operators when a real prospective user reaches an actionable
onboarding problem. The monitor describes these states as "needs attention"
rather than claiming that every incomplete signup has failed.

## Scope

- Production only, every five minutes.
- Authoritative database state, not browser analytics.
- Existing signup canary remains responsible for system-wide route failures.
- Include the person's name, email, organization, stage, and relevant timing.
- Exclude ADMIN users, monitor canaries, and DEMO/TEST/CANARY organizations.
- Never include passwords, tokens, IP addresses, or KYC evidence.

## State Classification

Each candidate has at most one current stage, in this priority order:

1. `rejected`: user, organization, or membership is rejected.
2. `organization_setup_expiring`: an unused pending organization registration
   is within one monitor interval of expiry.
3. `verification_stalled`: a created user remains unverified after the
   verification window.
4. `approval_required`: email is verified but user, organization, or
   membership approval is incomplete.
5. `first_login_overdue`: every approval is complete, two hours have elapsed
   since the final approval gate, and `last_login` is empty.
6. `complete`: no alert condition applies.

The final approval time is the latest known user-approval transition,
organization-approval audit event, membership review time, and account creation
time. This prevents the two-hour clock from starting before every independent
gate is complete.

## Delivery And Deduplication

A hardened systemd oneshot runs from the production checkout and reads the
existing Telegram bot configuration. A persistent JSON state file records the
last stage per candidate. The monitor sends once when entering or changing an
attention stage and once when a previously alerted candidate becomes complete.
Failed Telegram delivery does not advance state, so the next timer run retries.
State writes are atomic and contain identifiers/stages only, not email or names.
Before first activation, `--bootstrap` records existing historical stages as a
silent baseline. Those entries never emit stale alerts or recovery messages,
but a later change into a different attention stage is notified normally.

## Failure Isolation

The monitor is read-only against Verdaxis data. Database or Telegram failure
causes the oneshot to fail without affecting API availability or onboarding.
The service is production-bound, has a five-minute timeout, and cannot write
outside its systemd state directory.

## Verification

- Pure classification tests cover every stage, priority, exclusions, and the
  two-hour boundary.
- State tests prove one alert per stage, retry after delivery failure, and one
  recovery message.
- A no-send dry run lists current production candidates before the silent
  baseline is recorded and the timer is activated.
- After activation, systemd status and journal output confirm bounded execution.
