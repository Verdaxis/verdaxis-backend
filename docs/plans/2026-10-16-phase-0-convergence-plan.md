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
- The old `verdaxis` database is read-only input to this procedure. Do not change its schema, rows, ACLs, database name or settings. Do not run the new ACL convergence against it.
- Copy through a read-only source connection and a separate target owner/migrator connection. The runtime app must not gain import authority.
- A target may contain only its exact migration-generated baseline before import. Refuse all other non-empty targets. No merge/upsert mode for business records.
- Keep four Python workers, loopback listeners, distinct app/migrator roles, exact readiness, two deploy locks, durable state, and immutable unit installation. Current contracts: `origin/staging:scripts/deploy.sh:52–76,611–713`; `origin/staging:deploy/systemd/verdaxis-backend.service:9–48`.
- Retain one scheduler per job. Do not start monitor, demo, email, canary or recovery jobs from a rehearsal.
- Do not run the ordinary seed runner on production. It refuses production by design (`origin/staging:README.md:302–309`). A narrow reviewed reference-data bootstrap is not permission to run all seeds.
- No automatic post-write return to the old database. Keep public mutations blocked until the lossless rollback gate passes. After reopening, rollback requires a new data-reconciliation decision.

## Decision problems

1. **A new database cannot use the present production identity unchanged.** Settings require `verdaxis`; the ACL helper repeats that literal (`origin/staging:app/config.py:10–20,195–224`; `origin/staging:scripts/converge_runtime_acls.py:19–32`). Renaming the old database would violate “untouched.” Use the new exact name **`verdaxis_phase0`** in the canonical release. Keep the existing exact production role names. Update every identity consumer and backup job for that one new name. Do not introduce an arbitrary database-name override. The old release still requires `verdaxis` for rollback. Task 7 owns this change and its host approval.
2. **An empty database cannot pass the ordinary checkpoint helper.** Deploy requires a literal current revision and the helper requires one current row (`origin/staging:scripts/deploy.sh:577–615`; `origin/staging:scripts/apply_migration_checkpoint.py:109–154,283–317`). Task 4 adds a separate empty-database bootstrap. It runs real migrations to a literal approved target. No stamp and no new `base` alias in the normal checkpoint policy.
3. **“504/503 fail-closed” is not the current vault contract.** Missing/invalid keys produce submission **503**. Download decryption failure produces **500**, not 504 (`origin/staging:app/routers/kyc.py:283–287,330–337`). Test 503 and 500 exactly. Also inject a proxy 504 and prove that the client does not report success. Do not manufacture a vault 504 or weaken the retained-document transaction. A 504 can occur after a successful commit; it is not proof of no submission.
4. **Old discarded KYC files cannot be copied.** Production has KYC review fields and grace, but no `kyc_documents` migration at the pinned SHA. Staging explicitly documents discarded files as unrecoverable (`origin/staging:docs/security-hardening-v2.md:132–151`). Preserve review evidence and audit. Start the new vault empty. Do not invent document rows or ask users to resubmit as part of automatic import.
5. **Lossless rollback has a time boundary.** A frozen old database cannot contain writes accepted by the new one. The tested immediate rollback is lossless only before public writes resume. After that, preserve both databases and reconcile new writes; never silently discard them.
6. **Google Drive is not an unencrypted key vault.** The founder uploads an encrypted offline key-ring archive. Keep recovery access outside Drive and outside the database backup. Verify recovery before acceptance. Do not upload raw keys or customer documents.

These are explicit operational resolutions, not silent changes to founder decisions. Approve the exact new database name, bootstrap authority and pre-write rollback boundary before Task 10.

## Review Focus

These five cross-boundary failures are not proved by the existing single-branch tests. Each has a named test in its owning task.

| Failure mode | Required added test | Owner |
|---|---|---|
| Historical trades fail INSERT-time guards even when their stored state is valid; a naive import rewrites their snapshots | `test_historical_snapshot_import_restores_exact_guards` with version-0/NULL snapshots and changed current organization state | 5 |
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

**Reference regeneration is not destructive replacement.** Bootstrap only deterministic catalog rows, using frozen values from the canonical catalog seed/migrations. For each source product/delivery row, compare ID, name, fuel type/grade, unit, lane and minimum. Preserve original timestamps and mutable active state. Insert any noncanonical historical row with its original ID and inactive/active state, but do not admit it to the canonical public catalog. Refuse conflicting identities. Record source-derived rows separately from new canonical rows. The classification remains “regenerate reference baseline,” with explicit source preservation; it is not a claim that every source catalog row equals a seed.

New target-only tables: `supplier_offers`, `kyc_grace_notices`, `kyc_documents`. Leave offers/documents empty. Populate notices only from exact production sent-audit evidence. Source: `origin/staging:alembic/versions/fame_20260922_supplier_offers.py:40–82`; `origin/staging:alembic/versions/kycg_20261014_staging.py:33–40`; `origin/staging:alembic/versions/kycd_20261015_staging.py:17–37`.

### Column and constraint mapping

Use an explicit committed mapping manifest. No `SELECT *`, implicit ORM defaults or silent dropped columns. Shared columns copy by name with original values and SQL types. Require an exact source/target column set after applying the following differences.

| Table / object | Mapping and checks |
|---|---|
| rfqs | Preserve all shared columns. New `contract_terms`, `source_offer_id`, `target_supplier_org_id`, `source_offer_snapshot` = SQL NULL. No fabricated FAME declarations. |
| rfq_quotes | Preserve shared fields. `offer_terms` and `expires_at` = NULL; `revision` = 1. Do not turn a past quote into an executable offer. |
| orderbook_orders | Preserve `lng_terms` byte-equivalent JSON meaning; `fame_terms` = NULL. Keep acceptance ordinal and remaining quantity. |
| trades | Preserve every existing snapshot, including `lng_terms_snapshot`; `fame_terms_snapshot` = NULL. Keep NULL/version-0 historical snapshots unchanged. |
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

The copy script runs only on the production host for real data. Local mode accepts only a disposable numeric-loopback PostgreSQL cluster and synthetic fixtures. Host mode requires a root-owned manifest, exact source/target database names and OIDs, host ID, full release SHA, source revision and schema fingerprints. URLs come from no-follow mode-0600 files, never arguments or logs. Reject URL query routing and identical endpoints/database OIDs.

