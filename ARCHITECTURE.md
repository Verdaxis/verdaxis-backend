# Architecture

> FastAPI + SQLAlchemy 2 (async) + PostgreSQL 17.9/PostGIS 3.6.2 + Alembic + Pydantic v2
> PyJWT (HS256) + bcrypt + slowapi rate limiting + structlog JSON logging

## Deployed Topology

| Environment | Host | Backend | Local listener |
| --- | --- | --- | --- |
| Production | `169.58.37.164` (`vmi3623757`) | `/home/verdaxis-prod/verdaxis/prod/be` | `127.0.0.1:8000` |
| Staging | `194.233.68.86` (`vmi1840561.contaboserver.net`) | `/home/verdaxis-prod/verdaxis/staging/be` | `127.0.0.1:8001` |

The shared host's old production listener on port 8000 is retired, and its
production backend and recovery units are masked. Current release identity
comes from each environment's `/health/ready` response, not a dated SHA in
source documentation. The shared-host marker is
`/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`.

## Shared FuelEU reference values

`rs/read/src/market/fueleu.rs` owns the lifecycle GHG reference, the
regulation's reduction steps and the deficit penalty per tonne; every
reporting-year limit is derived from the reference and the steps. Voyage
calculations use its exact-decimal adapters; order-book CI projections use the
same reference for arithmetic and response fields. The shared schedule accepts
2025–9999; the MCP request retains its 2025–2050 reporting-year range.
`rs/read/tests/phase5_fueleu.rs` checks the boundaries, adapters and consumer
sources.

`rs/read/src/compliance/pricing.rs` provides the pure declared-lifecycle
comparison and overlay assumptions. It uses the shared reference, penalty and
reporting-year limit; missing declarations remain null, physical differences
round to 0.001 tonnes CO2e/MT, and financial benefits remain unpriced. The
optional `COMPLIANCE_EUR_USD_RATE` setting (`Config::compliance_eur_usd_rate`)
replaces the default exchange rate; startup refuses a value that is not
positive. `rs/read/tests/phase5_compliance_pricing_core.rs` covers the core and
its serialized client fields.

`rs/read/src/compliance/validation.rs` holds the pure fuel-mix and
reporting-year rules, and `rs/read/src/compliance/scoring.rs` the pure checked
score calculation; both use `market::fueleu` for the reference, the penalty and
the reporting-year limits. `rs/read/src/routes/compliance_fuels.rs` serves the
constant illustrative fuel-intensity table at `GET /api/compliance/fuels`, with
no database read or authentication. The other compliance routes reach the pure
cores through their authenticated adapters.
`rs/read/tests/phase5_compliance_scores_core.rs` and
`rs/read/tests/phase5_compliance_fuels.rs` cover them.

## Phase 5 public market reads

`runtime::admission::run_market_read` runs the work of one public market read
in the shared admitted owner task: one READ ONLY transaction with a
transaction-local five-second statement limit, committed by the owner, with the
permit and connection held through cleanup. `routes::market_data_error`
supplies the typed `{"detail": {"code", "message"}}` errors and the result-cap
helpers. Saturated admission, pool, transaction and owner failures of
`/api/availability`, `/api/benchmarks`, `/api/demand` and `/api/producers` use
that envelope; every other admitted read keeps its string detail.
`rs/read/tests/phase5_public_boundary.rs`, `rs/read/tests/http_contract.rs` and
`rs/read/tests/admission_postgres.rs` check the boundary.

## Optional fleet artifact configuration

`Config.fleet_artifact_path` accepts `FLEET_ARTIFACT_PATH` and defaults to
`data/fleet_demand.json`. Relative paths resolve under the backend unit's
WorkingDirectory. Relative and absolute paths must stay within its trusted
`data` directory, canonicalised once at startup. Startup preserves raw syntax
until validation and refuses empty paths, invalid encoding, trailing separators,
traversal and proven symlink escapes. Internal links resolve to an in-root target;
connection and state validation retain that representation without filesystem
resolution. Missing, inaccessible and non-directory storage remains optional. `runtime::config` provides the
resolver and one bounded two-second, 256 KiB JSON startup probe. The binary logs
only available/unavailable; missing or invalid optional data does not affect
process startup or the unchanged four-field database readiness response.
Full document validation and the fleet route remain the fleet reader's concern.
`rs/read/tests/runtime.rs` and `runtime_postgres.rs` cover this boundary.

## Phase 5 read lookup indexes

`alembic/versions/p5_20261020_read_indexes.py` (parent `prc_20261018_cache`)
adds three non-unique B-tree indexes for the Phase 5 reads:
`vessels(organization_id, id)`, `producer_projects(cod_year, name, id)` and
`producer_projects(status, cod_year, id)`. It takes SHARE locks on both tables
with NOWAIT and builds all three in one transaction. The models declare the
same indexes. `deploy/migration-checkpoints.tsv` adds the transition and its
pause; the Rust allowlist accepts both `prc_20261018_cache` and the new
revision. `tests/unit/test_phase5_indexes.py`,
`tests/unit/test_phase5_checkpoints.py` and
`tests/postgres/test_phase5_read_indexes.py` check it.

## Rust Phase 2 feedback

`rs/read/src/crud/feedback.rs` provides the Task 10 feedback submission and
administrator listing routers. Both use the shared authenticated transaction;
submission and pagination validation precede endpoint quota consumption through
`validate_and_admit`. The admin view retains identified support data and outer
joins. `rs/read/tests/run_feedback.sh` runs the focused HTTP/database checks in a
disposable PostgreSQL container; `tests/rust_contract/phase2/feedback.json` holds
validation vectors and scenario definitions. `feedback_scenario.py` registers
31 real-route twin-database actions with full committed-state, rollback,
postcommit no-replay and empty-mail-ledger assertions. The feature-gated
`feedback_contract` example and Python test adapter control IDs and clocks without
changing deployed settings. These routers are
not activated in the serving binary. Feedback stays on Python at cutover until
Task 22 provides the shared Python–Rust quota counter across real processes.


## Rust Phase 2 order-write boundary

Task 20 adds `rs/read/src/order_write/` for Python-derived lock identities,
request hashes, receipt replay, bounded retries, and a private PostgreSQL
transaction shell. The shell preserves caller-declared order-row acquisition
order and sorts side-independent slice locks. Its database methods and test
doubles remain inside the boundary. SQLx owns rollback on transaction drop.
The public command entry point refuses unsupported commands before database
access; no market HTTP write route is mounted. This task implements neither
matching nor trade lifecycle operations. Public-scope event generation (PD4)
remains mandatory before a future command can emit market events.

`tests/rust_contract/phase2/market-write-spec.json` pins the Python source and
hash vectors. The registered `order-write-postgres` group uses feature-gated
loopback adapters to prove twin state, rollback, replay, timeouts, observed
PostgreSQL advisory-lock order, bounded retries, races and mixed dispatch. See
`docs/rust-phase2/task20-write-boundary.md` for gate results and the resume point.

## Rust Phase 2 price-alert component

Task 16 adds `rs/read/src/crud/alerts.rs` for organization-owned alert create,
list, and delete. `routes::alerts_router` exposes the component router; runtime
composition must supply the existing verification settings and shared quota
authority. Each request uses the shared authenticated transaction owner and
current database organization. NUMERIC rounding remains PostgreSQL-owned;
product codes derive from the attested exact label tuple, including inactive
products. Python retains price triggering and participant-event production.
`tests/postgres/test_phase2_alert_interop.py` drives Rust storage and HTTP
component checks against the existing disposable PostgreSQL fixture, then runs
Python's trigger helper on the committed Rust row. The registered `alerts-crud`
and `alerts-crud-reverse` scenarios use the shared runner for paired HTTP/state,
rollback, retained-commit/no-replay and mixed dispatcher checks. Disposable-only
adapters control IDs/clocks and record approved PD8: Python's SQL-count barrier
reproduces six active rows; Rust takes a namespaced per-organization transaction
advisory lock after the authenticated user lock and before counting, admitting
one create and refusing the other with the existing 403, leaving five active rows.
No organization row lock, Python change, or runtime ACL grant is required. Evidence is in `docs/rust-phase2/task16-handoff.md`.
Coverage requirements are data in `tests/rust_contract/phase2/alerts-scenarios.json`.

## Rust Phase 2 watchlist feed and read state

Task 15 adds `rs/read/src/crud/watchlists/events.rs` for W07 and W08. The feed
pages by `(created_at DESC, id DESC)` with a cursor of RFC 3339 UTC plus the
event id, and applies the PD4-G guard in SQL before `LIMIT`: best-price events
and events whose payload names an order outside the full public order scope are
hidden, and a PIN target must name a public order. Acknowledgement commits
`is_read` only when the row is still unread and refuses a guarded event before
any write. Python remains the event producer; the trusted Rust producer is a
design proposal in `docs/rust-phase2-task15-producer-design.md`.

