# Phase 0 Convergence Implementation Plan

> Execute this plan task by task. Stop at each acceptance gate. This document authorizes no host operation. Do not use its command blocks until the founder approves that operation.

**Goal:** Run production and staging from one staging-derived Python source, with production data in a new database and a tested return to the old production release.

**Architecture:** Keep the staging migration graph. Add the missing production contracts to that graph. Build a new production database, copy production records on the production host, and switch the complete runtime tuple under maintenance protection. Keep the old database unchanged. Do not bridge, relabel, or upgrade it.

**Tech Stack:** Python, FastAPI, SQLAlchemy 2, Alembic, PostgreSQL 17/PostGIS 3.6, systemd, React/TypeScript/Vite, GitHub Actions and Vercel.

**Spec:** Founder decisions 44–49 in `outputs/Verdaxis Workstreams - 2026-10-14.md:47–52`; rewrite design sections 1, 6 and 9, including the 15 and 16 October notes; `outputs/research/phase0-convergence-analysis.md`, especially sections 7–9. Paths in this sentence are relative to the Verdaxis workspace, not this backend worktree. Decision 49 replaces the analysis's bridge method and activity-retention recommendation.

## Evidence and scope

This is a source-based plan, not a live inventory. No host or database was contacted. No implementation or application test ran while writing it.

Pinned sources:

| Prefix | Repository and immutable commit |
|---|---|
| `origin/prod:` | backend `c08266bd6969143812dec7c5033338428cd5f159` |
| `origin/staging:` | backend `b218ab264b92f99007bacc05d4871413e024b72a` |
| `FE origin/prod:` | frontend `3f16cabba6424ef08250b32f8f8e4b9a621c4379` |
| `FE origin/staging:` | frontend `3cfeac54e1aaf662cf4f48a3bf3243c445d8a087` |

Read a citation with `git show <pinned-sha>:<path>` and numbered lines. Frontend citations use the frontend repository. The earlier analysis used older commits. Its KYC inventory is no longer complete: staging now has encrypted documents and the gate switch (`origin/staging:alembic/versions/kycd_20261015_staging.py:10–37`; `origin/staging:app/services/kyc_execution.py:1–115`). Do not treat its source counts as live counts.

All proposed names, APIs, tests and commands below are **implementation requirements**, not claims that those tools exist. Current-code claims have branch-qualified citations. Inferences are labelled. Host inspection must replace unknown live values with attested values before approval.

## Global Constraints

- Write implementation work in isolated worktrees. Keep the frontend and Rust worktrees separate. Do not overwrite their work.
- Use `staging` as the canonical source branch in both repositories. Promote full SHAs, not a moving branch. The production path remains `prod/be`; it will track `staging` after the controlled checkout transition.
- Keep Python frozen except for this convergence, security fixes and required operations. No Rust implementation belongs in Phase 0.
- Keep B30, B100 and UCOME_B100 distinct. Keep all existing IDs, economic terms, order priorities, receipts and timestamps. No seed prices for LNG or UCOME.
- Keep `KYC_EXECUTION_GATE_ENABLED=false` in both pilot environments. Keep rejection and organization-binding rules. Do not send grace emails.
- Never copy a production dump, personal data, document, password hash or key to this Mac. Local tests use synthetic fixtures built through the pinned production migrations.
- After producers and the existing outbox sequencer drain, freeze the old `verdaxis` database as read-only input to this procedure. Do not change its schema, rows, ACLs, database name or settings. Do not run the new ACL convergence against it.
- Copy through a read-only source connection and a separate target owner/migrator connection. The runtime app must not gain import authority.
- A target may contain only its exact migration-generated baseline before import. Refuse all other non-empty targets. No merge/upsert mode for business records.
- Keep four Python workers, loopback listeners, distinct app/migrator roles, exact readiness, two deploy locks, durable state, and immutable unit installation. Current contracts: `origin/staging:scripts/deploy.sh:52–76,611–713`; `origin/staging:deploy/systemd/verdaxis-backend.service:9–48`.
- Retain one scheduler per job. Do not start monitor, demo, email, canary or recovery jobs from a rehearsal.
- Do not run the ordinary seed runner on production. It refuses production by design (`origin/staging:README.md:302–309`). A narrow reviewed reference-data bootstrap is not permission to run all seeds.
- No automatic post-write return to the old database. Keep public mutations blocked until the lossless rollback gate passes. After reopening, rollback requires a new data-reconciliation decision.

## Decision problems

1. **A new database cannot use the present production identity unchanged.** Settings require `verdaxis`; the ACL helper repeats that literal (`origin/staging:app/config.py:10–20,195–224`; `origin/staging:scripts/converge_runtime_acls.py:19–32`). Renaming the old database would violate “untouched.” Use the new exact name **`verdaxis_prod`** in the canonical release. Keep the existing exact production role names. Update every identity consumer and backup job for that one new name. Do not introduce an arbitrary database-name override. The old release still requires `verdaxis` for rollback. Task 7 owns this change and its host approval.
2. **An empty database cannot pass the ordinary checkpoint helper.** Deploy requires a literal current revision and the helper requires one current row (`origin/staging:scripts/deploy.sh:577–615`; `origin/staging:scripts/apply_migration_checkpoint.py:109–154,283–317`). Task 4a adds a separate empty-database bootstrap. It runs real migrations to a literal approved target. No stamp and no new `base` alias in the normal checkpoint policy.
3. **“504/503 fail-closed” is not the current vault contract.** Missing/invalid keys produce submission **503**. Download decryption failure produces **500**, not 504 (`origin/staging:app/routers/kyc.py:283–287,330–337`). Test 503 and 500 exactly. Also inject a proxy 504 and prove that the client does not report success. Do not manufacture a vault 504 or weaken the retained-document transaction. A 504 can occur after a successful commit; it is not proof of no submission.
4. **Old discarded KYC files cannot be copied.** Production has KYC review fields and grace, but no `kyc_documents` migration at the pinned SHA. Staging explicitly documents discarded files as unrecoverable (`origin/staging:docs/security-hardening-v2.md:132–151`). Preserve review evidence and audit. Start the new vault empty. Do not invent document rows or ask users to resubmit as part of automatic import.
5. **Lossless rollback has a time boundary.** A frozen old database cannot contain writes accepted by the new one. The tested immediate rollback is lossless only before customer writes resume and after internal-write reconciliation. After that, preserve both databases and reconcile new writes; never silently discard them.
6. **Google Drive is not an unencrypted key vault.** The founder uploads an encrypted offline key-ring archive. Keep recovery access outside Drive and outside the database backup. Verify recovery before acceptance. Do not upload raw keys or customer documents.

**PM decision:** use `verdaxis_prod` permanently. At rehearsal preflight, prove it does not already exist on the live cluster. Stop on a collision; do not drop or reuse it. Keep exact production roles and update identity/backup consumers together.

**PM-proposed, founder confirms before Task 10 — answers 2–6:**

2. Approve a one-use empty bootstrap to the exact reviewed union revision. Bind DDL to the attested connection. Rehearse in a separate host-local cluster. No stamp, generic `base` policy or mutation authority from a dry-run.
3. Accept storage 503 and decryption 500. Treat proxy 504 as uncertain. The client must not claim success or promise that nothing committed. Add no new KYC feature.
4. Copy all KYC review, audit and grace evidence. Start the document table empty. Do not fabricate files or require automatic resubmission. Investigate separately if authorized inspection finds retained files outside this schema.
5. Permit immediate rollback only before new customer writes. Prove the full switch-back in host rehearsal. Keep production closed until acceptance. Drain internal outbox writes too. After reopening, use a forward fix or approved reconciliation; never discard writes automatically.
6. The founder uploads only an encrypted complete key ring. Keep its decryption secret and recovery access separate from Drive and database backups. Prove restore on-host or in an approved offline boundary. Never put raw keys or customer documents in chat or CI.

These proposals do not authorize host operations. Record founder confirmation before Task 10.

## Review changes and scope cuts

This revision adopts `outputs/research/phase0-plan-review-2026-10-16.md` in full for M1–M4 and N1–N3. Task 4a binds Alembic to the checked connection. Task 10 uses exact production identities in a separate scratch cluster. Tasks 7/11 add an outbox-drain gate. Tasks 1/5 correct historical fixtures and catalog/JSON/sequence preservation. Task 8 tests the exact preference routes.

Adopt all five pilot cuts:
- Use one-use operator steps, small assertions, a durable fence and a simple journal. No coordinator, generic workflow engine or mount-namespace venv builder. Build the venv at its final path under maintenance.
- Extend the existing test runner and use one signed acceptance checklist. Do not build an acceptance framework.
- Inject copy failures at representative transaction boundaries, not every batch. Hash every table.
- Run one frontend promotion workflow with its own build-once candidate checks. Reuse existing evidence. If approval must pause, retain and promote the same immutable candidate through the existing release procedure; do not build a candidate-reuse platform.
- Run the full switch-back only in the isolated host rehearsal. Keep pre-write emergency rollback for production.

**Do not cut:** complete schema classification, source preservation, all data hashes, real PostgreSQL trigger/FK/sequence proofs, runtime ACL denials, provider egress isolation, KYC encryption/recovery, restored-backup verification, or full rollback proof. Keep existing deploy, unit-provenance, checkpoint, two-lock/start-guard and exact readiness gates. No seven-day soak is required.

## Review Focus

These five cross-boundary failures are not proved by the existing single-branch tests. Each has a named test in its owning task.

| Failure mode | Required added test | Owner |
|---|---|---|
| Historical trades fail INSERT-time guards even when their stored state is valid; a naive import rewrites their snapshots | `test_historical_snapshot_import_restores_exact_guards` with NULL historical snapshots and version-1 snapshots whose current party state later changed | 5 |
| Copy counts pass but cyclic foreign keys or global sequences cause the next user command to fail | `test_copy_cycles_and_next_sequence_values` with RFQ/quote and support/order cycles, sparse outbox sequences, consumed receipts | 5 |
| Failure after `.env` switch starts old code on new data, or new code on old data | `test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked` using real subprocess locks and filesystem replacement | 7 |
| Cached production frontend calls retired activity or stale quote methods after backend switch | `test_old_and_new_browser_contracts_during_cutover` plus network assertions for zero activity calls in the new build | 8 |
| UTC is set on the database but role overrides or old pooled connections retain Singapore time | `test_timezone_reconnect_and_role_override` against fresh app, migrator and CLI connections | 6 |

## Code convergence ledger

Task references below are the owners and required regression suites. Port behavior, not whole conflicting routers.