1. Open source with the existing read-only backup role. Start one `REPEATABLE READ READ ONLY` transaction. Set its session zone to UTC. Attest source revision and catalog fingerprint.
2. Open target as its migrator owner. Acquire an import advisory lock. Assert exact target revision, baseline fingerprints, no app sessions and no business rows. Baseline catalog/extension rows must equal the bootstrap manifest.
3. Start one target transaction. Lock import tables. Copy the explicit columns in bounded batches. Use text/binary codecs that retain Decimal, UUID, JSON and timestamptz; preserve PostGIS EWKB/SRID. Never use float or ORM constructors with timestamp defaults.
4. Cyclic FKs need a deliberate import phase. Capture exact FK definitions; temporarily replace only reviewed cyclic FKs with deferrable equivalents inside this transaction, defer them, then restore original definitions and validate before commit. Noncyclic FKs stay active.
5. Historical records may not satisfy **new INSERT** guards. Example: the trade trigger requires version 1 and current party provenance on INSERT (`origin/staging:alembic/versions/blng_20261014_staging.py:136–191`). Attest the complete named user-trigger set. Disable only these exact user triggers in the target import transaction as the table owner. Do not use `session_replication_role`, disable system FK triggers, or grant superuser. Check all target constraints and dedicated historical invariants. Re-enable and compare every trigger/function byte fingerprint before commit. No running app can access the import target.
6. Historical invariants preserve source-valid version-0/NULL snapshots; they do not require current-party equality for old trades. New version-1 rows require valid frozen identity and allowed provenance pairs. Prove that source and target snapshots match. Exercise the restored new-INSERT guard with a savepoint and an invalid synthetic row; it must reject.
7. Copy notice receipt mappings. Restore sequence state to at least both source `last_value` and target `max(column)`, retaining `is_called` semantics. Cover `orderbook_acceptance_ordinal_seq` and `market_event_stream_seq`, plus every discovered owned sequence. A rolled-back sequence change makes the target non-pristine; a retry must refuse it, not silently reuse it.
8. Verify counts, sorted-primary-key streaming hashes and column-value hashes. Check every FK, decimal total, order reservation, trade lifecycle, snapshot, grace deadline, notice claim, receipt, event cursor and reference identity. Run no external side effect.
9. Commit only after all checks pass. Write a private copy receipt with aggregate counts, source snapshot identity, manifest digest and target OID. If receipt publication fails after commit, the next attempt refuses the populated target. Recover by verification, not replay.
10. Converge/validate ACLs from the approved canonical bundle on the **new** database only. Test raw runtime denial of grace/evidence/receipt UPDATE, provenance UPDATE, vault DELETE and control-table writes. The exact current policy is `origin/staging:deploy/postgres/app_acl_policy.sql:22–88,101–217,225–229`.

Do not silently filter a row that fails a stronger constraint. Emit only table, count and private diagnostic reference. Store row IDs/details in a mode-0600 host report. An unresolved blocker prevents cutover.

## Frontend convergence

**Recommendation:** use frontend `staging` as the common source. Port production product contracts and production release/public-site behavior onto it. Do not use a wholesale merge or replace staging's FAME/LNG code. Production has 261 unique commits and staging has 235 according to the supplied fetch result; these are not feature counts.

### Backend-call divergence inventory

The call-site inventory covers the API wrapper, direct fetch calls, auth context, AI client and SSE hook. It distinguishes actual route gaps from dead client methods. Task 8 must turn this static inventory into an executable request manifest and fail on an unclassified call.

| Direction / calls | Difference and disposition | Evidence |
|---|---|---|
| Prod → staging: `POST /activity/events`; `GET /admin/users/{id}/activity` | Routes absent in staging. Remove capture, delivery reporting and drawer. Old tabs may receive 404; no retry loop or financial effect. | `FE origin/prod:src/services/api.ts:652–678,1238–1252`; `origin/prod:app/routers/user_activity.py:71–115` |
| Prod → staging: catalog products; orderbook list/bids/asks/snapshot/with-ci/aggregated/product-counts/map-summary; listings; availability; demand; prices/reference; trade-tape; curves forward/board/table/slice | B30/B100 missing from staging policy; UCOME added. Port IDs, filters, labels and eligibility. CI pricing intentionally differs; update UI to unpriced/nullable monetary values. | `FE origin/prod:src/services/api.ts:902–1123,1194–1228,1367–1410,1492–1503`; `origin/prod:app/market_catalog.py:97–118`; `origin/staging:ARCHITECTURE.md:250–267` |
| Prod → staging: order POST/PUT, trade POST and lifecycle reads, inventory add/update/publish, support authorization/final confirmation | Prod B30/B100 scalar contract must remain. LNG property name stays `lng_terms`, but scalar/certification derivation and strictness differ. Staging adds FAME paired snapshots and review mapping. | `FE origin/prod:src/services/api.ts:791–871,1012–1060,1128–1190,1339–1363`; `origin/prod:app/services/lng_terms.py:92–115`; `origin/staging:app/services/lng_order.py:57–70`; `FE origin/staging:src/services/orderTerms.ts:1–11` |
| Both: `GET/PUT /preferences`, watchlist slice targets | Staging preferences hardcode four alcohol products. Prod can persist B30/B100/LNG. Restore canonical enum; include UCOME. Keep saved filters after copy. | `FE origin/prod:src/services/api.ts:681–694,1514–1520`; `origin/staging:app/schemas/preferences.py:1–14`; `origin/prod:app/schemas/preferences.py:1–25` |
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
- Vercel production release workflow and all referenced tooling. The workflow checks out `prod`, and the script requires `origin/prod`; both must change together to pinned canonical `staging`. Preserve candidate-only checks, canary CORS, build-once promotion and previous deployment ID. Evidence: `FE origin/prod:.github/workflows/release-vercel.yml:24–81`; `FE origin/prod:scripts/release-vercel.sh:300–315,354–403,426–450`.
- Do **not** port `ActivityTrackingProvider`, `UserActivityDrawer`, activity queue/loss reports or their consent hooks. Retain unrelated analytics/cookie privacy behavior.

## Task order, lanes and estimates

Estimates are focused agent hours plus human/independent review hours, not elapsed calendar promises. They exclude waiting for founder approval. Optimize for one short rehearsal and one cutover, not a seven-day soak.

