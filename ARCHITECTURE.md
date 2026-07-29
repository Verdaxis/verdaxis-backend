# Architecture

> FastAPI + SQLAlchemy 2 (async) + PostgreSQL 17.9/PostGIS 3.6.2 + Alembic + Pydantic v2
> PyJWT (HS256) + bcrypt + slowapi rate limiting + structlog JSON logging

## File Map

```
app/
  main.py                       # FastAPI app, exact credentialed CORS, sanitized readiness/provenance, error handlers
  config.py                     # Pydantic Settings — release/CORS boundaries and pool/upload budget validation
  database.py                   # AsyncSession factory, role/max_connections startup attestation, bounded pooling
  seeds/safety.py               # Opt-in, environment/target-attested seeder connection gate
  models/legacy.py              # Metadata-only legacy FK table stubs excluded from Alembic drift checks
  migration_drift.py            # Explicit PostGIS/legacy exclusions and narrow Alembic comparison callbacks
  admin.py                      # SQLAdmin panel at /admin
  rate_limit.py                 # slowapi Limiter singleton (key=remote_address)
  cli/
    refresh_news.py             # External singleton news-refresh entrypoint (never a web-worker scheduler)
  core/
    security.py                 # PyJWT + bcrypt — access/refresh/stream token creation, decode_token
  models/
    __init__.py                 # Imports all models for Alembic autogenerate
    user.py                     # User (password_changed_at, must_change_password), Organization, enums
    port.py                     # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py              # InventoryItem, FuelType enum
    orderbook.py                # OrderBookOrder (BID/ASK), Trade, canonical availability_window strings, enums (OrderSide, TradeStatus)
    market_support.py           # Durable staff capabilities, opaque contexts, and exact one-use assisted-listing authorizations
    live_slice_benchmark.py     # Persisted same-side slice VWAP aggregates for fast marketplace benchmark reads
    orders.py                   # Commission (legacy match_id + trade_id FKs)
    matchmaking.py              # MatchSuggestion
    watchlist.py                 # Watchlist, typed targets, event feed for Market Radar
    notification.py             # Notification, NotificationType (11 types incl trade events)
    user_preference.py          # Per-user JSON preferences by namespace (market_watch, notifications, tutorial)
    compliance.py               # TraceabilityEvent, ComplianceLedger
    producer.py                 # ProducerProject (PostGIS, GENA import)
    audit.py                    # AuditLog (JSONB changes, indexed action/resource/timestamp)
  routers/
    auth_simple.py              # JWT auth — login/register, cookie-backed refresh rotation, password change, /me, RBAC
    orderbook.py                # Order/listing CRUD, supplier ASK template endpoint, certification guardrails
    market_support.py           # ADMIN capability, authorization, assisted ASK publication, and ETag cancellation APIs
    trades.py                   # Trade lifecycle — create/confirm/decline/deliver/pay + SSE events
    matchmaking.py              # Match suggestions — generate, list, dismiss
    price_discovery.py          # Public price ticker + daily VWAP reference prices
    trade_tape.py               # Public anonymized 7-day confirmed trade tape with exact delivery-point filtering
    curves.py                   # Forward curve data products plus legacy board/table/slice endpoints
    stream.py                   # SSE endpoints — /stream/prices, /stream/orderbook, /stream/trades (durable id/Last-Event-ID replay)
    compliance_api.py           # Compliance scoring — fleet scores, vessel scores, what-if scenarios
    admin_analytics.py          # Platform and product-usage aggregates (ADMIN only; Umami degrades independently)
    availability.py             # Fuel availability by port
    demand.py                   # Anonymized BID demand signals
    producers.py                # Producer project list (map data)
    notifications.py            # User notification CRUD
    preferences.py              # Authenticated server-persisted user preferences
    inventory.py                # Supplier inventory + publish-to-ASK
    ports.py                    # Port data with PostGIS
    vessels.py                  # Vessel data (org-scoped)
    compliance.py               # Unmounted legacy module; ledger/verify routes are absent
    ai.py                       # Gemini AI chat proxy
    orders.py                   # Admin commission management
    audit.py                    # Admin audit log query
    dashboard.py                # Unmounted legacy module; dashboard health route is absent
  schemas/
    user.py                     # UserCreate (min 8 chars pw), UserResponse, PasswordChangeRequest
    preferences.py              # Strict namespace schemas for user preferences
    organization.py             # OrganizationCreate/Response
    orderbook.py                # Order/Trade schemas, price summaries, supplier metadata pack, ASK template response, canonical availability window validation
    market_support.py           # Exact authorization, capability, organization-context, and assisted-listing schemas
    market_activity.py          # Shared source/scope/demo-status provenance enums for market data
    behavioral_analytics.py     # Typed privacy-bounded admin product-usage response
    [others unchanged]
  services/
    event_bus.py                # AsyncIO pub/sub — per-channel queues, 200 subscriber cap, backpressure
    market_event_dispatch.py    # Durable shared SSE transport — outbox sequencer (advisory-lock leader), LISTEN/NOTIFY wake + poll, org-bound hub fan-out (docs/market-event-dispatch.md)
    matching_engine.py          # Match-on-insert — price-time priority within canonical market identity, partial fills, auto-confirm
    market_support.py           # Authorization digest, ETag parsing, deterministic party locks
    request_party.py            # Immutable actor/effective-party resolution and support mutation allowlist
    market_support_post_only.py # Fail-closed crossing assessment for assisted ASK publication
    benchmarks.py               # External/manual benchmark lookup keyed by market_product + delivery_point + availability_window
    forward_curve_market_slices.py # Canonical public Forward Curve table/slice read models and label policy
    market_signal_ingestion.py    # Trusted signal importer: validate -> verified ingestion runs whose rows classify REAL (see docs/market-signal-ingestion.md)
    live_benchmarks.py          # Persisted live same-side slice VWAP rebuilds + read-through fallback
    availability_windows.py     # Canonical availability code parsing, sorting, display labels, legacy alias normalization
    compliance_scoring.py       # Pure function scoring — FuelEU/ETS/CII, 9 fuels, scenario engine
    audit_service.py            # record_audit() — non-committing async audit logging (call inside the caller's transaction, before its commit)
    audit_actions.py            # THE source of truth for covered audit actions (registry of constants; meta-test forbids string literals at call sites)
    ai_service.py               # Gemini chat + document analysis (stub)
    matchmaking.py              # Score-based BID/ASK matching (0-100)
    watchlists.py               # Market Radar helpers: default container, typed targets, slice summaries
    watchlist_events.py         # Slice/pin event emission from order lifecycle changes
    behavioral_analytics.py     # Optional async Umami client, token/aggregate caches, post-commit conversion events
    ci_pricing.py               # Carbon intensity adjusted pricing
  middleware/
    rbac.py                     # require_role() factory — FastAPI dependency for role-based access
    market_support_scope.py     # Fail-closed method/path gate for context-bearing mutations

tests/unit/                     # Unit coverage (auth, matching, runtime, compliance, events, pricing, schemas)
tests/integration/              # Mutating API tests; skipped unless an attested disposable target is explicit
tests/disposable_target.py      # Numeric-loopback ephemeral-port and exact identity-handshake guard
tests/disposable_server.py      # Test-only ASGI wrapper producing the disposable identity endpoint
tests/monitor/                  # Monitor artifact contract, ownership, and no-activation proof suites
tests/runtime_config.py          # Explicit/validated API target policy (staging tooling; suites gate via disposable_target)
deploy/systemd/                  # Backend plus singleton news/prune service-timer pairs per environment
deploy/postgres/                 # Declarative app ACL policy, owner-only convergence, role bootstrap, and validation SQL
deploy/migration-checkpoints.tsv # Exact expected-current/allowed-target cutover pairs
deploy/monitor/
  README.md                     # Source-only contract, ownership boundaries, integration seams
  runtime-v2-readiness-corpus.json # Shared exact four-key readiness examples
  local_health_check.py         # Fixed loopback runtime attestation + filesystem status; no secrets
  status_state.py               # Bounded hostile JSON validation, quarantine, fail-closed status
  backup_verify.py              # Read-only artifacts/status/attempt-journal verification
  alert_dispatch.py             # Per-destination Telegram/Healthchecks receipts + hourly dedupe
  legacy_retirement.py          # Config-derived 30-hour/signoff gate; validation-only by default
  verify_runtime_identity.py    # Attested-commit snapshot launcher for demo jobs
  artifact-manifest.json        # Static exact monitor-owned artifact inventory; no promotion logic
  verdaxis-monitor.{sysusers,tmpfiles} # Declarative identities and directory modes
  verdaxis-*.service            # Reader, alert, and isolated per-environment demo artifacts (never activated here)
  verdaxis-*.timer              # Independent health, backup, alert, and demo schedules (never armed here)
scripts/verify_migrations.sh    # Upgrade-to-head plus Alembic model/schema drift check
scripts/preflight_runtime.py    # Read-only exact deployed config/database identity gate
scripts/apply_migration_checkpoint.py # Source-attested literal deployment migration
scripts/converge_runtime_acls.py # No-follow env load + migrator-only post-migration ACL convergence
scripts/verify_migration_revision.py # Exact deployed-checkpoint startup gate
scripts/validate_health_response.py # Strict readiness JSON environment/SHA validator
scripts/install_systemd_units.sh # Dry-run-default install from pinned immutable unit staging
scripts/verify_systemd_source.py # Exact clean release-ref/commit archive + digest gate
alembic/versions/               # Migrations incl. canonical availability-window rewrite + runtime metadata alignment
```