| Keep / treatment | Current evidence | Test owner |
|---|---|---|
| B30 UUID `c4cecebc-3d2d-5840-8021-57a9a11bc673`, B100 UUID `74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5`; Biofuel grades; 200 MT minimum; B30 RF 380, B100 DFA/no HVO | `origin/prod:app/market_catalog.py:97–118,196–199`; `origin/prod:alembic/versions/catalog_20260926_biofuels.py:242–260` | 2: catalog/API/qualification tests |
| All production delivery IDs and labels; do not alias a label to a different UUID | `origin/prod:app/market_catalog.py:135–185` | 2: full tuple equality |
| Fixed biofuel ASK specification, whole-fuel lifecycle CI method, certification, MSDS, origin/feedstock and SQL/Python eligibility parity | `origin/prod:app/services/execution_policy.py:142–193,197–225` | 2: create/amend/take/match/inventory/support/public reads |
| Product enum and preferences accept production products, not just four alcohols | `origin/prod:app/schemas/preferences.py:1–25`; contrast `origin/staging:app/schemas/preferences.py:1–14` | 2: preference round trip for all eight products |
| Production benchmark catalog lookup and B30/B100 curve/demo coverage rules; preserve staging's provenance hardening and non-executable UCOME midpoint | `origin/prod:app/services/benchmarks.py:23–40`; `origin/prod:app/seeds/market_seed.py:219–260`; `origin/staging:app/services/forward_curve_market_slices.py:821–830,943–946` | 2: catalog slices and demo-only fixtures; no live seed run |
| LNG strict booleans, scalar certification list and product-aware restricted-lane errors | `origin/prod:app/schemas/lng_terms.py:52–61,92–110`; `origin/prod:app/services/lng_terms.py:92–115`; `origin/staging:app/services/market_catalog_validation.py:46–52` | 3: production payload vectors against staging implementation |
| Keep staging LNG inventory/support/negotiation storage, privacy and no reseller ASK; WTT never becomes lifecycle CI | `origin/staging:app/services/lng_order.py:45–70,96–143`; `origin/staging:ARCHITECTURE.md:223–248` | 3: all entry paths, public/private snapshots |
| KYC profile on PUT recomputes eligibility; dismissal stays cleanup/admitted, not execution-gated | `origin/prod:app/routers/auth_simple.py:1976–1997,2046`; `origin/staging:app/routers/auth_simple.py:1961–1985,2031`; `origin/prod:app/routers/matchmaking.py:155–164`; `origin/staging:app/routers/matchmaking.py:174–180` | 3: gate on/off, rejected, moved member, replay |
| Order-expiry job, email templates, leases, dedup and retry state | `origin/prod:app/services/order_expiry_reminders.py:25–36,59–103,114–235`; `origin/prod:app/cli/order_expiry_reminders.py:1–22`; `origin/prod:app/services/email.py:360–382` | 3: reminders and copied-notification rerun |
| Hide `_email_*`; omit `_email_only` from list and unread count | `origin/prod:app/routers/notifications.py:29–34,49–53,67–69` | 3: list/count/serialization privacy |
| Reminder service/timer in both source bundles; 15-minute schedule; no new staging activation | `origin/prod:deploy/systemd/runtime-units.manifest:16–19`; `origin/prod:deploy/systemd/verdaxis-order-expiry-reminders.timer:4–10` | 7: manifest and dangling-state guard |
| Canary CORS origin `https://canary.verdaxis.exchange` | `origin/prod:app/config.py:46–51` | 7: credentialed preflight, cross-environment rejection |
| Production external monitor/recovery/diagnosis source and tests; preserve current host-owned configuration, do not auto-install it | `origin/prod:deploy/external_monitor/verdaxis_monitor.py:33–87,138–163`; `origin/prod:deploy/external_monitor/verdaxis_recover.py:30–45,289–299`; `origin/staging:ARCHITECTURE.md:208–209` | 7: monitor tests and no-activation check |
| Onboarding attention is already shared; keep its dedup file and production-only activation | `origin/staging:app/cli/onboarding_attention.py:124–145`; `origin/staging:app/services/onboarding_attention.py:71–129` | 3/7: no historical alert replay |
| Shared command receipts, acceptance ordinals, locks, SSE outbox and replay; no duplicate effects | `origin/staging:ARCHITECTURE.md:276–314`; `origin/prod:alembic/versions/rcp_20261003_command_results.py:21–68` | 5/9: replay copied receipts and next sequence |
| Staging FAME/RFQ/offers, public nested-order redaction, corrected CI pricing and non-production MCP | `origin/staging:ARCHITECTURE.md:189,192,250–267`; `origin/staging:app/routers/matchmaking.py:29–36`; `origin/staging:app/main.py:139–160` | 2/3/9: unchanged staging suites |
| Staging worker diagnostic entrypoint stays staging-only | `origin/staging:app/runtime_server.py:20–61`; `origin/staging:deploy/systemd/verdaxis-backend-staging.service:23–27` | 7: unit command checks |
| CI retains current branch gates and adds external-monitor tests; do not restore retired main deploy assumptions | `origin/prod:.github/workflows/backend-ci.yml:7–11,33–38`; `origin/staging:scripts/deploy.sh:26–39` | 7/9: workflow assertions |

**Drop:** activity intake, delivery-loss reports, admin timeline, preference/watchlist activity hooks, activity prune branches, their indexes and runtime grants. Their two tables remain only in the old database. Evidence: `origin/prod:app/routers/user_activity.py:71–115`; `origin/prod:app/models/user_activity.py:25–107`; `origin/prod:scripts/prune_product_analytics.py:93–124`. Keep ordinary product analytics, login-day facts, audit, watchlists and activity SSE. Those are different features (`origin/staging:ARCHITECTURE.md:177,200–201`). Do not port production's illustrative CI cash savings over staging's corrected valuation. Do not restore external KYC document analysis.

## Data inventory and mapping

### Counting rule

The pinned production migrations declare **60 application tables after three renames**. Add `alembic_version` for **61 classified tables**. Source evidence for renames: `origin/prod:alembic/versions/14b159f248ac_rename_quote_request_to_direct_order.py:31–39` and `origin/prod:alembic/versions/f10cf6fa2019_rename_rfq_to_orders.py:23`. Do not double-count `quote_requests`, `quote_offers` or `rfq_matches` under their old names.

**Classification: 55 copy, 2 regenerate reference baseline, 4 skip.** PostgreSQL/PostGIS extension-owned relations are outside this application count. Recreate extensions at the pinned versions; compare extension membership. Task 1 must compare this list with actual catalogs in disposable schemas. Task 10 must compare it with the host dump. Any additional ordinary table, partition or column blocks import until classified. Source inventory is not proof of the live catalog.

Copy means preserve every row by default, including closed history and synthetic reference dependencies. This deliberately avoids a fragile `WHERE provenance='REAL'` filter. It preserves all real pilot data and does not relabel DEMO/TEST/CANARY as REAL. Copying existing synthetic rows is not running a seeder. If the founder wants synthetic cleanup, do it separately after convergence. Preserve UNKNOWN records without granting execution.

### Every production application table

All citations in this table are `origin/prod:`. `models/...` expands to `app/models/...`; `versions/...` expands to `alembic/versions/...`.

| Table | Class | Reason / source |
|---|---|---|
| organizations | Copy | Company IDs, provenance, approval; `models/user.py:61–90` |
| users | Copy | Password hashes, admission, membership FK, KYC review and queued approval mail; `models/user.py:93–210` |
| organization_join_requests | Copy | Pending and reviewed membership history; `models/registration.py:48–83` |
| pending_registrations | Copy | Preserve unfinished pilot onboarding and original expiry; `models/registration.py:16–45` |
| kyc_execution_grace | Copy | Preserve original UUID, enrollment and expiry; `versions/kycg_20261014_prod.py:34–88` |
| audit_logs | Copy | Append-only evidence, including old activity audit actions and notice receipts; `models/audit.py:17–42` |
| user_status_transitions | Copy | Admission history; `models/product_analytics.py:55–85` |
| user_login_days | Copy | Existing aggregate facts, not the retired tracker; `models/product_analytics.py:32–52` |
| user_preferences | Copy | User settings and notifications; `models/user_preference.py:12–28` |
| subscriptions | Copy | Existing plans and fees; `models/subscription.py:24–50` |
| referrals | Copy | Attribution and invitation history; `models/referral.py:29–65` |
| feedback_entries | Copy | Pilot feedback; `models/feedback.py:14–40` |
| ports | Copy | Preserve port identity and geography, not seed replacements; `models/port.py:16–26` |
| port_intelligence | Copy | Preserve port facts and duplicate historical rows; `models/port.py:29–38` |
| vessels | Copy | Customer fleet and geographies; `models/port.py:41–76` |
| producer_projects | Copy | Preserve current dataset, not a stale local seed; `models/producer.py:24–80` |
| products | Regenerate | Exact canonical union baseline, then verify/retain source catalog state as described below; `models/catalog.py:9–35`; `versions/catalog_20260926_biofuels.py:242–260` |
| delivery_points | Regenerate | Exact production IDs plus shared reference baseline; `models/catalog.py:38–60`; `app/market_catalog.py:135–185` |
| inventory_items | Copy | Stock, reservations and supplier records; `models/marketplace.py:24–85` |
| orderbook_orders | Copy | All statuses, owners, quantities, terms, priority and keys; `models/orderbook.py:114–260` |
| trades | Copy | All parties, snapshots, lifecycle, fees and idempotency; `models/orderbook.py:361–530` |
| commissions | Copy | Historical fee/invoice state; `models/orders.py:22–55` |
| match_suggestions | Copy | Preserve dismissal state and links; `models/matchmaking.py:24–55` |
| negotiations | Copy | Pilot negotiation history; `models/negotiation.py:41–136` |
| negotiation_rounds | Copy | Full quoted conversation history; `models/negotiation.py:138–170` |
| rfqs | Copy | Includes accepted historical contracts, not new execution authority; `models/rfq.py:39–86` |
| rfq_quotes | Copy | Quote history and private parties; `models/rfq.py:89–125` |
| staff_capability_assignments | Copy | Preserve grants, expiry and revocation; `models/market_support.py:127–170` |
| market_support_contexts | Copy | Required by receipts/authorization history; do not regenerate opaque IDs; `models/market_support.py:56–124` |
| market_support_authorizations | Copy | Exact customer authority, consumed state and terms; `models/market_support.py:173–290` |
| market_command_results | Copy | Committed replay is an economic record, not a session; `models/command_result.py:18–70` |
| market_event_outbox | Copy | Preserve private event IDs, dispatch state and replay cursors; `models/market_event.py:29–85` |
| notifications | Copy | User history and frozen email/dedup/lease state; `models/notification.py:27–49`; `app/services/order_expiry_reminders.py:114–235` |
| price_alerts | Copy | User alert thresholds and state; `models/alerts.py:9–45` |
| watchlists | Copy | Containers and ownership; `models/watchlist.py:48–75` |
| watchlist_entries | Copy | Legacy user pins; `models/watchlist.py:78–106` |
| watchlist_targets | Copy | Slice/pin identities and snapshots; `models/watchlist.py:111–162` |
| watchlist_events | Copy | Read state and history; `models/watchlist.py:165–215` |
| benchmarks | Copy | Operator reference observations; never replace with demo prices; `models/benchmark.py:12–42` |
| live_slice_benchmarks | Copy | Preserve initial public read values; verify against source, no refresh before acceptance; `models/live_slice_benchmark.py:13–50` |
| market_signal_ingestion_runs | Copy | Evidence ownership/provenance; `models/forward_monitoring.py:33–63` |
| market_indications | Copy | Trusted/unknown evidence and source linkage; `models/forward_monitoring.py:67–140` |
| fair_price_bands | Copy | Historical price evidence; `models/forward_monitoring.py:144–204` |
| physical_stems | Copy | Historical physical evidence; `models/forward_monitoring.py:208–265` |
| seed_runs | Copy | Provenance dependencies, not permission to reseed; `models/seed.py:13–32` |
| market_row_quarantines | Copy | Preserve quarantined original evidence and resolution; `models/seed.py:36–58` |
| organization_market_approvals | Copy | Trusted approval evidence; `models/seed.py:62–95` |
| news_items | Copy | Preserve pilot-visible history and source attribution; `models/news.py:11–45` |
| traceability_events | Copy | Historical evidence, even though old routes are unmounted; `models/compliance.py:10–25` |
| compliance_ledger | Copy | Historical balances; `models/compliance.py:28–55` |
| contracts | Copy | Retained legacy commercial history; `versions/contracts_2026_03_add_contract_supply_demand.py:19–63` |
| supply_listings | Copy | Retained legacy supplier history; same migration `:66–97` |
| demand_profiles | Copy | Retained legacy buyer history; same migration `:100–119` |
| public_listings | Copy | Old listing identities referenced by legacy orders; `versions/ccef95a9c9a0_add_rfq_models.py:24–42` |
| orders | Copy | Renamed RFQ matches; retain legacy history without re-running conversion; `versions/f10cf6fa2019_rename_rfq_to_orders.py:23` |
| direct_orders | Copy | Renamed quote requests; may be referenced by traceability; `versions/14b159f248ac_rename_quote_request_to_direct_order.py:31` |
| direct_order_offers | Copy | Renamed quote offers; preserve old commercial records; same migration `:32–39` |
| refresh_sessions | Skip | Founder permits sign-in again; copy no refresh hashes/families; `models/refresh_session.py:14–55` |
| user_browsing_events | Skip | Retired activity tracker; `models/user_activity.py:25–66` |
| user_activity_delivery_reports | Skip | Retired tracker loss reports; `models/user_activity.py:69–107` |
| alembic_version | Skip | Target is created by real canonical migrations, never copied/stamped; `scripts/apply_migration_checkpoint.py:283–317` |

**Reference regeneration is not destructive replacement.** Bootstrap only deterministic catalog rows, using frozen values from the canonical catalog seed/migrations. Preserve **every source catalog column**. Products: `id`, `name`, `fuel_type`, `fuel_grade`, `unit`, `min_lot_size`, `spec_description`, `is_active`, `created_at`. Delivery points: `id`, `name`, `region`, `timezone`, `is_active`, `created_at` (`origin/prod:app/models/catalog.py:11–19,40–45`). There are no approved value exceptions for source-derived rows. Compare every value, not only the column set. Database session UTC must not replace geographical `delivery_points.timezone` metadata. Validate code-owned lane policy separately. Insert any noncanonical historical row with its original ID and inactive/active state, but do not admit it to the canonical public catalog. Refuse conflicting identities. Record source-derived rows separately from new canonical rows. The classification remains “regenerate reference baseline,” with explicit source preservation; it is not a claim that every source catalog row equals a seed.

