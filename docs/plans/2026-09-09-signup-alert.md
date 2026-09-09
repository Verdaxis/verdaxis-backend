# Signup Alert Implementation Plan

**Goal:** Notify admin@verdaxis.exchange when an application is submitted, and verify existing approval emails.

**Architecture:** Reuse the Resend email service after either registration transaction commits. Include applicant name, email, role, organization, and the environment-specific admin link. Keep authenticated monitor canaries silent. Existing approval delivery and hourly retries remain in place.

**Tech Stack:** FastAPI, SQLAlchemy, httpx, pytest.

1. Extend the existing registration test harness to check both signup paths and monitor suppression. Mock the provider boundary and verify persisted accounts survive an email failure.
2. Add send_signup_alert_email in app/services/email.py. Escape applicant content; label staging alerts; use a user-scoped idempotency key.
3. Call it in app/routers/auth_simple.py after successful account creation, under the existing monitor guard.
4. Run focused email/registration tests, then backend CI. Review the diff for scope and maintainability.
5. Push isolated commits to staging and prod, run mandatory deploy dry runs and exact no-change migration checkpoints, then verify public readiness.
6. Verify Cedric's approval against production audit and provider-acceptance logs.

Limit: Signup alerts follow the existing verification-email delivery model. Delivery failure is logged and does not undo registration; no new durable queue or schema migration.