## Key Patterns

- **Match-on-insert:** `POST /orderbook` → `db.flush()` → `match_order()` → `db.commit()` (atomic); executable matches now require exact `product + delivery_point + availability_window`
- **Supplier ASK invariants:** ASK creation/update requires explicit `certification_declared=true` plus a non-empty `certification_scheme`; `GET /orderbook/my/latest-ask-template` returns safe defaults for the next listing and resets off-spec state
- **Assisted order entry:** When explicitly enabled, separately capability-gated administrators may publish exact post-only BID and ASK orders for an approved real organization. The organization is the economic party and the administrator is the accountable actor; no customer user is selected as a proxy. Orders may be GTC or dated, customer instructions have no artificial age cutoff, and confirmation requires a reference rather than evidence text. Customer/admin cancellation requires the current ETag, support-created orders cannot be edited, and public serializers omit support attribution. See `docs/market-support-assisted-listings.md`.
- **Assisted organization context:** The backend binds an approved real organization to the authenticated administrator through one opaque, short-lived database context. The normal `/orderbook/my`, order creation, and canonical cancellation routes resolve an immutable request party from `X-Verdaxis-Market-Support-Context`; an early deny-by-default middleware rejects every unclassified context-bearing mutation.
- **Assisted-context hardening:** Context mutations take deterministic capability/context locks and capability revocation atomically ends active contexts with audit. Final confirmations participate in context-mode idempotency, while instruction reference, time, and acknowledgement facts are persisted. Canonical delivery-point and post-only rules remain enforced. Context-invalid responses carry stable detail codes and `X-Verdaxis-Market-Support-Context-Invalid`.
- **SSE broadcasting:** `event_bus.publish(channel, event_type, data)` → subscribers via AsyncIO queues; order/trade payloads are append-only enriched with market source/scope/demo provenance. Private market lifecycle events additionally flow through the durable `market_event_outbox` → sequencer → hub pipeline, so delivery and `Last-Event-ID` replay survive worker restarts and cross Uvicorn workers (docs/market-event-dispatch.md; prune policy documented, nothing armed)
- **Compliance scoring:** Pure function `calculate_compliance_score()` — no DB, 100% testable
- **JWT auth:** 15-min access + 7-day refresh, plus 60-second `type="stream"` tokens from `/auth/stream-token` for SSE query-param auth. Ordinary API auth only accepts access tokens; activity SSE query auth only accepts stream tokens.
- **Cookie-backed refresh:** refresh token is also rotated through an HttpOnly `refresh_token` cookie scoped to `/api/auth`, while access tokens remain bearer tokens
- **Rate limiting:** slowapi per-route (5/min login, 3/min password, 60/min prices, 30/min reference)
- **Availability windows:** Persist canonical codes (`SPOT`, `YYYY-MM`, `YYYY-QN`, legacy-compatible `YYYY-CAL`); UI-relative labels like `M+1` must be resolved before persistence
- **Green-fuels market model:** Matching and live slice benchmarks key on `side + market_product + delivery_point + availability_window`; supplier sustainability/compliance fields stay out of the hard market key
- **Market provenance contract:** Market-data responses use shared `source_kind`, `scope`, and `demo_status` fields. Aggregate data exposes real/demo/unknown counts; unknown contributors remain `UNKNOWN` rather than being collapsed into real/demo/mixed.
- **Forward Curve monitoring:** `/curves/forward/table` and `/curves/forward/slice` use `forward_curve_market_slices.py` as the canonical public read model for approved `market_product + delivery_point + availability_window` slices. Products aggregate by canonical market product, delivery points are restricted to the approved trading ports, and public cells expose server-owned label policy plus redacted source/demo/staleness fields. `/curves/forward/board` remains for older clients.
- **Real signal ingestion:** `scripts/ingest_market_signals.py` (staging-guarded, dry-run default) + `services/market_signal_ingestion.py` produce verified `market_signal_ingestion_runs` whose rows satisfy the trust predicate in `forward_monitoring.py` and render as REAL. CSV formats, staleness semantics, redaction invariants, and rollback SQL: `docs/market-signal-ingestion.md`.
- **Price discovery provenance:** `/prices` 24h summaries classify confirmed trade buckets as `CONFIRMED_TRADE`, `DEMO_SEED`, `MIXED_SOURCE`, or `UNKNOWN` using the same demo organization rules as trade tape.
- **Trade tape scope:** `/trade-tape` returns anonymized confirmed trade prints for the last 7 days. Exact delivery-point history is available only when clients filter by `delivery_point_id` and entries return `scope="DELIVERY_POINT"` with delivery-point fields. `provenance_kind` is legacy-compatible; new clients should prefer `source_kind`/`demo_status` when present on newer surfaces.
- **Market Radar watchlists:** Watchlists are observer-only. Typed targets store either canonical slices or pinned order snapshots, and order create/update/cancel paths emit slice/pin events without feeding core matchmaking. New event payloads carry provenance at emission time; legacy events return `UNKNOWN` rather than doing response-time order lookups.
- **Behavioral analytics:** Umami is an optional failure-isolated dependency. `GET /admin/analytics/product-usage?days=7|30|90` combines bounded Umami aggregates with authoritative UTC-period database counts. Server conversion events and the documented browser reporting taxonomy use separate allowlists. Registration, organization, order, and trade events are scheduled only after commits, carry bounded originating request metadata for Umami bot classification/environment attribution, and use a strict property allowlist; collector drops/failures never alter endpoint response contracts. Aggregate successes cache for at most five minutes and failures for at most 30 seconds. `totaltime / visits` is exposed only as average session duration, not active engagement. See `docs/behavioral-analytics-contract.md`.
- **Live runtime topology:** Production systemd binds Uvicorn to `127.0.0.1:8000`; staging binds to `127.0.0.1:8001`; the public reverse proxy fronts those loopback ports. Both services require network-online and PostgreSQL, use four workers, and preflight the exact migration checkpoint published with the release rather than a later source head. News refresh has exactly one external owner per environment: the environment-specific systemd timer invokes `app.cli.refresh_news`, while a PostgreSQL transaction advisory lock rejects overlap. Application workers create no scheduler and the public API exposes no manual refresh mutation. Product-analytics pruning likewise has one timer owner per environment; it requires PostgreSQL/network readiness, uses bounded boot retries and the news sandbox, and must match that environment's `.runtime-release.env` identity before database access. Every runtime service checks the durable `.runtime-deploy/<environment>.state` and refuses blocked/readiness-pending starts.
- **Runtime budgets:** Production and staging share PostgreSQL `max_connections=100`; deployed settings require exactly two app services and at least 20 maintenance connections, so configuration cannot undercount the immutable topology. The default aggregate is `2 services × 4 workers × (2 + 1) + 20 reserve = 44` pool-side; the shared SSE dispatcher adds one dedicated non-pool LISTEN/leadership connection per worker (+4 per service), and `configured_connection_total` validates the FULL budget including those listeners — `24 pool + 8 listener + 20 reserve = 52` of 100 (docs/market-event-dispatch.md). KYC uploads are bounded at 10 MiB per file and 20 MiB aggregate by default. Measured steady state is approximately 530–538 MiB per service; systemd starts at `MemoryHigh=768M` and `MemoryMax=1G` with headroom, not a measured-safe claim.
- **Runtime identity and boundaries:** Production is exactly database `verdaxis`, app `verdaxis_app`, migrator `verdaxis_migrator`; staging is exactly database `verdaxis_staging`, app `verdaxis_app_staging`, migrator `verdaxis_migrator_staging`. Both deployed environments reject SQLite, default JWTs, auth bypass, URL query routing, different app/migrator endpoints, shared roles, absent credentials, and empty/decoded-whitespace/default passwords. Canonical deploys fix branch/state/tool paths, discard ambient Git/database/Python routing controls, and run database-bearing helpers with a minimal environment. Deploy uses an exact dry-run-approved SHA, serializes each environment with a durable flock before live revision verification, and keeps durable state through restart/readiness; later failures keep code/identity aligned and fail closed. Production/staging migrations additionally bind that SHA, an exact expected current revision, and one literal target to a committed checkpoint allowlist; missing approval, aliases such as `head`, unexpected live state, and unlisted graph traversal refuse before source mutation. Checkpoint application pins Alembic to the explicit migrator URL; exact-revision startup verification never substitutes `DATABASE_URL`. Post-migration ACL convergence runs from a digest-attested committed bundle before restart, opens `.env` with no-follow regular-file checks, ignores ambient URL overrides, and executes transactionally as the migrator. Strict readiness is exactly `status=ok`, `db=ok`, environment, and full SHA. Credentialed CORS uses exact per-environment origin allowlists.
- **Database authority split:** Runtime and Alembic separately attest `current_database()`, `current_user`, exact role properties, and absence of memberships. The migrator owns the database, public schema, and application tables/sequences. Expanded database/schema/default ACL rows must exactly equal the migrator-owner, app, and backup allowlist including grantor and grantability; bootstrap revokes every unrelated explicit and delegated grantee with intentional deterministic `CASCADE`. A central governed-object relation covers every ordinary table, child partition, and sequence; object and `pg_attribute.attacl` column grants, including PUBLIC, are rebuilt from zero before exact declarative app/backup grants. The runtime app receives authority only for named tables, privileges, and organization columns; organization verification/provenance and unknown/future columns are not app-writable, and signup relies on database-owned verification and `UNKNOWN` provenance defaults. Audit and status history are append-only (`SELECT, INSERT`). Migrator defaults give backup read-only table/sequence access. `alembic_version`, seed/quarantine controls, market evidence/provenance, operator/control tables, and organization verification/provenance columns are not app-writable. Validation proves app/backup cannot retain write via column grants or delegate authority. Object ACLs exclude `alembic_version`, `spatial_ref_sys`, and extension-owned objects.
- **Cross-branch runtime gate:** Runtime metadata stays directly after `pa_20260715_analytics_facts`; integration reparents `sec_20260720_identity` onto runtime and preserves security through `sec_20260720_device` before market revisions. Integration adds each reviewed identity/device/quarantine pause to the committed checkpoint policy and promotes one checkpoint at a time. At the security checkpoint it adds exact table/column policy for pending registration, organization-join, refresh-session, and user KYC/admission writes; unknown columns remain closed. The manifest-driven immutable installer accepts exact audited auth-maintenance service/timer entries and bytes without reopening the checkout. Combined integration must provide a separate approved operator identity for market-signal ingestion and other protected records, plus shared SSE transport (built: the market_event_dispatch outbox sequencer/hub; staging activation stays behind the combined gates), while preserving four Uvicorn workers. Identity-only rollback from local-monitor is forbidden after source changes because it would pair stale SHA with new bytes.
- **KYC trust boundary:** Gemini document analysis is advisory only and every submission remains pending. Only trusted administrator approve/reject routes may change KYC/account status. The removed legacy compliance and dashboard routes stay unmounted.
- **Local operational monitoring:** `deploy/monitor` is source-only and owns no application readiness producer, deploy transaction, backup producer, root installer, or activation path. Its reader requires the shared exact four-key runtime contract (`status=ok`, `db=ok`, exact environment, full SHA); the runtime owner must consume the corpus and publish matching identity. Hostile JSON rejects duplicate keys. Backup gzip evidence must meet a PostgreSQL marker and 1 KiB expanded floor. Readers atomically publish semantically consistent mode `0600` status and never receive alert secrets; alert dedupe is fixed hourly. Production and staging demo service/timer pairs are independent and execute source only from private read-only archives of the attested commit. The JSON manifest is static byte inventory for a future canonical immutable installer. Legacy retirement additionally requires healthy durable alert state with per-destination recovery newer than failure, and rechecks every timer immediately before the fixed disable command. Full owner seams: `deploy/monitor/README.md`.

## Revenue Streams

1. **Transaction fees (0.5%)** — commission_amount_usd on Trade model
2. **Compliance SaaS ($200-500/vessel/mo)** — /compliance/fleet, /compliance/scenario
3. **Data products ($1K-5K/seat/mo)** — /prices/reference (daily VWAP)
4. **Platform analytics** — /admin/analytics/overview, /admin/analytics/daily, /admin/analytics/product-usage