New target-only tables: `supplier_offers`, `kyc_grace_notices`, `kyc_documents`. Leave offers/documents empty. Populate notices only from exact production sent-audit evidence. Source: `origin/staging:alembic/versions/fame_20260922_supplier_offers.py:40–82`; `origin/staging:alembic/versions/kycg_20261014_staging.py:33–40`; `origin/staging:alembic/versions/kycd_20261015_staging.py:17–37`.

### Column and constraint mapping

Use an explicit committed mapping manifest. No `SELECT *`, implicit ORM defaults or silent dropped columns. Shared columns copy by name with original values and SQL types. Require an exact source/target column set after applying the following differences.

| Table / object | Mapping and checks |
|---|---|
| rfqs | Preserve all shared columns. New `contract_terms`, `source_offer_id`, `target_supplier_org_id`, `source_offer_snapshot` = SQL NULL. No fabricated FAME declarations. |
| rfq_quotes | Preserve shared fields. `offer_terms` and `expires_at` = NULL; `revision` = 1. Do not turn a past quote into an executable offer. |
| orderbook_orders | Preserve `lng_terms` byte-equivalent JSON meaning; `fame_terms` = NULL. Keep acceptance ordinal and remaining quantity. |
| trades | Preserve every existing snapshot, including `lng_terms_snapshot`; `fame_terms_snapshot` = NULL. Keep NULL historical snapshots and version-1 snapshots unchanged. Numeric `market_snapshot_version=0` is source drift and blocks import. |
| negotiations | `fame_terms_snapshot` = NULL. `lng_terms_snapshot` = NULL only for non-LNG rows. A source LNG row without a recorded paired snapshot is a blocker, not a reason to derive historical terms from today's orders. |
| inventory_items | `fame_terms` = NULL; `lng_terms` = NULL for non-LNG rows. Source Bio-LNG inventory without required terms blocks import pending an exact reviewed disposition. No fabricated declarations. |
| market_support_authorizations | New FAME/LNG terms = NULL for nonmatching products. LNG authorization lacking its frozen terms blocks import. Never infer customer authority from a current order. |
| kyc_execution_grace | Copy seven fields exactly. Keep staging RESTRICT FKs and unique name. Validate production status/duration invariants without resetting clocks. Do not re-run enrollment after users are copied. |
| kyc_grace_notices | For audit action `kyc.grace_notice_sent`, resource type `kyc_execution_grace`, map resource ID to grace UUID; use earliest matching audit timestamp for `claimed_at` and `sent_at`. Refuse malformed/orphan receipts. Never treat absent audit as proof an ambiguous prior send failed. Gate remains off. |
| users / organizations | Preserve password hashes, token cutoffs, KYC organization/status/evidence/reviewer, all approval-mail retry fields and membership. Keep reviewed approvals as data, not new transition events. No new KYC approval. |
| JSON receipts / outbox / notifications | Preserve full internal payloads, hashes, version, operation and actor/context fields. Redact only in API responses, not in stored copy. |
| product checks / trade function | Add B30/B100 beside UCOME and LNG. Preserve target FAME/LNG immutability and lane checks. Do not broaden all products or remove a check to accommodate one row. |
| LNG signal rows | Target restricts Bio-LNG to Rotterdam. Any source out-of-lane signal blocks import. Keep it in the old DB; obtain a row-specific evidence-preserving target disposition before proceeding. |
| old activity indexes | Do not recreate `ix_audit_logs_user_timestamp` or `ix_user_login_days_user_last_login` only for the retired timeline. Preserve the shared indexes. |

Evidence for the 15 new shared-table columns: `origin/staging:alembic/versions/fame_20260922_rfq_contract.py:16–21`; `origin/staging:alembic/versions/fame_20260922_supplier_offers.py:73–82`; `origin/staging:alembic/versions/fame_20260922_b100_orderbook.py:258–303`; `origin/staging:alembic/versions/blng_20261014_staging.py:229–269`. Grace definitions: `origin/prod:alembic/versions/kycg_20261014_prod.py:34–61`, `origin/staging:alembic/versions/kycg_20261014_staging.py:19–40`. Notice receipts: `origin/prod:app/services/kyc_grace_notices.py:58–74,105–124`. Product/lane checks and trigger: `origin/staging:alembic/versions/blng_20261014_staging.py:128–215,217–275`.

### Copy algorithm and authority

The copy script runs only on the production host for real data. Local mode accepts only a disposable numeric-loopback PostgreSQL cluster and synthetic fixtures. Host mode requires a root-owned manifest, exact source/target database names and OIDs, cluster system identifiers and endpoints, host ID, full release SHA, source revision and schema fingerprints. URLs come from no-follow mode-0600 files, never arguments or logs. Reject URL query routing and identical source/target database identity tuples. An OID is unique only within its cluster. Bind each tuple to the cluster system identifier, endpoint, database name and OID; obtain the cluster identifier through a separate trusted operator check without granting runtime control privileges.

1. Open source with the existing read-only backup role. Start one `REPEATABLE READ READ ONLY` transaction. Set its session zone to UTC. Attest source revision and catalog fingerprint.
2. Open target as its migrator owner. Acquire an import advisory lock. Assert exact target revision, baseline fingerprints, no app sessions and no business rows. Baseline catalog/extension rows must equal the bootstrap manifest.
3. Start one target transaction. Lock import tables. Copy the explicit columns in bounded batches. Use text/binary codecs that retain Decimal, UUID, JSON and timestamptz; preserve PostGIS EWKB/SRID. Never use float or ORM constructors with timestamp defaults. Preserve SQL NULL separately from JSON `null`. Use raw JSON text or an exact Decimal codec. Hash those values without merging them. Test high-precision JSON numbers, SQL NULL and JSON `null`.
4. Cyclic FKs need a deliberate import phase. Capture exact FK definitions; temporarily replace only reviewed cyclic FKs with deferrable equivalents inside this transaction, defer them, then restore original definitions and validate before commit. Noncyclic FKs stay active.
5. Historical records may not satisfy **new INSERT** guards. Example: the trade trigger requires version 1 and current party provenance on INSERT (`origin/staging:alembic/versions/blng_20261014_staging.py:136–191`). Attest the complete named user-trigger set. Disable only these exact user triggers in the target import transaction as the table owner. Do not use `session_replication_role`, disable system FK triggers, or grant superuser. Check all target constraints and dedicated historical invariants. Re-enable and compare every trigger/function byte fingerprint before commit. No running app can access the import target.
6. Historical invariants preserve source-valid NULL historical snapshots and version-1 snapshots; they do not require current-party equality for old trades. New version-1 rows require valid frozen identity and allowed provenance pairs. Prove that source and target snapshots match. Exercise the restored new-INSERT guard with a savepoint and an invalid synthetic row; it must reject.
7. Copy notice receipt mappings. Set each sequence so its **next** value exceeds every imported value and does not reuse a source-reserved value. For a positive increment, use a called high-water value at least as large as source `last_value` and imported maximum when rows exist. Preserve an uncalled initial value only for an empty sequence with no imported value at or above it. Verify increment, bounds and cache settings. Test empty, uncalled, sparse and cached sequences; do not call `nextval` on the source. Cover `orderbook_acceptance_ordinal_seq` and `market_event_stream_seq`, plus every discovered owned sequence. A rolled-back sequence change makes the target non-pristine; a retry must refuse it, not silently reuse it.
8. Verify counts, sorted-primary-key streaming hashes and column-value hashes. Check every FK, decimal total, order reservation, trade lifecycle, snapshot, grace deadline, notice claim, receipt, event cursor and reference identity. Run no external side effect.
9. Commit only after all checks pass. Write a private copy receipt with aggregate counts, source snapshot identity, manifest digest and target OID. If receipt publication fails after commit, the next attempt refuses the populated target. Recover by verification, not replay.
10. Converge/validate ACLs from the approved canonical bundle on the **new** database only. Test raw runtime denial of grace/evidence/receipt UPDATE, provenance UPDATE, vault DELETE and control-table writes. The exact current policy is `origin/staging:deploy/postgres/app_acl_policy.sql:22–88,101–217,225–229`.

Do not silently filter a row that fails a stronger constraint. Emit only table, count and private diagnostic reference. Store row IDs/details in a mode-0600 host report. An unresolved blocker prevents cutover.

## Frontend convergence

**Recommendation:** use frontend `staging` as the common source. Port production product contracts and production release/public-site behavior onto it. Do not use a wholesale merge or replace staging's FAME/LNG code. Production has 261 unique commits and staging has 235 according to the supplied fetch result; these are not feature counts.

### Backend-call divergence inventory

The call-site inventory covers the API wrapper, direct fetch calls, auth context, AI client and SSE hook. It distinguishes actual route gaps from dead client methods. Task 8 must generate an executable request manifest from reachable client calls and fail on an unclassified call. Paths in the table are under `/api`. Review exact methods and product-bearing cases; prose categories do not prove route coverage.

| Direction / calls | Difference and disposition | Evidence |
|---|---|---|
| Prod → staging: `POST /activity/events`; `GET /admin/users/{id}/activity` | Routes absent in staging. Remove capture, delivery reporting and drawer. Old tabs may receive 404; no retry loop or financial effect. | `FE origin/prod:src/services/api.ts:652–678,1238–1252`; `origin/prod:app/routers/user_activity.py:71–115` |
| Prod → staging: catalog products; orderbook list/bids/asks/snapshot/with-ci/aggregated/product-counts/map-summary; listings; availability; demand; prices/reference; trade-tape; curves forward/board/table/slice | B30/B100 missing from staging policy; UCOME added. Port IDs, filters, labels and eligibility. CI pricing intentionally differs; update UI to unpriced/nullable monetary values. | `FE origin/prod:src/services/api.ts:902–1123,1194–1228,1367–1410,1492–1503`; `origin/prod:app/market_catalog.py:97–118`; `origin/staging:ARCHITECTURE.md:250–267` |
| Prod → staging: order POST/PUT, trade POST and lifecycle reads, inventory add/update/publish, support authorization/final confirmation | Prod B30/B100 scalar contract must remain. LNG property name stays `lng_terms`, but scalar/certification derivation and strictness differ. Staging adds FAME paired snapshots and review mapping. | `FE origin/prod:src/services/api.ts:791–871,1012–1060,1128–1190,1339–1363`; `origin/prod:app/services/lng_terms.py:92–115`; `origin/staging:app/services/lng_order.py:57–70`; `FE origin/staging:src/services/orderTerms.ts:1–11` |
| Both: `GET /users/me/preferences`, `PUT /users/me/preferences/{namespace}`, watchlist slice targets | Staging preferences hardcode four alcohol products. Prod can persist B30/B100/LNG. Restore canonical enum; include UCOME. Keep saved filters after copy. | `FE origin/prod:src/services/api.ts:681–694,1514–1520`; `origin/staging:app/schemas/preferences.py:1–14`; `origin/prod:app/schemas/preferences.py:1–25` |
| Prod → staging: notification list/unread | Without port, hidden email-only records appear and private email fields leak. Port redaction before data copy becomes reachable. Read/mark methods stay supported. | `FE origin/prod:src/services/api.ts:876–899`; `origin/prod:app/routers/notifications.py:29–69` |
| Both: login, GET/PUT `/auth/me`, password/refresh/logout, signup/invitations | Session mechanics stay shared. PUT profile currently loses computed eligibility on staging; admin/no-org result differs. Keep explicit NOT_APPLICABLE for nontraders; require consistent trader profile after PUT/login/GET. Refresh rows are intentionally absent after copy. | `FE origin/prod:src/context/AuthContext.tsx:127,205,252,278,305`; `origin/prod:app/routers/auth_simple.py:1976–2046`; `origin/staging:app/routers/auth_simple.py:1961–2031` |
| Both: `POST /kyc/submit` | Same multipart call; target retains encrypted evidence and may return storage 503. It no longer calls an external document provider. No new review UI in Phase 0. | `FE origin/prod:src/pages/KycPage.tsx:38`; `origin/staging:app/routers/kyc.py:270–287`; `origin/staging:docs/security-hardening-v2.md:132–158` |
| Both: compliance fleet/vessel/scenario/fuels/pricing-overlay | Staging removes inferred cash savings; absent scenario data yields null ETS and UNPRICED/zero adjustments. Keep corrected UI and tests, not old financial claims. | `FE origin/prod:src/services/api.ts:760–787`; `origin/staging:ARCHITECTURE.md:264–267` |
| Prod → staging: RFQ create/list/detail/quote/cancel | Existing simple RFQs remain valid for non-UCOME. Staging adds contract terms, targeted source snapshot, quote expiry/revision and privacy. Preserve old copied RFQ views. | `FE origin/prod:src/services/api.ts:1454–1473`; `origin/staging:app/routers/rfq.py:421–509,589–683` |
| Prod wrapper: RFQ accept | Both backends disable acceptance with 409. Do not make it executable. | `FE origin/prod:src/services/api.ts:1468–1469`; `origin/prod:app/routers/rfq.py:552–568`; `origin/staging:app/routers/rfq.py:853–869` |
| Prod wrapper: RFQ decline/counter/seller-accept and PATCH quote | No matching prod router routes either. These are stale wrapper methods, not prod features to recreate. Remove them; check reachable callers. Staging uses PUT quote with `expected_revision`, and POST withdraw. | `FE origin/prod:src/services/api.ts:1474–1487`; prod router route set `origin/prod:app/routers/rfq.py:191,312,361,385,552,576`; `FE origin/staging:src/services/api.ts:1464–1468`; `origin/staging:app/routers/rfq.py:751–819` |
| Staging → prod: supplier-offers GET list/my/detail, POST create, PUT update, POST withdraw | Prod backend lacks this router. Keep staging backend and client. Do not release the new UI before backend convergence. | `FE origin/staging:src/services/api.ts:1412–1439`; `origin/staging:app/routers/supplier_offers.py:259–479` |
| Staging → prod: typed UCOME orders/trades/inventory/support; typed RFQ create/quote/revise | Old prod cannot interpret FAME terms or UCOME identity. Keep staging mappings and forms; test coexistence with B100 rather than renaming. | `FE origin/staging:src/services/api.ts:986–1004,1091–1100,1444–1468`; `FE origin/staging:src/services/orderTerms.ts:1–11`; `origin/staging:ARCHITECTURE.md:250–262` |
| Staging → prod: GET matchmaking suggestions, PATCH dismissal | Prod already serves these, but staging public nested projection is safer; dismissal admission differs. Keep projection and fix cleanup rule. | `FE origin/staging:src/services/api.ts:1195–1202`; `origin/staging:app/routers/matchmaking.py:29–36,163–180`; `origin/prod:app/routers/matchmaking.py:155–164` |
| Both: AI chat, private stream-token and SSE, referrals, vessels, ports, news, subscription, alerts, feedback, admin queues/audit/analytics | No new missing-route delta found in the inspected call sites. Product-bearing payloads and daily figures are still covered above. This is a static finding, not a browser test result. | `FE origin/prod:src/services/api.ts:698–757,1232–1335,1414–1450,1553–1570`; `FE origin/prod:src/hooks/useSSE.ts:39–54,187`; `FE origin/prod:src/services/ai-engine/generators.ts:9`; `FE origin/prod:src/components/ReferralsTab.tsx:63–85` |