## Rust Phase 2 referral routes

`rs/read/src/crud/referrals/` implements referral statistics, leaderboard,
public code resolution, serialized code allocation and invite preparation.
Routes use shared authentication/transaction authority; invite allocation commits
before the bounded provider call. Only the actor's referral code can change.
Mail settings are injected, default disabled, and tests use numeric loopback
stubs. No referral or outbox row is inserted by invite. Shared runtime projection
and twin-database/race qualification remain integration gates. See
`docs/rust-phase2-referrals.md`.

## Rust Phase 1 bootstrap

`rs/` is a Rust 1.95 workspace with two packages. `verdaxis-read` defines the
shared types and build-time release identity; its binary only prints that
identity. `verdaxis-contract` is an independent library and CLI that drives the
external HTTP contract comparison and never links `verdaxis-read`.

Task 2 adds `rs/read/src/runtime/` for configuration, pool authority, and health
checks. The pool has configured read admission slots plus one readiness connection
and read-only session defaults. Startup
checks the compiled revision allowlist, exact database role, memberships, and
connection budget. Required `DB_SESSION_TIME_ZONE` (`db_session_time_zone` in
`RUST_CONFIG`) is applied and verified by `after_connect` on every new read and
health connection, including replacements. Startup refuses a conflicting
`pg_db_role_setting` database default and logs the effective zone. SQLx 0.9's
hard-coded UTC startup field cannot be overridden through `.options()`.
Staging uses `Asia/Singapore` until Phase 0 convergence, then `UTC`. It does not open a
migrator connection. `scripts/prepare_rust_budget.py` creates private budget
metadata and verifies environment-file drift. The deploy transaction writes
that artifact to `.runtime-rust/budget.json`, and the reviewed staging unit
passes it to the binary with `--budget-file`; the contract harness may instead
carry the document in `RUST_CONFIG`. `runtime::config::config_from_deployed_environment`
builds the same validated `Config` from the flat deployed environment, so a
staging start needs no JSON document from an operator.
Task 3 separates the read pool from a one-connection readiness pool. Founder
decision 88 (18 Oct) makes `read_admission_slots` Rust-owned, default 16; the
read pool matches it and `pool_max` is slots + 1. Both configured and observed
PostgreSQL connection limits include Python, reserve, and the full Rust pool.
The prepared environment digest and release checks remain required. `runtime/admission.rs` transfers admission to a
query-owning Tokio task so caller cancellation does not release database
capacity. HTTP composition provides correlation, CORS, support boundaries and
fixed-window price limits. Empty route and MCP modules reserve later task
ownership. The binary accepts explicit `RUST_CONFIG` JSON and serves health;
data routes remain unmounted. See `docs/rust-phase1/progress.md` for the local
checks, remaining Task 3 gates, and resume point.

Task 4 adds the disposable contract harness. `scripts/rust_contract/compare.py`
is the single authoritative comparator; `record.py` captures the Python oracle;
`run_disposable.py` starts one disposable PostGIS container, both servers, and
the comparator; `seed_corpus.py` and `check_assertions.py` own the fixture and
assertion ledgers. `tests/rust_contract/` holds the corpus, the fixed clock
manifest, the meta-tests, and the golden corpus; `rs/read/examples/contract_server.rs`
serves one route group through the serving router. Health and echo are proven;
the seeded and MCP groups are deferred as recorded in
`docs/rust-phase1/progress.md`.
Task 4b adds attested corpus construction: legal pre-guard history is inserted at
`miq_20260720_market_quarantine`, then upgraded by the normal migration chain.
Current fixtures reuse the Task 6 matrix and add ports, news, exact terms and
trusted/untrusted signals. The runner pins the runtime KYC switch and records
all groups twice; incomplete or unstable recordings cannot replace golden data.

Task 14b adds the staging-only shadow content path. `shadow.py` reads private
reviewed cases, enforces serial rolling call/minute budgets, direct loopback
transport with an absolute deadline/body cap, separate Python/Rust readiness
identities, memory-pressure backoff and deploy-state stops. It delegates both
legs of each Python–Rust–Python sandwich to `compare.compare_responses` with
the byte profile. Only schema-specific validated request-clock tokens and the
existing reviewed exact-item tie exceptions may differ. Payloads and internal
capture hashes stay in memory; evidence is bounded metadata in a private path
outside the checkout. Missing coverage, empty-only results or an unapproved
short floor cannot return PASS. Candidate activation remains a separate gate;
see `docs/rust-phase1/readiness-task14b.md`.

Task 5 exports the catalog policy as data. `scripts/rust_contract/export_policy.py`
turns the approved `app.market_catalog` constants and the reviewed
qualification-profile map into `deploy/rust/market-policy.json` (identity, lanes,
execution mode, display order) and `deploy/rust/mcp-catalog.json` (the
`list_markets` payload); `deploy/rust/market-policy.manifest.json` holds their
digests. `rs/read/src/market/policy.rs` loads and validates that policy, matches
rows by the exact identity tuple, and projects database rows to the public
catalog response. Executable Rust holds no product name, code, or UUID; a test
scans `rs/read/src` and `rs/read/sql` for them.

Task 6 adds the shared market rules under `rs/read/src/market/`. `visibility.rs`
binds values into the two shared relations in `rs/read/sql/`:
`public_orders.sql` projects internal order rows and private digest input from the attested policy
plus the captured instants, and `trade_evidence.sql` selects formal trade
snapshots. `terms.rs` owns typed execution qualification, the reviewed-terms
digest, and the public redaction of private declarations; `decimal.rs`,
`windows.rs`, and `provenance.rs` own normalized decimal text, the canonical
availability-window grammar and aliases, and the evidence policy constants.
Qualification reads the catalog `qualification_profile`, so the LNG and B100
rules stay data-driven. `rs/read/tests/market_contract.rs` replays the synthetic
digest vectors from the Python oracle; `rs/read/tests/run_market_contract.sh`
seeds a disposable database and compares the selected ids with the Python
predicate under one fixed clock. Sealed `OrderQuery`/`TradeQuery` wrappers compose
only developer-owned static tails around `eligible_orders`/`eligible_trades`.
`OrderSurface` separates main orderbook, compatibility listings and candidate
reads; ranking, aggregation and pagination occur after the shared relation.
Digest decimals use text-based Python context normalization, while public terms
retain Pydantic scale, signed zero and schema defaults. Typed decimal validation
uses exact coefficient/exponent comparisons, not `rust_decimal` narrowing.
The runtime `Config.kyc_execution_gate_enabled` defaults true and supplies order
bind `$13`; extra consumer binds begin at `$14`. Both owner and assisted-member
admission use the same strict/advisory switch as Python. Request filters cannot
set that switch. Region filters match a region or delivery-point name; empty
means no filter. Curve and quarantine metadata policies remain later-task work.

Task 7a serves the public snapshot through one admitted, read-only REPEATABLE
READ transaction, with ranked depth and one benchmark batch sharing a captured
instant. `market/terms.rs::public_response_terms` mirrors the public FAME/LNG
response schemas separately from full execution validation: ignored private
fields cannot fail public projection, but still affect the digest and benchmark
qualification. `market/orderbook_query.json` records public schema field order.

Tasks 7b/7c add paginated lists, CI lists, map/count/distinct reads and compatibility
listings. `market/order_projection.rs` is the shared snapshot/list DTO; private
digest input remains separate. `market/map_decimal.rs` preserves exact aggregate
arithmetic, and `market/listing_terms.rs` retains compatibility schema ordering.
R18/R26 use decision 23's created-time/UUID descending pagination in both languages.
All projections apply shared eligibility before sorting, grouping and pagination.

Task 6a adds `market/live_benchmarks.rs` and `sql/live_benchmarks.sql` for
read-only, request-cached same-side benchmarks. It applies full typed/scalar
qualification after Candidates selection and retains the current REAL/DEMO
blend. `market/whitespace.rs` owns Python-compatible whitespace for scalar text
presence and certification normalization; SQL trimming is unchanged.

Task 8 ports the simple public reads. `rs/read/src/routes/ports.rs` serves
`/api/ports` and `/api/ports/{port_id}` from `rs/read/sql/{ports,port}.sql`,
casting each geography column to geometry before `ST_X`/`ST_Y` and returning
`location: null`; the port route applies no active filter and no ordering.
Intelligence is loaded separately in 500-port batches, preserving SQLAlchemy's
first-returned scalar relationship selection without duplicating port rows.
`routes/catalog.rs` serves `/api/catalog/products` and
`/api/catalog/delivery-points` by joining `products` and `delivery_points` to
the attested policy in `rs/read/sql/catalog_{products,points}.sql` and projecting
the rows through `market::policy::project_catalog`, so no identity or display
order is written in Rust. `routes/news.rs` serves `/api/news` from
`rs/read/sql/news.sql` and writes the ten-key dict contract, using Python
`isoformat()` semantics for its dates. All three modules run their database work
through `runtime::admission::run_admitted` and share the Python-compatible query
validator in `runtime/http.rs`. The route modules are merged into the serving
router by `rs/contract/tests/simple_reads.rs` (feature-gated group target) and
`rs/read/tests/simple_reads.rs` (offline vectors for both date formats, the port
and news field order, and the 422 envelopes). Task 8's local evidence, the
simple-read corpus limits, and the fixture-family decision are recorded in
`docs/rust-phase1/progress-task8.md`.

