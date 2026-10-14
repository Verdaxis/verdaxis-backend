# KYC Execution Gate: Design and Contract

Date: 2026-10-14
Status: approved by the founder ("go with all your recommendations", 2026-10-14). It must be live on production before a new supplier (Diesol GmbH) is onboarded.
Scope: backend `staging` and `prod`, frontend `staging` and `prod`. This document is the shared contract.
Evidence: an Astra code study of both branches, 2026-10-14. File references below are `[staging]/[prod]` line pairs.

## 1. Rule

A user may **execute** only when the user's KYC is `APPROVED` for the user's current organization, or the user holds a valid **grace** record.

| Action | Gated by the KYC rule? |
|---|---|
| Post an order (BID or ASK), amend an order, publish inventory to an ASK | Yes |
| Hit or take an order (create a trade); auto-match | Yes, for **both** economic parties |
| Confirm a trade | Yes, for both parties (the existing revalidation) |
| Cancel an order, decline a trade, mark delivered, mark paid, retract a supplier offer | **No**. Cleanup must never be blocked by KYC. Keep the existing ownership and authentication checks. |
| RFQ, negotiation and supplier offers (indicative, not executable; acceptance is disabled) | No change in this step |
| Exact replay of an already committed command result (owned, same payload) | Returns the stored result without the new KYC check. Only new executions are gated. Make the order of replay and eligibility checks consistent across create, amend, take and confirm. |

`REJECTED` always blocks and overrides grace. Unknown or legacy statuses (for example `REVIEW_REQUIRED`) count as not approved. KYC statuses are strings (`PENDING`, `SUBMITTED`, `APPROVED`, `REJECTED`); `app/models/user.py:143`.

No blanket exemptions: no shortcut for ADMIN, TEST or non-REAL organizations. The existing narrowly bounded **demo-to-demo** matching path (`app/services/matching_engine.py:215/214`, `:226/225`) stays as it is. Never create fake KYC approvals for demo users.

## 2. Grace

- **New table** `kyc_execution_grace`, preferred over a users column because `deploy/postgres/app_acl_policy.sql` grants blanket DML on users. Columns:
  - `id` (UUID primary key)
  - `user_id` (FK users)
  - `organization_id` (FK organizations)
  - `kyc_status_at_enrollment`
  - `enrolled_at` and `expires_at` (timezone-aware UTC)
  - `created_by`: the migration revision name

  Unique on (`user_id`, `organization_id`).
- **Who is enrolled:** the migration enrolls, once, every user who at migration time:
  - has an account in `APPROVED` status
  - belongs to an approved organization
  - has KYC `PENDING` or `SUBMITTED`

  `expires_at = enrolled_at + 14 days`. Rows are immutable, and a later upload never renews them.
- **Validity:** a grace row is valid only while the user still belongs to the same organization, the KYC is not `REJECTED`, and now < `expires_at`.
- **ACL:** the runtime app role gets **SELECT only** on the table. Writes happen only in the migration. Update the ACL policy and its validation SQL.
- **Users after enrollment:** a user created later never gets grace.

## 3. Assisted (market support) orders

Assisted execution acts for a customer **organization**, so the rule applies to the organization. The customer organization must have at least one active member whose KYC is `APPROVED` for that organization, or who holds valid grace. The administrator's own KYC is irrelevant. Apply this in the assisted owner policy (`app/services/execution_policy.py:54/52`) and in the older market support listing routes (`app/routers/market_support.py:918/867`, `:1154/1096`), through one shared helper.

## 4. API contract

- **Error:** a blocked execution returns HTTP 403 with detail:

  ```json
  {"code": "KYC_REQUIRED", "message": "Complete KYC verification to trade.", "kyc_status": "<status>", "grace_expires_at": "<ISO datetime or null>"}
  ```

  A blocked resting counterparty during matching is not exposed to the taker. Keep the existing "order unavailable" behavior for the counterparty side.
- **Profile:** `UserResponse` (`/api/auth/me`) gains:
  - `kyc_status: str`
  - `execution_eligibility: {"eligible": bool, "reason": "KYC_APPROVED" | "KYC_GRACE" | "KYC_REQUIRED" | "KYC_REJECTED" | "NOT_APPLICABLE", "grace_expires_at": datetime | null}`

  `NOT_APPLICABLE` covers admins and users with no organization.
- Replace the reject-only mirrors in `app/services/market_data_eligibility.py` (`:69/63`) and the trade eligibility projection (`app/routers/trades.py:302/248`) with the same rule, so that what users see matches what they can do.

## 5. Notice

- **In the app:** a banner for users whose `execution_eligibility.reason` is `KYC_GRACE` ("Complete KYC by <date> to keep trading", with a link to `/kyc`) or `KYC_REQUIRED`. Execution controls show the reason instead of a generic error.
- **Email:** a new template, "Complete your KYC to keep trading on Verdaxis", using the shared email layout (`app/services/email.py`). Send it once per grace row through a new operator CLI (`python -m app.cli.send_kyc_grace_notices --environment <env> --release-sha <sha>`, following the existing CLI and timer conventions: explicit environment, no development default, idempotent). Record each send so it is never sent twice. Web workers never send it.

## 6. Migrations (chained after bio-LNG)

The bio-LNG migration lands first on each branch. Write this migration with `down_revision` set to the bio-LNG revision. The integrator adjusts it if the merge order changes.

| Branch | down_revision | revision |
|---|---|---|
| staging | `blng_20261014_staging` (or `rcp_20261003_command_results` if bio-LNG has not merged) | `kycg_20261014_staging` |
| prod | `blng_20261014_prod` (or `uadl_20261003_delivery_reports`) | `kycg_20261014_prod` |

Add the checkpoint rows `<previous> → kycg_…` and `kycg_… → kycg_…`. The migration must:
- create the table
- enroll the grace snapshot with a fixed `enrolled_at` = the migration's own `now()`
- grant SELECT
- have a downgrade that drops the table

## 7. Tests (both branches)

- **Policy:** approved; pending without grace; valid grace; exact expiry boundary; re-upload does not renew; membership change voids grace; rejection overrides grace; legacy status.
- **Endpoints:** order create, amend, inventory publish, take and confirm return `KYC_REQUIRED`; cancel, decline, deliver and pay still work for a non-approved user.
- **Matching:** a resting ask from an ineligible organization does not match an eligible bid, and the taker learns nothing.
- **Assisted:** customer organization with no eligible member is denied; with one eligible member it is allowed.
- **Demo:** demo-to-demo activity still works.
- **Replay:** an exact replay returns the stored result.
- **ACL:** the runtime role cannot insert or update grace rows (in tests/postgres, where the pattern exists).
- **Profile:** the fields; the email CLI is idempotent.
- **Integration fixtures:** positive fixtures get explicit organization-bound KYC approval, and negative fixtures stay pending or rejected.

## 8. Frontend (both branches)

- `src/context/AuthContext.tsx`: read `kyc_status` and `execution_eligibility`.
- Add a deadline banner in the authenticated layout.
- Handle `KYC_REQUIRED` in `src/services/api.ts` error mapping, and in the order modal, take and confirm actions, with a localized message and a link to `/kyc`.
- Locale files: en and zh.
- Tests for the banner, the blocked action and a cleanup action that stays available.