MCP and vault-admin downloads are backend additions, not current frontend call gaps. Do not build a new frontend for them. MCP remains absent in production (`origin/staging:app/main.py:139–160`).

### Production-only frontend features to port

- B30/B100 types, product lists, fixed specifications, order/take/display behavior, maps, radar filters and bilingual labels. Evidence: `FE origin/prod:src/utils/marketProduct.ts:10–11`; `FE origin/prod:src/utils/marketProducts.ts:18–19`; `FE origin/prod:src/utils/biofuelSpecification.ts:1–12`.
- Public-site content and producer dataset notice; preserve production wording while retaining staging's corrected compliance claims. Inventory changed pages/locales explicitly, not a blanket “take ours.” Evidence: `FE origin/prod:src/pages/public/ProducerMapPage.tsx:19,495`; `FE origin/prod:src/components/public/ProducerDatasetNotice.tsx:1–40`.
- Public route lazy loading, prerender and hydration, metadata, mobile bundle checks and release smoke scripts. Evidence: `FE origin/prod:src/App.tsx:61–113,523`; `FE origin/prod:vite.config.ts:32–97`; `FE origin/prod:src/prerender/renderPublicRoute.tsx:1–40`.
- Vercel production release workflow and all referenced tooling. The workflow checks out `prod`, and the script requires `origin/prod`; both must change together to pinned canonical `staging`. Preserve candidate checks, the optional candidate-only mode, canary CORS, build-once promotion and previous deployment ID. Evidence: `FE origin/prod:.github/workflows/release-vercel.yml:24–81`; `FE origin/prod:scripts/release-vercel.sh:300–315,354–403,426–450`.
- Do **not** port `ActivityTrackingProvider`, `UserActivityDrawer`, activity queue/loss reports or their consent hooks. Retain unrelated analytics/cookie privacy behavior.

## Task order, lanes and estimates

Estimates are focused agent hours plus human/independent review hours, not elapsed calendar promises. They exclude waiting for founder approval. Optimize for one short rehearsal and one cutover, not a seven-day soak.

| Task | Owner | Start dependency; final gate | Agent + review |
|---|---|---|---|
| 1. Pins, mappings and fresh schema fixtures | Astra | none | 4 + 2 h |
| 2. Union catalog and market rules | Astra | 1 | 6 + 2 h |
| 3. Auth/LNG/reminders/privacy | Astra | 1; integrate with 2 | 6 + 2 h |
| 4a. Supplied connection and empty bootstrap proof | Astra | 1; production identity test with 7 | 4 + 2 h |
| 4b. Union revision and final checkpoint policy | Astra | 2, 4a; final contract check with 3 | 3 + 1 h |
| 5. Copy codecs, import and verification | Astra | 1; final acceptance after 4b | 10 + 3 h |
| 6. UTC policy | DeepSeek | 1 | 2 + 1 h |
| 7. Identity, fence and one-use operator steps | Astra | 1; final gate after 3, 4b, 6 | 5 + 3 h |
| 8. Frontend and release contract | Astra | 1; final contract gate after 2, 3 | 7 + 3 h |
| 9. Existing-runner acceptance and Rust handoff | Astra | 2, 3, 4b, 5–8 | 3 + 2 h |
| 10. Isolated host-cluster rehearsal | Astra | 9; founder confirmation and host approval | 4 + 2 h |
| 11. Supervised cutover; emergency rollback only | Astra | 10; short staging check and host approval | 2 + 1 h |

**Total: 56 agent hours + 24 review/operator hours = 80 focused hours.** This is 12 hours below the earlier 92-hour estimate. It includes the supplied-connection path, exact-identity scratch cluster and outbox-drain work. It is not a calendar promise. Re-estimate after the first fresh-schema proof. Host approval waits and data-volume-dependent dump/copy time are excluded.

Fastest safe order and file ownership:
1. Task 1 freezes pins, column mapping and the provisional union contract, then builds the pinned production and staging schemas immediately. Do not wait for all product ports. Task 4a starts as soon as these fixtures exist.
2. Run Tasks 2, 3, 6 and 8 in separate owned lanes. Task 2 owns catalog/qualification/market routers. Task 3 owns auth/LNG/reminder/notification files. The integration owner resolves shared `execution_policy.py` edits.
3. Task 5 starts codecs and FK/trigger/sequence inspection against pinned schemas after Task 1. Only final import acceptance waits for Task 4b. Task 7 starts fixed identity and fence tests after Task 1; it does not wait for the union migration. Task 4b alone owns final checkpoint-policy edits.
4. Publish a provisional frozen union contract after Task 1: eight product IDs, column mappings, UTC and KYC-off semantics. Rust may use it with synthetic fixtures. Mark its digest provisional. Deliver the final Python oracle/schema digest after Task 9, before production cutover; rerun affected goldens if it changes.
5. Run one integrated local gate, one authorized host-local rehearsal, one short staging check and one supervised production cutover. Prepare frontend artifacts before maintenance where the existing release permits. Canonical-backend smoke still waits for backend readiness. Live operations are serial.

### Task 1: Freeze source and build synthetic source fixtures — Astra

**Files:** Create `scripts/phase0/manifest.py`, `deploy/phase0/copy-manifest.json`, `deploy/phase0/provisional-union.json`, `tests/phase0/conftest.py`, `tests/phase0/test_inventory.py`, `scripts/run_phase0_postgres_tests.sh`. Read pinned `alembic/`, models and the analysis. Modify no deployed history.

**Interfaces:** `inspect_catalog(connection) -> CatalogManifest`; manifest records full source SHA, revision, tables/columns/types, FK/trigger/check/index/sequence definitions and reference rows. Test runner creates an isolated PostgreSQL 17/PostGIS 3.6 container with a unique loopback port. It accepts no deployed URL.

- [ ] **Failing test:** assert the manifest accounts for the 61 names above; detect an injected ordinary table/column and refuse. Fixture must include every copy table, all trade states, pending membership, rejected/moved KYC, expired grace, old notice audit, private notification fields, old snapshots, cycles and sparse sequence values.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_inventory.py`. Initially FAIL because runner/manifest are absent. A skipped suite is failure.
- [ ] **Implement:** materialize the pinned production tree using `git archive c08266bd6969143812dec7c5033338428cd5f159` under `work/phase0-fixtures/`. Build its schema using its real migrations and its own import path/venv. Populate NULL historical snapshots at the legal predecessor, then upgrade to `kycg_20261014_prod`. Include version-1 snapshots with later-changed party state. Never insert numeric `market_snapshot_version=0`: both pinned heads permit only NULL or 1 (`origin/prod:alembic/versions/blng_20261014_prod.py:119–130`; `origin/staging:alembic/versions/blng_20261014_staging.py:217–228`). A zero found in host data is drift, not a normal fixture. Never create the source schema from staging ORM metadata. Build the target independently from staging-derived migrations. Inspect catalogs, not only model declarations.

```python
PROD_SHA = "c08266bd6969143812dec7c5033338428cd5f159"
PROD_REVISION = "kycg_20261014_prod"


def require_complete_inventory(actual: set[str], declared: set[str]) -> None:
    if actual != declared:
        raise ValueError("Unclassified source schema; import refused")
```

- [ ] **Run:** runner again; require PostgreSQL version/extension assertions, nonzero tests, exact production source revision, no undeclared table/column and no live-network access. Include renamed legacy tables in comparison.
- [ ] **Commit:** `test: add Phase 0 source schema and copy inventory fixtures`.

**Resume/check:** Record fixture SHA, schema hashes, provisional union digest, table counts and test totals in `outputs/phase0/task-01.md`. If the fresh historical chain cannot build, fix the disposable bootstrap explicitly; do not stamp over it. Task 4a must consume that proof.

### Task 2: Preserve production products on the staging base — Astra

**Files:** Modify `app/market_catalog.py`, `app/market_constraints.py`, `app/models/catalog.py`, `app/models/marketplace.py`, `app/models/forward_monitoring.py`, `app/schemas/catalog.py`, `app/schemas/marketplace.py`, `app/schemas/preferences.py`, `app/services/execution_policy.py`, `app/services/market_data_eligibility.py`, `app/services/public_order_scope.py`, `app/services/benchmarks.py`, `app/services/live_benchmarks.py`, `app/services/forward_curve_market_slices.py`, `app/routers/orderbook.py`, `app/routers/inventory.py`, `app/routers/market_support.py`, `app/seeds/market_seed.py`, `app/seeds/forward_monitoring_seed.py`. Add `tests/unit/test_phase0_catalog.py`, `tests/postgres/test_phase0_market_contract.py`. Reuse production biofuel tests and staging FAME/LNG tests.

**Interfaces:** Canonical product union has eight codes. B100 remains the production scalar product; UCOME_B100 remains typed FAME. Shared qualification helpers expose identical Python and SQL decisions.

- [ ] **Failing test:** parametrize B30/B100 create, amend, take, automatic match, inventory publish, assisted post-only and public reads. Check all production delivery tuples, 200 MT minimum, wrong specification, missing CI method, zero/NULL CI, off-spec, MSDS and certification. Assert UCOME retains its ID, 1 MT minimum and Singapore lane.
- [ ] **Run:** `python -m pytest tests/unit/test_phase0_catalog.py -q`; then the disposable runner for `tests/postgres/test_phase0_market_contract.py`. Expect missing B30/B100 failures.
- [ ] **Implement:** port the exact production `ProductSpec` entries and `BIOFUEL_SPECIFICATION_STANDARDS`; preserve staging's extra execution-mode fields. Import canonical `MarketProduct` into preferences instead of its local four-value Literal. Compose biofuel qualification with existing FAME/LNG helpers, not a replacement policy.

```python
BIOFUEL_SPECIFICATION_STANDARDS = {
    PRODUCT_IDS["B30"]: "ISO 8217:2024 RF 380",
    PRODUCT_IDS["B100"]: "ISO 8217:2024 DFA",
}