Task 9 adds immutable price summaries, REAL-only daily VWAP/CSV, and anonymous
seven-day trade tape in `routes/prices.rs` and `routes/trade_tape.rs`. Shared
`formal_trade_snapshots.sql` feeds separate economic and quarantine-only metadata
relations. Tape totals apply Python's 28-digit Decimal context to exact SQL
products without changing quantity or price scale. Typed price query validation
precedes the shared rate bucket. An opt-in response extension preserves Python's
missing CORS/request-ID headers only for unhandled tape window errors.

Task 10b adds separate canonical table/slice and older-board projections in
`routes/curves_canonical{,_views}.rs`. Task-local path registration exposes
`market/curve_projection`, `curve_cells`, `curve_unknown_metadata`, `signals`,
`curve_keys`, and `benchmarks`; `curve_policy.json` contains source-derived
board defaults and seed quotes. UNKNOWN orders use the shared current-order
fragment only for counts. Economic book and depth tails aggregate over
`eligible_orders` in PostgreSQL; depth selects REAL before ten levels per side.
Table requests retain no raw order rows. A read transaction scopes `jit=off`
to these admission-heavy queries. Signal histories and trades rank in SQL,
with separate exact sparse-key selection. The disposable contract server's
SQLx counter records statement/returned-row counts without query text.
`curve_fixtures.py` supplies the same depth, trusted-evidence, subsecond and
10,000-order cases for differential checks and later golden publication.

Task 11 adds `rs/read/src/mcp/` for the stateless rmcp adapter, anonymous market
prices and pure voyage calculations. Database tools use shared `run_admitted`;
catalog and protocol discovery bypass admission. `validation.rs` reads exported
Python tool schemas and normalizes arguments before admission; the pure engine
uses that same boundary. `protocol.rs` validates exported Python JSON-RPC envelope
schemas and modern metadata rejection order before tool adaptation; rmcp retains
dispatch. Internal `ApiErrorKind` distinguishes expected, database and unexpected
failures through admission without changing REST status/detail serialization.
Date parsing reuses the existing price-query parser.
`export_mcp_validation.py` and `mcp-validation-vectors.json` retain reproducible
Python diagnostics and normalized inputs. This remains a partial acceptance
checkpoint; `docs/rust-phase1/progress-task11.md` lists unresolved input-domain,
protocol and error-class gaps. MCP wire recordings stay in untracked lane evidence
until integration publishes them into the single golden corpus.
`tests/ephemeral_ports.py` is the shared kernel-range authority for the disposable
allocator, server producer and target consumer. It reads Linux procfs or macOS
sysctl and fails closed without changing host settings. Database/Linux gates run
through the approved `verdaxis-prod-test` isolated harness; local Colima stays off.

Task 12c adds the reviewed staging unit
`deploy/systemd/verdaxis-rust-staging.service` and its manifest entry, so the
staging manifest now selects the backend, the Rust reads and the singleton
maintenance units. The unit loads `.runtime-release.env` and the deploy-written
`.runtime-rust/runtime.env` (never the secret-rich `.env`) and runs the deploy
start guard and isolated stdlib `verify_rust_start.py` before `ExecStart`.
The deploy verifies migration authority and budget drift under its lock, then
freezes the exact revision with the release SHA in private `revision.json`;
startup checks that record and budget identity without application imports,
`.env` or a migrator secret. Budget preparation uses atomic replacement, so
repeat deploys and retries do not fail on an existing artifact. It it serves `127.0.0.1:8011` with a 256 MiB cap on the same
sandbox directives as the running backend unit. The first staging release keeps
`deploy/rust/staging-policy.json` disabled. Deploy that bootstrap release, then
install its immutable unit bundle as root from the exact clean fixed checkout.
Only a later reviewed staging activation release enables the policy. Its
canonical deploy refuses an unloaded/masked Rust unit before source mutation,
verifies the signed artifact before state publication and preflights exact
noninteractive sudo service commands against reviewed environment sudoers.
Python restarts and passes exact readiness first; Rust restarts separately.
Rust-only start/readiness failure stops and disables Rust while retaining
exact-ready Python at the new SHA, with durable `rust-failed` state for canonical
retry. Python failures remain fail-closed. Both readiness documents are required
before successful deploy state clears. Decisions
99 and 101–103 use normal staging publication and one shared Python/Rust release
SHA; there is no independent candidate activation path. Production
now has a separate policy-off bootstrap record and unit; no production Rust
activation occurs merely by promoting this source. `deploy/rust/production-policy.json`
selects `verdaxis-rust.service` at `127.0.0.1:8010`. The shared transaction selects
only its own environment policy, verifies production provenance against
`refs/heads/staging`, prepares app-only runtime credentials, and requires the same
Python/Rust release SHA and both readiness documents. Production startup requires
`verdaxis_app` on `127.0.0.1:5432/verdaxis_prod` and exact checkpoint
`p0_20261016_union`. Production browser CORS follows the Python production origin
set, not staging's origin. The production unit caps Rust at 256 MiB and one CPU;
Python retains four workers. Conservative default connection accounting is 69
including the 20-connection reserve and Rust's 16 reads plus readiness.
Production needs its own B' policy-off deploy, root manifest install, then S'
policy-on prod-push artifact; neither staging artifacts nor an independent SHA
are accepted. `scripts/rust_caddy_candidate.py` prepares private per-group Caddy
candidates and exact rollback bytes without touching live config. It reuses Task
13's exact matchers and refuses production MCP activation. Production uses the
single EU Caddyfile: P0 adds a bounded redacted API access log, then each group
inserts its handle after analytics and before the unchanged Python fallback,
retaining client-IP forwarding, flush and keepalive policy. Each read group lists
Rust first and the Python process second with `lb_policy first`, a ten-second
`lb_try_duration`, a 250 ms `lb_try_interval`, and passive health
(`fail_duration 30s`, `max_fails 1`), so a stopped Rust service answers from
Python. The `fallback` candidate adds a 15-second retry window to the Python
catch-all so a request waits out a Python restart. MCP keeps one upstream because
it carries writes: Caddy retries any method after a dial error, but a non-GET
request after a successful connection is not retried. Staging keeps snippet
imports and takes the same options from `deploy/caddy/rust-public-staging.caddy`.
`scripts/rust_passive_no_harm.py` evaluates equal ordinary-traffic log
windows and journal error classes; it performs no requests or measurements. An
empty or missing journal window is invalid evidence (exit 2 with a reason), never
"no new error classes".
Both Rust units launch through `/usr/bin/env` so real systemd verification works
before the first artifact exists without weakening startup checks. See
`docs/plans/2026-10-19-rust-phase-1-production.md`; host acceptance remains separate.
`docs/rust-phase1/progress-task12c.md` holds the unit digest, the artifact
digest and the systemd 249/255 and staging-ABI start evidence.

## Phase 1 same-meaning successor

The explicit `phase1-meaning-v1` profile reuses `phase2_meaning.py` and its single
raw-response comparator. The default byte path remains historical and unchanged.
The successor CI runner supplies the reviewed ledger digest explicitly.
`phase1_meaning/corpus/{enabled,disabled}` retains all 487 requests per KYC state,
recorded twice on disposable data. Canonical tape pages come from the complete
Python-filtered relation before full-UUID pagination, not a sorted Python page.
`runtime/cache.rs` stores only bounded public reference payloads for 60 seconds;
request headers are rebuilt. Trading caches remain unimplemented and disabled.
Their infrastructure is prepared: migration `prc_20261018_cache` adds the
generation, event and incarnation tables and an always-enabled statement trigger
on each trading dependency. `runtime/cache_authority.rs` grants a hit only under an
owner record outside the database (`scripts/rust_cache_authority.py` writes it),
a lease, a matching database identity and a bound listener; `runtime/cache_events.rs`
owns the one budgeted listener connection. Nothing starts either yet. See
`docs/rust-phase1/cache-dependencies.md`.
Exact `market/amount.rs` operations retain intermediate economic rounding while
removing wire-spelling emulation. See `docs/rust-phase1/same-meaning-contract.md`.

## Rust Phase 2 shared route runner

`phase2_scenarios.py` owns lane registrations, per-action HTTP/state/effect checks,
barrier races and mixed-writer schedules. `phase2_routes.py` adapts them to the
existing `run_disposable.Runner` lifecycle and app-role cloned databases.
`phase2_mail.py` owns the per-run loopback captured-send provider;
`phase2_oracle.py` redirects Python email only in disposable tests. The complete
synthetic registration is `phase2_example.py`, not CRUD qualification. Groups
without real registrations refuse rather than execute harness smoke. See
`docs/rust-phase2-runner.md` for lane adapters and acceptance boundaries.

