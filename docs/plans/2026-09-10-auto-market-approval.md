# Automatic company market classification implementation plan

**Goal:** Remove the separate live-market classification step after administrator company approval.

**Architecture:** Extend the existing database provenance trigger to classify UNKNOWN organizations on a company approval transition. Preserve synthetic classifications, direct-write ACLs, and all user execution gates. Include the classification change in the existing company approval audit. Use the existing exact-organization operator command to repair CMB.TECH; do not bulk-promote historical records.

**Tech Stack:** PostgreSQL, Alembic, FastAPI, SQLAlchemy.

1. Add the source-attested migration checkpoint and provenance trigger change in `alembic/versions/oa_20260910_auto_real_orgs.py` and `deploy/migration-checkpoints.tsv`.
2. Extend `approve_organization` in `app/routers/auth_simple.py` to read and audit the database classification.
3. Add PostgreSQL checks for approval, rejection, synthetic identities, unauthorized mutation, audit, and rollback. Run the existing PostgreSQL harness and unit suite.
4. Update architecture and operator instructions; request independent code review and fix findings.
5. Promote exact reviewed commits through CI and the guarded staging/production deploy process. Repair CMB.TECH using the existing dry-run/snapshot/apply operation and verify its resulting eligibility.