def supplier_product_metadata_error(product_id, *, specification_standard,
                                    carbon_intensity_method):
    required = BIOFUEL_SPECIFICATION_STANDARDS.get(product_id)
    if required is None:
        return None
    name = PRODUCTS_BY_ID[product_id].name
    if (specification_standard or "").strip().upper() != required:
        return f"{name} ASK orders require specification_standard {required}"
    if not (carbon_intensity_method or "").strip():
        return f"{name} ASK orders require a lifecycle carbon_intensity_method for the whole supplied fuel"
    return None
```

Port the SQL mirror from `origin/prod:app/services/execution_policy.py:173–193`. Preserve all current scalar checks. Do not reuse the UCOME constant for B100. Port seed *definitions* for test/demo parity, but never invoke market seeds during production copy.

- [ ] **Run:** new tests plus `tests/unit/test_fame_catalog_execution.py`, `test_lng_catalog_execution.py`, `test_market_seed.py`, `test_public_market_catalog_contract.py`, `test_preferences.py`, and all affected market PostgreSQL suites. Assert no inferred compliance cash savings.
- [ ] **Commit:** `feat: preserve production biofuel contracts on canonical staging`.

**Resume/check:** Record product/delivery policy digest and all API fixtures. Task 4b uses frozen SQL literals from this reviewed union. Task 8 uses these IDs, not display labels.

### Task 3: Integrate auth, LNG, reminders and privacy — Astra

**Files:** Modify `app/routers/auth_simple.py`, `app/routers/matchmaking.py`, `app/schemas/lng_order.py`, `app/services/lng_order.py`, `app/services/market_catalog_validation.py`, `app/services/email.py`, `app/routers/notifications.py`. Create `app/cli/order_expiry_reminders.py`, `app/services/order_expiry_reminders.py` from pinned production. Add `tests/unit/test_phase0_behavior.py`; port `tests/unit/test_order_expiry_reminders.py`. Retain staging vault, retention and gate-switch files.

**Interfaces:** `PUT /auth/me` returns `_user_profile`; dismissal uses admitted cleanup authority; LNG accepts only real booleans; reminder retry identity survives copy; public notification payloads never expose email internals.

- [ ] **Failing test:** PUT profile equals subsequent GET eligibility; dismissal works without approved KYC but retains ownership/admission. Reject `"false"`, 0 and 1 as LNG booleans. Assert production certification scalar list. Load copied reminder notifications, rerun job and assert no duplicate email or inbox record. Include expired/reclaimed leases and email-only rows.
- [ ] **Run:** `python -m pytest tests/unit/test_phase0_behavior.py tests/unit/test_order_expiry_reminders.py -q`. Expected failures expose current omissions.
- [ ] **Implement:** preserve staging's one KYC switch and public matchmaking projection. After PUT refresh, `return await _user_profile(db, current_user)`. Select the existing admitted non-execution dependency for dismissal. Use `StrictBool` for the four input booleans and public reseller projection. Add certifications to LNG scalar derivation without filling lifecycle CI. Port production reminder service/CLI and email additions exactly, then integrate imports.

Use this notification validator:

```python
@field_validator("data")
@classmethod
def hide_email_delivery_state(cls, value: dict | None) -> dict | None:
    if value is None:
        return None
    return {key: item for key, item in value.items()
            if not key.startswith("_email_")}
```

Apply `func.coalesce(Notification.data["_email_only"].as_boolean(), False).is_(False)` to both list and unread count. Copy the original email retry/dedup state, not a new scheduler. Do not port user activity.

- [ ] **Run:** new tests; all `test_kyc_*`, auth/profile/org-binding, LNG, trade retry, matchmaking privacy and reminder tests. Run PostgreSQL account rejection/session and LNG suites. Check gate off still blocks REJECTED and moved-review users.
- [ ] **Commit:** `fix: retain production admission reminders and notification privacy`.

**Resume/check:** Record production request vectors and target results. No unit activation. Task 7 owns unit ports. Notice reconciliation belongs to Task 5, not a sender rewrite.

### Task 4a: Bind Alembic to the checked connection — Astra

**Files:** Modify `alembic/env.py`. Create `scripts/phase0/bootstrap.py`, `deploy/phase0/bootstrap-policy.json`, `tests/phase0/test_bootstrap.py`. Reuse Task 1 fixtures. Do not change deployed revision parents.

**Interfaces:** One supplied SQLAlchemy synchronous connection enters Alembic through `Config.attributes`. The bootstrap owns the outer transaction. The ordinary environment-driven deploy path remains unchanged. The one-use CLI takes an approved manifest, explicit target env file and `--apply`; its default checks only manifest structure.

Current `env.py` overwrites the Config URL and opens a new engine (`origin/staging:alembic/env.py:26–29,83–112`). Its normal attestation path commits before migration (`origin/staging:alembic/env.py:65–80`). A Config connection attribute alone does not fix this.

- [ ] **Failing test:** supply target connection A while valid Settings and Config URLs point at unrelated B and source C. Assert DDL uses only A; neither B nor C changes. Test wrong endpoint, cluster identity, database OID, role, absent outer transaction and absent bootstrap approval. Test rollback of DDL on an injected failure. Keep ordinary checkpoint tests unchanged.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_bootstrap.py`. Expect the current environment to ignore the supplied connection.
- [ ] **Implement:** separate connection setup from the migration body. For the supplied path, require a typed, validated bootstrap identity and an already-open caller transaction. On that exact connection, immediately before DDL, attest endpoint, database name/OID, role and role properties, plus the approved cluster binding. Recheck emptiness while holding the bootstrap lock. Never call `commit`, `rollback`, `close`, `asyncio.run` or create an engine in this path. The caller commits only after exact revision/schema verification. Preserve the ordinary path's transaction behavior.

```python
# In env.py; configure_migrations contains the existing context.configure/body.
supplied = config.attributes.get("connection")
if supplied is not None:
    if context.is_offline_mode() or not isinstance(supplied, Connection):
        raise ValueError("Invalid bootstrap connection")
    if not supplied.in_transaction():
        raise ValueError("Bootstrap must own an outer transaction")
    identity = config.attributes.get("phase0_bootstrap_identity")
    attest_bootstrap_connection(supplied, identity)
    configure_migrations(supplied)
else:
    # Retain the current offline/online routing and Settings URL selection.
    run_existing_environment_path()
```

The named helpers are narrow implementation interfaces, not existing functions. Move the Settings URL override into the ordinary path. Do not let conflicting Settings silently select a different engine. Runtime Settings still validate their own values; the bootstrap identity, not their URL, authorizes supplied-connection DDL. Implement the async caller with `AsyncConnection.run_sync`: set `config.attributes["connection"]` to that synchronous connection inside the callback, invoke `command.upgrade`, then remove the attributes in `finally`. The caller holds `async with connection.begin()` across attestation, upgrade and verification. Reject migrations that require an autocommit escape. Test that `context.begin_transaction()` joins this outer transaction rather than committing it.

Use pinned staging head `kycd_20261015_staging` for early synthetic proof only. No host bootstrap is approved to this intermediate target. Task 4b pins the final policy to union. Tests with production Settings use Task 7's exact identity and the separate-cluster model, not arbitrary deployed-name overrides.

- [ ] **Run:** supplied/ordinary path tests, conflicting URL tests and transaction failure tests. Prove zero customer/grace rows after fresh schema build. Verify no new runtime control privileges.
- [ ] **Commit:** `fix: bind Phase 0 bootstrap to its attested Alembic connection`.

**Resume/check:** Record exact engine connection count, cluster/database identities, untouched B/C hashes and transaction outcome. Task 5 can continue codec/import work. No host action yet.

### Task 4b: Add the union revision and final bootstrap policy — Astra

**Files:** Create `alembic/versions/p0_20261016_union.py`, `tests/postgres/test_phase0_union_migration.py`. Modify `deploy/migration-checkpoints.tsv`, `deploy/phase0/bootstrap-policy.json`, `app/market_constraints.py` and exact drift fingerprints. Do not alter earlier revision parents.

**Interfaces:** Revision `p0_20261016_union`, parent `kycd_20261015_staging`. Production bootstrap is a one-use `EMPTY -> p0_20261016_union` operation with exact source/graph hashes and target `verdaxis_prod`. It uses Task 4a's supplied connection. Normal deploy still requires a current revision.

- [ ] **Failing test:** upgrade populated staging and independently build an empty union target. Compare schema, reference data and ACLs. Refuse nonempty target, wrong SHA/cluster/OID, source DB, missing approval and aliases. Prove no customer grace enrollment.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_bootstrap.py tests/postgres/test_phase0_union_migration.py`. Expect the union revision to be absent.
- [ ] **Implement:** add B30/B100 identities, four product-domain checks and union trade function. Keep FAME/LNG constraints and immutable snapshots. Keep staging grace RESTRICT semantics. Freeze SQL; do not import the mutable runtime catalog.

```python
revision = "p0_20261016_union"
down_revision = "kycd_20261015_staging"
branch_labels = None
depends_on = None
```

Append only these normal checkpoint rows:

```text
kycd_20261015_staging	p0_20261016_union
p0_20261016_union	p0_20261016_union
```

Run the historical chain before data import. Never copy `alembic_version`, stamp, or add `base` to normal deploy policy. Bootstrap manifest checking is not deploy approval. Real DDL needs explicit approval and an empty attested target. Local synthetic tests use disposable attestation. Host rehearsal runs `ENVIRONMENT=production` with exact identities in its own cluster; never relabel real data as test data.

- [ ] **Run:** both upgrade paths, `alembic check`, complete schema/ACL comparison and interruption tests. Prove no activity tables, valid B30/B100 history and rejection of invalid UCOME lanes.
- [ ] **Commit:** `feat: add canonical Phase 0 union schema and bootstrap policy`.

**Resume/check:** Record final revision, graph digest and pristine baseline. Task 5's final acceptance and Task 7's final deploy checks now consume these artifacts. Publish the final migration contract to Rust; Task 9 supplies its tested oracle.

### Task 5: Implement host-local copy and verification — Astra

**Files:** Create `scripts/phase0/copy.py`, `scripts/phase0/verify.py`, `scripts/phase0/codecs.py`, `tests/phase0/test_copy.py`, `tests/phase0/test_copy_history.py`, `tests/phase0/test_copy_authority.py`. Modify only the Task 1 copy manifest and test runner as needed.

**Interfaces:** `copy_database(manifest, source, target) -> CopyReceipt`; `verify_database(...) -> VerificationReport`. CLI: `python -m scripts.phase0.copy --manifest <private-path> --source-environment-file <path> --target-environment-file <path> --source-sha <40hex> --target-sha <40hex> --apply`. Default is validation only. Secret files are opened once, no-follow; URLs never print.

- [ ] **Failing test:** implement both Review Focus import tests. Inject failures at six representative boundaries: before data, during a cyclic table, after trigger restoration, after sequence changes, after commit, and before receipt publication. Do not test every batch number. Still hash every table. Assert old source is unchanged and a second copy refuses a populated/partially prepared target. Include unexpected columns and different DB OIDs behind the same URL text.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_copy.py tests/phase0/test_copy_history.py tests/phase0/test_copy_authority.py`. Expect missing implementation, not collection skips.
- [ ] **Implement:** follow the ten-step algorithm and mapping above. A small explicit mapping primitive is:

```python
from dataclasses import dataclass
from typing import Mapping

@dataclass(frozen=True)
class TableMap:
    name: str
    copied_columns: tuple[str, ...]
    added_values: Mapping[str, object]


def map_row(row: Mapping[str, object], mapping: TableMap) -> dict[str, object]:
    if set(row) != set(mapping.copied_columns):
        raise ValueError("Source column set changed")
    return {**row, **mapping.added_values}

RFQ_ADDED = {
    "contract_terms": None,
    "source_offer_id": None,
    "target_supplier_org_id": None,
    "source_offer_snapshot": None,
}
```

Generate reviewed column tuples from the fixture catalog, commit them, then require exact equality on every run. Use SQLAlchemy quoted identifiers from this manifest, bound values and asyncpg batched COPY/INSERT. Do not interpolate caller table names. Business data stays in memory or host-local private files. Collect source sequences through the read-only backup role before closing the snapshot. Store original `is_called` and high-water values.