## Rust Phase 2 contract preparation

`tests/rust_contract/phase2/routes.json` pins the 27 CRUD method/path pairs,
OpenAPI operations, DTO exports, effect classes and Python source digests.
`export_inventory.py` reproduces the export through the full HTTP OpenAPI route
on the isolated test host. `evidence-manifest.json` declares required route and
scenario coverage and separates disposable-artifact qualification from
production-cutover acceptance. It contains no run results or cutover verdict.
See `outputs/rust-phase2/source-checkpoint.md` for the oracle identity and checks.
Authentication issuance, market writers, SSE and dispatch remain Python-owned.
`compare.py::compare_responses` is the raw-response profile boundary. Its explicit
`phase2-meaning-v1` branch delegates to private `phase2_meaning.py`, with a
SHA-256-pinned `normalization.json` ledger. It uses exact tagged JSON values,
schema-local decimal/timestamp rules and complete-item multisets inside approved
ties only. The Phase 1 default and existing recording comparator stay unchanged.

Task 5 adds verification-only `rs/read/src/auth/{token,claims}.rs`. It verifies
HS256 current/previous keys and Python-compatible access claims against an explicit
clock. Its account projection validates cutoffs and mutable status without issuing
sessions or exposing database writes. It is not mounted in the serving router;
Task 6 must load account state under the shared transaction/user-row lock.
`phase2_auth_vectors.py` exports synthetic full-HTTP Python results for Rust's
in-process HTTP tests. See `docs/rust-phase2-task5.md` for scope, source findings
and every gate result.

### Phase 2 Task 8: notifications

`rs/read/src/crud/notifications.rs` serves N01–N04 through `auth::run_authenticated`.
List/count filter by recipient and PostgreSQL's `_email_only` boolean coercion;
response projection removes top-level `_email_` keys only. Both writes include
email-only rows and create no events. `notification_authority.rs` accepts explicit
verification settings and a shared quota authority at composition time; it adds
no runtime settings or secrets. `routes::notification_routes` exposes the route
group without activating it in the serving binary.
`rs/read/tests/run_notifications.py` reuses the existing disposable lifecycle to
run the Rust component tests against the migrated schema and audited app ACLs.
The twin-cell HTTP scenario remains pending the runner lane. Its task-owned data
is `tests/rust_contract/phase2/notification-scenario.json`; current gate evidence
and integration requirements are in `docs/rust-phase2/task8-handoff.md`.

### Phase 2 Task 9: preferences

Task 9 adds `crud/preferences.rs` and the marked `routes::preferences_router`
registration seam for P01/P02. Both routes use the shared authenticated transaction;
PUT retains the exclusive user lock through commit and preserves the original
validated JSON. The integration owner supplies verification settings and shared
quota authority; this task does not activate deployed routes. The disposable
`phase2_preferences.py` scenario uses the existing runner lifecycle, separate
Python/Rust databases, controlled IDs/clocks, full-state comparison, and observed
same-user lock waits. Evidence and remaining boundaries are in
`docs/rust-phase2-task9.md`.

### Frozen Phase 1 measurement reference

Decisions 86/87 freeze the Python small-workload performance reference in
`tests/rust_contract/python-baseline-small.json`, pinned by `gate-config.json`.
`scripts/rust_contract/baseline.py` validates its bytes and PM attestation;
`frozen_gate.py` and `rs/contract/src/frozen_gate.rs` enforce the Rust-only
identity, successful-sample floor, frozen p95 thresholds and error-rate rule.
`measure.py --rust-only` starts no Python service and records baseline, manifest,
source and harness identities. Python remains the differential correctness oracle
at both sizes. Historical paired evidence cannot satisfy the new Rust-only gate.

## Shared same-meaning comparator

The successor branch reuses Phase 2's reviewed comparator commit `64179cd1`.
Authentication issuance, market writers, SSE and dispatch remain Python-owned.
`compare.py::compare_responses` is the raw-response profile boundary. Its explicit
`phase2-meaning-v1` branch delegates to private `phase2_meaning.py`, with a
SHA-256-pinned `normalization.json` ledger. It uses exact tagged JSON values,
schema-local decimal/timestamp rules and complete-item multisets inside approved
ties only. The Phase 1 default and existing recording comparator stay unchanged.

## File Map