| Task | Lane / owner | Dependencies | Agent + review |
|---|---|---|---|
| 1. Freeze manifest and disposable source fixtures | Astra | none | 4 + 2 h |
| 2. Union catalog and production market rules | Astra | 1 | 6 + 2 h |
| 3. Auth/LNG/reminders/privacy integration | Astra | 1; integrate after 2 | 6 + 2 h |
| 4. Canonical migration and empty bootstrap | Astra | 2, 3 | 6 + 3 h |
| 5. Copy and verification tool | Astra | 1, 4 | 12 + 4 h |
| 6. UTC policy and daily figures | DeepSeek | 1 | 2 + 1 h |
| 7. Deployment identity and reversible switch | Astra | 3, 4, 6 | 10 + 4 h |
| 8. Frontend convergence and release contract | Astra | 1; contract gate after 2, 3 | 8 + 3 h |
| 9. Integrated acceptance and Rust handoff | Astra | 2–8 | 5 + 3 h |
| 10. Host rehearsal, operator-run | Astra | 9 and explicit approval | 3 + 2 h |
| 11. Cutover and pre-write rollback gate | Astra | 10 and explicit approval | 2 + 2 h |

**Total: 64 agent hours + 28 review/operator hours = 92 focused hours.** Budget 3–5 working days with parallel lanes, subject to import blockers. No measured migration duration exists yet. Tasks 10/11 estimates exclude data-volume-dependent copy time.

Parallel work: Task 8 in a frontend worktree; Task 6 in a narrow backend worktree; Task 2 and Task 3 in separate backend worktrees with explicit file ownership. Task 2 owns catalog/qualification/market routers. Task 3 owns auth/KYC/LNG schemas/reminder/notification files; coordinate any shared `execution_policy.py` edit through the integration owner. Tasks 4 and 7 serialize deploy/migration policy edits. Task 5 can build codecs/fixtures before Task 4, but cannot pass until the target schema is fixed. No parallel live operations.

### Task 1: Freeze source and build synthetic source fixtures — Astra

**Files:** Create `scripts/phase0/manifest.py`, `deploy/phase0/copy-manifest.json`, `tests/phase0/conftest.py`, `tests/phase0/test_inventory.py`, `scripts/run_phase0_postgres_tests.sh`. Read pinned `alembic/`, models and the analysis. Modify no deployed history.

**Interfaces:** `inspect_catalog(connection) -> CatalogManifest`; manifest records full source SHA, revision, tables/columns/types, FK/trigger/check/index/sequence definitions and reference rows. Test runner creates an isolated PostgreSQL 17/PostGIS 3.6 container with a unique loopback port. It accepts no deployed URL.

- [ ] **Failing test:** assert the manifest accounts for the 61 names above; detect an injected ordinary table/column and refuse. Fixture must include every copy table, all trade states, pending membership, rejected/moved KYC, expired grace, old notice audit, private notification fields, old snapshots, cycles and sparse sequence values.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_inventory.py`. Initially FAIL because runner/manifest are absent. A skipped suite is failure.
- [ ] **Implement:** materialize the pinned production tree using `git archive c08266bd6969143812dec7c5033338428cd5f159` under `work/phase0-fixtures/`. Build its schema using its real migrations and its own import path/venv. Populate pre-guard historical rows at the legal predecessor, then upgrade to `kycg_20261014_prod`. Never create the source schema from staging ORM metadata. Build the target independently from staging-derived migrations. Inspect catalogs, not only model declarations.

```python
PROD_SHA = "c08266bd6969143812dec7c5033338428cd5f159"
PROD_REVISION = "kycg_20261014_prod"


def require_complete_inventory(actual: set[str], declared: set[str]) -> None:
    if actual != declared:
        raise ValueError("Unclassified source schema; import refused")