For notice conversion use exact action/resource matching and earliest successful audit time. Do not run the notice CLI. Preserve all pending approval/reminder claims so timers cannot duplicate past sends. No changes to historical hashes or timestamps to make a response serializer happy.

- [ ] **Run:** all Phase 0 PostgreSQL tests and existing runtime-role tests. Verify each of 55 copy tables, both catalog baselines and four skip policies. Re-run as app role: import must fail before writes. Assert no source writes, no external provider calls, and no plaintext/URL in error logs.
- [ ] **Commit:** `feat: add verified host-local production data copy`.

**Resume/check:** Save synthetic report, counts, source/target fingerprints and measured runtime. A real import with blockers stops before switching configuration. Rebuild a failed disposable target; do not add a force flag.

### Task 6: Make database time UTC — DeepSeek

**Files:** Create `deploy/phase0/set_timezone.sql`, `tests/postgres/test_phase0_timezone.py`. Modify `docs/runtime-hardening.md` to remove stale timezone assumptions. Do not change FAME certificate-date rules.

**Interfaces:** Database default UTC; role/database overrides also UTC for the app, migrator and backup. New connections are required. SQL is parameterized through psql identifiers and an attested database name.

- [ ] **Failing test:** `test_timezone_reconnect_and_role_override`; insert events at 15:59:59, 16:00:00 and 23:59:59 UTC. Check price reference and admin daily buckets under Singapore before change and UTC after reconnect. Check unchanged explicit-UTC login facts and unchanged Singapore certificate expiry.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/postgres/test_phase0_timezone.py`; expect old Singapore connection to fail the UTC expectation.
- [ ] **Implement:** apply the following template through a connection to `postgres`, with literal allowlisted names selected by the tool:

```sql
ALTER DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"app_role" IN DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"migrator_role" IN DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"backup_role" IN DATABASE :"database_name" SET timezone TO 'UTC';
```

Production target is `verdaxis_prod`; staging is `verdaxis_staging`. Do not modify old `verdaxis`. Inspect and reject unexplained higher-priority connection options. Recycle API pools and all CLI processes. Database defaults do not change already-open sessions.

Current daily SQL uses session-sensitive date conversion (`origin/staging:app/routers/price_discovery.py:440`; `origin/staging:app/routers/admin_analytics.py:406,421`). Explicit UTC product analytics already converts with `timezone('UTC', ...)` (`origin/staging:app/services/product_analytics.py:189–202`). **Effect:** staging's session-sensitive day boundary moves from 16:00 UTC to 00:00 UTC. An event at 16:30 UTC stays on its UTC date rather than the next Singapore date. Rolling durations, stored timestamptz instants and all-time totals do not change. Never shift stored timestamps by eight hours. Naive timestamps copy literally; do not guess a zone.

- [ ] **Run:** new tests against fresh app/migrator/backup connections, plus FAME date and analytics suites. Assert epoch equality before/after settings change.
- [ ] **Commit:** `ops: standardize canonical database sessions on UTC`.

**Resume/check:** Record before/after daily fixture values and `SHOW TimeZone` for all three roles. Task 10 repeats this on host scratch; Task 11 checks fresh live pools.

### Task 7: Prepare identity and one-use fenced operator steps — Astra

**Files:** Modify `app/config.py`, `scripts/converge_runtime_acls.py`, `scripts/deploy.sh`, `deploy/systemd/runtime-units.manifest`, both reminder service/timer pairs, `.github/workflows/backend-ci.yml`, `docs/runtime-hardening.md`, `README.md`, `ARCHITECTURE.md`. Port production monitor source/tests without activation. Create `scripts/phase0/assert_host.py`, `scripts/phase0/start_fence.py`, `tests/phase0/test_cutover.py`, `tests/unit/test_phase0_runtime.py`, and `docs/runbooks/phase0-cutover.md` as the one-use operator checklist. Do not create a coordinator or journal schema engine.

**Interfaces:** Exact production database `verdaxis_prod`; exact existing roles; canonical branch `staging` in both fixed checkout paths. The assertion script is read-only and fails closed. It checks one named boundary and emits bounded facts. It does not advance state or run arbitrary commands. The journal is a root-owned append-only text record of operator-verified steps and artifact digests.

- [ ] **Failing test:** `test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked` uses real processes/flocks and filesystem renames. Kill at tuple replacement, unit replacement, restart and readiness boundaries. Test wrong SHA, role, cluster, DB, pending job, dangling fence and stale start permission. Add `test_outbox_drain_before_final_snapshot`: an in-flight producer commits after the first zero count; snapshot must wait for producer drain, sequencer drain and the post-stop zero check.
- [ ] **Run:** `python -m pytest tests/phase0/test_cutover.py tests/unit/test_phase0_runtime.py tests/unit/test_deploy_health.py -q`; expect missing assertions/fence.
- [ ] **Implement:** change only fixed production identity and branch bindings. Start this work after Task 1, without waiting for Task 4b.

```python
_DEPLOYED_DATABASE_IDENTITIES = {
    "production": {
        "database": "verdaxis_prod",
        "app_role": "verdaxis_app",
        "migrator_role": "verdaxis_migrator",
    },
    "staging": {
        "database": "verdaxis_staging",
        "app_role": "verdaxis_app_staging",
        "migrator_role": "verdaxis_migrator_staging",
    },
}
```

Update the ACL helper identity table in the same commit. Audit backup, monitor, migration, preflight and CLI consumers. Explicitly forbid `verdaxis_prod` in seed paths. Keep production canary CORS. Change only production `DEFAULT_BRANCH` to `staging` in deploy; retain paths, roles, service and health URL. Task 4b alone owns final checkpoint rows. Port reminder units and dangling-state denial. Keep vault credentials and four workers.

**Small fixed fence:** install a root-owned systemd drop-in and guard outside the checkout, before replacing source. While `/var/lib/verdaxis/phase0/fence` exists (including a dangling symlink), the guard denies starts unless a root-owned permit binds the exact environment, SHA, database identity and current operator-lock inode. The operator must hold that one-use flock, and the existing deploy guard must independently pass its normal two-lock check. Missing/malformed paths, a released operator lock or stale permit deny. Test this composition; do not replace the existing deploy guard. The permit never authorizes a migration. Recovery/timers remain stopped. Keep the fence until acceptance; do not build automatic recovery.

**Simple journal:** record old/new cluster identity, DB OID, SHA, revision, env/unit/monitor digests, rollback directory, active/enabled/masked timers, outbox count and hash evidence. Append only after each manual check passes. A failed step leaves the fence and maintenance closed. An operator inspects actual state before retry; the journal is not an automatic resume program.

**One-use source switch:** prepare the exact new Git tree and dependencies before maintenance. Preserve the old complete checkout, venv, env, units and runtime/monitor metadata. Under the fence, rename the old directory to its approved rollback location, then rename the clean new tree into the fixed path. Build/install the new venv at that final path from approved cached packages. No mount-namespace build, symlinked root or destructive Git reset. Verify shebangs and source/env identity. The old venv is usable again only when its directory returns to the original path. Publish matching runtime/monitor identity before executing new code with credentials.

Run the existing helper dry-run, exact-SHA unit installer and normal same-revision deploy. The operator does not hold either normal deploy flock; the helper owns them. No extra wrapper engine. The external fence permit only composes with the existing guard for this supervised start. Remove its permit after the helper exits. Failures stay blocked.

**Emergency rollback:** stop new workers, revoke the permit, retain new DB/evidence, restore the old complete tuple to its original path, reinstall its attested units and verify old DB/revision. Use the existing guard ABI under a reviewed one-use restart block holding both deploy locks. Publish exact old restart-authorized state only while both locks are held; require exact old readiness before clearing it. Do not run old SHA through the new graph or change only metadata. Exercise this block in Task 10, not routinely in Task 11.

- [ ] **Run:** boundary kill tests, outbox-drain tests, existing migration/runtime/health/unit/key-source/monitor suites. Prove no old DB grants change and no provider/recovery writer escapes maintenance. Final integration waits for Tasks 3, 4b and 6.
- [ ] **Commit:** `ops: prepare fixed production identity and one-use fenced cutover`.

**Resume/check:** Record tested assertion CLI, fence/drop-in bytes, manual restart block and journal example in `outputs/phase0/task-07.md`. No generic state machine, automatic repair or arbitrary command execution. Missing host-specific commands block Task 10.

### Task 8: Converge the frontend and release tooling — Astra

**Files:** In a separate frontend worktree, modify `src/services/api.ts`, `src/services/orderTerms.ts`, `src/types.ts`, `src/utils/marketProduct.ts`, `src/utils/marketProducts.ts`, `src/components/OrderPlaceModal.tsx`, `Marketplace.tsx`, `MyTrades.tsx`, `src/components/market-support/MarketSupportFinalConfirmation.tsx`, `src/components/notifications/NotificationList.tsx`, `src/App.tsx`, public pages/locales, `vite.config.ts`, `vercel.json`, `src/services/config.ts`, `.github/workflows/release-vercel.yml`, `scripts/release-vercel.sh` and its smoke/build dependencies. Port `src/utils/biofuelSpecification.ts`, public prerender/loaders/dataset notice. Add `src/tests/phase0-api-contract.test.ts`, `src/tests/phase0-cutover.test.tsx`, `scripts/test_phase0_release.py`.

**Interfaces:** One canonical frontend source; separate environment builds/API URLs. Production workflow requires an explicit 40-hex `source_sha` equal to approved canonical branch tip. Candidate and promotion bind the same backend release identity. Old and new frontend builds must be tested against the new backend before switching.

- [ ] **Failing test:** build a manifest of all static and direct API calls from both pinned trees; reject any unmapped method/path. Implement `test_old_and_new_browser_contracts_during_cutover`. Test exact `GET /api/users/me/preferences` and `PUT /api/users/me/preferences/{namespace}` calls with saved B30/B100/LNG filters. Test B30/B100/UCOME distinction, LNG/FAME terms, old RFQ detail, PUT revision, no activity network calls, private notifications, null compliance values and re-login after refresh loss.
- [ ] **Run:** `npm ci && npm test -- --run src/tests/phase0-api-contract.test.ts src/tests/phase0-cutover.test.tsx`; `python -m pytest scripts/test_phase0_release.py -q`. Expect missing ports and stale branch checks.
- [ ] **Implement:** follow the frontend ledger. Keep typed FAME/LNG response normalization. Port production public content/build behavior deliberately. Remove stale RFQ wrapper methods with no server route instead of inventing endpoints. No new KYC admin screen.

```ts
export const FIXED_BIOFUEL_STANDARD: Readonly<Record<string, string>> = {
  B30: 'ISO 8217:2024 RF 380',
  B100: 'ISO 8217:2024 DFA',
};

