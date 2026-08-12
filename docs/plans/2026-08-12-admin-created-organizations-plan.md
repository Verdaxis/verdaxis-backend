# Admin-Created Organizations Implementation Plan

1. Extend the invitation request schema with an exactly-one-of organization
   source and bounded new-organization fields.
2. Add backend contract tests before implementation, then create and approve
   the organization inside the existing invitation transaction with domain,
   side, database-owned provenance, conflict, and audit enforcement.
3. Update backend architecture and route documentation.
4. Extend the Admin Users dialog and API types with existing/new organization
   modes, country selection, role-compatible type selection, and translations.
5. Add focused frontend interaction tests and update frontend architecture.
6. Run backend and frontend quality gates, adversarially review the privilege
   and transaction boundary, deploy to staging, and dogfood the full flow.
7. Promote the identical reviewed commits to production only after staging
   passes, then rerun live health, signup canary, and invitation smoke checks.