```
app/
  main.py                       # FastAPI app, exact credentialed CORS, sanitized readiness/provenance, error handlers
  mcp_server.py                 # Read-only market/voyage MCP tools, admission and transport security; exact /mcp ASGI route and lifespan in main (non-production only)
  runtime_server.py             # Staging-only worker-exit diagnostics; delegates CLI handling to pinned Uvicorn
  config.py                     # Pydantic Settings — release/CORS boundaries and pool/upload budget validation
  database.py                   # AsyncSession factory, role/max_connections startup attestation, bounded pooling
  seeds/safety.py               # Opt-in, environment/target-attested seeder connection gate
  models/legacy.py              # Metadata-only legacy FK table stubs excluded from Alembic drift checks
  migration_drift.py            # Explicit PostGIS/legacy exclusions and narrow Alembic comparison callbacks
  admin.py                      # SQLAdmin panel at /admin
  rate_limit.py                 # slowapi Limiter singleton (key=remote_address)
  cli/
    refresh_news.py             # External singleton news-refresh entrypoint (never a web-worker scheduler)
    order_expiry_reminders.py   # Production-only bounded expiry reminder job; no activation in Phase 0
    send_kyc_grace_notices.py    # Operator-only, environment/SHA-bound at-most-once grace notice sender
  core/
    security.py                 # PyJWT + bcrypt — access/refresh/stream token creation, decode_token
  models/
    __init__.py                 # Imports all models for Alembic autogenerate
    user.py                     # User (password/authentication cutoffs, must_change_password), Organization, enums
    kyc_document.py              # Append-only encrypted KYC evidence with RESTRICT ownership
    kyc_execution_grace.py       # Migration-owned grace snapshot and separate durable notice claims
    port.py                     # Port (PostGIS), PortIntelligence, Vessel
    marketplace.py              # InventoryItem, FuelType enum
    orderbook.py                # OrderBookOrder (BID/ASK), Trade, canonical availability_window strings, enums (OrderSide, TradeStatus)
    market_support.py           # Durable staff capabilities, opaque contexts, and exact one-use assisted-listing authorizations
    live_slice_benchmark.py     # Persisted same-side slice VWAP aggregates maintained by explicit rebuilds
    orders.py                   # Commission (legacy match_id + trade_id FKs)
    matchmaking.py              # MatchSuggestion
    watchlist.py                 # Watchlist, typed targets, event feed for Market Radar
    notification.py             # Notification, NotificationType (11 types incl trade events)
    user_preference.py          # Per-user JSON preferences by namespace (market_watch, notifications, tutorial)
    compliance.py               # TraceabilityEvent, ComplianceLedger
    rfq.py                      # RFQ contract JSON, supplier offer JSON, expiry and revision; no acceptance
    supplier_offer.py           # Independent indicative UCOME supplier offers, revisions and idempotency identity
    producer.py                 # ProducerProject (PostGIS, GENA import)
    audit.py                    # AuditLog (JSONB changes, indexed action/resource/timestamp)
  routers/
    auth_simple.py              # JWT auth, registration, admin pre-approved invitations, refresh rotation, password change, /me
    orderbook.py                # Order/listing CRUD, supplier ASK template endpoint, certification guardrails
    market_support.py           # ADMIN capability, authorization, assisted ASK publication, and ETag cancellation APIs
    trades.py                   # Trade lifecycle — create/confirm/decline/deliver/pay + SSE events
    matchmaking.py              # Match suggestions — generate, list, dismiss
    price_discovery.py          # Public price ticker + daily VWAP reference prices
    trade_tape.py               # Public anonymized 7-day confirmed trade tape with exact delivery-point filtering
    curves.py                   # Forward curve data products plus legacy board/table/slice endpoints
    stream.py                   # SSE endpoints — /stream/prices, /stream/orderbook, /stream/trades (durable id/Last-Event-ID replay)
    compliance_api.py           # Compliance scoring — fleet scores, vessel scores, what-if scenarios
    rfq.py                      # Admitted RFQ create/list/detail/cancel and private supplier quote/revise/withdraw
    supplier_offers.py          # Supplier offer discovery and owner lifecycle; never invokes matching
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
    voyage.py                   # Frozen voyage request/result, catalog, slice and price contracts for MCP and the pure engine
    [others unchanged]
  services/
    email.py                   # Shared light-mode email layout, seven message types, admin Reply-To, and Resend transport
    account_approval_email.py   # Transition-scoped Resend delivery and bounded durable retry
    order_expiry_reminders.py   # Per-order/recipient/expiry dedup; frozen email payload and durable retry lease
    event_bus.py                # AsyncIO pub/sub — per-channel queues, 200 subscriber cap, backpressure
    market_event_dispatch.py    # Durable shared SSE transport — outbox sequencer (advisory-lock leader), LISTEN/NOTIFY wake + poll, org-bound hub fan-out (docs/market-event-dispatch.md)
    matching_engine.py          # Match-on-insert — best price, then persisted acceptance order within canonical market identity, partial fills, auto-confirm
    trade_fees.py               # Seller-only plan resolution and public per-MT fee schedule; immutable trade snapshots
    market_support.py           # Authorization digest, ETag parsing, deterministic party locks
    request_party.py            # Immutable actor/effective-party resolution and support mutation allowlist
    market_support_post_only.py # Fail-closed crossing assessment for assisted ASK publication
    benchmarks.py               # External/manual benchmark lookup keyed by market_product + delivery_point + availability_window
    forward_curve_market_slices.py # Canonical public Forward Curve table/slice read models and label policy
    market_slice_filters.py     # Exact slice predicates; compact axis filters for complete grids, tuple filters for sparse keys
    market_signal_ingestion.py    # Trusted signal importer: validate -> verified ingestion runs whose rows classify REAL (see docs/market-signal-ingestion.md)
    live_benchmarks.py          # Explicit VWAP rebuilds and batched read-only calculations from eligible orders
    availability_windows.py     # Canonical availability code parsing, sorting, display labels, legacy alias normalization
    compliance_scoring.py       # Pure function scoring — FuelEU/ETS/CII, 9 fuels, scenario engine
    voyage_fuel_plan.py         # Pure Decimal voyage cost engine; voyage schemas, sea distances, catalog and shared ETS/FuelEU rules; prices/today supplied by caller
    voyage_prices.py            # Batched public ASK/benchmark price resolution and anonymous market slice snapshots; read-only
    sea_distances.py            # Static symmetric catalog-port sea distances, default and supported Cape routes
    audit_service.py            # record_audit() — non-committing async audit logging (call inside the caller's transaction, before its commit)
    audit_actions.py            # THE source of truth for covered audit actions (registry of constants; meta-test forbids string literals at call sites)
    gemini_provider.py          # PID-scoped google-genai client, per-request retry/timeout budget, cancellation-safe worker capacity
    ai_service.py               # Bounded Gemini chat; unavailable providers fail without exposing provider details
    kyc.py                      # Human-review state constant; no external document provider
    kyc_document_vault.py       # Strict key-ring loading and AES-256-GCM envelope encryption
    kyc_document_retention.py   # Account/organization retention conflict checks
    kyc_execution.py            # Organization-bound KYC/grace decision behind KYC_EXECUTION_GATE_ENABLED and its SQL predicate
    execution_policy.py         # Execution admission and shared assisted customer eligibility
    matchmaking.py              # Score-based BID/ASK matching (0-100)
    watchlists.py               # Market Radar helpers: default container, typed targets, slice summaries
    watchlist_events.py         # Slice/pin event emission from order lifecycle changes
    behavioral_analytics.py     # Optional async Umami client, token/aggregate caches, post-commit conversion events
    ci_pricing.py               # Declared lifecycle/energy comparison; unsupported financial benefit remains unpriced
    fame_rfq.py                 # Declared UCOME compatibility, Singapore delivery deadline and quote expiry validation
    fueleu.py                   # Shared Article 4 target schedule for pricing and compliance scoring
  middleware/
    rbac.py                     # require_role() factory — FastAPI dependency for role-based access
    market_support_scope.py     # Fail-closed method/path gate for context-bearing mutations

scripts/phase0/assert_host.py    # Read-only named boundary assertions; no orchestration or journal mutation
scripts/phase0/start_fence.py     # External root-owned operator-lock condition; composes with deploy guard
docs/runbooks/phase0-cutover.md  # One-use manual fence, journal, outbox drain and complete-tuple rollback
scripts/phase0/bootstrap.py      # Empty bootstrap; checked connection owns one DDL transaction
scripts/phase0/production.py     # Root-private operator authority, release-byte binding and production CLI integration
scripts/phase0/database_fence.py # Exact target datacl and source/target session observations; never changes grants
scripts/phase0/manifest.py       # Read-only catalog inspection and frozen source inventory comparison
scripts/run_phase0_postgres_tests.sh # Owns pinned disposable PostgreSQL; no caller database URL
deploy/phase0/                   # Explicit copy mapping and provisional eight-product union contract
tests/phase0/                    # Independently migrated pinned schemas and synthetic source history
  test_host_assertions_postgres.py # Read-only asyncpg assertions; every other session blocks drain
  test_tuple_interruptions.py     # Executable synthetic tuples, killed locks, write fence and rollback
  tuple_fixture.py                # Test-only loaded-unit cache and release executables; not host orchestration
outputs/phase0/task-01.md        # Task 1 acceptance evidence and fixture resume command

tests/unit/                     # Unit coverage (auth, matching, runtime, compliance, events, pricing, schemas)
tests/integration/              # Mutating API tests; skipped unless an attested disposable target is explicit
tests/disposable_target.py      # Numeric-loopback ephemeral-port and exact identity-handshake guard
tests/disposable_server.py      # Test-only ASGI wrapper producing the disposable identity endpoint
tests/monitor/                  # Monitor artifact contract, ownership, and no-activation proof suites
tests/runtime_config.py          # Explicit/validated API target policy (staging tooling; suites gate via disposable_target)
deploy/systemd/                  # Backend, staging Rust reads, and singleton news/prune service-timer pairs per environment
deploy/external_monitor/         # Public/recovery monitoring plus production onboarding-attention timer
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
scripts/verify_deploy_start_guard.py # Isolated exact-state and two-lock backend start guard
scripts/install_systemd_units.sh # Dry-run-default install from pinned immutable unit staging
scripts/verify_systemd_source.py # Exact clean release-ref/commit archive + digest gate
scripts/export_openapi.py       # Deterministic export of the committed openapi.json contract; unit test fails on drift
alembic/versions/               # Migrations incl. canonical availability-window rewrite + runtime metadata alignment
```

## Phase 0 acceptance boundary

`outputs/phase0/acceptance.md` freezes the candidate Python oracle, frontend
identity, union schema and test evidence. Task 9's local migration-test and HTTP
fixture blockers are closed after P0-2; independent sign-off and Linux-only
proofs remain pending. Local tests do not authorize host rehearsal. The existing
Phase 0 runner now checks every Phase 0 suite and local Review Focus case and
records JUnit counts and artifact hashes under its disposable fixture directory.
The frontend two-bundle evidence is reviewed separately, with its explicit PM
exception. Host tools now have a root-private production authority path, proved
on synthetic production-named Linux storage; live activation still requires
independent review and operator approval. See docs/phase0/progress-hosttools.md
for exact interfaces and the evidence boundaries.

## Key Patterns