```

- [ ] **Run:** runner again; require PostgreSQL version/extension assertions, nonzero tests, exact production source revision, no undeclared table/column and no live-network access. Include renamed legacy tables in comparison.
- [ ] **Commit:** `test: add Phase 0 source schema and copy inventory fixtures`.

**Resume/check:** Record fixture SHA, schema hashes, table counts and test totals in `outputs/phase0/task-01.md`. If the fresh historical chain cannot build, fix the disposable bootstrap explicitly; do not stamp over it. Task 4 must consume that proof.

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

**Resume/check:** Record product/delivery policy digest and all API fixtures. Task 4 uses frozen SQL literals from this reviewed union. Task 8 uses these IDs, not display labels.

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

### Task 4: Add the union revision and empty-database bootstrap — Astra

**Files:** Create `alembic/versions/p0_20261016_union.py`, `scripts/phase0/bootstrap.py`, `deploy/phase0/bootstrap-policy.json`, `tests/phase0/test_bootstrap.py`, `tests/postgres/test_phase0_union_migration.py`. Modify `deploy/migration-checkpoints.tsv`, `app/market_constraints.py` and exact drift fingerprints where needed. Do not alter earlier revision parents.

**Interfaces:** New revision `p0_20261016_union`, parent `kycd_20261015_staging`. Bootstrap CLI requires immutable source, manifest, explicit target env file and mode. The normal checkpoint helper remains unchanged for empty state.

- [ ] **Failing test:** upgrade a populated staging fixture to union; independently bootstrap a completely empty target. Prove schema/catalog equality, one revision and no users/grace enrolled by bootstrap. Refuse nonempty target, wrong SHA/OID, source DB, missing approval and aliases.
- [ ] **Run:** `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_bootstrap.py tests/postgres/test_phase0_union_migration.py`. Expect missing revision/helper.
- [ ] **Implement:** add B30/B100 reference identities; update the four product-domain checks and trade function with the union. Preserve FAME/LNG checks and immutable snapshots. Keep staging grace RESTRICT semantics. Freeze SQL in this migration; no imports from a mutable runtime catalog.

```python
revision = "p0_20261016_union"
down_revision = "kycd_20261015_staging"
branch_labels = None
depends_on = None
```

Append these literal policy rows:

```text
kycd_20261015_staging	p0_20261016_union
p0_20261016_union	p0_20261016_union
```

Bootstrap policy separately binds `EMPTY -> p0_20261016_union` to exact source/migration graph hashes and production target `verdaxis_phase0` or explicitly disposable target. The helper checks no application/control tables exist, attests target migrator ownership and calls `alembic.command.upgrade(config, "p0_20261016_union")` with the attested connection. It executes the historical chain **before data import**, so enrollment migrations see no customers. Never copy old `alembic_version`; never call `stamp`.

The bootstrap is an approved mutating operation, not a deploy dry-run. It must support `--check` that inspects manifests only without candidate execution or secrets. Disposable mode must have a separate positive loopback/container attestation; do not label a real target `ENVIRONMENT=test` to bypass identity checks.

- [ ] **Run:** fresh and staging-upgrade paths, `alembic check`, full check/function/FK/index/ACL comparison and bootstrap interruption tests. Prove no activity tables. Prove union schema permits production B30/B100 snapshots and rejects UCOME outside its lane.
- [ ] **Commit:** `feat: add canonical Phase 0 schema and guarded empty bootstrap`.

**Resume/check:** Record revision, graph digest, pristine-baseline manifest and target-only rows. A production data copy cannot start before this is green.

### Task 5: Implement host-local copy and verification — Astra

**Files:** Create `scripts/phase0/copy.py`, `scripts/phase0/verify.py`, `scripts/phase0/codecs.py`, `tests/phase0/test_copy.py`, `tests/phase0/test_copy_history.py`, `tests/phase0/test_copy_authority.py`. Modify only the Task 1 copy manifest and test runner as needed.

**Interfaces:** `copy_database(manifest, source, target) -> CopyReceipt`; `verify_database(...) -> VerificationReport`. CLI: `python -m scripts.phase0.copy --manifest <private-path> --source-environment-file <path> --target-environment-file <path> --source-sha <40hex> --target-sha <40hex> --apply`. Default is validation only. Secret files are opened once, no-follow; URLs never print.

- [ ] **Failing test:** implement both Review Focus import tests. Add failure injection after each table batch, constraint phase, sequence phase, commit and receipt publication. Assert old source is unchanged and a second copy refuses a populated/partially prepared target. Include unexpected columns and different DB OIDs behind the same URL text.
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

Production target is `verdaxis_phase0`; staging is `verdaxis_staging`. Do not modify old `verdaxis`. Inspect and reject unexplained higher-priority connection options. Recycle API pools and all CLI processes. Database defaults do not change already-open sessions.

Current daily SQL uses session-sensitive date conversion (`origin/staging:app/routers/price_discovery.py:440`; `origin/staging:app/routers/admin_analytics.py:406,421`). Explicit UTC product analytics already converts with `timezone('UTC', ...)` (`origin/staging:app/services/product_analytics.py:189–202`). **Effect:** staging's session-sensitive day boundary moves from 16:00 UTC to 00:00 UTC. An event at 16:30 UTC stays on its UTC date rather than the next Singapore date. Rolling durations, stored timestamptz instants and all-time totals do not change. Never shift stored timestamps by eight hours. Naive timestamps copy literally; do not guess a zone.

- [ ] **Run:** new tests against fresh app/migrator/backup connections, plus FAME date and analytics suites. Assert epoch equality before/after settings change.
- [ ] **Commit:** `ops: standardize canonical database sessions on UTC`.

**Resume/check:** Record before/after daily fixture values and `SHOW TimeZone` for all three roles. Task 10 repeats this on host scratch; Task 11 checks fresh live pools.

### Task 7: Build the exact deployment and rollback transaction — Astra

**Files:** Modify `app/config.py`, `scripts/converge_runtime_acls.py`, `scripts/deploy.sh`, `deploy/systemd/runtime-units.manifest`, both reminder service/timer pairs, `.github/workflows/backend-ci.yml`, `docs/runtime-hardening.md`, `README.md`, `ARCHITECTURE.md`. Port production `deploy/external_monitor/` source and `tests/monitor/` additions without activation. Create `scripts/phase0/cutover.py`, `deploy/phase0/cutover.schema.json`, `tests/phase0/test_cutover.py`, `tests/unit/test_phase0_runtime.py`.

**Interfaces:** Fixed production database becomes `verdaxis_phase0`; staging unchanged. Canonical deploy branch becomes `staging` for both fixed directories. One-off cutover coordinator owns maintenance, tuple preparation and rollback. It calls normal deploy only after exact target revision exists. It must not hold the deploy flock while calling a child that attempts the same flock.

- [ ] **Failing test:** `test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked`. Use real child processes/flocks and real file/dir renames. Kill at every boundary: source staging, env staging, tuple publish, unit install, restart authorization, local readiness, public readiness and state clear. Test wrong SHA, moved remote, dirty tree, symlink state, unknown DB, wrong role, pending timer and old source/new DB mismatch.
- [ ] **Run:** `python -m pytest tests/phase0/test_cutover.py tests/unit/test_phase0_runtime.py tests/unit/test_deploy_health.py -q`; initially FAIL.
- [ ] **Implement:** use these fixed policy edits, not ambient overrides:

```python
_DEPLOYED_DATABASE_IDENTITIES = {
    "production": {
        "database": "verdaxis_phase0",
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

Update the ACL helper's separately attested identity table and tests in the same commit. Audit all `verdaxis` literals: backup, monitor, preflight, migration, CLI and seed-denial paths. Do not loosen seed denial for `verdaxis_phase0`. Keep it explicitly forbidden. Production CORS includes the existing canary exact origin.

In `scripts/deploy.sh` change the production fixed `DEFAULT_BRANCH` to `staging`; retain environment, service, path, role, health URL and locks. Preserve normal dry-run behavior and all four approval values. Port reminder units through the manifest. Make reminder start guards reject both existing and dangling state paths. Preserve four workers and staging diagnostic entrypoint. Keep vault SetCredential/LoadCredential bytes from staging.

The coordinator must implement this finite state machine:

```text
PREPARED -> WRITERS_DRAINED -> DATA_VERIFIED -> TUPLE_INSTALLED
         -> UNITS_INSTALLED -> DEPLOY_VERIFIED -> ACCEPTED
any pre-ACCEPTED failure -> BLOCKED -> RESTORE_OLD_TUPLE -> OLD_VERIFIED
```

A root-owned mode-0600 journal outside the checkout records old/new DB OIDs, SHA, revision, env digest, unit digest, virtualenv/release directory, monitor identity digest and timer state. It holds a separate one-off cutover lock throughout. It establishes a persistent external maintenance/start fence before replacing a directory. Do not rely on a state file inside a directory that is about to move.

Implement the external start fence as a root-owned systemd drop-in plus a fixed root-owned guard outside either checkout. The guard refuses while the journal is incomplete. During an authorized restart it requires the approved environment/SHA tuple and contention of both normal deploy locks, using the same semantics as `verify_deploy_start_guard.py`. It must reject dangling paths, stale authorization and a dead coordinator. Install and test this fence before moving any directory. Keep timers stopped and recovery disabled throughout; the fence protects new starts, not existing workers.

Prepare the new clean checkout and venv outside the fixed path. A virtualenv contains absolute script paths: build it for the final canonical path in a private mount namespace, or recreate/verify its entrypoints after the rename while the fence is closed. Do not assume a moved venv works. Preserve the old checkout, venv, `.env`, runtime artifact, installed units and monitor identity as a complete private rollback bundle. Under stopped writers, atomically rename directories into the fixed path; no `git reset --hard`, checkout over dirty files or symlinked canonical root. Publish the new env/runtime/monitor tuple before any new code executes with credentials. Keep an absent normal deploy state only during the handoff to normal `deploy.sh`; the persistent external start fence prevents unattended starts. The coordinator is the sole authorized caller. A crash leaves the fence and journal in place.

After fixed-path installation: run new helper dry-run; require printed SHA; installer dry-run/apply at matching HEAD; run same-revision normal deploy with `p0_20261016_union` as expected and target. The normal helper owns its two locks and deploy state. Release the external start fence only through the reviewed handoff that permits this controlled restart, not an unguarded `systemctl start`. Keep public mutation maintenance until acceptance.

Rollback restores the **whole** old tuple under the same journal/fence, installs old attested unit bytes, uses old DB `verdaxis` and old SHA/revision, then performs a guarded restart with strict old readiness. This exceptional rollback is not the normal forward-only deploy helper. Test its guard ABI and old venv. Do not try to deploy the old SHA through the new canonical migration graph. Preserve failed new DB and journal for investigation.

- [ ] **Run:** cutover harness, migration checkpoint, runtime hardening, health, unit provenance, key-source preflight and all monitor suites. Prove no runtime grants changed on old DB. Prove external monitor cannot restart or seed either DB during maintenance. Verify backup command selects `verdaxis_phase0` after acceptance.
- [ ] **Commit:** `ops: add guarded canonical production cutover and tuple rollback`.

**Resume/check:** Publish the exact CLI help, approved journal schema and fault matrix in `outputs/phase0/task-07.md`. Host commands below are conditional on this implementation passing. A placeholder helper is not acceptable for execution.

### Task 8: Converge the frontend and release tooling — Astra

**Files:** In a separate frontend worktree, modify `src/services/api.ts`, `src/services/orderTerms.ts`, `src/types.ts`, `src/utils/marketProduct.ts`, `src/utils/marketProducts.ts`, `src/components/OrderPlaceModal.tsx`, `Marketplace.tsx`, `MyTrades.tsx`, `src/components/market-support/MarketSupportFinalConfirmation.tsx`, `src/components/notifications/NotificationList.tsx`, `src/App.tsx`, public pages/locales, `vite.config.ts`, `vercel.json`, `src/services/config.ts`, `.github/workflows/release-vercel.yml`, `scripts/release-vercel.sh` and its smoke/build dependencies. Port `src/utils/biofuelSpecification.ts`, public prerender/loaders/dataset notice. Add `src/tests/phase0-api-contract.test.ts`, `src/tests/phase0-cutover.test.tsx`, `scripts/test_phase0_release.py`.

**Interfaces:** One canonical frontend source; separate environment builds/API URLs. Production workflow requires an explicit 40-hex `source_sha` equal to approved canonical branch tip. Candidate and promotion bind the same backend release identity. Old and new frontend builds must be tested against the new backend before switching.

- [ ] **Failing test:** build a manifest of all static and direct API calls from both pinned trees; reject any unmapped method/path. Implement `test_old_and_new_browser_contracts_during_cutover`. Test B30/B100/UCOME distinction, LNG/FAME terms, old RFQ detail, PUT revision, no activity network calls, private notifications, null compliance values and re-login after refresh loss.
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

Workflow checkout uses the approved input SHA, fetches canonical staging, and verifies exact equality before build. Keep `candidate_only=true` default. Add `backend_release_sha` input and assert exact readiness before canary/promotion. Production release still uses `release-vercel.yml`; do not push a manual Vercel build. Keep current project/org, Mapbox gate, immutable candidate, before/after alias checks and dependency-aware rollback. Staging builds from the same SHA with its staging API URL; never reuse the production-configured JS bundle there.

- [ ] **Run:** full Vitest/typecheck/build suites, release script self-test, artifact checker and synthetic browser smoke against disposable canonical backend. Build both environments and assert exactly the intended API origin in artifacts. Test cached old bundle against target backend; classify expected activity 404 as retired, not an API outage.
- [ ] **Commit:** `feat: converge frontend contracts and canonical release source`.

**Resume/check:** Record frontend SHA, build hashes, API call manifest, candidate workflow tests and prior production release procedure. Do not release during this task.

### Task 9: Prove the integrated release and hand it to Rust — Astra

**Files:** Create `tests/phase0/test_acceptance.py`, `scripts/phase0/acceptance.py`, `outputs/phase0/acceptance-template.md`; update `ARCHITECTURE.md` and runtime/KYC runbooks. Coordinate, but do not edit the other task's Rust worktree without its owner's agreement.

**Interfaces:** `acceptance.py --manifest ...` returns nonzero for any missing table, skipped required suite, unclassified API difference, failed permission check or incomplete rollback proof. Reports contain aggregates, not secrets.

- [ ] **Failing test:** a report missing one table, one role, one API family or rollback evidence must fail. Require all five Review Focus tests.
- [ ] **Run:** `python -m pytest tests/phase0/test_acceptance.py -q`; expect missing acceptance checker.
- [ ] **Implement:** aggregate the exact suite results and identities. Do not infer success from a subprocess starting or an empty result list.

```python
REQUIRED_GATES = frozenset({
    "inventory", "schema", "copy", "acl", "products", "auth",
    "privacy", "timezone", "vault", "frontend", "rollback",
})


def require_gates(results: dict[str, str]) -> None:
    if set(results) != REQUIRED_GATES or any(v != "passed" for v in results.values()):
        raise ValueError("Phase 0 acceptance is incomplete")
```

- [ ] **Run:** all unit and monitor suites, complete disposable PostgreSQL runner, both frontend builds and disposable HTTP integration. Use explicit test JWT/issuer/audience and loopback ephemeral integration token. Preserve the repository's `--run-disposable-integration` guard (`origin/staging:README.md:257–295`). Record totals and failures, not just “green.”
- [ ] **Commit:** `test: gate Phase 0 convergence and Rust oracle handoff`.

**Rust handoff:** deliver one canonical schema at `p0_20261016_union`, one reviewed revision allowlist and one Python oracle SHA. Regenerate catalog policy, goldens and SQLx metadata from that SHA with UTC and pilot KYC off. Include all eight products, no prices for absent liquidity, historical snapshots, notification privacy and copied reference IDs. The Phase 1 plan's Task 2 still names `kycg_20261014_staging` and Singapore defaults (`feat/rust-phase-1-20261015:docs/plans/2026-10-15-rust-phase-1-plan.md:434,473`); its owner must replace those assumptions after compatibility proof. Do not grant Rust SELECT on `alembic_version`: keep the migrator-backed prestart and compiled allowlist (`same plan:47–48`). Phase 0 makes prod a valid target for a **separately approved** Rust Phase 1 production rollout; it does not itself authorize that rollout. Keep the founder go/no-go and measured simulation gate, not a seven-day shadow clock (`same plan:1978–1983`; rewrite design:97–101).

**Resume/check:** Acceptance artifact records backend/frontend SHAs, schema/policy/corpus hashes, UTC, gate-off, role proof and rollback result. Any later code change invalidates the corresponding evidence before host rehearsal.

## Operator command conventions

The following blocks are **not run by this planning task**. Commands marked `scripts.phase0.*` are proposed tools owned by Tasks 1, 4, 5, 7 and 9. Task 7 must implement and test their exact CLI before operator use. Do not substitute ad hoc SQL or skip a failed check.

The coordinator's manifest supplies host-specific maintenance/recovery commands after reading the host authority files. There is no safe universal command for the current Caddy and external-monitor ownership without inspecting those files. Missing reviewed maintenance commands are a hard stop, not permission to improvise. The coordinator validates that all commands are exact allowlisted argv arrays, not shell text from a data file.

Private host paths:

```text
/root/verdaxis-phase0/approved.json             root:root 0600, no passwords
/root/verdaxis-phase0/source.env                root:root 0600, read-only source URL
/root/verdaxis-phase0/target.env                root:root 0600, exact new app/migrator URLs
/root/verdaxis-phase0/rehearsal-source.env      scratch source only
/root/verdaxis-phase0/rehearsal-target.env      scratch target only
/var/lib/verdaxis/phase0/                      root:root 0700, journal/reports/dumps
```

The approved manifest includes old/new full backend SHAs, old/new revisions, frontend SHA and prior Vercel deployment ID, exact DB names/OIDs, backup role, host, extension versions, source catalog hash, target baseline hash, env file digests, unit manifest digests, maintenance commands and timer list. Secrets never appear in the manifest. Unknown current old SHA/revision must be read and approved; do not assume this plan's pinned source is still live.

### Task 10: Rehearse on the production host — Astra, operator-run

**Files:** Produce private `/var/lib/verdaxis/phase0/rehearsal-report.json` and encrypted dump/restore evidence. No source edits. Tool ownership is Tasks 4/5/7/9.

**Interfaces:** Rehearsal reads live prod through backup authority, restores to scratch source, imports to scratch target and runs a fenced scratch service on an ephemeral loopback port. It never changes live routing or sends external mail.

- [ ] **Failing test:** first run Task 7's Linux/systemd fixture failure matrix with an intentionally wrong target OID and missing key; expect refusal/503. Do not create a live fault to prove this.
- [ ] **Run local acceptance:** require Task 9's exact release evidence and founder approval for host inspection/rehearsal.
- [ ] **Implement the rehearsal checklist:** at the authorized EU administrator prompt:

```bash
set -euo pipefail
set +x
test "$(hostname)" = vmi3623757
sudo -n less /home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md
# Read the separately approved copy of shared-host VPS_MAP.md supplied by its owner.
# Do not assume an EU path for a shared-host authority document.
getent ahostsv4 api.verdaxis.exchange
getent ahostsv4 app.verdaxis.exchange
sudo -n systemctl show verdaxis-backend.service -p User -p WorkingDirectory -p ExecStart
sudo -n install -d -m 0700 -o root -g root /root/verdaxis-phase0 /var/lib/verdaxis/phase0
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 -Atc \
  "SELECT datname, oid FROM pg_database WHERE datname IN ('verdaxis','verdaxis_phase0');"
```

Check API DNS points to the approved EU topology; frontend may resolve to Vercel. Verify service user `verdaxis-prod` and working directory `/home/verdaxis-prod/verdaxis/prod/be`. Read both authority documents before continuing. If the old source/revision differs from the manifest, stop and repeat inventory/review.

Use the implemented coordinator to create a fresh private dump with the existing backup role and a consistent snapshot, restore to `verdaxis_phase0_source_test`, and bootstrap `verdaxis_phase0_target_test`. It invokes `pg_dump --format=custom --no-owner --no-acl` using a private libpq service/passfile, not a password in argv. It verifies exit status, archive readability and a complete restore. It records `pg_dump --version`, server/PostGIS versions, dump digest and source revision. Do not assume a nonempty dump file is valid.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  rehearse --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/source.env \
  --scratch-source verdaxis_phase0_source_test \
  --scratch-target verdaxis_phase0_target_test
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/rehearsal-source.env \
  --target-environment-file /root/verdaxis-phase0/rehearsal-target.env \
  --report /var/lib/verdaxis/phase0/rehearsal-report.json
```

`/opt/verdaxis-phase0` is a root-controlled, exact-SHA tool installation prepared from the approved release; it is not the mutable live checkout. The rehearsal command must reproduce normal deploy/unit/readiness behavior in a private namespace with scratch services and scratch locks. Scratch app/migrator roles are separate non-superusers; no global edits to live roles. Block Resend, Gemini, Webz, Telegram, Umami and public routing at the scratch service boundary. Keep real data on host. Synthetic KYC round trips may run in the scratch copy only, with no customer document download.

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

### Task 11: Cut over, verify, and test rollback before writes — Astra, operator-run

**Files:** Private journal, final dump, final copy report, old tuple backup and acceptance record. No application edits during the window.

**Interfaces:** Explicit founder approval for production cutover, staging UTC/deploy and frontend promotion are separate. The canonical helper uses all four approval values. The coordinator implements only the reviewed transition from the attested old tuple to the attested new tuple.

- [ ] **Failing test / preflight:** run the coordinator `check` with a copied manifest containing the wrong expected current revision. It must refuse without mutation. Then run the approved check. This is a manifest/identity check, not permission to execute candidate code in a deploy dry-run.
- [ ] **Run:** verify the exact release is still canonical remote tip; require clean prepared checkouts, Task 10 report, disk capacity for old DB + new DB + dump + scratch, and the encrypted key backup acknowledgement.
- [ ] **Implement the operator checklist in order:**

**A. Stage first.** Deploy canonical backend to staging through its normal literal `kycd_20261015_staging -> p0_20261016_union` row, or the actually attested allowlisted current revision. Stop/drain its recorded timers before installation. On the separately authorized staging host, set UTC with the committed Task 6 SQL:

```bash
test "$(hostname -s)" = vmi1840561
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 \
  -v database_name=verdaxis_staging -v app_role=verdaxis_app_staging \
  -v migrator_role=verdaxis_migrator_staging -v backup_role=verdaxis_backup_staging \
  -f /home/verdaxis-prod/verdaxis/staging/be/deploy/phase0/set_timezone.sql
```

The new production database uses the same command on the EU host with `database_name=verdaxis_phase0`, `app_role=verdaxis_app`, `migrator_role=verdaxis_migrator`, `backup_role=verdaxis_backup` and the approved release's SQL path. Never run it against old `verdaxis`. The bootstrap/coordinator runs that production operation before import. Reconnect and verify. Keep KYC off and no new reminder activation. Release the common frontend SHA to staging with staging API configuration. Run the short approved acceptance check, not mutating production tests.

**B. Prepare production frontend, but do not promote.** The updated workflow takes the exact frontend and backend SHAs. Its normal production API canary must run only after the canonical backend is available; pre-cutover testing uses the disposable backend. Do not mistake candidate-only deployment for a production alias change.

**C. Record and stop all writers.** The coordinator enumerates timers from installed units and host authority, records active/enabled/masked state, then stops the relevant timers. Include news, auth maintenance, analytics prune, reminders, onboarding attention, demo jobs, signup canary and auto-recovery. Preserve backup scheduling under a reviewed nonconflicting window. Do not merely stop the four known application timers and ignore external writers. Let running oneshots finish naturally. Require no pending job. Set maintenance at the proxy for all API mutations and close/stop existing API workers/SSE before the final dump. Disable SQLAdmin mutations and all direct writer jobs.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  quiesce --manifest /root/verdaxis-phase0/approved.json
sudo -n systemctl list-jobs --no-pager
sudo -n systemctl show verdaxis-backend.service -p ActiveState -p SubState
sudo -n -u postgres psql -X -d postgres -v ON_ERROR_STOP=1 -Atc \
  "SELECT datname, usename, state, count(*) FROM pg_stat_activity
   WHERE datname IN ('verdaxis','verdaxis_phase0')
   GROUP BY datname, usename, state ORDER BY 1,2,3;"
```

Expected: backend stopped, no relevant pending/running jobs, no writer connection on either database. The approved coordinator verifies exact writer-role/session absence, not only `state='active'`. Do not kill unknown sessions or change old DB ACLs as a shortcut. Find their owner and stop them through the approved service boundary.

**D. Final dump, new database, copy.** Keep the start fence and maintenance active. Create the new DB with exact owner, PostGIS extension and UTC; do not reuse rehearsal target. Bootstrap to union before copying users. Create all roles through reviewed bootstrap policy without changing live old-role passwords/memberships/global settings. Apply database-scoped grants only on the new DB.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  final-copy --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/source.env \
  --target-environment-file /root/verdaxis-phase0/target.env
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.verify \
  --manifest /root/verdaxis-phase0/approved.json \
  --source-environment-file /root/verdaxis-phase0/source.env \
  --target-environment-file /root/verdaxis-phase0/target.env \
  --report /var/lib/verdaxis/phase0/final-copy-report.json
```

`final-copy` makes and restores/verifies the final backup, executes the approved empty bootstrap, then the copy. It refuses an existing non-pristine target. It records counts and all invariants. No customer data leaves the host. Do not rerun seeds after copy.

**E. Switch the tuple and deploy.** Update **both** `DATABASE_URL` and `MIGRATOR_DATABASE_URL` in the staged environment to `verdaxis_phase0`, keeping exact roles and endpoint. Update component settings consistently; remove conflicting routes. Preserve all other production secrets, issuer/audience, URLs, provider settings and gate-off. Preserve old env privately. Do not print env diffs. Skip refresh sessions; keep password/token-cutoff fields unchanged. Existing access tokens may remain valid until expiry; the mutation fence prevents use during acceptance. Do not rotate the JWT secret merely for the copy, since that can affect outstanding signed links.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  install-tuple --manifest /root/verdaxis-phase0/approved.json
# The coordinator now owns the external maintenance/start fence.
# Run at the fixed, clean canonical production checkout:
sudo -n -u verdaxis-prod -H \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh --dry-run
```

Require printed `APPROVED_RELEASE_SHA` to equal the approved canonical SHA. The dry-run does not prove DB state or execute the candidate. Repeat this now-live helper dry-run even if an old helper checked the release before tuple installation.

```bash
read -r -p 'Approved canonical backend full SHA: ' RELEASE_SHA
[[ "$RELEASE_SHA" =~ ^[0-9a-f]{40}$ ]]
sudo -n /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --dry-run --environment production --source-ref "$RELEASE_SHA"
sudo -n /home/verdaxis-prod/verdaxis/prod/be/scripts/install_systemd_units.sh \
  --apply --environment production --source-ref "$RELEASE_SHA"
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  deploy --manifest /root/verdaxis-phase0/approved.json \
  --approved-release-sha "$RELEASE_SHA" \
  --migration-approved-source-sha "$RELEASE_SHA" \
  --migration-expected-current-revision p0_20261016_union \
  --migration-target-revision p0_20261016_union
```

The last command performs the reviewed fence handoff and invokes normal deploy with:

```bash
sudo -n -u verdaxis-prod -H env \
  APPROVED_RELEASE_SHA="$RELEASE_SHA" \
  MIGRATION_APPROVED_SOURCE_SHA="$RELEASE_SHA" \
  MIGRATION_EXPECTED_CURRENT_REVISION=p0_20261016_union \
  MIGRATION_TARGET_REVISION=p0_20261016_union \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/deploy.sh
```

Do not run this inner command independently of the coordinator's fence handoff. Normal deploy verifies the new DB's existing union revision, uses the union→union allowlist row, converges target ACLs, publishes exact runtime metadata, holds both locks for restart and clears state only after readiness. No old-production revision appears in the canonical graph or allowlist.

**F. Verify while public writes remain blocked.**

```bash
sudo -n systemctl is-active --quiet verdaxis-backend.service
curl --fail --silent --show-error https://api.verdaxis.exchange/health/ready |
  /home/verdaxis-prod/verdaxis/prod/be/venv/bin/python \
  /home/verdaxis-prod/verdaxis/prod/be/scripts/validate_health_response.py \
  --environment production --release-sha "$RELEASE_SHA"
sudo -n test ! -e /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
sudo -n test ! -L /home/verdaxis-prod/verdaxis/prod/be/.runtime-deploy/production.state
sudo -n journalctl -u verdaxis-backend.service --since '15 minutes ago' --no-pager \
  --grep 'KYC execution gate:|KYC document storage unavailable'
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  verify-live --manifest /root/verdaxis-phase0/approved.json
```

Require exact four-key readiness, local and public; full new SHA; `db=ok`; environment production; target DB/revision/role; UTC on fresh connections; four worker gate-disabled log lines for the current start. No vault-unavailable line is allowed for acceptance. Check vault round trip in scratch using the installed credential contract; use a separately approved synthetic production canary only if required, and retain its audited encrypted records. Never use a customer's ID as a smoke test. Readiness alone is not a vault check.

Verify B30/B100/UCOME IDs and lanes, copied customer counts, read-only account/trade/order views, private notification redaction, receipts and old frontend compatibility. No real trade mutation test. Keep the command receipt and outbox verification in the disposable suite.

**G. Exercise the pre-write rollback once, then reapply.** Founder approval covers this planned rollback drill. Under maintenance, invoke:

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  rollback --manifest /root/verdaxis-phase0/approved.json --require-no-public-writes
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  verify-old --manifest /root/verdaxis-phase0/approved.json
```

The rollback command stops new workers, restores the old DB URL pair and old complete source/venv/unit/runtime/monitor tuple, then uses the tested two-lock restart guard and exact old readiness SHA/revision. The old DB remains unchanged by import. No Alembic downgrade, metadata-only reset, `git reset --hard` or deletion of new evidence. Restore the old frontend alias only if it changed. Timers stay stopped. If any public mutation occurred, this command refuses.

Reapply the already verified new tuple through `install-tuple`, repeat helper dry-run, installer dry-run/apply and the same-revision deploy. Do not copy again into the populated new DB. Re-verify source/target preservation and no unexpected writes. If canary auth/audit writes occurred, record them explicitly and prove no customer mutation before declaring the drill lossless.

**H. Release frontend, then open writes.** Run the updated protected workflow against the approved common frontend SHA and new backend SHA. First candidate-only, then promotion; do not assume separate runs produce the same candidate unless the implemented workflow supports reuse. Each promotion run must build once, test its own candidate and promote those same bytes.

```bash
gh workflow run release-vercel.yml --repo Verdaxis/verdaxis-frontend --ref staging \
  -f source_sha="$FRONTEND_SHA" -f backend_release_sha="$RELEASE_SHA" \
  -f candidate_only=true -f notify_release=false
# Review the completed candidate report and obtain promotion approval.
gh workflow run release-vercel.yml --repo Verdaxis/verdaxis-frontend --ref staging \
  -f source_sha="$FRONTEND_SHA" -f backend_release_sha="$RELEASE_SHA" \
  -f candidate_only=false -f notify_release=true
```

`FRONTEND_SHA` must be read from the approved manifest, not guessed. The workflow needs its Task 8 inputs and branch checks before these commands are valid. Confirm actual rendered bundle/API URL, all product IDs, KYC errors and no activity requests. Existing Vercel rollback retains the prior deployment ID; backend rollback remains a separate complete tuple operation.

Open public mutations only after founder acceptance. Record this instant as the end of the lossless old-DB rollback window. Restore **only** timers recorded active before the window, and only after deploy state is absent. Do not enable previously inactive or masked units. Require no duplicate scheduler and correct new DB identity for each restored job. Keep grace sender disabled. Update the approved backup target to the new database; keep old DB and final dump under restricted retention.

```bash
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  accept --manifest /root/verdaxis-phase0/approved.json
sudo -n /opt/verdaxis-phase0/bin/python -m scripts.phase0.cutover \
  restore-recorded-timers --manifest /root/verdaxis-phase0/approved.json
```

- [ ] **Run final checks:** both environments use the same canonical source policy and union schema; fresh connections show UTC; gate remains off; vault backup/recovery is proved; correct release identity and backup target; no old DB writer; no active activity tracker; prior timer state restored exactly. Record any environment-specific activation difference.
- [ ] **Commit/checkpoint:** later authorized implementation session commits only redacted acceptance evidence and updated architecture/runbooks. Never commit dumps, keys, env files, host journals or customer identifiers.

**Resume/check:** If stopped before acceptance, leave maintenance/fence in place and resume from the root-owned journal's last verified state. Do not clear state by hand. After acceptance, failures use a forward fix or separately approved reconciliation, not automatic old-DB rollback.

## Final acceptance and durable resume point

Implementation starts at **Task 1**. This planning commit runs none of the tasks.

For each completed task, record: commit SHA, exact commands, pass/fail totals, artifact paths/digests, unresolved blockers and the next task. Do not mark a task complete from source inspection alone. A resume session reads this plan and the latest `outputs/phase0/task-*.md`, then verifies the working tree and approved SHAs before continuing.

Phase 0 is complete only when:

- [ ] One canonical backend branch and frontend branch serve both environments with explicit environment activation/configuration.
- [ ] All 61 source application/control tables are accounted for; copy/reference/skip counts and invariants are verified on the host.
- [ ] B30, B100 and UCOME_B100 remain distinct; original production IDs and economic history survive.
- [ ] Old production database and old complete release tuple remain recoverable; pre-write rollback was exercised.
- [ ] UTC is effective on fresh connections; no stored timestamps were shifted.
- [ ] Production vault works; encrypted offline key backup and recovery are verified; pilot KYC gate stays off.
- [ ] Frontend production release passes its protected Vercel candidate/promotion checks; no activity capture remains.
- [ ] Rust receives the canonical revision/policy/oracle artifacts and its owner updates the Phase 1 assumptions before targeting prod.
