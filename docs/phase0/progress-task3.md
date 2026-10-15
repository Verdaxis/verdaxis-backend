# Phase 0 Task 3 — auth, LNG, reminders and privacy

## Scope and source

Lane: `feat/phase0-task3`, starting at `a92dc44`. Production behavior source:
`c08266bd6969143812dec7c5033338428cd5f159`. No host, live database, provider,
seed runner, timer activation or push was used. All records are synthetic.
No packages were installed. The KYC switch, vault and notice sender are unchanged;
pilot deployment must retain `KYC_EXECUTION_GATE_ENABLED=false`.

## Implemented contracts and frontend handoff (Task 8)

- `PUT /api/auth/me` now returns the same computed `execution_eligibility` as
  GET and login. The object keys remain `eligible`, `reason`, `grace_expires_at`.
  Admin and no-organization profiles return `NOT_APPLICABLE`. With the gate off,
  eligible traders return `KYC_NOT_ENFORCED`; rejected and moved-review users
  remain blocked. No profile fields were removed.
- `PATCH /api/matchmaking/suggestions/{order_id}/dismiss` uses staging's existing
  non-execution admission helper. Pending KYC does not block dismissal in either
  gate state. Rejected KYC, a foreign review binding and unapproved accounts still
  fail admission. Dismissals and repeated requests remain organization-scoped.
  Responses stay `{"status":"dismissed"}` or `{"status":"already_dismissed"}`.
  Public nested-order projection is unchanged.
- Four LNG input fields require JSON booleans, not strings or numbers:
  `union_database_registered`, `seller_is_physical_supplier`,
  `require_union_database`, `accept_resellers`. Public `reseller` is also typed
  strictly. Existing boolean request/response shapes are unchanged.
- LNG scalar `certifications` is now the production label list: `ISCC EU` and/or
  `REDcert-EU` for BID accepted schemes; one declared scheme for ASK. BID scalar
  `certification_scheme` is null. ASK scalar scheme keeps the matching label.
  Both lifecycle CI scalars remain null; WTT stays only in typed LNG terms.
- Invalid Bio-LNG delivery lanes return HTTP 400 with
  `Bio-LNG is available for Rotterdam only`, not the old Singapore message.
  UCOME's Singapore restriction is unchanged.
- Notification responses omit every top-level `_email_*` field. List and unread
  count exclude `_email_only=true` rows. Null data and ordinary notifications
  remain visible. Stored payloads are not redacted or rewritten by reads.
- Reminder service is the exact pinned production implementation: dedup by
  order/recipient/expiry, frozen payload, notification-ID provider key, durable
  15-minute retry lease, 23-hour retry limit and terminal state cleanup.
  Email-only copied rows retain dedup authority without appearing in the inbox.

## Test-first evidence

- Initial prescribed two-file run failed collection because the reminder module
  was absent. Behavior-only run: 27 failed, 1 passed. Two fixture assumptions
  were corrected (synchronous SQLAlchemy result and `Singapore` catalog key).
- Privacy fix passed before reminder code was introduced. A formatter removed an
  initially unused validator import; the first privacy commit therefore had an
  import error. The next small corrective commit restored it and passed the test.
- Dismissal vectors before dependency replacement: 4 failed, 4 passed.
- Nonproduction reminder CLI/transport vectors before guards: 6 failed.
- Auth/LNG port gate: 67 passed. Reminder integration gate: 43 passed.
- Final Task 3 contracts: 45 passed. These include copied sent rows, active and
  reclaimed leases, expired deadlines, no duplicate inbox rows, stable provider
  keys, profile gate on/off, and dismissal replay scope.
- Final focused KYC/auth/LNG/matchmaking/retry/onboarding run: 301 passed.
  Onboarding historical baseline behavior is unchanged and covered by existing tests.
- Final full unit run: 2132 passed, 19 failed, all failures in
  `test_deploy_health.py` under macOS Bash 3.2 (`mapfile` is unavailable).
  No other test failed. `git diff --check` passed.
- Required `bash scripts/run_phase0_postgres_tests.sh`: 39 passed, no skips.
- PostgreSQL account rejection/session and both LNG suites: 8 passed, no skips.
  Both runners exited successfully and removed their containers with volumes.

## Deviations and integration notes

- The pinned reminder CLI and email transport had no environment guard. Added
  production-only checks to meet this task's explicit staging/test restriction.
  Tests replace transport with mocks; no real reminder or grace email is sent.
  Task 7 still owns units and all activation. No scheduler was added to workers.
- `execution_policy.py` is untouched; Task 2 scalar qualification is preserved.
- `ARCHITECTURE.md` has two file-map entries for the new CLI/service. This small
  shared-document edit is required by repository instructions; retain both when
  merging other lanes. No other lane-owned implementation file was changed.
- The existing general PostgreSQL runner removes containers without volumes.
  An untracked copy under `work/` changes only root resolution and cleanup to
  `docker rm -f -v`; it runs the existing account/LNG suites on the pinned image.
  No committed runner or migration/ACL policy was changed.
- Test-first commits include failing vectors before later ports; merge the full
  lane, not its first commit alone. An initial focused command had a nonexistent
  `test_org_binding*` glob; the corrected run includes `test_kyc_org_binding.py`.

## Resume / acceptance

Task 3 is complete. Merge the full lane into the PM integration branch.
Implementation commits: `2d56b80`, `17bfc06`, `7bbed04`, `49426bb`, `5f43daa`.
All requested local test gates ran. Live/browser cutover checks were not run:
they belong to later tasks and no host operation is authorized here.
Task 4b owns the union schema checkpoint. Task 5 owns notice reconciliation.
Task 7 owns deployed runtime identity and unit changes. Task 8 must consume the
response notes above. No changes to `progress.md`, units, migrations, runtime
ACLs, catalog IDs or the existing KYC switch are included.

## Review follow-ups (lane `feat/phase0-task3minor`)

The independent review of 2026-10-16 raised N1 and N2. Both are closed here.

- N1: `_user_profile` now uses the production condition
  (`role not in (BUYER, SUPPLIER) or organization_id is None`) for the
  `NOT_APPLICABLE` branch, so a role-less onboarding profile no longer reports
  `KYC_NOT_ENFORCED`. Login, GET and PUT vectors cover both gate states.
- N2: `tests/postgres/test_order_expiry_reminders_concurrency.py` proves
  concurrent scan dedup, the persisted retry lease across the uncertain-send
  window, and copied-row inbox privacy on PostgreSQL 17. Run with
  `bash scripts/run_product_analytics_postgres_tests.sh <file>`; 3 passed.

Pinned production behavior, the privacy and environment guards, the KYC switch
and the reminder service are unchanged.