- **Match-on-insert:** `POST /orderbook` → `db.flush()` → `match_order()` → `db.commit()` (atomic); executable matches now require exact `product + delivery_point + availability_window`. Order and trade create routes build responses before commit; committed replays keep separate return paths, and analytics stays after commit.
- **Supplier ASK invariants:** ASK creation/update requires explicit `certification_declared=true` plus a non-empty `certification_scheme`; `GET /orderbook/my/latest-ask-template` returns safe defaults for the next listing and resets off-spec state
- **Assisted order entry:** When explicitly enabled, separately capability-gated administrators may publish exact post-only BID and ASK orders for an approved real organization. The organization is the economic party and the administrator is the accountable actor; no customer user is selected as a proxy. Orders may be GTC or dated, customer instructions have no artificial age cutoff, and confirmation requires a reference rather than evidence text. Customer/admin cancellation requires the current ETag, support-created orders cannot be edited, and public serializers omit support attribution. See `docs/market-support-assisted-listings.md`.
- **Assisted organization context:** The backend binds an approved real organization to the authenticated administrator through one opaque, short-lived database context. The normal `/orderbook/my`, order creation, and canonical cancellation routes resolve an immutable request party from `X-Verdaxis-Market-Support-Context`; an early deny-by-default middleware rejects every unclassified context-bearing mutation.
- **Assisted-context hardening:** Context mutations take deterministic capability/context locks and capability revocation atomically ends active contexts with audit. Final confirmations participate in context-mode idempotency, while instruction reference, time, and acknowledgement facts are persisted. Canonical delivery-point and post-only rules remain enforced. Context-invalid responses carry stable detail codes and `X-Verdaxis-Market-Support-Context-Invalid`. Credentialed CORS wraps early request rejection paths and exposes retry, request-correlation, and support-context invalidation headers to browsers.
- **SSE broadcasting:** `event_bus.publish(channel, event_type, data)` → subscribers via AsyncIO queues; order/trade payloads are append-only enriched with market source/scope/demo provenance. Private market lifecycle events additionally flow through the durable `market_event_outbox` → sequencer → hub pipeline, so delivery and `Last-Event-ID` replay survive worker restarts and cross Uvicorn workers (docs/market-event-dispatch.md; prune policy documented, nothing armed)
- **Compliance scoring:** Pure function `calculate_compliance_score()` — no DB, 100% testable
- **JWT auth:** 15-min access + 7-day refresh, plus 60-second `type="stream"` tokens from `/auth/stream-token` for SSE query-param auth. Ordinary API auth only accepts access tokens; activity SSE query auth only accepts stream tokens.
- **Authentication locks:** Only audited normal GET route templates share the user-row lock. Mutations, unclassified routes, and all assisted-context requests keep exclusive locks. Account/session writes still conflict with shared readers. Password login returns the same sanitized profile as `/auth/me` with its access token.
- **Authentication revocation:** Password changes invalidate tokens through `password_changed_at`. Account rejection advances the separate `authentication_revoked_at` cutoff and revokes every refresh session under the locked user row. Rejected owners retain only the narrow authenticated cleanup path; approval does not revive tokens issued before rejection.
- **Password workers:** Async authentication paths offload bcrypt through one four-slot process-local limiter. Queued requests can cancel before admission. Admitted work retains its slot until completion, even if the caller cancels; synchronous maintenance tools retain the synchronous helpers.
- **AI provider bounds:** Chat and news classification use the pinned `google-genai` adapter and one client per process. A monotonic deadline starts before thread scheduling and covers SDK retries. Each request carries its own timeout options; cancelled or timed-out callers do not release capacity while the provider thread still runs. Automatic function calling is disabled.
- **Cookie-backed refresh:** refresh token is also rotated through an HttpOnly `refresh_token` cookie scoped to `/api/auth`, while access tokens remain bearer tokens
- **Account-side organization binding:** New-organization registration, domain-derived join requests, membership approval, and admin pre-approved invitations enforce the same account-role to organization-type boundary in the backend: supported buy-side organization types belong to `BUYER`; `FUEL_SUPPLIER` belongs to `SUPPLIER`.
- **Account approval email:** The canonical administrator account-approval route persists the exact pending status-transition UUID, immutable provider payload, and due time with account status and audit state. After commit, delivery briefly locks and revalidates the user, then commits a five-minute claim in the existing retry timestamp before provider I/O. It replays the frozen payload with the transition UUID as Resend's idempotency key, then re-locks and finalizes only the same transition and claim. A crashed worker leaves a retryable claim; failures move the due time forward by one hour so one bad recipient cannot starve the queue. Rejection invalidates pending approval mail without waiting for an in-flight send, whose completion cannot clear a newer transition. Idempotent re-approval does not enqueue again, and email failure cannot roll back admission. Organization, membership, KYC, and trading-access approvals remain independent gates.
- **Admin pre-approved invitations:** An administrator may prepare a `BUYER` or `SUPPLIER` account in an existing approved `REAL` or `UNKNOWN` organization, or atomically create a `REAL`, onboarding-approved organization and an invited account. The audited administrator action is the trust decision for a new live-market organization; ordinary registration still receives database-owned `UNKNOWN` provenance, and a later administrator company-approval transition automatically classifies ordinary UNKNOWN organizations as REAL. The database trigger preserves synthetic identities and direct provenance-write restrictions; the company approval audit records the classification. Previously approved UNKNOWN records still use exact operator remediation. Role/type validation and owned-domain collision checks happen before persistence; any account or organization conflict rolls back the entire operation. The seven-day claim secret reuses the existing hashed one-time password-reset slot, while the unclaimed state is explicitly `APPROVED + unverified + must_change_password`; acceptance records possession of the administrator-delivered secret as email verification, records Terms/Privacy agreement in the append-only audit trail, progresses referral attribution, and issues the normal device-bound session. Ordinary password reset excludes unverified accounts so the two token purposes cannot be confused. See `docs/plans/2026-08-05-admin-preapproved-invites-design.md` and `docs/plans/2026-08-12-admin-created-organizations-design.md`.
- **Rate limiting:** slowapi per-route (5/min login, 3/min password, 60/min prices, 30/min reference)
- **Market request admission:** Orderbook and trade commands cap the aggregate JSON request body at 64 KiB before body parsing; larger bodies return HTTP 413. Selected public market GETs and MCP database tools share one per-worker limiter created in `main.py`, with capacity `max(DB_POOL_SIZE + DB_MAX_OVERFLOW - 1, 0)`; the in-memory MCP catalog bypasses it. This reserves one database connection per worker against those reads; a busy gate returns HTTP 503 with `Retry-After: 1`, while private reads and commands bypass the gate.
- **Availability windows:** Persist canonical codes (`SPOT`, `YYYY-MM`, `YYYY-QN`, legacy-compatible `YYYY-CAL`); UI-relative labels like `M+1` must be resolved before persistence
- **Managed demo book:** The four alcohol and two scalar biofuel demo markets share one UTC pricing clock and ten distinct bids and asks per port/window across the five-year horizon. Spot anchors stay fixed; forwards add a disclosed synthetic 3% annual carry on an ACT/365 basis. Bootstrap and periodic refresh use the same managed coverage; activity adds matched history only. Managed demo-only curves use the current book midpoint while retaining historical evidence. UCOME B100 has no synthetic seed liquidity. Refresh preserves order identity and pins; pruning retains linked customer and trade history.
- **B100 shared orderbook:** UCOME B100 is ORDERBOOK-enabled on the existing Singapore lane, with typed side-specific `fame_terms`, shared execution compatibility and immutable trade/negotiation snapshots. Marketplace, Map, price evidence and forward curves use the same catalog identity. Prior indicative supplier offers/RFQs retain their original meaning and are not converted into executable orders. The deployment checkpoint is `fame_20260922_b100_orderbook`.
- **Green-fuels market model:** Matching and live slice benchmarks key on `side + market_product + delivery_point + availability_window`; supplier sustainability/compliance fields stay out of the hard market key
- **Orderbook read performance:** List responses reuse joined product/delivery-point rows and eager-load organization data. Benchmark inputs are fetched once for all requested slices, including cached missing results within the request. `/orderbook/map-summary` returns all eligible compact groups and the latest ASK per delivery point; `/orderbook/product-counts` uses the public listing filters for canonical product totals.
- **Market provenance contract:** Market-data responses use shared `source_kind`, `scope`, and `demo_status` fields. Aggregate data exposes real/demo/unknown counts; unknown contributors remain `UNKNOWN` rather than being collapsed into real/demo/mixed.
- **Forward Curve monitoring:** `/curves/forward/table` and `/curves/forward/slice` use `forward_curve_market_slices.py` as the canonical public read model for approved `market_product + delivery_point + availability_window` slices. Products aggregate by canonical market product, delivery points are restricted to the approved trading ports, and public cells expose server-owned label policy plus redacted source/demo/staleness fields. Complete grid lookups use compact axis predicates; sparse selections keep exact tuple predicates. Table projection validates cell attributes without serializing nested detail. Focus reads select trusted REAL evidence before applying their limit and use DEMO only when no REAL result exists. `/curves/forward/board` remains for older clients.
- **Real signal ingestion:** `scripts/ingest_market_signals.py` (staging-guarded, dry-run default) + `services/market_signal_ingestion.py` produce verified `market_signal_ingestion_runs` whose rows satisfy the trust predicate in `forward_monitoring.py` and render as REAL. CSV formats, staleness semantics, redaction invariants, and rollback SQL: `docs/market-signal-ingestion.md`.
- **Price discovery provenance:** `/prices` 24h summaries classify confirmed trade buckets as `CONFIRMED_TRADE`, `DEMO_SEED`, `MIXED_SOURCE`, or `UNKNOWN` using the same demo organization rules as trade tape.
- **Trade tape scope:** `/trade-tape` returns anonymized confirmed trade prints for the last 7 days. Exact delivery-point history is available only when clients filter by `delivery_point_id` and entries return `scope="DELIVERY_POINT"` with delivery-point fields. `provenance_kind` is legacy-compatible; new clients should prefer `source_kind`/`demo_status` when present on newer surfaces.
- **Market Radar watchlists:** Watchlists are observer-only. Typed targets store either canonical slices or pinned order snapshots, and order create/update/cancel paths emit slice/pin events without feeding core matchmaking. New event payloads carry provenance at emission time; legacy events return `UNKNOWN` rather than doing response-time order lookups.
- **Behavioral analytics:** Umami is an optional failure-isolated dependency. `GET /admin/analytics/product-usage?days=7|30|90` combines bounded Umami aggregates with authoritative UTC-period database counts. Server conversion events and the documented browser reporting taxonomy use separate allowlists. Registration, organization, order, and trade events are scheduled only after commits, carry bounded originating request metadata for Umami bot classification/environment attribution, and use a strict property allowlist; collector drops/failures never alter endpoint response contracts. Aggregate successes cache for at most five minutes and failures for at most 30 seconds. `totaltime / visits` is exposed only as average session duration, not active engagement. See `docs/behavioral-analytics-contract.md`.
- **Live runtime topology:** Production binds `127.0.0.1:8000` on the EU host; staging binds `127.0.0.1:8001` on the shared host. Each host's reverse proxy fronts only its live local backend. Both services require network-online and PostgreSQL, use four workers, and preflight the exact migration checkpoint published with the release rather than a later source head. News refresh has exactly one external owner per environment: the environment-specific six-hourly systemd timer invokes `app.cli.refresh_news`, while a PostgreSQL transaction advisory lock rejects overlap. Production prefers one bounded licensed Webz.io news request and falls back to maritime RSS; staging normally uses RSS only. Open-web news never enters trusted market-signal or benchmark ingestion. Application workers create no scheduler and the public API exposes no manual refresh mutation. Product-analytics pruning likewise has one timer owner per environment; it requires PostgreSQL/network readiness, uses bounded boot retries and the news sandbox, and must match that environment's `.runtime-release.env` identity before database access. Backend units allow a normal start only when `.runtime-deploy/<environment>.state` is absent. During deploy restart, their isolated stdlib guard requires exact `restart-authorized` environment/SHA state and verifies that both the main deployment lock and restart-window lock are busy. News-refresh and product-analytics-prune units reject every present state path, including a dangling symlink. The production-only order-expiry-reminder unit and both authentication-maintenance units retain their existing conditions. Authentication maintenance also loads `.runtime-release.env` and preflights the effective `ENVIRONMENT` before execution. These guards govern new service starts; they do not stop or drain an already-running process.
- **Runtime budgets:** Production and staging budget connections independently against PostgreSQL on their own hosts. `configured_connection_total` includes every configured app pool, one dedicated SSE LISTEN/leadership connection per worker, and the maintenance reserve; the source formula must not be read as evidence that both live services share one database. KYC uploads are bounded at 10 MiB per file and 20 MiB aggregate by default. The systemd source sets `MemoryHigh=768M` and `MemoryMax=1G`; dated measurements are evidence, not a current capacity claim.
- **Runtime identity and boundaries:** The canonical Phase 0 release uses exactly production database `verdaxis_prod` (the preserved old release still requires `verdaxis`), app `verdaxis_app`, migrator `verdaxis_migrator`; staging is exactly database `verdaxis_staging`, app `verdaxis_app_staging`, migrator `verdaxis_migrator_staging`. Both deployed environments reject SQLite, default JWTs, auth bypass, URL query routing, different app/migrator endpoints, shared roles, absent credentials, and empty/decoded-whitespace/default passwords. Canonical deploys fix branch/state/tool paths, discard ambient Git/database/Python routing controls, and run database-bearing helpers with a minimal environment. Deploy uses an exact dry-run-approved SHA, serializes each environment with a durable main flock before live revision verification, acquires a dedicated restart-window flock while state is blocked, and writes restart authorization only after both locks are held. Both descriptors remain open until exit, and durable state remains through restart/readiness; later failures keep code/identity aligned and fail closed. Crash or reboot releases the locks, so stale restart authorization cannot permit another backend start. The initial unit transition follows the stop-first timer and bootstrap order in [runtime hardening](docs/runtime-hardening.md). Installed backend units reference the helper by absolute path and fixed CLI, so forward reverts preserve that ABI until a compatible unit transition. Production/staging migrations additionally bind that SHA, an exact expected current revision, and one literal target to a committed checkpoint allowlist; missing approval, aliases such as `head`, unexpected live state, and unlisted graph traversal refuse before source mutation. Checkpoint application pins Alembic to the explicit migrator URL; exact-revision startup verification never substitutes `DATABASE_URL`. Post-migration ACL convergence runs from a digest-attested committed bundle before restart, opens `.env` with no-follow regular-file checks, ignores ambient URL overrides, and executes transactionally as the migrator. Dependency installation resets restrictive caller umasks and leaves the virtualenv read-only/executable to isolated maintenance identities. Strict readiness is exactly `status=ok`, `db=ok`, environment, and full SHA. Credentialed CORS uses exact per-environment origin allowlists.
- **Database authority split:** Runtime and Alembic separately attest `current_database()`, `current_user`, exact role properties, and absence of memberships. The migrator owns the database, public schema, and application tables/sequences. Expanded database/schema/default ACL rows must exactly equal the migrator-owner, app, and backup allowlist including grantor and grantability; bootstrap revokes every unrelated explicit and delegated grantee with intentional deterministic `CASCADE`. A central governed-object relation covers every ordinary table, child partition, and sequence; object and `pg_attribute.attacl` column grants, including PUBLIC, are rebuilt from zero before exact declarative app/backup grants. The runtime app receives authority only for named tables, privileges, and organization columns. It may insert organization provenance so an authenticated admin invitation can create a new `REAL` organization, but it cannot update provenance or promote an existing organization. Ordinary signup omits provenance and retains the database-owned `UNKNOWN` default. Audit and status history are append-only (`SELECT, INSERT`). Migrator defaults give backup read-only table/sequence access. `alembic_version`, seed/quarantine controls, market evidence/provenance, operator/control tables, and provenance updates are not app-writable. Validation proves app/backup cannot retain write via column grants or delegate authority. Object ACLs exclude `alembic_version`, `spatial_ref_sys`, and extension-owned objects.
- **Integrated release gates:** The staging graph preserves runtime metadata, identity/device security, quarantine, integrity, SSE, assisted-listing, FAME/UCOME B100, authentication-cutoff, acceptance-priority, and command-receipt pauses in its separate checkpoint policy. Promotion remains one reviewed literal checkpoint at a time. Exact ACL convergence keeps pending registration, organization joins, refresh sessions, KYC/admission, signal ingestion, and other protected records outside undeclared app authority. The manifest-driven unit installer accepts only audited entries and bytes. Four Uvicorn workers and durable SSE transport remain part of the release. Forward reverts must preserve the authentication cutoff, acceptance priority, and command receipts; identity-only and database downgrade rollback remain forbidden.
- **KYC trust boundary:** KYC sends no documents to external providers. Every submission remains pending and retains two encrypted document rows with its audit. ADMIN downloads commit a view audit before returning bytes. A DeepSeek V4.1 Flash advisory check on a non-PRC host is planned, not built. Only trusted administrator approve/reject routes may change KYC/account status. The removed legacy compliance and dashboard routes stay unmounted.
- **Local operational monitoring:** `deploy/monitor` is a source-only contract, not installed-state evidence. Its current dual-loopback reader assumes both backends are on one host, so it must not be installed unchanged across the split topology. It owns no readiness producer, deploy transaction, backup producer, root installer, or activation path. The exact four-key readiness corpus, hostile-JSON handling, private status, independent demo ownership, and retirement gates remain source contracts. The installed shared-host external monitor uses a reviewed selective patch process described by `/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md`; source presence does not authorize replacement. Full owner seams: `deploy/monitor/README.md`.
- **Onboarding attention:** A production-only five-minute oneshot reads canonical account, organization, join-request, approval-transition, and login state to alert operators about actionable onboarding stages. Demo/test/canary/admin accounts are excluded; persisted dedupe state contains no email or organization data. Initial activation silently baselines historical cases, while new stages alert once and non-baselined recoveries notify once.