export function fixedBiofuelStandard(product: string): string | undefined {
  return FIXED_BIOFUEL_STANDARD[product];
}
```

Use one approved promotion run with its own build-once candidate checks. Do not require a separate candidate-only rebuild. If approval must pause, retain that immutable candidate and use the existing reviewed promotion procedure; add no candidate-reuse subsystem. Remote browser checks use synthetic fixtures or public pages only. Never publish private captures, customer screenshots or row payloads as CI/Vercel artifacts.

Workflow checkout uses the approved input SHA, fetches canonical staging, and verifies exact equality before build. Keep `candidate_only=true` default. Add `backend_release_sha` input and assert exact readiness before canary/promotion. Production release still uses `release-vercel.yml`; do not push a manual Vercel build. Keep current project/org, Mapbox gate, immutable candidate, before/after alias checks and dependency-aware rollback. Staging builds from the same SHA with its staging API URL; never reuse the production-configured JS bundle there.

- [ ] **Run:** full Vitest/typecheck/build suites, release script self-test, artifact checker and synthetic browser smoke against disposable canonical backend. Build both environments and assert exactly the intended API origin in artifacts. Test cached old bundle against target backend; classify expected activity 404 as retired, not an API outage.
- [ ] **Commit:** `feat: converge frontend contracts and canonical release source`.

**Resume/check:** Record frontend SHA, build hashes, API call manifest, candidate workflow tests and prior production release procedure. Do not release during this task.

### Task 9: Run integrated acceptance and hand the oracle to Rust — Astra

**Files:** Extend `scripts/run_phase0_postgres_tests.sh` and its runner tests. Add one signed checklist at `outputs/phase0/acceptance.md`. Update architecture/runtime/KYC docs. Do not create `acceptance.py` or a separate status-dictionary framework.

**Interfaces:** The existing runner records commands, test counts, required-suite coverage and artifact hashes. It fails on a subprocess error, missing table, zero tests or skipped required suite. A reviewer signs eleven categories: inventory, schema, copy, ACL, products, auth, privacy, timezone, vault, frontend and rollback. Local rollback proof is required here; host proof is added in Task 10.

- [ ] **Failing test:** omit a required suite or return zero tests; the runner must fail. Use existing runner tests rather than a new acceptance engine. Require all five Review Focus tests plus supplied-connection and outbox-drain cases.
- [ ] **Run:** runner self-tests; expect missing required-suite checks.
- [ ] **Implement:** collect existing suite results and digests once. Link them in the signed checklist. Do not duplicate product/KYC suites or infer success from an empty report.
- [ ] **Run:** full unit/monitor suites, complete disposable PostgreSQL runner, both frontend builds and disposable HTTP integration. Preserve explicit JWT/issuer/audience and ephemeral-loopback integration guards (`origin/staging:README.md:257–295`). Record pass/fail/skip counts and evidence for every category.
- [ ] **Commit:** `test: record Phase 0 integrated gates and Rust oracle handoff`.

**Rust handoff:** Rust may already use Task 1’s provisional union contract with synthetic fixtures. After this local integrated gate, deliver the final tested digest; do not wait for production cutover. Rerun affected goldens if it changes. Deliver one canonical schema at `p0_20261016_union`, one reviewed revision allowlist and one Python oracle SHA. Regenerate catalog policy, goldens and SQLx metadata from that SHA with UTC and pilot KYC off. Include all eight products, no prices for absent liquidity, historical snapshots, notification privacy and copied reference IDs. The Phase 1 plan's Task 2 still names `kycg_20261014_staging` and Singapore defaults (`feat/rust-phase-1-20261015:docs/plans/2026-10-15-rust-phase-1-plan.md:434,473`); its owner must replace those assumptions after compatibility proof. Do not grant Rust SELECT on `alembic_version`: keep the migrator-backed prestart and compiled allowlist (`same plan:47–48`). Phase 0 makes prod a valid target for a **separately approved** Rust Phase 1 production rollout; it does not itself authorize that rollout. Keep the founder go/no-go and measured simulation gate, not a seven-day shadow clock (`same plan:1978–1983`; rewrite design:97–101).

**Resume/check:** Acceptance artifact records backend/frontend SHAs, schema/policy/corpus hashes, UTC, gate-off, role proof and rollback result. Any later code change invalidates the corresponding evidence before host rehearsal.

## Operator command conventions

No commands below ran while writing this plan. The small `scripts.phase0` assertions/bootstrap/copy tools must first pass their owning tasks. They are not orchestration commands. Host-specific proxy, monitor and service commands are written into the approved one-use checklist after reading both authority files. Missing commands are a hard stop.

Use root-owned mode-0600 manifests and secret files under `/root/verdaxis-phase0/`. Keep dumps, hashes and the simple journal under `/var/lib/verdaxis/phase0/` (0700). Record old/new SHAs, revisions, frontend SHA, prior Vercel deployment ID, endpoint and cluster system identifier, DB name/OID, source/target fingerprints, env/unit/monitor digests and timer states. Passwords remain in separate no-follow env/libpq files. The tools do not execute command strings from the manifest.

`/opt/verdaxis-phase0` below is a root-controlled exact-SHA tool installation, prepared and checked before the window. Assertion CLI: `python -m scripts.phase0.assert_host --manifest <path> --check <fixed-name>`. Checks are read-only. Fixed names are `identity`, `quiesced`, `outbox-empty`, `tuple`, `readiness`, `rollback-safe`, `timers`. No check mutates services or advances a journal. Task 7 defines and tests them.

### Task 10: Rehearse in a separate host-local PostgreSQL cluster — Astra, operator-run

**Files:** Private rehearsal manifest, scratch cluster configuration, restored dump, signed checklist and reports. No new orchestration code.

**Interfaces:** Run old and new production-mode runtimes against a **separate host-local scratch PostgreSQL cluster**. Old DB is `verdaxis`; new DB is `verdaxis_prod`. Both use exact roles `verdaxis_app`, `verdaxis_migrator`, `verdaxis_backup`. These are different role objects and credentials from the live cluster. Bind every operation to endpoint, cluster system identifier and database OID. Use `ENVIRONMENT=production`; do not add name overrides or use test mode with real data.

- [ ] **Failing test:** in the synthetic Linux fixture, point an assertion at the live-cluster identity or wrong scratch OID. It must refuse. A missing vault key returns 503 in the isolated service. Never induce a live failure for this test.
- [ ] **Run:** require Task 9 evidence, founder confirmation of decision answers 2–6 and separate host approval.
- [ ] **Implement this one-use rehearsal:** at the authorized EU administrator prompt:

```bash
set -euo pipefail
set +x
test "$(hostname)" = vmi3623757
sudo -n less /home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md
# Read the approved copy of /home/jons-openclaw/VPS_MAP.md from its owner.
getent ahostsv4 api.verdaxis.exchange
getent ahostsv4 app.verdaxis.exchange
sudo -n systemctl show verdaxis-backend.service -p User -p WorkingDirectory -p ExecStart
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 -Atc \
  "SELECT datname, oid FROM pg_database WHERE datname IN ('verdaxis','verdaxis_prod');"
```

Require **no live `verdaxis_prod` database**. If present, stop for review; never drop/reuse it. Verify host/DNS/user/path and current old SHA/revision. Do not infer live identity from this dated plan.

Create a fresh private dump with the existing backup role. Use a root-managed libpq service/passfile, with no password in argv. Example service names below must resolve to the approved endpoints and roles:

```bash
sudo -n /usr/bin/pg_dump --dbname=service=phase0-live-backup \
  --format=custom --no-owner --no-acl \
  --file=/var/lib/verdaxis/phase0/rehearsal.dump
sudo -n /usr/bin/pg_restore --list /var/lib/verdaxis/phase0/rehearsal.dump >/dev/null
```

Use a fresh PostgreSQL 17/PostGIS 3.6 scratch cluster with a separate data directory and system identifier. The reviewed one-use host checklist supplies exact `initdb`, `pg_ctl`, HBA and role-provisioning commands after binary/version/path checks. Bind it only inside an isolated local rehearsal service boundary. Allocate a separate loopback endpoint or Unix socket; no public listener. Capture its system identifier via trusted operator access. Do not reuse live role credentials or change live global role attributes/memberships.

Run the real production units/helpers in an isolated host-local Linux service environment with its own systemd, fixed checkout paths and loopback listeners. Use an existing container/service isolation facility; do not engineer a venv namespace builder. The cluster and runtime boundary have no route to the live database, public API or outbound providers. Verify network denial for Resend, Gemini, Webz, Telegram and Umami before restoring real data. Prepare Git objects and packages before isolation. Provide a read-only local Git mirror with the exact approved branch/SHA and a local package cache, so normal helper fetch/install checks run without outbound access. Attest mirror refs against the approved full SHA; never substitute a fake remote response. Local-only test DNS/proxy maps readiness URLs to scratch Caddy; it must never fall through to public DNS. The exact mechanism and commands are approval prerequisites, not assumed host capabilities.

Restore the dump to scratch `verdaxis`, with scratch production ownership/ACLs. Recreate source grants from the pinned old policy, never mutate live roles. Bootstrap scratch `verdaxis_prod` through Task 4a/4b, then copy and verify. Each service-file endpoint must be attested against the scratch cluster manifest before use:

```bash
sudo -n /usr/bin/pg_restore --exit-on-error --no-owner --no-acl \
  --dbname=service=phase0-scratch-old-owner /var/lib/verdaxis/phase0/rehearsal.dump
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.bootstrap \
  --manifest /root/verdaxis-phase0/rehearsal.json \
  --target-environment-file /root/verdaxis-phase0/rehearsal-target.env --apply
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.copy \
  --manifest /root/verdaxis-phase0/rehearsal.json \
  --source-environment-file /root/verdaxis-phase0/rehearsal-source.env \
  --target-environment-file /root/verdaxis-phase0/rehearsal-target.env \
  --source-sha "$OLD_SHA" --target-sha "$RELEASE_SHA" --apply
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/rehearsal.json \
  --source-environment-file /root/verdaxis-phase0/rehearsal-source.env \
  --target-environment-file /root/verdaxis-phase0/rehearsal-target.env \
  --report /var/lib/verdaxis/phase0/rehearsal-report.json
```

Read SHAs from the approved manifest. Restore exit success and complete table hashes are required; `pg_restore --list` alone is not backup proof. Rehearse Task 11's actual operator steps, deploy helper, immutable installer, fence and old-tuple emergency restoration in this boundary. Seed a pending outbox case only in synthetic tests; a fresh host dump can also contain pending events. Drain scratch producers/sequencer before freezing its source. Preserve hashes around old and new starts and the full switch-back. No SSE clients or authenticated side-effecting GETs during the rollback proof. The live old runtime is not restarted by this rehearsal.

Before and after each runtime start, run the same snapshot/hash verifier. Require zero pending outbox after producer drain, then stop workers and repeat the count before freezing. The full old→new→old tuple switch must preserve the frozen source hashes. Resolve any intentional synthetic test write separately; do not ignore an unexplained delta.

Keep real row IDs, payloads, SQL error details, logs and customer screenshots on the host. Remote/CI checks use synthetic data or public pages only. Customer documents never enter artifacts. Only the founder-approved encrypted key archive may leave through its approved recovery boundary.

**KYC key creation (after separate approval, on prod host):** inspect `/etc/verdaxis` first; if a key exists, verify it and stop rather than replacing it. Create only a new file:

```bash
sudo -n /usr/bin/python3 -I -S - <<'PY'
import base64, json, os, stat
path = '/etc/verdaxis/kyc-document-keys.json'
for parent in ('/', '/etc', '/etc/verdaxis'):
    item = os.lstat(parent)
    if not stat.S_ISDIR(item.st_mode) or item.st_uid != 0 or item.st_mode & 0o022:
        raise SystemExit('Unsafe key directory')
value = {'active': '20261016', 'keys': {
    '20261016': base64.b64encode(os.urandom(32)).decode('ascii')}}
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o400)
with os.fdopen(fd, 'w') as output:
    json.dump(value, output)
    output.flush()
    os.fsync(output.fileno())
os.chown(path, 0, 0)
os.chmod(path, 0o400)
PY
sudo -n stat -c '%U:%G %a %F' /etc/verdaxis/kyc-document-keys.json
```

If `/etc/verdaxis` is absent, inspect `/etc` and create it root-owned without group/other write permission before that command. Expected file metadata: `root:root 400 regular file`. Never print JSON. Unit bytes must contain:

```ini
SetCredential=kyc-document-keys:
LoadCredential=kyc-document-keys:/etc/verdaxis/kyc-document-keys.json
```

These are already in the staging-derived production unit contract (`origin/staging:docs/security-hardening-v2.md:160–178`). The installer checks metadata, not key contents (`origin/staging:scripts/install_systemd_units.sh:230–270`).

Founder step: create an encrypted archive of the complete ring using the approved offline encryption tool; transfer that archive through the approved secure channel; founder uploads it to restricted Google Drive storage. Store recovery secret separately. Download the encrypted archive and recover it offline; prove a synthetic encrypt/decrypt round trip matches the host-created key without printing it. Record archive checksum, access owner and recovery date, not key material. Do not send keys through an agent or chat.

- [ ] **Run acceptance:** copy counts/hashes, historical snapshots, grace/notice states, UTC fresh connections, app-role ACL denials, old/new frontend synthetic requests, four-worker KYC startup, missing/invalid-key 503, decryption 500, injected proxy 504 ambiguity, two-row vault submission, audited no-store download and RESTRICT retention. Test the full switch-back in scratch with old source/DB/unit/env/venv tuple.
- [ ] **Commit/checkpoint:** no host-data commit. Commit only a redacted aggregate rehearsal sign-off under `outputs/phase0/` in a later authorized implementation session. Record elapsed dump/copy/verify/rollback time and free disk reserve. Do not delete scratch/dumps before acceptance; cleanup needs separate approval.

**Resume/check:** A failed rehearsal blocks Task 11. Rehearse again from a fresh dump after any mapping/deploy change. No multi-day wait is required.

### Task 11: Cut over once; retain pre-write emergency rollback — Astra, operator-run

**Files:** Private journal, final dump/copy report, old tuple backup and signed acceptance checklist. No source edits during the window.

**Interfaces:** Founder approves production, staging operations and frontend promotion separately. Operators run the reviewed one-use checklist and existing helpers directly. Assertion scripts only verify boundaries. There is no `cutover` coordinator CLI.

- [ ] **Failing test / preflight:** in the synthetic fixture, wrong expected revision and nonzero pending outbox must fail assertions. On host, run only the approved read-only assertions. Do not execute a deliberately wrong live deploy.
- [ ] **Run:** verify Task 10 evidence, exact remote SHA, clean prepared trees, cached dependencies, disk reserve, permanent-name availability and encrypted key recovery acknowledgement. Reserve time for one supervised cutover, not a routine live rollback/reapply cycle.
- [ ] **Implement these steps in order:**

**A. Short staging check.** After host rehearsal, deploy the canonical backend to staging through the exact allowlisted current→union transition. Stop/drain its recorded timers. Use existing helper dry-run, four approval variables and immutable installer. Set UTC on staging and recycle connections:

```bash
test "$(hostname -s)" = vmi1840561
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 \
  -v database_name=verdaxis_staging -v app_role=verdaxis_app_staging \
  -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging \
  -f /home/verdaxis-prod/verdaxis/staging/be/deploy/phase0/set_timezone.sql
