# Admin Invitation Market Status Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make organizations created by an administrator live-market verified immediately, and let later invitations select approved live or pending-verification organizations.

**Architecture:** Keep the database provenance enum unchanged. Permit `REAL` only on organization insertion, while the existing immutable-provenance trigger continues to block direct updates. The admin invitation endpoint sets `REAL`; ordinary registration still omits provenance and therefore receives `UNKNOWN`. The organization selector returns approved `REAL` and `UNKNOWN` rows with a translated display status.

**Tech Stack:** FastAPI, SQLAlchemy, Alembic/PostgreSQL ACLs, React, TypeScript, Vitest.

---

### Task 1: Encode the backend contract

**Files:**
- Modify: `tests/unit/test_admin_invitations.py`

1. Change the new-organization invitation assertions to require `OrganizationProvenance.REAL`.
2. Add a test that creates an organization and then creates a second invitation against its ID before the first invitation is accepted.
3. Add a list-endpoint test that includes approved `REAL` and `UNKNOWN` organizations and excludes unapproved or synthetic organizations.
4. Run `./venv/bin/pytest tests/unit/test_admin_invitations.py -q` and confirm the new assertions fail.

### Task 2: Implement the backend behavior

**Files:**
- Modify: `app/routers/auth_simple.py`
- Modify: `deploy/postgres/app_acl_policy.sql`
- Create: `alembic/versions/ai_20260831_admin_invite_real_orgs.py`
- Modify: `tests/postgres/test_runtime_role_policy.py`

1. Add `provenance` to `InvitationOrganizationResponse`.
2. Return approved organizations whose provenance is `REAL` or `UNKNOWN`.
3. Use the same eligibility rule when an administrator creates an invitation for an existing organization.
4. Set `provenance=OrganizationProvenance.REAL` only in the admin-created organization branch and include this trust decision in the invitation audit event.
5. Grant the runtime role `INSERT` access to `organizations.provenance`, but do not grant `UPDATE` access.
6. Revise the immutable-provenance trigger so that insertion of `REAL` is allowed while the existing controlled `UNKNOWN -> REAL` update rule remains unchanged.
7. Add PostgreSQL policy checks that a raw runtime insert can create `REAL`, but a raw runtime update still cannot promote an existing organization.
8. Run the targeted unit and PostgreSQL policy tests.

### Task 3: Show clear market-status labels

**Files:**
- Modify: `../fe/src/services/api.ts`
- Modify: `../fe/src/components/admin/AdminDashboard.tsx`
- Modify: `../fe/src/locales/en/admin.json`
- Modify: `../fe/src/locales/zh/admin.json`
- Modify: `../fe/src/tests/admin-onboarding-review.test.tsx`

1. Add `provenance: 'REAL' | 'UNKNOWN'` to the invitation-organization API type.
2. Render each selector option as organization name, domain, and one of these translated labels: `Verified for live market` or `Market verification pending`.
3. Replace the empty-state text that says only real organizations are available.
4. Add a frontend test that verifies both labels and selection of an approved pending-verification organization.
5. Run the targeted Vitest file, type check, and build check.

### Task 4: Synchronize documentation and release

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/plans/2026-08-12-admin-created-organizations-design.md`

1. Document that an admin-created organization is inserted as `REAL`, that this is an audited administrator trust decision, and that approved `UNKNOWN` organizations remain selectable.
2. Run backend and frontend full test suites and documentation checks.
3. Deploy staging, create two disposable invitations in the same new organization before either is accepted, verify the status labels, and remove the disposable records.
4. Promote the tested commits to production and run live smoke checks.