## KYC execution admission

New order creation/amendment, inventory publication, taking, matching and confirmation require organization-bound APPROVED KYC or unexpired migration-enrolled grace. REJECTED overrides grace. Assisted orders require at least one admitted customer member; staff KYC cannot substitute for customer eligibility. Exact owned command replays precede the KYC check. Cancel, decline, deliver, pay and retract retain authentication/ownership checks without KYC admission. Indicative RFQs, negotiations and offers retain their prior admission policy. The exact allowlisted demo-to-demo matching path is unchanged and has no fabricated KYC.

`kyc_execution_grace` is runtime SELECT-only. Migration `kycg_20261014_staging` snapshots eligible existing users for fourteen days. Uploads do not renew grace; changed membership cannot use another organization's row. `/api/auth/me` and login profiles expose `kyc_status` and `execution_eligibility`; public order and trade-action projections share the SQL rule.

The rule is behind one switch, `KYC_EXECUTION_GATE_ENABLED` (default true, fail safe). It is read only in `app/services/kyc_execution.py`: `kyc_execution_eligibility` and `kyc_execution_clause` (with the `advisory_kyc_allows` / `advisory_kyc_clause` mirror) and every layer above them — route gates, `execution_party_is_eligible` / `market_party_is_admitted`, the assisted customer-organization helper, matching, public order visibility and the trade action-required projection — follow it without their own gate checks. With the gate off (Private Beta on prod and staging) the EXECUTION policy follows the pre-gate advisory rule: only `REJECTED` or a review bound to another organization blocks, and an eligible user reports the new `KYC_NOT_ENFORCED` reason with no grace deadline; a blocked result keeps `KYC_REJECTED` or `KYC_REQUIRED`. Non-KYC resting orders show and match again. The lifespan logs one line (`KYC execution gate: enabled|disabled`), and `app/cli/send_kyc_grace_notices` refuses with a non-zero exit. The migration chain, ACLs, grace rows and admin KYC routes are unchanged. See `docs/security-hardening-v2.md`.