```

Keep KYC off. Do not activate previously inactive reminders. Build the common frontend SHA with staging API settings and run a short approved check. Restore only recorded active staging timers after state is absent.

**B. Prepare production artifacts.** Read `OLD_SHA`, `RELEASE_SHA` and `FRONTEND_SHA` from the approved manifest. Prepare the clean backend Git tree, cached dependencies, old tuple backup and frontend release evidence before maintenance. Keep production frontend alias unchanged. Its canonical-backend smoke must wait for new backend readiness. One promotion workflow will test and promote its own immutable candidate.

**C. Fence producers, then drain the outbox.** Install Task 7's durable start fence and close public API traffic except readiness/public nonmutating checks. Block all producers, including authenticated GET side effects, SSE clients and SQLAdmin. Record active/enabled/masked timer state. Stop news, auth maintenance, analytics prune, reminders, onboarding, demo, signup canary and recovery timers through the approved host list. Let active oneshots and in-flight requests finish naturally. Require no pending job. Stop independent writer services. Keep the old API workers running briefly **only to drain their sequencer**.

The dispatcher is an automatic writer, not a timer (`origin/staging:app/main.py:119–124`). It updates `stream_seq`, `dispatched_at` and `delivery_attempts` (`origin/prod:app/services/market_event_dispatch.py:74–89`). A public mutation fence alone is insufficient.

After all producers have finished, poll the exact pending predicate to zero with a bounded deadline. Record count, event high-water mark and hashes. Then stop the last old worker and recheck zero plus absence of writer sessions. Timeout/nonzero means stop the procedure; do not sequence rows with ad hoc SQL.

```bash
sudo -n systemctl list-jobs --no-pager
# phase0-live-backup is the approved read-only libpq service.
sudo -n psql -X --dbname=service=phase0-live-backup -v ON_ERROR_STOP=1 -Atc \
  "SELECT count(*) FROM market_event_outbox WHERE stream_seq IS NULL;"
# Continue only after producers drained and this count is zero.
sudo -n systemctl stop verdaxis-backend.service
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/approved.json --check quiesced
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/approved.json --check outbox-empty
```

The assertions check every writer session, not only active queries. No SSE connection or authenticated request may reopen. Do not terminate unknown sessions or alter old DB grants; find and stop their owner. The post-stop zero check and frozen hash mark the start of the untouched old-DB guarantee. No routine old-runtime restart follows.

**D. Final backup, bootstrap and copy.** With the fence and maintenance closed, take a fresh consistent dump. Restore it in the isolated scratch cluster and verify completeness. Do not accept only archive readability. Never copy a live PostgreSQL data directory.

```bash
sudo -n pg_dump --dbname=service=phase0-live-backup \
  --format=custom --no-owner --no-acl \
  --file=/var/lib/verdaxis/phase0/final.dump
sudo -n pg_restore --list /var/lib/verdaxis/phase0/final.dump >/dev/null
```

Run the already rehearsed restore/hash commands against a newly prepared scratch restore target. Keep all customer data on the host. Record the dump digest and source snapshot hash in the journal.

Recheck that live `verdaxis_prod` is absent. Create it with exact migrator owner and reviewed PostGIS setup. Keep existing role credentials/properties unchanged; do not run global role bootstrap on the live cluster. Apply only target-scoped ownership/grants. Set UTC on the new DB with Task 6 SQL and exact production roles. Do not alter old `verdaxis`:

```bash
sudo -n -u postgres createdb --owner=verdaxis_migrator verdaxis_prod
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 \
  -v database_name=verdaxis_prod -v app_role=verdaxis_app \
  -v migrator_role=verdaxis_migrator -v backup_role=verdaxis_backup \
  -f /opt/verdaxis-phase0/source/deploy/phase0/set_timezone.sql
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.bootstrap \
  --manifest /root/verdaxis-phase0/approved.json \
  --target-environment-file /root/verdaxis-phase0/target.env --apply
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.copy \
  --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/source.env \
  --target-environment-file /root/verdaxis-phase0/target.env \
  --source-sha "$OLD_SHA" --target-sha "$RELEASE_SHA" --apply
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/source.env \
  --target-environment-file /root/verdaxis-phase0/target.env \
  --report /var/lib/verdaxis/phase0/final-copy-report.json
```

The manifest binds the preapproved new database name and cluster. After creation, record its OID in a separately signed identity record before bootstrap; an absent/changed OID refuses. Complete the rehearsed extension and target-scoped ACL setup before bootstrap. Bootstrap runs real migrations on the supplied connection before copying customers. Copy refuses any non-pristine target. No seeds or upserts.

**E. Install the new tuple under the durable fence.** Follow Task 7's tested rename procedure. Preserve old source/venv/env/runtime/units/monitor tuple. Install the clean new tree at the fixed path. Build its venv there from the approved cached dependencies, and check entrypoints. Update both explicit URLs and component settings to `verdaxis_prod`; preserve all other production secrets and gate-off. Do not print env diffs. Publish matching source/runtime/monitor identity. Skip refresh sessions but preserve password hashes and token cutoffs; do not rotate JWT merely for this copy.

Append tuple digests to the journal, then assert them. Hold the one-use operator lock for authorized starts; do not hold the helper's deploy locks. Install the root-owned start permit only after tuple verification. It does not bypass the existing guard.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/approved.json --check tuple
sudo -n -u verdaxis-prod -H \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh --dry-run
sudo -n /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --dry-run --environment production --source-ref "$RELEASE_SHA"
sudo -n /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --apply --environment production --source-ref "$RELEASE_SHA"
sudo -n -u verdaxis-prod -H env \
  APPROVED_RELEASE_SHA="$RELEASE_SHA" \
  MIGRATION_APPROVED_SOURCE_SHA="$RELEASE_SHA" \
  MIGRATION_EXPECTED_CURRENT_REVISION=p0_20261016_union \
  MIGRATION_TARGET_REVISION=p0_20261016_union \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh
```

The operator must compare the dry-run printed SHA with the approved manifest before continuing. Repeat this now-live helper dry-run even if the old helper inspected the candidate. The dry-run is not DB or application proof. The installer validates immutable bytes but starts nothing. Normal deploy owns both locks, verifies the union→union row, converges new-target ACLs, publishes exact identity and checks readiness before clearing state. Remove the external permit after deploy exits; keep the fence and public maintenance until acceptance. A failure leaves recovery to the operator.

**F. Verify before new customer writes.**

```bash
sudo -n systemctl is-active --quiet verdaxis-backend.service
curl --fail --silent --show-error https://api.verdaxis.exchange/health/ready |
  /home/verdaxis-prod/verdaxis/prod/be/venv/bin/python \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/validate_health_response.py \
  --environment production --release-sha "$RELEASE_SHA"
sudo -n test ! -e /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
sudo -n test ! -L /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.assert_host \
  --manifest /root/verdaxis-phase0/approved.json --check readiness
```

Require exact local/public four-key readiness, full SHA, DB/revision/role, UTC and four worker gate-disabled log lines for the current start. Require no vault-unavailable line. Readiness alone is not a vault check: use the rehearsal credential round trip and installer metadata proof. If a synthetic production submission is separately required, record its durable evidence; do not test with customer documents.

Compare source and target hashes after new startup. Drained outbox rows must remain unchanged. Any unexpected internal write blocks acceptance. Keep authenticated side-effecting reads and SSE blocked. Product IDs, lanes and public privacy checks may run; private account/order/trade tests stay in the host rehearsal or synthetic fixtures. Never export customer captures to CI. Preserve receipts and audit values.

**G. Emergency rollback only.** Do not run a routine live drill. Task 10 already proved the full switch-back. If a pre-write check fails, keep maintenance, stop new workers and run `assert_host --check rollback-safe`. Require no customer writes and account for all internal/canary writes since the snapshot. Unexplained writes block automatic assumptions of a lossless return.

Restore the old complete tuple and URL pair (`verdaxis`) with Task 7's tested manual steps. Keep the failed new DB and evidence. Restore original paths so the old venv works. Install exact old units. With the operator lock and both deploy/restart locks held, use the tested old guard ABI and exact old SHA/revision for the one-use restart. Check old local/public readiness before clearing state. No Alembic downgrade, metadata-only rollback or destructive Git reset. Do not try old source through the canonical migration graph. If frontend promotion occurred, restore its recorded previous deployment too. Timers stay stopped until the operator accepts the recovered state. After reopening customer writes, use a forward fix or separately approved data reconciliation instead.

**H. One frontend promotion, then acceptance.** With promotion approval already granted, run one protected workflow. It builds once, tests its candidate against the canonical backend and promotes the same bytes. Keep alias comparisons and the prior deployment ID. Do not add a separate candidate-only rebuild just for a second report.

```bash
gh workflow run release-vercel.yml --repo Verdaxis/verdaxis-frontend --ref staging \
  -f source_sha="$FRONTEND_SHA" -f backend_release_sha="$RELEASE_SHA" \
  -f candidate_only=false -f notify_release=true
```

These Task 8 inputs must exist and pass release tests first. If separate approval must pause promotion, retain the immutable candidate and use the existing approved release procedure; do not build a new candidate-reuse platform. Smoke must not call customer-authenticated APIs while the rollback window is open. Use public pages/synthetic evidence. Verify intended API URL, products, no activity capture and release identity.

After founder acceptance, append the write-open time and signed eleven-category checklist. Remove the one-use fence/drop-in through its approved cleanup steps; daemon-reload and verify normal guard ownership remains. Open customer traffic. Restore **only** timers recorded active, and only after deploy state is absent. Do not enable inactive/masked jobs. Check their new DB identity and single scheduler. Keep grace sender off. Update backup target to `verdaxis_prod`; retain old DB and final dump under restricted retention.

- [ ] **Run final checks:** canonical source/schema, UTC fresh connections, KYC off, vault recovery, correct backup target, no old DB writer, no activity tracker and exact restored timer state.
- [ ] **Commit/checkpoint:** later implementation session commits only redacted sign-off and docs. No dumps, keys, env, host journal, customer payloads or identifiers.

**Resume/check:** A failed step leaves maintenance/fence closed. Inspect actual files, processes, locks and DB identities against the journal before any manual retry. Do not clear a guard to make a failed start work. No automatic resumption or repair engine.

## Final acceptance and durable resume point

Implementation starts at **Task 1**. This planning commit runs none of the tasks.

For each completed task, record: commit SHA, exact commands, pass/fail totals, artifact paths/digests, unresolved blockers and the next task. Do not mark a task complete from source inspection alone. A resume session reads this plan and the latest `outputs/phase0/task-*.md`, then verifies the working tree and approved SHAs before continuing.

Phase 0 is complete only when:

- [ ] One canonical backend branch and frontend branch serve both environments with explicit environment activation/configuration.
- [ ] All 61 source application/control tables are accounted for; copy/reference/skip counts and invariants are verified on the host.
- [ ] B30, B100 and UCOME_B100 remain distinct; original production IDs and economic history survive.
- [ ] Old production database and old complete release tuple remain recoverable; full pre-write rollback was exercised in the isolated host rehearsal, not routinely on the live old database.
- [ ] UTC is effective on fresh connections; no stored timestamps were shifted.
- [ ] Production vault works; encrypted offline key backup and recovery are verified; pilot KYC gate stays off.
- [ ] Frontend production release passes its protected Vercel candidate/promotion checks; no activity capture remains.
- [ ] Rust receives the canonical revision/policy/oracle artifacts and its owner updates the Phase 1 assumptions before targeting prod.