The operator runs `python -m app.cli.send_kyc_grace_notices --environment staging --release-sha <full-sha>` with the deployed runtime environment loaded. No timer or web worker sends notices. A separate `kyc_grace_notices` claim is committed before provider I/O. Successful sends record `sent_at`; failed or interrupted claims are deliberately not retried automatically, so the operator must review uncertain delivery rather than risk duplicate mail.

KYC follows `blng_20261014_staging` in the combined migration chain and checkpoint policy.

## Bio-LNG shared execution

`BIO_LNG` is an ORDERBOOK product on the Rotterdam lane only, priced in USD/MT
with a 1 MT order minimum and no synthetic seed liquidity. `schemas/lng_order.py`
defines side-specific declarations and requirements. `services/lng_order.py`
owns validation, the eight compatibility rules, scalar derivation and redaction.
Its posting-admission helper rejects reseller ASKs with HTTP 422
`RESELLER_ALLOCATION_REQUIRED` under the 14 October 2026 founder decision until
the verified supplier-allocation tool replaces the block. Create, amend and
assisted authorization share this check; BID terms and matching are unchanged.
`reference/iso_countries.py` provides the 249 assigned ISO alpha-2 origin codes.

Orders, assisted authorizations and negotiations retain typed LNG terms; trades
freeze `lng_terms_snapshot` under the canonical immutable-snapshot trigger.
Public orders and pending trades omit certificate numbers and physical supplier
names. Confirmed counterparties receive the full snapshot. Direct takes accept
`lng_terms` and return HTTP 409 `LNG_TERMS_INCOMPATIBLE` without party detail.
Inventory publication is refused; suppliers post asks directly. Staging retains
the inventory terms storage column for parity with its other product terms.

Supplier scalars come from the declaration. WTT emissions remain in `lng_terms`,
not lifecycle CI; CI pricing skips Bio-LNG. Public reads and live benchmarks use
the Rotterdam lane and declared terms. FUEL_TRADER organizations may receive
SUPPLIER accounts by admin invitation; public signup and domain joins remain
restricted to the existing signup types. Voyage planning remains out of scope.
Deploy checkpoint: `blng_20261014_staging`; downgrade refuses stored LNG history.

## Production scalar biofuels (Phase 0)

The canonical catalog has eight products. Production B30 (`c4cecebc-3d2d-5840-8021-57a9a11bc673`)
and B100 (`74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5`) remain separate scalar
Biofuel products with a 200 MT order minimum. They are not UCOME B100.
B30 requires ISO 8217:2024 RF 380; B100 requires ISO 8217:2024 DFA and excludes
HVO. Shared execution qualification and its SQL mirror require whole-fuel
lifecycle CI metadata, certification, MSDS, feedstock and origin. Zero CI is
valid; missing CI is not. Suppliers fill scalar biofuel BIDs through qualified
ASK matching, not an undeclared direct take. Public reads use the same rule.
Production demo definitions remain labelled synthetic and add no LNG/UCOME prices.
Revision `p0_20261016_union` follows `kycd_20261015_staging`, adds missing canonical
reference rows, extends four product checks and freezes the union trade guard.
The copy manifest pins its complete schema. Empty bootstrap uses the checked
connection and exact graph/schema/reference hashes; host apply remains disabled.
See `docs/phase0/progress-task4b.md` for acceptance and remaining host gates.

## UCOME B100 shared execution

The canonical UCOME B100 ID remains `e561e43f-d9b2-598e-981c-f1d28d515ddc`. Its execution mode is `ORDERBOOK`; the existing wholesale Singapore lane and 1 MT initial order minimum remain. Standard fractional fills use the same rules as other fuels. No synthetic UCOME B100 liquidity is seeded.

`app/schemas/fame_order.py` defines BID requirements and ASK declarations within the standard order contract. `app/services/fame_order.py` validates current declarations, compares standard/edition/grade, cold-flow limits, comparable CI and certification/evidence requirements, and forms immutable trade snapshots. Matching and direct taking use the same checks. B100 direct takes bind the reviewed order version under the row lock; changed offers require a fresh review, while committed idempotent requests remain replayable. Inventory and assisted publication carry the same terms. Negotiation acceptance remains disabled for all fuels under the existing policy.

Public order responses redact private certificate, batch, site and documentary references; owner and authorized trade responses retain them. Unknown measurements remain null, zero remains zero, and declared evidence is not platform verification. Operator certificates must be current at execution; availability windows remain matching buckets, not promised delivery dates. Required batch/consignment evidence has an explicit loading/delivery milestone.

Market-data queries enforce the catalog lane and current B100 declarations. The curve shows B100 with no price data when appropriate. Product/port/window aggregates can combine different specifications, so the B100 midpoint is labelled as a reference and does not claim executable compatibility. Individual B100 orders remain executable through the shared trading path.

Historical `supplier_offers` and RFQs preserve their commercial terms, revisions, private visibility and withdrawal/cancellation semantics. They do not reserve inventory or become standing orders. New UI order entry uses the standard order API.

Deploy through `fame_20260922_supplier_offers` to the literal checkpoint `fame_20260922_b100_orderbook`. Exact runtime column grants cover the new terms; database checks enforce side/version/lane and immutable snapshots. Downgrade refuses to remove stored B100 execution or signal history.

FuelEU targets use the shared regulatory schedule. Lifecycle CI reductions do
not imply ETS cash savings. Unprovided in-scope combustion emissions produce
null ETS costs and an `UNPRICED` status; prototype overlay financial adjustments
remain zero until the necessary eligibility and annual balance inputs exist.

## Revenue Streams

1. **Transaction fees** — buyers are free; new trades snapshot the seller's per-MT rate and plan. Defaults are Pilot $2/MT and Professional $1.50/MT, with configurable future rates and negotiated Enterprise rates. Delivery uses the stored rate and final quantity; legacy trades retain their stored percentage basis. See `docs/transaction-fees.md`.
2. **Compliance SaaS ($200-500/vessel/mo)** — /compliance/fleet, /compliance/scenario
3. **Data products ($1K-5K/seat/mo)** — /prices/reference (daily VWAP)
4. **Platform analytics** — /admin/analytics/overview, /admin/analytics/daily, /admin/analytics/product-usage

## Reviewed trading and snapshot contracts (2026-10-03)

`services/order_terms.py` binds a direct hit to the selected order's exact terms.
New requests compare `expected_terms_digest` after market and row locks; missing or
changed terms return `ORDER_TERMS_REVIEW_REQUIRED` before reservation. Successful
same-key replay returns the stored trade before current-term validation.

`GET /orderbook/snapshot` returns at most 15 individual orders per side for one
canonical product, delivery point and window. A fresh repeatable-read transaction
and one captured UTC time govern depth, expiry and quote references. Ranked depth
and batched references use two data statements. The response reports generation
time, scope and demo provenance; it does not represent confirmed trade prices.

Orders persist `acceptance_ordinal`, allocated after the market lock. Price, ordinal
and UUID determine queue priority. Price changes, quantity increases and broader
execution terms reset priority; reductions and fills retain it. Migration
`obp_20261003_acceptance_priority` follows `auth_20261002_session_cutoff`, backfills
legacy rows deterministically and grants the runtime role sequence USAGE only.
The 100-candidate matching limit remains an atomic HTTP 409 rejection.

Private event replay pages authorized rows through a captured high-water mark and
merges ordered live events. Global sequence holes are valid. Queue overflow closes
the connection with an explicit full-REST-resync contract. Payloads carry schema
version 1. Committed public changes send a sanitized `market_invalidated` signal
through the existing dispatcher and public hubs. Private payloads omit the internal
marker; public signals contain no identity, economics or private cursor. REST
refresh, polling and reconnect remain authoritative; this is not an L2 delta feed.

Confirm, decline, deliver, pay, amend and cancel accept an optional idempotency key.
The six command receipts use actor-scoped idempotency locks; create and publish flows use organization-scoped locks.
Current authentication and actor context are checked before replay. Successful
results bind the actor, organization, support context, target and request hash;
they preserve the original status and response after later state changes. Reusing
the same operation/key with a changed request, effective organization, support
context or target returns HTTP 409. Idempotency-lock contention returns HTTP 503;
clients retry the unchanged request. The receipt commits with the economic effects
and outbox. Runtime access is SELECT
and INSERT only; receipts are retained. Migration `rcp_20261003_command_results`
follows acceptance priority. Forward corrective releases must retain receipts.
