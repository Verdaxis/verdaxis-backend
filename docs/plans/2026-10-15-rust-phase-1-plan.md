# Rust Phase 1 Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` or `superpowers:subagent-driven-development` to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax. This document is a plan, not permission to deploy.

**Goal:** Port the staging public read API and MCP to Rust with zero unapproved contract differences, at least 50% lower measured route-service memory, and no p95 latency regression.

**Architecture:** Add a Cargo workspace in `rs/`. Keep Python as the writer and Alembic as the only schema owner. Caddy selects exact public methods and paths for Rust; all other traffic stays on Python. Both processes use the existing app role. One canonical deployment transaction controls both services.

**Tech Stack:** Rust 1.95, axum 0.8, Tokio, tower, SQLx 0.9, rust_decimal, serde, tracing JSON, utoipa, and rmcp stateless streamable HTTP. Pin resolved dependency versions in `Cargo.lock`. Verify the proposed SQLx and rmcp releases in Task 1; do not claim that a locally installed Rust compiler proves crate compatibility. PostgreSQL 17/PostGIS 3.6 remains the database family (`ARCHITECTURE.md:3`).

**Spec:** Read all of `/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/Verdaxis Rust Backend Rewrite - Design - 2026-10-14.md`. Read sections 1–5 and 8 of `/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/research/phase0-convergence-analysis.md`. The founder's 15 October instructions supersede the design's start sequence and memory baseline. Use `docs/plans/2026-10-14-voyage-mcp-plan.md:1-38` as the accepted plan format.

## Global Constraints

- Start from staging commit `b218ab264b92f99007bacc05d4871413e024b72a` (merged into the build branch). Reconcile later Phase 0 changes before each route cutover.
- Phase 1 can proceed beside Phase 0. This plan contains no production task.
- Freeze feature changes in each module during its port. A parity repair is not a new feature.
- **Python freeze (founder, 15 Oct, tracker decision 47):** no new features in Python at all. Allowed Python changes: Phase 0 convergence; security fixes, user-facing bug fixes and data repairs; operations and settings. New features wait for Rust. Two stacks are transitional; the design's phase 10 removes Python.
- **Pilot pace (founder, 16 Oct):** Verdaxis is in a pilot with no real money at risk, and the founder wants Rust in production before the pilot ends. Prefer parallel lanes and folded fix rounds. Keep these gates at full strength: privacy and KYC redaction, zero contract differences on the populated corpus and the differential simulation, backups before any prod change, and the one-change Caddy rollback.
- Retain distinct B30, B100, and UCOME_B100 identities. Do not rename or merge products. Do not insert B30/B100 into staging as a side effect of this port.
- Rust catalog and order-book selection must use reviewed data. Do not put product names, product UUIDs, or product-code branches into Rust source or SQL literals.
- Keep Alembic as the only schema owner. Do not introduce SQLx migrations, database views, functions, or new grants for this port.
- Never widen `deploy/postgres/app_acl_policy.sql`. Runtime SELECT grants already cover products, ports, news, orders, trades, and KYC grace (`deploy/postgres/app_acl_policy.sql:22-87`).
- Preserve four Python workers. Do not claim memory savings by reducing them (`deploy/systemd/verdaxis-backend-staging.service:18-23`).
- Do not port authentication, writes, SSE, timers, ingestion, benchmark administration, or matching. Python retains the dispatcher (`app/main.py:108-128`).
- Do not proxy Rust route implementations to Python. That would not demonstrate a port or its memory benefit.
- Do not contact live hosts or databases while implementing local tasks. Obtain separate operator approval for every live checklist, including a dry run (`CLAUDE.md:46-47`).
- Run integration tests only against an attested numeric-loopback disposable target. Never weaken their collection guard (`tests/disposable_server.py:32-45`; `README.md:266-289`).
- All implementation paths below are **proposed new paths**, unless marked Modify. Current-source claims use this worktree's `file:line` references. Estimates and recommendations are not live observations.
- Keep temporary evidence in `work/rust-phase1/`; Task 1 adds `/work/` to `.gitignore` so it is never committed. Keep the progress file and final reports in `docs/rust-phase1/` (committed with the code). Never commit database credentials, live bodies, or live query strings.
- Every implementation commit must be independently reviewable. End commit messages with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

These five failures are not covered by the initial happy-path port tests. Each owning task below adds the named regression.

1. **An offset beyond the last ASK still needs the true total.** Test `ask_offset_past_end_keeps_total` in Task 7b. The window-count query has an explicit fallback (`app/routers/orderbook.py:1003-1012`).
2. **A public read produces a digest that Python rejects on a later take.** Test `private_terms_digest_matches_python` in Task 6. Compute the digest before redaction; never expose its private input (`app/services/order_terms.py:49-122`).
3. **Cancellation releases capacity before database work stops.** Test `cancelled_query_keeps_slot_until_drop` in Task 3. Test REST and MCP against the same gate. Python holds admission until the ASGI call exits (`app/middleware/public_market_read_admission.py:104-113`).
4. **An old binary claims a new source SHA after a failed deployment.** Test `wrong_binary_refuses_new_release` in Task 12a and transaction failure retention in Task 12b. Bind compiled identity, artifact digest, runtime identity, and both readiness results before clearing state (`scripts/deploy.sh:619-649,685-712`).
5. **A midnight or grace-expiry boundary becomes a false parity pass.** Test `clock_boundary_changes_are_not_normalized` in Task 4. Never remove expiry, staleness, or evidence timestamps from comparison (`app/services/market_data_eligibility.py:230-274`; `app/services/kyc_execution.py:20-30`).

## Design corrections and decisions

1. **Route count:** The seven named groups have **26 public GET routes**, not about 40. Include their existing ASK compatibility route, `/api/listings`, for **27 public data routes**. There are also **3 health GET routes** and **3 MCP transport method/path pairs**. Total planned method/path pairs: **33**. MCP has three tools, not three separate mounts (`app/main.py:152-157`; `app/mcp_server.py:107-142`). The inventory below is the scope, not every public route in the application.
2. **Readiness failure:** Four keys apply to success. Current failure is HTTP 503 with exactly `{"status":"error","db":"unavailable"}`. Preserve this two-key failure; do not add release identity on failure (`app/main.py:329-363`). The prose in `docs/runtime-hardening.md:490-496` is stale on this point.
3. **Revision authority:** The app cannot SELECT `alembic_version`. Convergence revokes its control-table grants and restores backup SELECT, not app SELECT (`deploy/postgres/converge_runtime_object_acls.sql:54-87`). Retain the separate migrator-backed `ExecStartPre` check. Add Rust's compiled revision allowlist check against the exact attested `MIGRATION_REVISION`. Do not give the HTTP process a migrator connection. This replaces the design's literal suggestion that the Rust app role reads that table itself.
4. **Catalog is not fully stored in the database:** Code, lane, and execution mode are Python properties over constants (`app/models/catalog.py:23-38`; `app/market_catalog.py:25-36,47-112`). Export a source-attested, versioned policy data file from the approved Python tree. Load it in Rust. Join it to actual database rows using the same strict identity tuple. This avoids a schema change and arbitrary active-row admission. Phase 0 owns the B30/B100 policy additions. Regenerate this file and re-record parity when Phase 0 changes its source.
5. **Reseller block is a write admission rule, not a public-read predicate.** The block returns 422 `RESELLER_ALLOCATION_REQUIRED` (`app/services/lng_order.py:49-57`). The current read predicate checks lane, side, expiry, and off-spec status, not `seller_is_physical_supplier` (`app/services/market_data_eligibility.py:266-273`). Copy current reads exactly. Do not silently hide historical rows. If this is a policy defect, fix Python under separate approval, then refresh both contracts before cutover.
6. **Curves overlap Phase 5:** Port the five existing public curve routes and the read-only benchmark calculations they call now. Leave `/api/benchmarks` ownership and all ingestion on Python. Do not defer curve dependencies merely because Phase 5 also says “forward curves” (`app/routers/curves.py:850-962`; `app/main.py:312`).
7. **Integration target:** The suite is not pointed at live staging Caddy. It runs unchanged against a disposable Caddy, whose identity endpoint remains served by `tests.disposable_server` (`tests/disposable_server.py:48-73`).
8. **Memory baseline:** Founder-supplied measurements on 15 October are production 754 MiB steady, 807 MiB peak, cap 1 GiB, four workers at 108–261 MiB, about 6% of one core; staging 642 MiB. These are context, not a route-level benchmark. The older 530–538 MiB figure is historical (`docs/runtime-hardening.md:46-50`).
9. **Approved difference AD-01 — rate-limit counts:** The PM defines the contract as 60/30/10 requests per minute per client on prices/reference/export, enforced once per client and route. Python's four process-local counters can admit up to four times those counts; that is an implementation artifact (`app/rate_limit.py:16-33`; `deploy/systemd/verdaxis-backend-staging.service:18-23`; `app/routers/price_discovery.py:381-392,551-566,603-616`). Rust enforces the documented limits exactly. Do not change Python or multiply Rust limits by four. This is the **one approved, listed contract difference**. Exclude threshold counts and the resulting accept/reject sequence from golden/shadow parity. Retain exact 429 body/header and client-key tests. Rust-only burst/reset tests prove the boundary. All other differences remain failures.
10. **Approved difference AD-02 — admission capacity (PM, 15 Oct):** Rust's single public-read gate has 8 slots, equal to Python's 4 workers x 2 slots. Rust never refuses a request that Python's aggregate capacity admits; Python's per-worker gates can refuse earlier under uneven load. Tested in Task 3 and reported in Task 14.
11. **Admission owns the database work (PM, 15 Oct, after the Task 3 stop):** SQLx 0.9 returns a dropped `PoolConnection` in a spawned task that pings first (`sqlx-core/src/pool/connection.rs:199-209,307-325`), so a permit held only by the request or body can be released while the database is still busy. Contract:
    - Two pools: `read` (max 8, equal to the AD-02 admission slots) and `health` (max 1, readiness only). The total stays 9, so the Task 2 budget is unchanged.
    - Every admitted unit of database work, REST and MCP tools alike, goes through one function in `runtime/admission.rs`: `pub async fn run_admitted<T: Send + 'static>(state: &AppState, work: impl for<'c> FnOnce(&'c mut sqlx::PgConnection) -> futures::future::BoxFuture<'c, Result<T, ApiError>> + Send + 'static) -> Result<T, ApiError>`. It takes the permit with the plan's existing admission rule, then spawns a Tokio task that owns the permit and the pooled connection. The request future only awaits the task, so a client cancellation never cancels a query in flight; `statement_timeout` (30 s) bounds it.
    - On success the task calls `ping()` on the connection (this drains every pending response), drops the connection, and then drops the permit. On any error it calls `close()` on the connection instead of returning it to the pool.
    - A CSV body is produced in full inside the task, so the permit covers all database work. Python's `/api/prices/reference/export` has no row bound and reads the full result before it writes (`app/routers/price_discovery.py:443-492,630-641`); Rust does the same (feature rule). The 30 s statement timeout and the AD-01 limit of 10 exports per minute bound it. A row cap is a separate Python change for later, listed under "Product issues found".
    - Route code never touches the plain pool. A test with 8 concurrent slow reads (`pg_sleep`), 8 client cancellations and 8 new requests proves: no acquire timeout, the 9th concurrent request gets the unchanged 503 + `Retry-After`, and `pg_stat_activity` never shows more than 9 connections for the app role.
12. **Disposable PostgreSQL (PM, 15 Oct):** Docker runs locally through Colima. Contract tests start a disposable `postgis/postgis:17-3.6-alpine` container with the pinned digest from `scripts/run_product_analytics_postgres_tests.sh`, on a Docker-assigned loopback port, and remove it at exit. Never use staging or prod.
13. **Approved difference AD-03 — news admission (PM, 15 Oct, after the Task 8 review):** Rust admits `/api/news` through the 8-slot public-read limiter; Python's gate does not cover news (`app/middleware/public_market_read_admission.py:10-41`). Under full capacity Rust answers news 503 with `Retry-After` and the busy body, and news load can make market reads 503. Accepted because every Rust database use then holds a permit, permits never exceed read connections, and no request waits on the pool. Tested in both contention directions (`rs/read/tests/admission_postgres.rs`). Reported in Task 14.
14. **Comparator keeps exact JSON body bytes (PM, 15 Oct):** stricter than the key-order rule in the route inventory. Rust already writes Python's key order, and exact bytes keep differential runs simple. If a future consumer cannot match Python's order, record an ordered-object exception instead of relaxing the comparator globally.
15. **Task 4b owns the populated corpus (PM, 15 Oct):** Task 4 deferred the fixture families of the `seed_corpus` rule in Task 4. Task 4b builds all of them once, after the build branch is rebased onto current staging, and records the golden corpus once. Final acceptance of Tasks 6, 6a, 7a-7c, 8, 9, 10a, 10b and 11 waits for their populated gates; development continues.
16. **Consumer policies stay in the shared module (PM, 16 Oct, after the Task 6 re-review R3):** curve UNKNOWN metadata, legacy curve catalog scope and price quarantine metadata need relations that `OrderSurface` does not provide yet. Tasks 9, 10a and 10b add each policy to `rs/read/src/market/` as a named surface or metadata relation with its own comparison tests. Do not copy predicates into routes, and do not broaden the economic relation.
17. **No multi-day shadow clock (founder, 16 Oct, tracker decision 48):** "we don't need 7-days to review, we just run it against our own tests/simulations". The Phase 1 gate is: the populated corpus (Task 4b), a seeded differential simulation at the Task 14 workload sizes, the unchanged integration suite (Task 13), and one short read-only differential pass on staging data. Task 14 is rewritten to this at its start; its seven-day text below is superseded.

18. **News tie groups (PM, 16 Oct):** Python orders `/api/news` by `published_at DESC` without a tie-breaker. The reviewed `tests/rust_contract/news-tie-paths.json` permits only the news root items array to compare equal-key groups as multisets of exact item bytes; the sequence of `published_at` keys must match exactly. Recorder stability uses the same rule. Changed/missing items and misordered keys fail. Every other path retains decision 14's exact bytes. Rust must retain descending published-time order.
19. **MCP golden recording deferred (PM, 16 Oct):** exclude `mcp` from Task 4b golden publication. Task 11 owns a protocol-aware bounded recorder and records MCP under its then-current oracle SHA. Every other group must be complete and stable across two fixed-clock passes in both KYC states.
20. **Stored-form positive fixtures (PM, 16 Oct):** positive fixture declarations must match Python-written storage, including FAME ASK `nomination_status`. Repair SQL-only positive rows. Keep malformed declarations in separately labelled negative cases and record Python's response, including 500 where applicable. Re-run the Task 6 matrix. Health HEAD/405 differences remain Task 13 exact-routing work, not Task 4b route fixes.

### Product issues found (not changed in Phase 1)

The current same-side live-slice VWAP combines qualified REAL and DEMO orders under a key that omits provenance (`app/services/live_benchmarks.py:134-204`). Public order responses expose this benchmark and premium/discount (`app/routers/orderbook.py:429-466`). Rust copies this behavior, including additional typed qualification. The founder may review separation as a later product change. REAL-only confirmed-trade reference prices remain separate and unchanged.

Founder (15 Oct): the benchmark should use REAL orders only in the end; keep the blend while there are no real orders.

`/api/prices/reference/export` has optional dates and no row limit; Python reads the whole result into memory (`app/routers/price_discovery.py:443-492,630-641`). Rust copies this. A row cap or required date range is a later Python change.

Staging and prod PostgreSQL servers use different default time zones (staging `Asia/Singapore`, prod `UTC`), so day grouping differs between the environments. Phase 0 picks one.

Python edge contracts found during Task 6 repair (not changed): scalar CI has no upper bound; digest normalization uses Python's default 28-digit half-even context. Window query normalization accepts Unicode decimal years and year zero, while date construction rejects year zero. Rust preserves these behaviors rather than tightening Python inputs.

### Review disposition

Applied B1 using the PM's AD-01 decision instead of changing Python or reducing scope. Applied M1 with a dedicated shared live-benchmark task. Applied M2–M6 and N1–N3 through executable task gates, test-only complete clock injection, effective budget checks, an assertion ledger, smaller checkpoints, and one external comparator. The review is `/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/research/rust-phase1-plan-review.md`. No review finding authorizes live execution.

### Pre-plan review

- **Architecture — WARN:** A second runtime needs a binary-aware extension to the one deploy transaction. A separate deploy script would create an unsafe second owner. Task 12b owns the extension; 12a and 12c have separate artifact and unit gates.
- **Code quality — WARN:** Use shared visibility, evidence, and serialization modules. Do not copy predicates into each route. Tasks 5–6 own them.
- **Test coverage — WARN:** Existing integration tests alone do not cover all public reads. Tasks 4 and 13 add the full route corpus without changing the existing suite.
- **Performance — WARN:** Bound reads and batch benchmark lookups. Preserve the two-statement snapshot design (`app/routers/orderbook.py:770-811`). Tasks 7 and 14 verify query count and measured cost.
- No local runtime or PostgreSQL result is claimed by this plan. Live activation remains blocked until all listed proofs pass.

## Exact route inventory

All paths below include their mounted prefix. The API mounts are in `app/main.py:276-313`. Every row is unauthenticated. A caller may affect CORS, correlation, and rate-limit results, but a bearer token must not unlock a richer response.

Notation: `?` means optional with default `None`; `!` means required. UUID/date/enum/int/bool query parse failures use FastAPI's 422 validation envelope. `S` means data-stable on an unchanged database. `T` means clock-dependent. `C` means caller-dependent headers or counters. `R` means generated request ID if no valid one is supplied. Every row is C/R at middleware level (`app/main.py:232-270`). No handler below generates random business data.

Shared query sets:

- **L:** `product_id:UUID?`, `delivery_point_id:UUID?`, `fuel_type:str?`, `market_product:MarketProduct?`, `region:str?`, `availability_window:str?`, `include_off_spec:bool=false`.
- **P:** `product_id:UUID?`, `delivery_point_id:UUID?`, `fuel_type:str?`, `region:str?`, `market_product:str?`, `availability_window:str?`.
- **D:** `skip:int=0, >=0`; `limit:int=20, 1..100`; `sort_by=newest`, one of `newest|price_asc|price_desc|quantity_desc`.
- No undocumented query key acquires meaning in Rust. Preserve Python's handling of unknown and repeated scalar query keys through golden cases.

| ID | Method and path | Query parameters | Auth | Python handler | Response model / wire contract | Clock |
|---|---|---|---|---|---|---|
| R01 | GET `/api/ports` | none | public | `app/routers/ports.py:14` | `list[PortResponse]`; no pagination; no SQL order; location null, lat/lng extracted | S |
| R02 | GET `/api/ports/{port_id}` | none; path is string | public | `app/routers/ports.py:39` | `PortResponse`; 404 `Port not found` | S |
| R03 | GET `/api/catalog/products` | none | public | `app/routers/catalog.py:21` | `list[ProductResponse]`; canonical display order then name | S |
| R04 | GET `/api/catalog/delivery-points` | none | public | `app/routers/catalog.py:37` | `list[DeliveryPointResponse]`; canonical display order then name | S |
| R05 | GET `/api/news` | `limit=20:1..100`, `category:str?`, `min_relevance=1:1..5` | public | `app/routers/news.py:12` | no response model; array of ten-key dicts; published time descending; nullable ISO dates | S |
| R06 | GET `/api/trade-tape` | `fuel_type:str?`, `market_product:str?`, `delivery_point_id:UUID?`, `region:str?`, `availability_window:str?`, `demo_only:bool=false`, `skip=0:>=0`, `limit=50:1..200` | public | `app/routers/trade_tape.py:63` | `TradeTapeResponse`; items,total,market_hours; seven-day cutoff; confirmed time descending | T |
| R07 | GET `/api/curves/forward` | `product_id:UUID!`, `delivery_point_id:UUID?` | public | `app/routers/curves.py:923` | `ForwardCurveResponse`; real-only economics; canonical window order | T |
| R08 | GET `/api/curves/forward/export` | `product_id:UUID!`, `delivery_point_id:UUID?` | public | `app/routers/curves.py:944` | CSV, not JSON; LF rows; attachment `forward_curve_{product_id}.csv` | T |
| R09 | GET `/api/curves/forward/table` | repeated `windows:list[str]?`, `market_products:list[MarketProduct]?`, `delivery_point_ids:list[UUID]?` | public | `app/routers/curves.py:851` | `ForwardCurveTableResponse`; compact matrix; ValueError -> 422 detail string | T |
| R10 | GET `/api/curves/forward/slice` | `market_product:MarketProduct!`, `delivery_point_id:UUID!`, `availability_window:str!` | public | `app/routers/curves.py:881` | `ForwardCurveSliceResponse`; bounded evidence; ValueError -> 422 detail string | T |
| R11 | GET `/api/curves/forward/board` | `availability_window=SPOT`, `focus_market_product:MarketProduct?`, `focus_delivery_point_id:UUID?` | public | `app/routers/curves.py:901` | `ForwardCurveBoardResponse`; focus defaults/fallback; unsupported lane 422 | T |
| R12 | GET `/api/prices` | P; `hours=24:1..168` | public | `app/routers/price_discovery.py:383` | `PriceDiscoveryResponse`; 60/min; generated_at; separate REAL/DEMO/quarantine buckets | T |
| R13 | GET `/api/prices/reference` | P; `date_from:date?`, `date_to:date?`, aliases `from:date?`,`to:date?`; `visibility=external:internal|external` | public | `app/routers/price_discovery.py:553` | `ReferencePriceResponse`; 30/min; REAL-only daily VWAP; date descending | T |
| R14 | GET `/api/prices/reference/export` | P; `from_date:date?`, `to_date:date?`, `format:str=csv` | public | `app/routers/price_discovery.py:605` | CSV; 10/min; CRLF rows; attachment `verdaxis_reference_prices.csv`; format is accepted but unused | S |
| R15 | GET `/api/orderbook/snapshot` | `market_product:MarketProduct!`, `delivery_point_id:UUID!`, `availability_window:str!` | public | `app/routers/orderbook.py:750` | `OrderBookSnapshotResponse`; max 15 orders per side; repeatable read | T |
| R16 | GET `/api/orderbook/bids` | L + D | public | `app/routers/orderbook.py:856` | `PaginatedResponse[OrderResponse]`; items,total,skip,limit | T |
| R17 | GET `/api/orderbook/asks` | L + D | public | `app/routers/orderbook.py:937` | `PaginatedResponse[OrderResponse]`; same total beyond end of page | T |
| R18 | GET `/api/orderbook/with-ci` | `product_id:UUID?`, `delivery_point_id:UUID?`, `side:BID|ASK?`, `include_off_spec=false`, `skip=0:>=0`, `limit=50:1..100` | public | `app/routers/orderbook.py:1037` | `list[OrderResponseWithCI]`; deprecated; created_at descending; LNG CI overlay null | T |
| R19 | GET `/api/orderbook/aggregated` | L; `limit=1024:1..2048` | public | `app/routers/orderbook.py:1329` | `list[AggregatedOrderbookResponse]`; separate evidence classes | T |
| R20 | GET `/api/orderbook/product-counts` | `side:BID|ASK!`, `delivery_point_id:UUID?`, `region:str?`, `availability_window:str?`, `include_off_spec=false` | public | `app/routers/orderbook.py:1355` | `ProductCountResponse`; zero entries for all approved products; total | T |
| R21 | GET `/api/orderbook/map-summary` | none | public | `app/routers/orderbook.py:1664` | `MapSummaryResponse`; groups and latest public asks by point; no page | T |
| R22 | GET `/api/orderbook/map-summary/compact` | none | public | `app/routers/orderbook.py:1673` | `MapCompactSummaryResponse`; markets,demo_groups,recent_asks | T |
| R23 | GET `/api/orderbook/products` | none | public | `app/routers/orderbook.py:1689` | `list[str]`; distinct visible names, ascending | T |
| R24 | GET `/api/orderbook/regions` | none | public | `app/routers/orderbook.py:1708` | `list[str]`; distinct visible regions, ascending | T |
| R25 | GET `/api/orderbook/fuel-types` | none | public | `app/routers/orderbook.py:1728` | `list[str]`; distinct visible fuel types, ascending | T |
| R26 | GET `/api/orderbook` | `product_id:UUID?`, `delivery_point_id:UUID?`, `side:BID|ASK?`, `availability_window:str?`, `include_off_spec=false`, `skip=0:>=0`, `limit=50:1..100`, `page:int?:>=1`, `page_size:int?:1..100` | public | `app/routers/orderbook.py:1751` | `list[OrderResponse]`; deprecated; page_size overrides limit; page overrides skip | T |
| R27 | GET `/api/listings` | `skip=0:>=0`, `limit=50:1..100` | public | `app/routers/inventory.py:805` | no response model; compatibility dict array; created_at desc then ID; match_count=0 | T |
| H01 | GET `/health/ready` | none | public | `app/main.py:330` | success exact four keys; failure exact two keys; 2-second default DB deadline | DB |
| H02 | GET `/health/live` | none | public | `app/main.py:325` | `{"status":"ok"}`; no DB call | S |
| H03 | GET `/health` | none | public | `app/main.py:320` | readiness alias | DB |
| M01 | POST `/mcp` | none; JSON-RPC body | public agent-only | `app/main.py:155`; `app/mcp_server.py:160` | SDK initialize/list/call/notification envelopes; stateless JSON responses | T/C |
| M02 | GET `/mcp` | SDK transport | public agent-only | `app/main.py:155`; `app/mcp_server.py:160` | exact recorded SDK response, including unsupported stream behavior | C |
| M03 | DELETE `/mcp` | SDK transport | public agent-only | `app/main.py:155`; `app/mcp_server.py:160` | exact recorded stateless SDK response; no session deletion side effect | C |

### Inventory details that apply to every task

- **Serialization:** Preserve decimal strings, including scale. Do not convert NUMERIC to f64. Ports' intelligence prices and coordinates are floats, not Decimal (`app/schemas/port.py:11-34`). Product minimum lot is Decimal (`app/schemas/catalog.py:7-20`). Order, aggregate, and price Decimal fields are declared in `app/schemas/orderbook.py:234-269,401-570`.
- **Null versus absent:** Keep declared optional response fields as null unless the schema explicitly excludes them. The curve table excludes `benchmark_observed_at` (`app/schemas/curves.py:396`). Public terms omit private fields; do not replace omissions with null (`app/services/lng_order.py:123-134`; `app/services/fame_order.py:162`). News dates are explicit null or `isoformat()` strings (`app/routers/news.py:27-39`). Listings use their own serializer, not OrderResponse (`app/routers/inventory.py:762-800`).
- **Key order:** Superseded by decision 14: the comparator compares exact JSON body bytes, so Rust must write Python's key order. (Original text: no named consumer in this review requires JSON object key order. Compare exact object membership, types, values, Decimal scale, and null versus absence; ignore object key order by default.) An ordered-object exception needs a named consumer, source citation, and exact JSON path in `ordered-object-paths.json` (initially empty). Keep raw bytes for diagnosis and array order exact. Never sort every array to hide a mismatch. Ports have no ORDER BY; prices have incompletely ordered aggregate groups. Test these separately as source nondeterminism, not as permission to reorder all results (`app/routers/ports.py:18-24`; `app/routers/price_discovery.py:193-205,484-489`).
- **Cache and ETag:** These public handlers set no Cache-Control or ETag. CSV handlers add only their attachment header (`app/routers/curves.py:958-962`; `app/routers/price_discovery.py:643-647`). The public OrderResponse contains a terms digest, not an HTTP ETag; owner ETags belong to OrderMyResponse (`app/schemas/orderbook.py:234-298`). Record header absence as part of the contract. Do not add caching or conditional 304 responses. The disposable identity route alone uses `no-store` (`tests/disposable_server.py:62-70`). Capture proxy-added headers separately; the live Caddy configuration is not established by this source review.
- **Errors:** Preserve detail strings, structured detail objects, and Pydantic 422 lists as distinct forms. Invalid order-book window normalization returns 400 (`app/routers/orderbook.py:300-306`). Price alias conflicts and reverse date ranges return 422 (`app/routers/price_discovery.py:94-136,573-582`). The export's `format` argument is not validated (`app/routers/price_discovery.py:603-647`). Do not “fix” it during the port.
- **Global database failures:** Retryable market contention or canceled GET/HEAD queries return 503, `Retry-After: 1`, and `Market is temporarily busy; retry shortly.` Other database errors return 500 and `Database operation failed.` (`app/main.py:187-214`). Match the market-path classifier from `app/services/db_errors.py`, not all routes indiscriminately.
- **Method and slash behavior:** Record HEAD, OPTIONS, trailing slash, invalid UUID, missing required query, repeated scalar query, malformed enum, and unknown path behavior. axum GET automatically supports HEAD; FastAPI's explicit GET registration need not. Override this where the Python recording differs. Do not route a private order-book path to Rust just because it shares a prefix (`app/routers/orderbook.py:1100-1153`).

### Explicitly excluded public surfaces

Keep `/api/availability`, `/api/demand`, `/api/producers`, `/api/benchmarks`, `/api/compliance/fuels`, public subscription fees, referral resolution, `/`, docs/OpenAPI, and SSE on Python. These are separate mounted handlers, not hidden additions to the seven named groups (`app/main.py:291-313,315-317`; `app/routers/compliance_api.py:266`; `app/routers/subscriptions.py:27`; `app/routers/referrals.py:207`). Keep `/api/orderbook/my`, `/api/orderbook/my/latest-ask-template`, and `/api/listings/my` on Python; they are not public (`app/routers/orderbook.py:1100-1153`; `app/routers/inventory.py:835-844`).

## Public market rules to copy

| Rule | Current source evidence | Port requirement |
|---|---|---|
| Strict canonical product and point identity | `app/services/market_data_eligibility.py:131-218` | Match ID/name/type/grade and active status; match point ID/name/region and active status; enforce execution mode and lane. Load identity tuples from policy data. |
| Public orders | `app/services/public_order_scope.py:35-88` | OPEN/PARTIALLY_FILLED; canonical window; current owner; REAL/DEMO only; qualification fields; off-spec default false. Do not add a remaining-quantity filter if the Python query lacks one. |
| Expiry | `app/services/market_data_eligibility.py:221-274` | DEMO needs a future expiry. REAL/UNKNOWN may have null expiry in this helper. Other callers still apply their own provenance scope. |
| KYC and organization ownership | `app/services/market_data_eligibility.py:49-114`; `app/services/kyc_execution.py:20-30` | Owner must still belong to the organization, be approved/verified, and not require password change. KYC must be org-bound approved or unexpired org-bound grace; rejection wins. Ownerless legacy rows retain the existing exception. Assisted rows require the approved ADMIN actor and an eligible customer member; never use administrator KYC as customer KYC. |
| ASK declarations | `app/services/public_order_scope.py:60-66` | Certification, scheme, specification, MSDS, feedstock, and required origin/CI fields. Typed FAME/LNG profiles carry their current exemptions. |
| FAME date/lane | `app/services/market_data_eligibility.py:231-249` | Singapore-local certificate date, same-side terms, lane, off-spec false, current ASK evidence. Do not substitute UTC date. |
| Bio-LNG | `app/services/market_data_eligibility.py:266-273`; `app/services/lng_order.py:60-72,123-134` | Rotterdam lane, same-side terms, no off-spec; omit certificate number and physical supplier name. WTT stays out of lifecycle CI. No synthetic benchmark or seed price (`CLAUDE.md:5-9`). Preserve the separate writer reseller block. |
| Public serializer | `app/schemas/orderbook.py:234-269`; `app/services/order_terms.py:49-122` | Never expose organization, owner, actor, support, or inventory identifiers. Digest uses the full reviewed terms before public redaction. |
| Trade evidence | `app/services/market_provenance.py:144-239` | Use immutable canonical snapshots and confirmed/delivered/paid states. Admit REAL/REAL and DEMO/DEMO separately. Exclude TEST/CANARY and mismatched classes from economics. Do not join mutable order metadata to reconstruct history. |
| Aggregate provenance | `app/services/market_provenance.py:51-95`; `app/routers/price_discovery.py:295-375` | Prefer REAL then disclosed DEMO in selected aggregates. UNKNOWN contributes metadata only where Python does so. Never turn unknown into real. Preserve each surface's evidence selection; the live-slice benchmark below intentionally differs. |
| Live-slice order benchmark | `app/services/live_benchmarks.py:35-93,124-204`; `app/routers/orderbook.py:429-466` | Combine qualified REAL and DEMO remaining quantities in same-side product/point/window VWAP. Provenance is not part of its key. Require positive remaining quantity, no off-spec, current catalog/lane, typed FAME/LNG execution qualification, and scalar ASK qualification. Public visibility alone is insufficient. No rebuild/write call. |
| Trade privacy | `app/routers/trade_tape.py:30-59` | Eight-character public ID; no party IDs. Delivery-point detail appears only for an exact point filter. Market hours is always true. |
| Admission | `app/middleware/public_market_read_admission.py:10-43,92-113`; `app/main.py:139-150` | Selected GETs and database MCP tools share one fail-fast budget. News bypasses it. Reserve one pool slot. Busy REST is 503 plus Retry-After 1 with the exact public-market detail. |
| Rate limits | `app/rate_limit.py:16-33`; `app/main.py:168-173`; `app/routers/price_discovery.py:382,552,604` | Trust the final X-Forwarded-For hop only from loopback. Preserve route-specific 60/30/10 minute buckets and exact 429 detail. No invented Retry-After on that handler. |
| MCP safety | `app/mcp_server.py:74-93,147-176` | Reject any Origin header, including empty; exact Host allowlist; stateless JSON transport; no production mount. Busy tools use ToolError, not the REST 503 body. |

## File map and task order

Create one library/binary package, `verdaxis-read`, plus an independent `verdaxis-contract` test/CLI package in the workspace. Do not create a crate per route.

| Owner | Proposed paths | Responsibility |
|---|---|---|
| Astra | `rs/Cargo.toml`, `rs/Cargo.lock`, `rs/rust-toolchain.toml`, `rs/read/Cargo.toml`, `rs/read/src/lib.rs`, `rs/read/src/main.rs` | workspace, composition, startup |
| Astra | `rs/read/src/runtime/{config,db,health,http,admission,error,logging}.rs` | runtime boundaries |
| Astra | `rs/read/src/market/{policy,visibility,provenance,terms,decimal,windows}.rs` | shared rules |
| DeepSeek | `rs/read/src/routes/{ports,catalog,news}.rs` | simple read ports |
| Astra | `rs/read/src/routes/{orderbook_snapshot,orderbook_lists,orderbook_map,curves_legacy,curves_canonical,prices,trade_tape,listings}.rs` | independently reviewed read projections |
| Astra | `rs/read/src/mcp/{mod,voyage,prices,transport}.rs` | MCP and pure voyage engine |
| Astra | `rs/contract/`, `tests/rust_contract/`, `scripts/rust_contract/` | oracle, fixtures, comparison, shadow, measurement |
| Astra | `deploy/rust/`, `deploy/caddy/`, staging unit, CI and deploy helper edits | release and operator boundary |

**21 independently accepted tasks (Task 4b added 15 Oct):** 1 -> 2 -> 3 -> 4 -> 4b (after the staging rebase) -> 5 -> 6 -> 6a -> 7a -> 7b -> 7c -> 8 -> 9 -> 10a -> 10b -> 11 -> 12a -> 12b -> 12c -> 13 -> 14. Letter suffixes preserve review references; each is a separate test/commit checkpoint.

Parallel lanes after 6: 8 (simple reads), 9 (prices/tape), 10a then 10b (curves), and 6a then 7a/7b/7c (orderbook). Task 11 requires 9 and 10b. Tasks 12a/12b/12c may start after 3/5 using fixture binaries and fixture units; their local acceptance does not require route HTTP parity. Artifact publication waits for Task 13's complete contract job. Task 13 requires all route and release checkpoints. Task 14 follows 13. No task's acceptance claims a future handler exists.

Only the integration owner in Tasks 1, 3, and 4 edits root module registration, test-server dispatch, and Cargo manifests. In Task 3, pre-register empty route modules with `pub fn routes() -> axum::Router<AppState> { axum::Router::new() }` for ports, catalog, news, orderbook_snapshot, orderbook_lists, orderbook_map, listings, prices, trade_tape, curves_legacy, and curves_canonical. Create an empty `mcp_router` at the same time. These return 404, so later corpus tests fail honestly. Each route worker replaces only their module and uses a task-local test server that merges that module's `routes()` with health and the test-only disposable identity wrapper. Task 13 assembles all completed modules in the serving binary. This lets route tests pass before global integration without file clashes. Task 5 owns policy data. Task 6 owns shared predicates. Do not let route workers edit either. Tasks 12a -> 12b -> 12c alone edit their listed CI, deploy, and unit files, sequentially. Tasks 4 and 14 share harness files and therefore run sequentially. Phase 0 keeps its own worktree and migration ownership. Stop at a shared-policy conflict; reconcile source and golden corpus before continuing.

Astra owns every subtle port. DeepSeek owns Task 8 only; its fixed pattern and golden corpus must exist first. A later independent Astra review is an implementation acceptance step, not an agent started while writing this plan.

## Common interfaces and test commands

These are the required public Rust interfaces. Tasks define their implementation. Types exported from the crate root must use these names:

```rust
pub type Json = serde_json::Value;
pub type ApiResult = Result<axum::response::Response, ApiError>;

#[derive(Clone)]
pub struct AppState {
    pub config: std::sync::Arc<Config>,
    pub pool: sqlx::PgPool,
    pub policy: std::sync::Arc<MarketPolicy>,
    pub reads: std::sync::Arc<tokio::sync::Semaphore>,
}

pub struct ApiError {
    pub status: axum::http::StatusCode,
    pub detail: Json,
    pub retry_after: Option<u32>,
}

#[derive(Clone, serde::Deserialize)]
pub struct Config {
    pub environment: String,
    pub release_sha: String,
    pub migration_revision: String,
    pub database_url: String,
    pub bind: std::net::SocketAddr,
    pub pool_max: u32,
    pub python_budget: u32,
    pub configured_max_connections: u32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MarketPolicy {
    pub source_sha: String,
    pub products: Vec<ProductPolicy>,
    pub points: Vec<PointPolicy>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProductPolicy {
    pub id: uuid::Uuid,
    pub code: String,
    pub name: String,
    pub fuel_type: String,
    pub fuel_grade: String,
    pub execution_mode: String,
    pub lanes: Vec<uuid::Uuid>,
    pub qualification_profile: String,
    pub display_order: u32,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PointPolicy {
    pub id: uuid::Uuid,
    pub name: String,
    pub region: String,
    pub timezone: String,
    pub display_order: u32,
}
```

Use `qualification_profile` for generic rules (`scalar`, `fame_v1`, `lng_v1`). Profile names are rule versions, not product identities. The serialized policy may contain product names; executable Rust must not. Extend profile data for Phase 0 contracts only after that Python contract is merged and tested.

**Build environment (required at every task/resume):** Run `export RELEASE_SHA=$(git rev-parse HEAD)` from the implementation worktree before every Cargo build/test/check/clippy command below. This is the full compiled source identity, not the unit-test Python `test` sentinel. CI sets it to its checked-out full `GITHUB_SHA`. Every runner must set it explicitly for child Cargo commands. This paragraph is the shared command prelude; never rely on a prior session's environment.

**Test classes:** Pure/offline Rust tests run without external services. Put every HTTP/PostgreSQL-dependent test in a test target with `required-features = ["contract-tests"]`; declare `contract-tests = []` in the read manifest. Do not use `#[ignore]` or silently return success for absent URLs. SQLX_OFFLINE only controls compile-time query metadata. The explicit contract job starts its own database and servers, then enables this feature. Each task runs both its pure tests and its named contract group before acceptance.

Use these commands from the implementation worktree. Do not run them from a live checkout:

```bash
export RELEASE_SHA=$(git rev-parse HEAD)
cargo test --manifest-path rs/Cargo.toml --locked --workspace
cargo fmt --manifest-path rs/Cargo.toml --all -- --check
cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings
ENVIRONMENT=test RELEASE_SHA=test \
 JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
 DATABASE_URL='sqlite+aiosqlite:///:memory:' \
 venv/bin/python -m pytest tests/unit -q
```

Each task's commit command includes only its listed files. Append the attribution trailer. Before each commit, run `git diff --check`. Record the task number, commit, commands, results, and remaining failures in `docs/rust-phase1/progress.md`. A failed or skipped gate never becomes a checked box.

## Implementation tasks

### Task 1: Establish the workspace and compiled identity — Astra

**Files:** Create `rs/Cargo.toml`, `rs/Cargo.lock`, `rs/rust-toolchain.toml`, `rs/read/Cargo.toml`, `rs/read/build.rs`, `rs/read/src/lib.rs`, `rs/read/src/main.rs`, `rs/read/tests/identity.rs`. Create `rs/contract/Cargo.toml`, `rs/contract/src/lib.rs`, and `rs/contract/src/main.rs` for the corpus runner. Create `docs/rust-phase1/progress.md` during execution, not during plan writing.

**Interfaces:** Consumes `RELEASE_SHA` at build time. Produces `pub fn compiled_sha() -> &'static str` and `pub fn verify_release(value: &str) -> Result<(), &'static str>` in `verdaxis_read`. Task 4 fills the independent contract package; the read package never imports it.

- [ ] **Step 1: Add the identity test.**

```rust
use verdaxis_read::{compiled_sha, verify_release};

#[test]
fn binary_identity_cannot_be_overridden() {
    assert_eq!(compiled_sha().len(), 40);
    assert!(verify_release(compiled_sha()).is_ok());
    let other = if compiled_sha().starts_with('a') { "b" } else { "a" }.repeat(40);
    assert!(verify_release(&other).is_err());
    assert!(verify_release("test").is_err());
}
```

- [ ] **Step 2: Run** `RELEASE_SHA=$(git rev-parse HEAD) cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test identity`. Expected failure: manifest/package or functions do not exist. Keep this failure evidence.
- [ ] **Step 3: Add workspace and build identity.** Use edition 2024 and `rust-version = "1.95"`. Set the toolchain to `1.95.0`, profile `minimal`, components `rustfmt,clippy`. Use the following workspace and build code.

```toml
# rs/Cargo.toml
[workspace]
members = ["read", "contract"]
resolver = "3"

[workspace.package]
edition = "2024"
rust-version = "1.95"

[profile.release]
lto = "thin"
codegen-units = 1
strip = "debuginfo"
```

```rust
// rs/read/build.rs
fn main() {
    println!("cargo:rerun-if-env-changed=RELEASE_SHA");
    let sha = std::env::var("RELEASE_SHA").expect("build requires RELEASE_SHA");
    assert!(sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert!(sha.bytes().any(|b| b != b'0'));
    println!("cargo:rustc-env=VERDAXIS_BUILD_SHA={sha}");
}
// rs/read/src/lib.rs
pub fn compiled_sha() -> &'static str { env!("VERDAXIS_BUILD_SHA") }
pub fn verify_release(value: &str) -> Result<(), &'static str> {
    if value == compiled_sha() { Ok(()) } else { Err("release identity mismatch") }
}
// rs/read/src/main.rs
fn main() { println!("{}", verdaxis_read::compiled_sha()); }
```

Create the package manifests with `[package] name = "verdaxis-read"` and `name = "verdaxis-contract"`, respectively, `version = "0.1.0"`, `edition.workspace = true`, and `rust-version.workspace = true`. Add `[dev-dependencies] verdaxis-contract = { path = "../contract" }` only to the read package. The contract package must not depend on the read package; it drives external HTTP processes. Add `fn main() {}` and an empty library to the contract package in this bootstrap; Task 4 replaces them before any contract test can pass. Add the common types from this plan to the read library in this task.

Resolve dependencies with explicit commands:

```bash
cargo add --manifest-path rs/read/Cargo.toml axum@0.8
cargo add --manifest-path rs/read/Cargo.toml sqlx@0.9 --no-default-features \
  --features runtime-tokio,postgres,rust_decimal,chrono,uuid,macros
cargo add --manifest-path rs/read/Cargo.toml tokio --features full
cargo add --manifest-path rs/read/Cargo.toml serde --features derive
cargo add --manifest-path rs/read/Cargo.toml serde_json --features preserve_order
cargo add --manifest-path rs/read/Cargo.toml tracing
cargo add --manifest-path rs/read/Cargo.toml tracing-subscriber --features json,env-filter
cargo add --manifest-path rs/read/Cargo.toml tower --features util
cargo add --manifest-path rs/read/Cargo.toml tower-http --features cors
cargo add --manifest-path rs/read/Cargo.toml uuid --features serde,v4
cargo add --manifest-path rs/read/Cargo.toml chrono --features serde
cargo add --manifest-path rs/read/Cargo.toml chrono-tz rust_decimal sha2 utoipa
cargo add --manifest-path rs/read/Cargo.toml rmcp --features server,transport-streamable-http-server
cargo add --manifest-path rs/contract/Cargo.toml serde --features derive
cargo add --manifest-path rs/contract/Cargo.toml serde_json --features preserve_order
cargo add --manifest-path rs/contract/Cargo.toml reqwest --features blocking,json
```

Read each resolved crate's local source for version-specific APIs before coding. Pin the resulting lockfile. If the requested SQLx release does not resolve or cannot support the toolchain, stop that dependency choice and report the exact error; do not silently choose another stack. No system package installation is authorized by this plan.

- [ ] **Step 4: Run** the identity test, `cargo check --workspace`, rustfmt, and clippy. Expected pass. Record resolved crate versions and target triple in the progress file.
- [ ] **Step 5: Commit.** `git add rs .gitignore docs/rust-phase1/progress.md && git commit -m 'build: establish Rust read workspace and release identity'` with the required trailer.

**Resume point:** Task 2 starts from this commit. Verify `cargo test -p verdaxis-read --test identity` with the same build SHA before changing runtime configuration. Dependency resolution is a local implementation gate, not evidence of a Linux release build.

### Task 2: Validate configuration, pool authority, and startup revision — Astra

**Files:** Create `rs/read/src/runtime/mod.rs`, `config.rs`, `db.rs`, `health.rs`, `rs/read/tests/runtime.rs`, `deploy/rust/allowed-revisions.json`, and `scripts/prepare_rust_budget.py`. The budget helper exposes a pure `derive_budget(settings, rust_pool)` and a no-follow environment-file CLI; Task 12b invokes it under the deploy lock. Modify the library exports. Reuse, but do not weaken, `scripts/verify_migration_revision.py:20-33`.

**Interfaces:** Consumes `Config`, `compiled_sha()`, `verify_release()`. Produces:

```rust
pub fn validate_config(config: &Config) -> Result<(), String>;
pub fn check_revision(actual: &str, allowed: &[String]) -> Result<(), String>;
pub async fn connect(config: &Config) -> Result<sqlx::PgPool, ApiError>;
pub async fn ready(state: axum::extract::State<AppState>) -> axum::response::Response;
pub async fn live() -> axum::Json<Json>;
```

- [ ] **Step 1: Add failure tests.**

```rust
#[test]
fn revisions_fail_closed() {
    let allowed = vec!["kycg_20261014_staging".to_owned()];
    assert!(check_revision("kycg_20261014_staging", &allowed).is_ok());
    for revision in ["head", "", "kycg_20261014_prod", "unexpected"] {
        assert!(check_revision(revision, &allowed).is_err());
    }
}
#[tokio::test]
async fn readiness_failure_has_no_identity() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/unreachable_test").unwrap();
    let response = probe(&pool, "staging", "a".repeat(40).as_str()).await;
    assert_eq!(response.0, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.1, serde_json::json!({"status":"error","db":"unavailable"}));
}
```

Define the test-visible `pub async fn probe(pool: &sqlx::PgPool, environment: &str, sha: &str) -> (axum::http::StatusCode, Json)` in `health.rs`. Add tests for wrong role, URL query routing, non-loopback deployed database, empty/encoded-whitespace password, unsafe role membership, and configured or observed max_connections below the computed effective total. Use a disposable PostgreSQL role test for authority; SQLite is not proof.

- [ ] **Step 2: Run** `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test runtime`. Expected failure: missing runtime exports.
- [ ] **Step 3: Implement config and health.** Only accept development/test/staging; refuse production in this Phase 1 binary. Require staging database `verdaxis_staging`, user `verdaxis_app_staging`, host `127.0.0.1`, port 5432, no URL query, explicit non-placeholder password, loopback HTTP bind, and matching compiled SHA. For tests require a numeric-loopback database ending `_test`; never derive it from `.env`. Match role attestation SQL from `app/database.py:178-185` and role flags from `app/database.py:148-175`.

```rust
pub fn check_revision(actual: &str, allowed: &[String]) -> Result<(), String> {
    if allowed.iter().any(|value| value == actual) {
        Ok(())
    } else {
        Err("migration revision is outside the compiled allowlist".into())
    }
}
pub async fn probe(pool: &sqlx::PgPool, environment: &str, sha: &str)
    -> (axum::http::StatusCode, Json)
{
    let result = tokio::time::timeout(std::time::Duration::from_secs(2),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool)).await;
    match result {
        Ok(Ok(1)) => (axum::http::StatusCode::OK,
            serde_json::json!({"status":"ok","db":"ok","environment":environment,"release_sha":sha})),
        _ => (axum::http::StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"status":"error","db":"unavailable"})),
    }
}
```

Set a single Rust pool to min 0, max `config.pool_max` (exactly 9, see AD-02), acquire timeout 30 seconds. Configure each connection with `statement_timeout=30000`, `lock_timeout=3000`, `idle_in_transaction_session_timeout=60000`. Use read-only transactions for every public route. Do not set a SQL session `TimeZone` in Rust. Python sets none (`app/database.py`), and no role or database setting sets one, so both processes inherit the server default: staging `Asia/Singapore`, prod `UTC` (read by the PM with `SHOW TimeZone` on 15 Oct 2026). Daily grouping such as `date_trunc('day', ...)` follows it. At startup, Rust reads `SHOW TimeZone` once and logs it. The disposable oracle database sets `TimeZone` to the target environment's value (staging: `Asia/Singapore`), the harness records it with each golden run, and a contract run fails when Python and Rust report different values. Add a test that a non-UTC session zone changes no date bucket between Python and Rust. Do not use 500 ms/5 s write-lock constants for these reads. Those are later matching concerns; current pool timeouts are in `app/database.py:14-33` and `docs/runtime-hardening.md:318-324`.

```rust
let options: sqlx::postgres::PgConnectOptions = config.database_url.parse()
    .map_err(|_| ApiError::internal())?;
let options = options.options([
    ("statement_timeout", "30000"),
    ("lock_timeout", "3000"),
    ("idle_in_transaction_session_timeout", "60000"),
]);
let pool = sqlx::postgres::PgPoolOptions::new()
    .min_connections(0).max_connections(config.pool_max)
    .acquire_timeout(std::time::Duration::from_secs(30))
    .connect_with(options).await.map_err(|_| ApiError::internal())?;
```

Define `ApiError::internal() -> ApiError` in this task with status 500, detail string `Database operation failed.`, and no retry header. Task 3 adds its HTTP conversion; its displayed constructor is the required unchanged definition, not a second implementation. This keeps Task 2 independently compilable. Do not print parse errors containing credentials.

**Effective budget gate:** Trusted runtime preparation reads effective Python Settings from the attested environment file and derives `services * workers * (pool_size + overflow + 1 dispatcher) + reserve`. Add Rust `config.pool_max`. Bind all inputs, total, configured max_connections, release SHA, and environment-file digest into the private runtime budget artifact. Startup rechecks the total against both configured and observed `SHOW max_connections`. A short-lived prestart control-plane check recomputes the environment digest and rejects drift until preparation reruns under the canonical deployment lock. The Rust HTTP process checks the prepared release identity and budget, but does not read the secret-rich `.env` file. Never use 55 as a constant gate. Defaults give 52 + 3 = 55; pool size 8 with the other defaults gives 100 + 3 = 103 and must fail against 100 (`app/database.py:49-61,102-107`).

```rust
pub fn connection_budget(services: u32, workers: u32, pool: u32,
    overflow: u32, reserve: u32, rust_pool: u32) -> Option<u32> {
    services.checked_mul(workers)?
        .checked_mul(pool.checked_add(overflow)?.checked_add(1)?)?
        .checked_add(reserve)?.checked_add(rust_pool)
}
#[test]
fn budget_uses_effective_settings() {
    assert_eq!(connection_budget(2, 4, 2, 1, 20, 9), Some(61));
    assert_eq!(connection_budget(2, 4, 8, 1, 20, 9), Some(109));
    let need = connection_budget(2, 4, 8, 1, 20, 9).unwrap();
    assert!(need <= 109);
    assert!(!(need <= 108));
}
```

Add `pub fn validate_budget(total: u32, configured_max: u32, observed_max: u32) -> Result<(), String>`; accept only `total <= configured_max && total <= observed_max`. Test equality and one-below for each limit independently, changed workers/overflow/reserve, stale config digest, and overflow. `Config` carries `python_budget: u32` and `configured_max_connections: u32` from trusted preparation. Keep `pool_max` validated at exactly 9 during Phase 1 (8 admission slots plus 1 reserved connection; see Task 3) and use it consistently for SQLx and admission (`pool_max - 1`). Shadow opens no DB pool. This is a conservative configured bound, not discovery of the physical topology.

Initial compiled revision data is exactly `["kycg_20261014_staging"]`. A new Phase 0 revision needs an explicit compatibility test and allowlist update. The systemd prestart performs the live singleton revision read with the existing separate migrator-backed verifier. The Rust process checks this attested revision against its embedded allowlist before listening. The HTTP process must not open `MIGRATOR_DATABASE_URL` or run Alembic.

- [ ] **Step 4: Run** runtime tests and the raw-role disposable PostgreSQL tests. Expected pass. Test zero/multiple `alembic_version` rows through the prestart verifier; neither can start Rust. Test that the app role still cannot SELECT that table.
- [ ] **Step 5: Commit** the listed runtime files and allowlist: `git commit -m 'feat: add bounded Rust runtime and checkpoint admission'` with trailer.

**Resume point:** Record the tested revision, database role, connection count, and health corpus result. Task 3 must not start until config and authority tests pass.

### Task 3: Match HTTP middleware and error contracts — Astra

**Files:** Create `rs/read/src/runtime/{http,admission,error,logging}.rs`, `rs/read/tests/http_contract.rs`. Modify `main.rs`, library exports, and runtime module registration.

**Interfaces:** Consumes `AppState`, `probe`, and config. Produces:

```rust
pub fn router(state: AppState) -> axum::Router;
pub fn request_id(supplied: Option<&str>) -> String;
pub fn busy() -> ApiError;
pub fn database_error(path: &str, method: &str, sqlstate: Option<&str>) -> ApiError;
pub fn selected_read(method: &str, path: &str) -> bool;
pub fn try_read(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError>;
impl ApiError { pub fn internal() -> Self; }
impl axum::response::IntoResponse for ApiError;
```

- [ ] **Step 1: Add tests.**

```rust
#[test]
fn request_ids_are_bounded() {
    assert_eq!(request_id(Some("client-1:a.b")), "client-1:a.b");
    for value in ["", "bad id", "bad\nheader", "abcdefghijklmnopqrstuvwxyz01234567890"] {
        assert_ne!(request_id(Some(value)), value);
    }
}
#[tokio::test]
async fn cancelled_query_keeps_slot_until_drop() {
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = gate.clone().try_acquire_owned().unwrap();
    assert!(gate.clone().try_acquire_owned().is_err());
    drop(permit);
    assert!(gate.try_acquire_owned().is_ok());
}
```

Extend the cancellation test with a real disposable `pg_sleep` query: cancel the HTTP client, wait for database cancellation and connection return, then assert admission returns to eight slots. Do not spawn detached database work that outlives the permit. Add HTTP tests for allowed and denied origins on 200/422/429/503, preflight, invalid support context headers, malformed query validation, HEAD, 405/Allow, slash redirects, and fixed-window reset. Port assertions from `tests/unit/test_public_market_read_admission.py:48-113,168-227`.

- [ ] **Step 2: Run** `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test http_contract`. Expected failure: missing middleware exports.
- [ ] **Step 3: Implement the error and gate core.**

```rust
impl ApiError {
    pub fn internal() -> Self {
        Self { status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            detail: Json::String("Database operation failed.".into()), retry_after: None }
    }
}
impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        use axum::response::IntoResponse;
        let mut response = (self.status, axum::Json(serde_json::json!({"detail":self.detail}))).into_response();
        if let Some(seconds) = self.retry_after {
            response.headers_mut().insert(axum::http::header::RETRY_AFTER,
                seconds.to_string().parse().expect("integer header"));
        }
        response
    }
}
pub fn busy() -> ApiError {
    ApiError { status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        detail: Json::String("Public market data is temporarily busy; retry shortly.".into()),
        retry_after: Some(1) }
}
pub fn try_read(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state.reads.clone().try_acquire_owned().map_err(|_| busy())
}
pub fn request_id(supplied: Option<&str>) -> String {
    match supplied {
        Some(s) if (1..=36).contains(&s.len()) && s.bytes().all(|b|
            b.is_ascii_alphanumeric() || b"._:-".contains(&b)) => s.to_owned(),
        _ => uuid::Uuid::new_v4().to_string(),
    }
}
```

Create the semaphore with eight slots, equal to Python's aggregate public-read admission (4 workers x 2 per-worker slots, read from the attested release configuration; refuse to start if they differ). Rust pool 9 = 8 slots + 1 reserved connection. This is AD-02. Apply it to GETs selected by the Python path table and to `/api/news` (AD-03, decision 13). Hold the owned permit through response-body completion, including CSV streaming. Use an RAII body wrapper, not a permit dropped when response headers are returned. MCP uses this same semaphore inside database tools, not at initialize/list-tools/list-markets.

Compose middleware from outside inward: request ID/logging, CORS, public admission, support-context boundary, route dispatch. Preserve exact staging origin `https://staging.verdaxis.exchange`, credentials, reflected requested headers, allowed methods, Vary, and exposed headers from the recorded Python corpus (`app/main.py:225-243`). Preserve the support-scope behavior of `app/middleware/market_support_scope.py`; public GETs must not become private projections when the context header is present.

Use JSON logs with `request_id`, method, route template, status, duration_ms, error class, and SQLSTATE. Do not log SQL parameters, URLs, authorization, MCP arguments, or bodies. This deliberately omits Python's client-address field from new operational logs; it is not an HTTP contract change (`app/main.py:260-267`). Rate counters still use the correctly resolved client IP in memory.

For price limits, implement AD-01 with one atomic fixed-window bucket per resolved client IP and route, shared across all Rust connections. A window begins on the first admitted request and expires after 60 monotonic seconds. Request L+1 is rejected; rejection does not extend the window. Add `pub struct RateBucket { pub started_ms: u64, pub used: u32 }` and `pub fn admit(bucket: &mut RateBucket, limit: u32, now_ms: u64) -> bool`.

```rust
pub fn admit(bucket: &mut RateBucket, limit: u32, now_ms: u64) -> bool {
    if now_ms.saturating_sub(bucket.started_ms) >= 60_000 {
        bucket.started_ms = now_ms;
        bucket.used = 0;
    }
    if bucket.used >= limit { return false; }
    bucket.used += 1;
    true
}
#[test]
fn exact_limit_and_reset() {
    for limit in [60, 30, 10] {
        let mut b = RateBucket { started_ms: 1_000, used: 0 };
        for _ in 0..limit { assert!(admit(&mut b, limit, 1_000)); }
        assert!(!admit(&mut b, limit, 60_999));
        assert!(admit(&mut b, limit, 61_000));
    }
}
```

Add concurrent HTTP bursts over fresh and reused connections with the same client key: exactly L succeed and the rest receive 429. Test independent clients/routes, reset boundary, and spoofed forwarded headers. Keep these Rust-only threshold tests outside golden/shadow parity. Record Python's four-worker count difference only as AD-01 evidence; do not require equal counts. Preserve the source's final-hop loopback trust rule. Record exact slowapi 429 text from Python; do not assume its formatting from the limit declaration. Custom query extractors must produce Python's 422 `detail` entries, `loc`, `type`, `msg`, `input`, and `ctx` where present. Do not expose axum's default plain-text rejection.

- [ ] **Step 4: Run** middleware tests, health corpus, and cancellation test under PostgreSQL. Expected pass. Verify no secret sentinel appears in captured JSON logs. Require Rust exact-boundary burst/reset tests and record AD-01 separately; no rate-count parity gate remains.
- [ ] **Step 5: Commit** only the listed middleware/composition files: `git commit -m 'feat: preserve public HTTP admission and error contracts'` with trailer.

**Resume point:** Task 4 records the full Python oracle. Re-run these middleware tests after every later route registration.

### Task 4: Build the disposable oracle and golden comparator — Astra

**Files:** Create `scripts/rust_contract/{record,compare,seed_corpus,run_disposable,check_assertions}.py`, `tests/rust_contract/{test_compare,test_inventory,test_clock,clock}.py`, `tests/rust_contract/{cases,assertions,ordered-object-paths}.json`, `tests/rust_contract/golden/`, `rs/contract/src/{lib,main}.rs`, `rs/read/examples/contract_server.rs`. Task 4 alone registers feature-gated test targets and all test-server dispatch groups in Cargo/module files. Modify no live test target guard. Add a test-only wrapper `tests/rust_contract/server.py` around `tests.disposable_server.DisposableIdentityApp`.

**Interfaces:** Consumes the 33 method/path pairs and `tests.disposable_server.validate_config`. Produces the single authoritative Python comparator `def compare(expected: dict, actual: dict) -> list[str]` and recorder `def record(base_url: str, token: str, cases: list[dict]) -> list[dict]`. Rust exposes only `pub fn compare_group(group: &str) -> Result<(), String>`, a subprocess adapter to `venv/bin/python -m scripts.rust_contract.compare --group GROUP`. It supplies explicit `PYTHON_CONTRACT_URL`, `RUST_CONTRACT_URL`, and `DISPOSABLE_TEST_TOKEN`; the external comparator attests both endpoints before requests. Missing inputs, failed handshakes, unknown groups, zero cases, or any mismatch fail. The Rust package contains no second comparator or normalization implementation. Shadow imports this same Python comparator. Diagnostics contain schema paths/categories, never body values.

```rust
pub fn compare_group(group: &str) -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new(root.join("venv/bin/python"))
        .current_dir(&root).args(["-m", "scripts.rust_contract.compare", "--group", group])
        .status().map_err(|_| "comparator could not start")?;
    if status.success() { Ok(()) } else { Err(format!("contract group failed: {group}")) }
}
```

- [ ] **Step 1: Add comparator tests.**

```python
from scripts.rust_contract.compare import compare


def test_decimal_scale_and_null_are_not_normalized():
    base = {"status": 200, "headers": {}, "body": {"price": "1.00", "x": None}}
    assert compare(base, {**base, "body": {"price": "1", "x": None}})
    assert compare(base, {**base, "body": {"price": "1.00"}})


def test_clock_boundary_changes_are_not_normalized():
    base = {"status": 200, "headers": {}, "body": {"items": [{"id": "a"}], "staleness_status": "FRESH"}}
    assert compare(base, {**base, "body": {"items": [], "staleness_status": "STALE"}})


def test_object_membership_and_array_order():
    assert not compare({"body": {"a": 1, "b": 2}}, {"body": {"b": 2, "a": 1}})
    assert compare({"body": {"a": 1}}, {"body": {"b": 1}})
    assert compare({"body": [1, 2]}, {"body": [2, 1]})
```

- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/rust_contract/test_compare.py -q`. Expected failure: comparator module absent.
- [ ] **Step 3: Implement strict comparison and capture.** The core comparator is:

```python
def compare(expected: dict, actual: dict) -> list[str]:
    differences = []
    def walk(left, right, path):
        if type(left) is not type(right):
            differences.append(path + ":type")
        elif isinstance(left, dict):
            if left.keys() != right.keys():
                differences.append(path + ":keys")
            for key in sorted(left.keys() & right.keys()):
                walk(left[key], right[key], path + "/" + key)
        elif isinstance(left, list):
            if len(left) != len(right):
                differences.append(path + ":length")
            for index, (a, b) in enumerate(zip(left, right)):
                walk(a, b, path + "/" + str(index))
        elif left != right:
            differences.append(path + ":value")
    walk(expected, actual, "")
    return differences
```

Reject duplicate JSON keys before comparison. Parse JSON numbers without converting decimals to binary floats. Compare CSV bytes exactly, including header order, quoting, and LF versus CRLF. Compare status and Content-Type, Content-Disposition, Cache-Control, ETag, Location, Allow, Retry-After, Vary, and all CORS headers, including absence. Fix a valid X-Request-ID per case and compare its echo. Exclude only transport-generated Date, Server, Connection, Transfer-Encoding, and Content-Length from semantic comparison; verify body length independently. Keep the raw response in the disposable corpus.

**Golden layout:**

```text
tests/rust_contract/golden/<source-sha>/<case-id>/
  request.json          # method, exact escaped path/query, safe headers, body
  response.json         # status, ordered headers, parsed body or CSV bytes path
  response.body         # exact bytes; disposable data only
  meta.json             # source SHA, schema revision, seed manifest hash, UTC clock
  schema.json           # relevant Python OpenAPI/MCP schema fragment
```

Store synthetic fixture UUIDs and timestamps, not replacements for real data. Record a seeded database once and replay Rust against the same unchanged database. Never independently seed Python and Rust and then strip IDs to make them match.

**Seed procedure:** Extend the existing disposable PostgreSQL runner pattern, not a live database. It already pins a PostGIS image, binds a unique loopback port, creates exact test roles, migrates, converges ACLs, and cleans only its own container (`scripts/run_product_analytics_postgres_tests.sh:23-32,94-107,136-168,176-190`). The new runner must keep its own database alive through Python recording, Rust replay, and Caddy integration. Use a `_test` database name and explicit app/migrator/backup URLs. Do not reuse the existing runner after its EXIT cleanup. Seed through `seed_all` and `seed_session` with these explicit environment values (`app/seeds/__init__.py:9-17`; `app/seeds/safety.py:45-84,132-159`):

```bash
export ENVIRONMENT=test
export RELEASE_SHA=$(git rev-parse HEAD)
export SEED_RUNTIME_ENV=disposable
export SEED_TARGET_DATABASE=verdaxis_rust_test
export ALLOW_SEED_MUTATIONS=I_UNDERSTAND_SEED_MUTATIONS
: "${SEED_DATABASE_URL:?disposable migrator URL required}"
: "${DATABASE_URL:?disposable app URL required}"
: "${MIGRATOR_DATABASE_URL:?disposable migrator URL required}"
venv/bin/python scripts/seed.py
venv/bin/python -m scripts.rust_contract.seed_corpus
```

`seed_corpus` must first attest environment, numeric loopback, connected `_test` name, and non-superuser fixture writer. Add fixed UUID cases for ports/intelligence/news, admitted REAL owners, DEMO ownerless rows with expiry, TEST/CANARY/UNKNOWN rows, rejected/moved/unverified owners, KYC grace boundary, private FAME/LNG terms, real/demo/mixed trade snapshots, missing benchmark, and trusted/untrusted signals. Use the test harness's explicit trusted fixture setup for provenance; never grant app writes or run it on staging. Do not seed fake demo KYC. A synthetic REAL-class fixture may carry an explicit test-only LNG order price to test serialization; label it as fixture data and keep it in the disposable database. Never insert an LNG benchmark or DEMO quote. The ordinary seed remains unchanged and creates no LNG synthetic liquidity.

**Clock control:** Use a test-only injection wrapper; change no production Python helper. Passing outer `as_of` does not reach owner KYC (`app/services/market_data_eligibility.py:71,89,230,264`). Patch the exact imported `kyc_execution_clause` symbol in `app.services.market_data_eligibility` before serving; both normal-owner and assisted-customer branches then use the same explicit SQL timestamp. Also patch `current_public_order_clause` at every imported call site used by a ported route, and patch module-local datetime symbols for SQL cutoffs, generation times, certificate checks, and window aliases. Keep an explicit symbol manifest in `tests/rust_contract/clock.py`; source drift invalidates it.

```python
from datetime import date, datetime, UTC
from unittest.mock import patch
from contextlib import ExitStack
from app.models.user import User
from app.services import market_data_eligibility as eligibility
from app.services import kyc_execution


def fixed_clock_patches(now):
    original_current = eligibility.current_public_order_clause
    original_kyc = kyc_execution.kyc_execution_clause
    class FrozenDatetime(datetime):
        @classmethod
        def now(cls, tz=None):
            return now.astimezone(tz) if tz else now.replace(tzinfo=None)
    class FrozenDate(date):
        @classmethod
        def today(cls):
            return now.date()
    def current(order, *, now_expression=None):
        return original_current(order, now_expression=now)
    def kyc(user=User, *, now=None):
        return original_kyc(user, now=fixed)
    fixed = now
    stack = ExitStack()
    stack.enter_context(patch.object(eligibility, "kyc_execution_clause", kyc))
    windows = __import__("app.services.availability_windows", fromlist=["date"])
    stack.enter_context(patch.object(windows, "date", FrozenDate))
    for module in ("app.services.market_data_eligibility", "app.services.fame_order",
                   "app.services.live_benchmarks", "app.services.forward_curve_market_slices",
                   "app.services.forward_monitoring", "app.services.availability_windows", "app.schemas.curves",
                   "app.routers.orderbook", "app.routers.curves",
                   "app.routers.price_discovery", "app.routers.trade_tape", "app.mcp_server"):
        # The checked symbol manifest lists only symbols present in this tree.
        imported = __import__(module, fromlist=["datetime"])
        if hasattr(imported, "datetime"):
            stack.enter_context(patch.object(imported, "datetime", FrozenDatetime))
    for module in (eligibility, __import__("app.services.public_order_scope", fromlist=["x"]),
                   __import__("app.services.live_benchmarks", fromlist=["x"]),
                   __import__("app.routers.curves", fromlist=["x"]),
                   __import__("app.routers.inventory", fromlist=["x"]),
                   __import__("app.services.forward_curve_market_slices", fromlist=["x"])):
        stack.enter_context(patch.object(module, "current_public_order_clause", current))
    return stack
```

The wrapper first requires `ENVIRONMENT=test` and the disposable database/HTTP attestations. Test the symbol manifest against source imports; the guarded `hasattr` above must not silently omit a previously recorded clock. Add any date.today call to the manifest explicitly. Use fixed clocks immediately before, exactly at, and after grace expiry for **full HTTP** normal-owner and assisted-customer cases. Inspect compiled SQL parameters to prove both KYC EXISTS clauses bind the frozen timestamp rather than `now()`. Add Singapore-midnight certificate cases. Rust's test server receives the same UTC instant. Reject clock overrides in staging, add no public clock header, and never normalize admission results.

**Volatile fields permitted for paired live observations only:**

- `/generated_at` on price summaries/reference, snapshot, curve base/table/slice/board (`app/routers/price_discovery.py:413-416,597-600`; `app/routers/orderbook.py:842-846`; `app/routers/curves.py:815,939`; `app/schemas/curves.py:435,477`).
- Nested curve signal provenance `generated_at` fields, at schema-derived exact paths, and market-cell `generated_at` (`app/schemas/curves.py:88-111,318-340`). Do not use a recursive “delete all timestamps” rule.
- MCP `get_market_slice` result `observed_at`, which is explicitly the supplied request clock, not a stored observation (`app/services/voyage_prices.py:374-395`). Normalize the same field inside the SDK's text JSON and structured content, if both exist.
- X-Request-ID only in tests that deliberately request a generated ID; validate UUID syntax and uniqueness instead.

For each permitted timestamp, first require valid UTC syntax and a value within that request's measured start/end interval. Replace it with one fixed marker only after validation. Golden runs with a fixed clock compare it exactly. Never normalize `created_at`, `confirmed_at`, `expires_at`, certificate dates, stored `observed_at`, benchmark observation time, staleness status, price, quantity, digest, UUID, pagination, or array order. A changed data/time boundary is an inconclusive shadow pair, not a pass.

Record all query defaults and boundary cases. Add empty result, one result, populated page, out-of-range page, tie sort, invalid/duplicate/unknown query key, malformed UUID/enum/date, percent encoding, Unicode, and each handler's documented error. Record MCP initialize, tools/list, each tool, notifications, malformed JSON, unsupported method/version, missing Accept, empty Origin, invalid Host, and body limit. Snapshot Python OpenAPI and MCP tool schemas before writing Rust DTOs.

**Assertion migration ledger (required before route implementation):** Create `tests/rust_contract/assertions.json` and `scripts/rust_contract/check_assertions.py`. Each row records Python test node ID, assertion source line, parametrized cases, behavior, owner task, Rust test or golden case ID, and status. Exclusions need a reason and reviewer approval; excluded writes/seed mechanics do not excuse shared read assertions. Start from this route-to-source ledger and enumerate every assertion in these files, including class methods and parametrization:

| Owner/surface | Existing tests to inventory | Required destination |
|---|---|---|
| 3, health/admission | `tests/unit/test_public_market_read_admission.py:48-227`; runtime/readiness tests found by import search | runtime/http pure tests and contract cases |
| 5/8, catalog/news/ports | `tests/unit/test_public_market_catalog_contract.py:19`; `test_catalog_models.py`; `test_news_feed.py`; integration port assertions | pure policy vectors and simple_reads cases; exclude news-fetch mutations explicitly |
| 6/6a, visibility/benchmark | `test_public_owner_admission.py`; `test_market_evidence_policy.py`; `test_orderbook_live_benchmarks.py:291-355,440-499,660` | market_contract/live_benchmarks vectors; rebuild write assertions excluded |
| 7a, snapshot | `test_orderbook_live_benchmarks.py:149-248` | coherent depth/expiry/query-count tests |
| 7b/7c, lists/map | `test_orderbook_marketplace_filters.py`; `test_orderbook_crossing_filters.py`; `test_orderbook_schemas.py`; `test_ci_orderbook_fields.py`; `test_orderbook_map_compact.py:70-239`; `test_orderbook_router.py` | orderbook list/map cases; private templates and mutation-only assertions excluded with reasons |
| 9, prices/tape | `test_price_discovery_router.py:151-1176`; `test_reference_price_export.py`; `test_trade_tape.py` | prices/tape/CSV cases and pure rounding tests |
| 10a, legacy curves | `test_curves.py:34-116,424-896` | two-sided, one-sided, empty, filters, rounding and CSV cases |
| 10b, board/table/slice | `test_curves.py:141-378,915`; `test_forward_curve_market_slices.py` | canonical/board DTO, evidence, label and bound cases |
| 11, MCP/voyage | `test_mcp_server.py`; `test_voyage_prices.py`; `test_voyage_fuel_plan.py`; `test_sea_distances.py` | transport, privacy, pure arithmetic and price-source cases |

Test filenames without line suffix are under `tests/unit/`; they are scan inputs, not a claim that every assertion applies. Also search tests importing each ported handler/helper and add missing sources. Store the source SHA and a digest of each scanned file. This ledger transfers behavior, not Python mocks.

```python
def test_every_discovered_assertion_is_accounted_for(discovered, ledger):
    assert set(discovered) == {row["source_id"] for row in ledger}
    for row in ledger:
        assert row["target_id"] or (row["exclusion_reason"] and row["reviewed_by"])
```

The checker must prove referenced target tests/cases exist and pass in that route's result manifest. Before a route task starts, its mapping must be complete; pending target implementation is allowed. At acceptance, require **zero unmapped and zero unpassed applicable assertions**. Each split task owns its subset and records its own count.

**One disposable runner, explicit launches:** `run_disposable` builds with a full RELEASE_SHA, starts its own database and Python wrapper, then starts `rs/read/examples/contract_server.rs` with `--group` selecting only completed route modules. The example exposes the disposable identity endpoint only in test mode and numeric loopback. It allocates ephemeral ports and validates both handshakes. It then invokes the relevant `cargo test --features contract-tests --test TARGET` and external comparator, with explicit URLs/token. It cleans only its children/database in a finally block. Unknown group or unavailable route fails. No task depends on Task 13's global router.

| Checkpoint | Exact launch/acceptance command |
|---|---|
| 6 | `venv/bin/python -m scripts.rust_contract.run_disposable --group market_contract --rust-target market_contract` |
| 6a | `venv/bin/python -m scripts.rust_contract.run_disposable --group live_benchmarks --rust-target live_benchmarks` |
| 7a | `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_snapshot --rust-target orderbook_snapshot` |
| 7b | `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_lists --rust-target orderbook_lists` |
| 7c | `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_map --rust-target orderbook_map` |
| 8 | `venv/bin/python -m scripts.rust_contract.run_disposable --group simple_reads --rust-target simple_reads` |
| 9 | `venv/bin/python -m scripts.rust_contract.run_disposable --group prices --rust-target prices` |
| 10a | `venv/bin/python -m scripts.rust_contract.run_disposable --group curves_legacy --rust-target curves_legacy` |
| 10b | `venv/bin/python -m scripts.rust_contract.run_disposable --group curves_canonical --rust-target curves_canonical` |
| 11 | `venv/bin/python -m scripts.rust_contract.run_disposable --group mcp --rust-target mcp` |

Task 4 validates this runner with health and a test-only echo projection; it does not require future routes to pass. Add all group dispatch/module declarations here with 404 initial routers. Later workers replace only their assigned modules. Pure tests remain independently runnable without the feature. PostgreSQL tests in Tasks 2/3 use the existing disposable runner until this harness exists; they do not depend on Task 4.

- [ ] **Step 4: Run** comparator unit tests, inventory/ledger meta-tests, runner self-test, and the complete Python recording twice against the unchanged database. Expected: comparator tests pass; repeated captures differ only at explicitly validated volatile paths. Any unstable array is a recorded source limitation that blocks claiming exact parity until resolved. Do not silently sort it.
- [ ] **Step 5: Commit** the scripts, test wrapper, synthetic fixtures, and contract package: `git commit -m 'test: record public Python contracts on disposable PostgreSQL'` with trailer.

**Resume point:** Record corpus SHA, seed manifest SHA, schema revision, tool versions, and exact route count. Tasks 5–11 consume this corpus. Never regenerate expected results merely because Rust differs.

### Task 4b: Build every corpus fixture family and record the golden corpus once — Astra

**Added:** 15 Oct by the PM (decision 15). **Starts after:** the build branch is rebased onto current `origin/staging`, including the KYC execution gate switch.

**Files:** Modify `scripts/rust_contract/seed_corpus.py`, `scripts/rust_contract/run_disposable.py`, `tests/rust_contract/cases.json`, `tests/rust_contract/assertions.json`. Create `tests/rust_contract/test_seed_corpus.py`. Regenerate `tests/rust_contract/golden/<oracle-sha>/`, `deploy/rust/market-policy.json`, the MCP artifact, the contract manifest and the terms vectors from the same oracle SHA.

**Interfaces:**
- Consumes: the `seed_corpus` rule in Task 4 (attested environment, numeric loopback, `_test` database name, non-superuser fixture writer); `scripts/rust_contract/export_policy.py`; the Task 6 vector exporter.
- Produces: `python -m scripts.rust_contract.seed_corpus` that builds every family and exits non-zero on any missing family; `run_disposable.py --seed-mode corpus` as the default gate for every group; one golden directory for one oracle SHA.

**Families (each with fixed UUIDs, row counts and its assertion ledger rows):** the families named in the Task 4 seed rule; ports with and without intelligence, one port with two intelligence rows (fixed row order), high-precision PostGIS coordinates; news with tied dates, null optional fields and every category; orders at delivery-point names and region names (`Singapore` and `Asia`); stored FAME and LNG terms with small and scientific decimals (`1E-29`, `0E-100`); KYC owners in both gate states (pin the oracle's `KYC_EXECUTION_GATE_ENABLED` per run and record it); historical version-0 terms and quarantined trade snapshots built through the legal path of a pre-guard revision, then upgraded by the real migration chain (Task 6 N3).

- [ ] **Step 1: Write the failing test.** `tests/rust_contract/test_seed_corpus.py` asserts each family's exact row count and that `seed_corpus` refuses a superuser writer, a non-loopback host and a non-`_test` database.
- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/rust_contract/test_seed_corpus.py -q`. Expected: FAIL (families missing).
- [ ] **Step 3: Implement the families** in `seed_corpus.py` through the fixture writer only. Never disable a database guard or trigger to insert a row.
- [ ] **Step 4: Run** the test again. Expected: PASS.
- [ ] **Step 5: Record once.** Record every group twice under the fixed clock; require `TOTAL_UNSTABLE=0`. Regenerate the policy, MCP artifact, manifest and vectors from the same oracle SHA. Run every group with `--seed-mode corpus`; list each Rust difference by group.
- [ ] **Step 6: Commit** the fixtures, tests and regenerated artifacts in separate commits.

**Resume point:** every group records and compares under `--seed-mode corpus`. Route differences found here go back to the owning task as fix items; Task 4b does not change route code.

### Task 5: Export catalog policy as data — Astra

**Files:** Create `scripts/rust_contract/export_policy.py`, `deploy/rust/market-policy.json`, `rs/read/src/market/{mod,policy}.rs`, `rs/read/tests/policy.rs`, `tests/rust_contract/test_policy_export.py`.

**Interfaces:** Consumes `CANONICAL_PRODUCTS`, `CANONICAL_DELIVERY_POINTS`, and the approved qualification rules. Produces `pub fn load_policy(bytes: &[u8], expected_sha: &str) -> Result<MarketPolicy, String>` and `pub fn product_matches(policy: &ProductPolicy, id: uuid::Uuid, name: &str, fuel_type: &str, fuel_grade: &str, active: bool) -> bool`. Policy `source_sha` identifies the Python oracle commit, not necessarily the release commit that contains the generated file.

- [ ] **Step 1: Add policy tests.**

```rust
#[test]
fn active_alias_is_not_a_canonical_product() {
    let p = ProductPolicy { id: uuid::Uuid::nil(), code: "TEST_CODE".into(),
        name: "Fixture product".into(), fuel_type: "Fixture type".into(),
        fuel_grade: "Fixture grade".into(), execution_mode: "ORDERBOOK".into(),
        lanes: vec![], qualification_profile: "scalar".into(), display_order: 1 };
    assert!(product_matches(&p, p.id, &p.name, &p.fuel_type, &p.fuel_grade, true));
    assert!(!product_matches(&p, p.id, "Alias", &p.fuel_type, &p.fuel_grade, true));
    assert!(!product_matches(&p, p.id, &p.name, &p.fuel_type, &p.fuel_grade, false));
}
```

Add a fixture-data test with three distinct B30/B100/UCOME identities. Assert no remapping and no code change when a new policy record is supplied. This proves the mechanism, not permission to widen staging's live catalog. Add a source scan that refuses those names and their UUIDs in `rs/read/src` and SQL files.

- [ ] **Step 2: Run** `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test policy`. Expected failure: policy functions absent.
- [ ] **Step 3: Export the source-attested tuples and implement matching.**

```python
# Core of export_policy.py. Run only against the approved clean source tree.
products = [
    {"id": str(p.id), "code": p.market_product.value, "name": p.name,
     "fuel_type": p.fuel_type, "fuel_grade": p.fuel_grade,
     "execution_mode": p.execution_mode,
     "lanes": [str(x) for x in (p.available_delivery_point_ids or tuple(q.id for q in CANONICAL_DELIVERY_POINTS))],
     "qualification_profile": profiles[p.market_product.value],
     "display_order": i}
    for i, p in enumerate(CANONICAL_PRODUCTS)
]
```

`profiles` is a reviewed Python export mapping: current UCOME uses `fame_v1`, LNG uses `lng_v1`, and the four alcohol contracts use `scalar`. Keep it outside Rust. Include all source fields needed by MCP (`unit`, `min_lot_size`, `spec_description`) in the policy payload or a separate exported MCP catalog payload. Do not infer them from labels. Validate duplicate codes/UUIDs, unknown profiles, empty lanes, point references, and source SHA before loading. Commit a digest in the artifact manifest.

```rust
pub fn product_matches(p: &ProductPolicy, id: uuid::Uuid, name: &str,
    fuel_type: &str, fuel_grade: &str, active: bool) -> bool {
    active && p.id == id && p.name == name && p.fuel_type == fuel_type && p.fuel_grade == fuel_grade
}
```

Use `jsonb_to_recordset($1::jsonb)` or bound UUID/label arrays to join policy records in SQL. Do not select all active products and filter after pagination. The database remains the source for mutable fields; the policy remains the source for approved identity, lane, and display order. Unknown active rows are excluded.

- [ ] **Step 4: Run** policy export/load tests and pure catalog-projection vectors captured from `app/routers/catalog.py:20-49` and `app/mcp_server.py:50-71`. Expected pass. Do not require unavailable HTTP handlers here; R03/R04 acceptance belongs to Task 8 and list_markets to Task 11. Add `pub fn project_catalog(policy: &MarketPolicy, rows: &[Json]) -> Result<Json, String>` for ordered, strict identity projection of synthetic database rows. Regenerate export into `work/` and byte-compare with the committed file; drift fails CI.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: load canonical market policy from attested data'` with trailer.

**Resume point:** Record the policy source SHA and digest. If Phase 0 changes catalog policy, repeat this task and Task 4 before port acceptance.

### Task 6: Port shared visibility, decimal, digest, and public projection rules — Astra

**Files:** Create `rs/read/src/market/{visibility,provenance,terms,decimal,windows}.rs`, `rs/read/sql/{public_orders,trade_evidence}.sql`, `rs/read/tests/market_contract.rs`. Create synthetic digest vectors in `tests/rust_contract/vectors/terms/` (synthetic vectors, not golden pairs, so they stay outside the `golden/<source-sha>/<case-id>/` layout).

**Interfaces:** Consumes `MarketPolicy`, SQLx transactions, and policy-bound JSON. Produces:

```rust
pub fn decimal_text(value: rust_decimal::Decimal) -> String;
pub fn digest_terms(canonical_terms: &Json) -> Result<String, String>;
pub fn redact_terms(profile: &str, terms: &Json) -> Result<Json, String>;
pub fn normalize_window(value: &str, today: chrono::NaiveDate) -> Result<String, String>;
pub fn public_order_sql() -> &'static str;
pub fn trade_evidence_sql() -> &'static str;
// market::visibility: sealed static-SQL composition; no request text in SQL.
pub enum OrderSurface { Orderbook, Listings, Candidates }
pub struct OrderQuery(/* private */);
pub struct TradeQuery(/* private */);
pub fn order_query_sql(tail: &'static str) -> OrderQuery;
pub fn trade_query_sql(tail: &'static str) -> TradeQuery;
pub fn bind_order_query<'q>(sql: &OrderQuery, policy: &MarketPolicy,
    clock: &VisibilityClock, config: &Config, filters: &PublicOrderFilters<'_>)
    -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, sqlx::Error>;
pub fn bind_trade_query<'q>(sql: &TradeQuery, policy: &MarketPolicy,
    filters: &TradeEvidenceFilters<'_>)
    -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, sqlx::Error>;
// Digest input never narrows unconstrained NUMERIC to rust_decimal.
pub fn canonical_decimal_text(text: &str) -> Result<String, String>;
pub fn canonical_terms_from_row(row: &Json) -> Result<Json, String>;
// In market::terms. Port of app/services/execution_policy.py:112-150 order_is_execution_qualified.
// Tasks 6a and 10b consume it; no other task reimplements it. The product rules come from
// MarketPolicy (Task 5), never from product names in code.
pub struct QualificationInput<'a> {
    pub product_id: Option<uuid::Uuid>,
    pub market_product: Option<&'a str>,
    pub side: &'a str,
    pub delivery_point_id: Option<&'a str>,
    pub availability_window: &'a str,
    pub off_spec: bool,
    pub msds_available: bool,
    pub certification_scheme: Option<&'a str>,
    pub certification_declared: bool,
    pub fame_terms: Option<&'a Json>,
    pub lng_terms: Option<&'a Json>,
}
pub fn order_is_execution_qualified(policy: &MarketPolicy, order: &QualificationInput<'_>) -> bool;
pub fn order_is_execution_qualified_at(policy: &MarketPolicy,
    order: &QualificationInput<'_>, singapore_date: chrono::NaiveDate) -> bool;
```

`public_order_sql()` and `trade_evidence_sql()` now return unpaged internal relations. The order relation projects order columns, catalog labels, and `private_digest_input` with exact NUMERIC text. Never serialize that internal object publicly. `order_query_sql(static_tail)` wraps it as `eligible_orders`; `trade_query_sql(static_tail)` wraps snapshots as `eligible_trades`. Sealed query types prevent callers from passing request strings to the bind helpers. Static tails may add CTEs, exact tuple filters, ranking, aggregation and pagination. Run the bound query on the admitted transaction connection.

Order binds are policy, UTC instant, Singapore date, off-spec toggle, product, point, side, fuel, market product, region, window, surface, runtime KYC gate (`$1..$13`). Additional tail binds start at `$14`. The gate comes only from `Config.kyc_execution_gate_enabled` (default true), never request filters. Trade binds are policy, inclusive since, exclusive before, product, point, window (`$1..$6`); tail binds start at `$7`. `PublicOrderFilters.surface` is `Orderbook` by default. `Listings` applies its narrower ASK query without main scalar qualification. `Candidates` retains shared catalog/current/provenance rules but leaves scalar/typed qualification to benchmarks and curves. No relation applies a positive remaining-quantity filter. Consumers apply it where Python does. Existing ID helpers remain test/convenience adapters, not route pagination APIs. `skip`/`limit` on filter structs are used only by those adapters.

Tasks 6a and 10b must use `VisibilityClock::at(now).singapore_date` with `order_is_execution_qualified_at`; never read the process clock within a captured-clock request.

- [ ] **Step 1: Add regression tests.**

```rust
#[test]
fn private_terms_digest_matches_python() {
    let vector: Json = serde_json::from_str(include_str!(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/rust_contract/vectors/terms/private.json"))).unwrap();
    assert_eq!(digest_terms(&vector["canonical_terms"]).unwrap(), vector["digest"]);
    let public = redact_terms("lng_v1", &vector["lng_terms"]).unwrap();
    assert!(public.get("scheme_certificate_number").is_none());
    assert!(public.get("physical_supplier_name").is_none());
}
#[test]
fn zero_and_null_stay_distinct() {
    assert_eq!(decimal_text(rust_decimal::Decimal::ZERO), "0");
    assert_ne!(serde_json::json!("0.00"), serde_json::Value::Null);
}
```

Add a PostgreSQL visibility matrix for each row in the public rules table. Include REAL/DEMO/UNKNOWN/TEST/CANARY, ownerless, moved owner, rejected KYC with grace, grace exactly at expiry, assisted admin/customer combinations, off-spec, wrong lane, missing terms, expired certificate, null expiry, and historical reseller declarations. Compare selected IDs with Python under the same clock. Test trade evidence independently; do not reuse current owner admission for immutable trades.

- [ ] **Step 2: Run** `cargo test --manifest-path rs/Cargo.toml -p verdaxis-read --test market_contract`. Expected failure: missing shared functions or SQL.
- [ ] **Step 3: Implement pure digest helpers and SQL rules.**

```rust
// Superseded by the M3 repair: digest decimals use the text-based 28-digit
// half-even normalization in rs/read/src/market/decimal.rs, not rust_decimal.
pub fn digest_terms(canonical_terms: &Json) -> Result<String, String> {
    fn sorted(value: &Json) -> Json {
        match value {
            Json::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for key in keys { out.insert(key.clone(), sorted(&map[key])); }
                Json::Object(out)
            }
            Json::Array(items) => Json::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    use sha2::Digest;
    let bytes = serde_json::to_vec(&sorted(canonical_terms)).map_err(|_| "invalid terms")?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}
```

The canonical input builder must emit every field in `app/services/order_terms.py:60-112`, including private identifiers, exact boolean defaults, omitted absent LNG terms, normalized Decimal strings, and UTC microsecond `Z` timestamps. Do not hash public DTOs. `decimal_text` is for the digest only; wire amounts retain Python scale. Add non-ASCII and escaping vectors because different JSON encoders can hash different bytes.

Implement visibility SQL from the cited predicates, with SQL three-valued null behavior intact. Use the exact enabled KYC clause below inside the full owner/assisted branches. With runtime `KYC_EXECUTION_GATE_ENABLED=false`, use `u.kyc_status <> 'REJECTED' AND (u.kyc_organization_id IS NULL OR u.kyc_organization_id = u.organization_id)` instead; grace does not override a foreign review in advisory mode:

```sql
u.kyc_status <> 'REJECTED'
AND (
  (u.kyc_status = 'APPROVED' AND u.kyc_organization_id = u.organization_id)
  OR EXISTS (
    SELECT 1 FROM kyc_execution_grace g
    WHERE g.user_id = u.id AND g.organization_id = u.organization_id
      AND g.expires_at > $2::timestamptz
  )
)
```

Do not infer `is_verdaxis_verified` from declared certificates. Use allowlisted public term fields from the Python public schema rather than deleting only two known secrets. Preserve derived `reseller` and `fueleu_recognition` fields. Match FAME's public model separately. Port window aliases and sort keys from `app/services/availability_windows.py`; invalid query handling remains route-specific.

- [ ] **Step 4: Run** all pure vectors and PostgreSQL visibility/evidence comparisons. Expected pass. Use runtime app grants for reads. Verify every generated SQL query is parameterized. Confirm no secret sentinel appears in serialized public JSON.
- [ ] **Step 5: Commit** listed shared modules and vectors: `git commit -m 'feat: preserve public market visibility and reviewed terms'` with trailer.

**Resume point:** Publish function signatures and policy/vector digests in progress. Route workers may now start. They must not fork these predicates.

### Task 6a: Port read-only live-slice benchmarks — Astra

**Files:** Create `rs/read/src/market/live_benchmarks.rs`, `rs/read/sql/live_benchmarks.sql`, `rs/read/tests/live_benchmarks.rs`. Task 3 pre-registers the module; Task 6a owns it exclusively.

**Interfaces:** Consumes shared visibility, policy, typed qualification and Decimal rules. Produces: Uses `order_query_sql(&'static str) -> OrderQuery` and `bind_order_query(&OrderQuery, &MarketPolicy, &VisibilityClock, &Config, &PublicOrderFilters) -> Result<Query, sqlx::Error>` with `OrderSurface::Candidates`; applies typed qualification with `order_is_execution_qualified_at` before aggregation.

```rust
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct LiveBenchmarkKey {
    pub side: String, pub product: String, pub point: uuid::Uuid, pub window: String,
}
pub struct LiveBenchmark {
    pub price: rust_decimal::Decimal,
    pub quantity: rust_decimal::Decimal,
    pub count: u64,
}
pub async fn live_benchmarks(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy: &MarketPolicy, keys: &[LiveBenchmarkKey], now: chrono::DateTime<chrono::Utc>)
    -> Result<std::collections::HashMap<LiveBenchmarkKey, LiveBenchmark>, ApiError>;
pub fn qualified_vwap(rows: &[(rust_decimal::Decimal, rust_decimal::Decimal)])
    -> Option<rust_decimal::Decimal>;
```

- [ ] **Step 1: Write the failing arithmetic and PostgreSQL fixture tests.**

```rust
#[test]
fn mixed_real_demo_vwap_matches_current_python() {
    use rust_decimal::Decimal as D;
    // Fixture provenance: REAL 10 MT at 100, DEMO 30 MT at 200.
    assert_eq!(qualified_vwap(&[(D::from(10), D::from(100)),
        (D::from(30), D::from(200))]), Some(D::from(175).round_dp(2)));
}
```

Add `visible_but_benchmark_ineligible`: a typed ASK whose stored declaration satisfies the SQL side/date/lane tests but fails a required typed declaration field remains visible to the shallow listing predicate and is excluded by benchmark execution qualification (`app/services/market_data_eligibility.py:231-249,266-273`; `app/services/live_benchmarks.py:73-76`). Also test an OPEN zero-remaining-quantity row where the listing predicate permits it but the benchmark rejects it. Do not invent an allowed-scheme list: `normalize_certification_scheme` only trims and uppercases (`app/services/execution_policy.py:107-111`). Add typed FAME and LNG invalid-declaration cases, positive remaining-quantity checks, mixed classes, sparse keys, missing results, and as-of expiry. Use a small test-only HTTP projection of the Python/Rust helper through the Task 4 runner; no public route or writer is added.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group live_benchmarks --rust-target live_benchmarks`. Expected failure: helper absent.
- [ ] **Step 3: Implement the batched SELECT and typed qualification before aggregation.**

```rust
pub fn qualified_vwap(rows: &[(rust_decimal::Decimal, rust_decimal::Decimal)])
    -> Option<rust_decimal::Decimal> {
    use rust_decimal::{Decimal, RoundingStrategy};
    let quantity: Decimal = rows.iter().map(|(q, _)| *q).sum();
    if quantity <= Decimal::ZERO { return None; }
    let weighted: Decimal = rows.iter().map(|(q, p)| *q * *p).sum();
    Some((weighted / quantity).round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven))
}
```

Port `public_slice_order_qualified` exactly, including catalog tuple, lane, status, expiry, positive remaining quantity, off-spec, typed FAME/LNG `order_is_execution_qualified` (call `market::terms::order_is_execution_qualified_at` from Task 6 with the request's captured Singapore date; never the live-clock wrapper; do not reimplement it), and normalized scalar ASK certification (`app/services/live_benchmarks.py:35-93`; `app/services/execution_policy.py:114-145`). The map key has side/product/point/window only: do not add provenance. Cache missing keys within the request. Never invoke rebuild or persist `live_slice_benchmarks`.

- [ ] **Step 4: Re-run** the named runner, pure arithmetic tests, and assertion ledger check. Expected pass with zero helper differences and one batched data query. This task does not depend on Task 7's routes.
- [ ] **Step 5: Commit** the three listed files and fixture cases: `git commit -m 'feat: preserve current live slice benchmark reads'` with trailer.

**Resume point:** Tasks 7a/7b consume this exact interface. Curves use external benchmark quotes separately; Task 10 does not own or redefine this helper.

### Task 7a: Port the coherent order-book snapshot — Astra

**Files:** Create `rs/read/src/routes/orderbook_snapshot.rs`, `rs/read/sql/orderbook/snapshot.sql`, `rs/read/tests/orderbook_snapshot.rs`.

**Interfaces:** Consumes shared visibility, policy, digest, `live_benchmarks`, and read-only SQLx transactions. Produces `pub fn routes() -> axum::Router<AppState>` and `pub async fn snapshot(state: axum::extract::State<AppState>, query: axum::extract::RawQuery) -> ApiResult`. RawQuery uses the Python-compatible validation layer. Covers R15 only. Uses the Task 6 `order_query_sql`/`bind_order_query` signatures with `OrderSurface::Orderbook`; ranks `eligible_orders` after shared eligibility, and reads `private_digest_input` before redaction.

- [ ] **Step 1: Add corpus and edge tests.**

```rust
#[test]
fn snapshot_contract_matches() {
    verdaxis_contract::compare_group("orderbook_snapshot").unwrap();
}
```

Add a concurrent writer test around snapshot reads: update a price between depth and benchmark queries; assert each snapshot is entirely before or entirely after the update. Test 16 orders per side, equal-price acceptance order, differing provenance, typed benchmark exclusion, and public/private route exclusion.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_snapshot --rust-target orderbook_snapshot`. Expected failure: Rust route absent or nonmatching.
- [ ] **Step 3: Implement read transactions and ranked depth.**

```rust
let mut tx = state.pool.begin().await.map_err(|_| ApiError::internal())?;
sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
    .execute(&mut *tx).await.map_err(|_| ApiError::internal())?;
let generated_at = chrono::Utc::now();
```

```sql
row_number() OVER (
  PARTITION BY o.side
  ORDER BY
    CASE WHEN o.side = 'BID' THEN o.price_per_mt_usd END DESC,
    CASE WHEN o.side = 'ASK' THEN o.price_per_mt_usd END ASC,
    o.acceptance_ordinal ASC,
    o.id ASC
) AS depth_rank
```

Select only ranks <=15. Keep the captured time through visibility and benchmark reads. Fetch benchmark inputs once for all visible keys, including missing results. Keep `is_crossed` separated by provenance and exact product/point/window (`app/routers/orderbook.py:290-297,813-838`). Do not calculate it from all products at a port.

- [ ] **Step 4: Run** the named runner, snapshot concurrency, digest vectors, and query-count tests. Expected pass for R15 and zero unmapped snapshot assertions. Confirm one batched benchmark read.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port coherent public orderbook snapshot'` with trailer.

**Resume point:** Record R15 acceptance. Task 7b can start; its lists do not change snapshot isolation.

### Task 7b: Port paginated and legacy order lists — Astra

**Files:** Create `rs/read/src/routes/orderbook_lists.rs`, `rs/read/sql/orderbook/{bids,asks,list,with_ci}.sql`, `rs/read/tests/orderbook_lists.rs`.

**Interfaces:** Consumes `live_benchmarks`, public projection and digest helpers. Produces `pub fn routes() -> axum::Router<AppState>` and `pub fn page_offset(skip: u64, limit: u64, page: Option<u64>, page_size: Option<u64>) -> Option<(u64,u64)>`. Covers R16–R18 and R26. Uses the Task 6 `order_query_sql`/`bind_order_query` signatures with `OrderSurface::Orderbook`; static tails own ordering, count and pagination.

- [ ] **Step 1: Add the failing test.**

```rust
#[test]
fn ask_offset_past_end_keeps_total() {
    verdaxis_contract::compare_group("ask_offset_past_end").unwrap();
}
```

Add all sort ties, page aliases, full public DTO/digest, crossed-state provenance, mixed live benchmark, and null LNG CI overlay cases.
- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_lists --rust-target orderbook_lists`. Expected failure: handlers absent.
- [ ] **Step 3: Implement list queries with bound pagination.**

```rust
pub fn page_offset(skip: u64, limit: u64, page: Option<u64>, page_size: Option<u64>)
    -> Option<(u64,u64)> {
    let size = page_size.unwrap_or(limit);
    let offset = match page { Some(p) => p.checked_sub(1)?.checked_mul(size)?, None => skip };
    Some((offset, size))
}
```

For ASK, use `count(*) OVER()` and a fallback total query when a nonzero offset returns no row (`app/routers/orderbook.py:973-1012`). Preserve BID total semantics, all stable sort clauses, and legacy created-time order. Use one benchmark batch per request. CI remains UNPRICED with zero financial adjustment and no LNG overlay (`app/routers/orderbook.py:1080-1095`). Do not reuse snapshot's 15-row cap.
- [ ] **Step 4: Re-run** the named runner, offset regression, and assertion ledger. Expected pass for R16–R18/R26; no per-row benchmark queries.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port public orderbook list projections'` with trailer.

**Resume point:** Record four-route acceptance and sorted-page evidence. Task 7c owns map/compatibility output, not this module.

### Task 7c: Port map, count, distinct, and compatibility projections — Astra

**Files:** Create `rs/read/src/routes/{orderbook_map,listings}.rs`, `rs/read/sql/orderbook/{aggregate,counts,recent_asks,distinct,listings}.sql`, `rs/read/tests/orderbook_map.rs`.

**Interfaces:** Consumes shared visibility and policy. Produces `pub fn routes() -> axum::Router<AppState>` in both modules and `pub fn product_counts(codes: &[String], rows: &[(String,u64)]) -> std::collections::BTreeMap<String,u64>`. Covers R19–R25/R27. Uses the Task 6 `order_query_sql`/`bind_order_query` signatures; `Orderbook` for maps/counts/distinct, `Listings` for R27. No predicate copying.

- [ ] **Step 1: Add the failing test.**

```rust
#[test]
fn map_and_compatibility_match_python() {
    verdaxis_contract::compare_group("orderbook_map").unwrap();
}
```

Include empty zero-count products, separate evidence-class groups, all-window totals, nearest demo window, latest ASK per point, distinct lexical order, and listings visible under its narrower qualification.
- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group orderbook_map --rust-target orderbook_map`. Expected failure: projections absent.
- [ ] **Step 3: Implement zero-filled counts and route-specific projections.**

```rust
pub fn product_counts(codes: &[String], rows: &[(String,u64)])
    -> std::collections::BTreeMap<String,u64> {
    let mut counts: std::collections::BTreeMap<_,_> = codes.iter().map(|c| (c.clone(),0)).collect();
    for (code, count) in rows { if let Some(value) = counts.get_mut(code) { *value = *count; } }
    counts
}
```

Apply eligibility before grouping. Preserve compact map reduction (`app/routers/orderbook.py:1663-1685`). Listings retains its own dict serializer and narrower query (`app/routers/inventory.py:762-832`); do not impose every main-orderbook qualification.
- [ ] **Step 4: Re-run** the named runner and assertion ledger. Expected pass for eight routes, no private fields, no lost totals.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port public map and compatibility reads'` with trailer.

**Resume point:** Task 13 merges the three orderbook modules plus listings. Keep all private GETs and writes on Python.

### Task 8: Port ports, catalog, and news from the fixed pattern — DeepSeek

**Files:** Create `rs/read/src/routes/{ports,catalog,news}.rs`, `rs/read/sql/{ports,port,catalog_products,catalog_points,news}.sql`, `rs/read/tests/simple_reads.rs`.

**Interfaces:** Consumes `AppState`, policy, `ApiError`, and Python-compatible query parsing. Produces `pub fn routes() -> axum::Router<AppState>` in each module. Covers R01–R05. DeepSeek must not edit shared modules, manifests, or the global router.

- [ ] **Step 1: Add the fixed-pattern tests.**

```rust
#[test]
fn simple_reads_match_python() {
    for group in ["ports", "catalog", "news"] {
        verdaxis_contract::compare_group(group).unwrap();
    }
}
```

Cases include null location/intelligence, inactive port still returned by the unfiltered port route, unknown port 404, inactive/alias catalog exclusion, empty news, nullable dates, category empty string, relevance bounds, and repeated limit. Port coordinates remain JSON numbers, not strings.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group simple_reads --rust-target simple_reads`. Expected failure: missing routes or contract differences.
- [ ] **Step 3: Implement parameterized queries and ordered DTOs.**

```sql
-- news.sql
SELECT id, title, summary, source, source_url, url, category, relevance,
       published_at, fetched_at
FROM news_items
WHERE relevance >= $1 AND ($2::text IS NULL OR $2 = '' OR category = $2)
ORDER BY published_at DESC
LIMIT $3
```

```rust
#[derive(serde::Serialize)]
pub struct NewsResponse {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub source: String,
    pub source_url: Option<String>,
    pub url: String,
    pub category: String,
    pub relevance: i32,
    pub published_at: Option<String>,
    pub fetched_at: Option<String>,
}
```

Check each database column's nullability against `app/models/news.py` before finalizing SQLx row types; keep wire nulls from the corpus. Format news dates using Python `isoformat()` semantics, not the Pydantic datetime formatter. Ports must cast geography to geometry before `ST_X/ST_Y` and return `location:null` (`app/routers/ports.py:18-34`). Do not add an `is_active` filter to ports. Catalog joins policy tuples before applying active status and display order. Do not derive product codes from text in Rust.

- [ ] **Step 4: Run** R01–R05 corpus, rustfmt, and clippy. Expected pass. Astra reviews all five SQL queries, field types, ordering, and header absence before merge.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port ports catalog and news reads'` with trailer.

**Resume point:** Record the reviewed commit and five route results. Task 13 registers these modules without changing their SQL.

### Task 9: Port price discovery and trade tape — Astra

**Files:** Create `rs/read/src/routes/{prices,trade_tape}.rs`, `rs/read/sql/prices/{summary,latest,quarantine,reference,tape,tape_count}.sql`, `rs/read/tests/prices.rs`.

**Interfaces:** Consumes trade evidence policy, Decimal formatting, window normalization, rate limiter, and query validation. Produces `pub fn routes() -> axum::Router<AppState>` in both modules and `pub fn reference_csv(rows: &[Json]) -> Result<Vec<u8>, String>`. Covers R06 and R12–R14. Uses `trade_query_sql(&'static str) -> TradeQuery` and `bind_trade_query(&TradeQuery, &MarketPolicy, &TradeEvidenceFilters) -> Result<Query, sqlx::Error>`; static tails separate evidence classes and aggregate/page `eligible_trades`.

- [ ] **Step 1: Add evidence and CSV tests.**

```rust
#[test]
fn prices_and_tape_match_python() {
    for group in ["prices", "reference", "reference_csv", "trade_tape"] {
        verdaxis_contract::compare_group(group).unwrap();
    }
}
#[test]
fn unknown_trade_has_no_economics() {
    verdaxis_contract::compare_group("unknown_trade_quarantine").unwrap();
}
```

Include mismatched buyer/seller provenance, mutable product/order edits after confirmation, seven-day and hour-cutoff equality, exact delivery-point privacy, demo_only, no trade rows, zero-volume reference, alias conflicts, invalid date order, and `format=other` still returning current CSV behavior. Tape fuel/product matching is exact; region matches region OR point name; with a point filter the output region becomes the point name (`app/routers/trade_tape.py:30-59,106-118`). Prices use case-insensitive substring fuel/region filters and arithmetic average, not VWAP (`app/routers/price_discovery.py:162-165,198,265-268`). Tape's invalid window is not caught as price's 422; record its actual HTTP server-error response with exception propagation disabled (`app/routers/trade_tape.py:117-118`). Do not silently normalize that error contract.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group prices --rust-target prices`. Expected failure: absent Rust handlers.
- [ ] **Step 3: Implement immutable aggregation.**

```sql
SELECT product_id, product_name, market_product, fuel_type, fuel_grade,
       delivery_point_id, delivery_point_name, delivery_point_region,
       availability_window, date(confirmed_at) AS trade_date,
       sum(price_per_mt_usd * quantity_mt) AS weighted_sum,
       sum(quantity_mt) AS total_volume, count(*) AS trade_count
FROM trades
WHERE status IN ('CONFIRMED','DELIVERED','PAID')
  AND buyer_provenance = 'REAL' AND seller_provenance = 'REAL'
  AND confirmed_at IS NOT NULL
GROUP BY product_id, product_name, market_product, fuel_type, fuel_grade,
         delivery_point_id, delivery_point_name, delivery_point_region,
         availability_window, date(confirmed_at)
ORDER BY trade_date DESC
```

This is the aggregation core, not the full admissibility query. Apply `trade_evidence_sql()` canonical snapshot/version/window predicates and bound filters before GROUP BY. Preserve separate REAL/DEMO summaries and quarantine-only UNKNOWN metadata. Use round-half-even where Python `round`/default Decimal context does so; do not reuse voyage half-up rounding. Build CSV with the exact columns at `app/routers/price_discovery.py:524-547` and CRLF termination. Trade tape uses the first eight hex characters of trade UUID and only exposes delivery-point fields when that filter is supplied (`app/routers/trade_tape.py:30-59`). No current organization lookup changes immutable historical evidence.

- [ ] **Step 4: Run** corpus, Decimal tie-rounding vectors, SQLx query checks, and rate-limit tests. Expected pass. Confirm no benchmark or demo values enter the REAL reference endpoint.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port price discovery and anonymized trade tape'` with trailer.

**Resume point:** Publish CSV byte fixtures and historical-evidence results. Task 10 may reuse read helpers but must not change these contracts.

### Task 10a: Port legacy curve JSON and CSV — Astra

**Files:** Create `rs/read/src/routes/curves_legacy.rs`, `rs/read/sql/curves/legacy_orders.sql`, `rs/read/tests/curves_legacy.rs`.

**Interfaces:** Consumes shared visibility and Decimal/window rules. Produces `pub fn routes() -> axum::Router<AppState>` and `pub fn curve_csv(points: &[Json]) -> Result<Vec<u8>, String>`. Covers R07/R08 only.

- [ ] **Step 1: Add the failing contract test.**

```rust
#[test]
fn legacy_curve_contracts_match() {
    verdaxis_contract::compare_group("curves_legacy").unwrap();
}
```

Map two-sided and one-sided curve assertions from `tests/unit/test_curves.py:34-84`; add no data, aliases, real-only economics, null-to-empty CSV, half-even ties, and exact LF bytes.
- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group curves_legacy --rust-target curves_legacy`. Expected failure: handlers absent.
- [ ] **Step 3: Implement REAL-only grouping and nullable midpoint.**

```rust
pub fn midpoint(bid: Option<rust_decimal::Decimal>, ask: Option<rust_decimal::Decimal>)
    -> Option<rust_decimal::Decimal> {
    bid.zip(ask).map(|(b,a)| ((b+a)/rust_decimal::Decimal::from(2))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointNearestEven))
}
```

Export midpoint from this module. Keep source prices REAL-only (`app/routers/curves.py:177-185`). Preserve all seven CSV fields, empty nulls, LF, and attachment name (`app/routers/curves.py:826-846,958-962`). Do not add board benchmark fallback.
- [ ] **Step 4: Re-run** the runner and assertion ledger. Expected pass for two routes with exact CSV bytes and zero unmapped applicable assertions.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port legacy public curve projections'` with trailer.

**Resume point:** Record R07/R08 acceptance. Task 10b has separate canonical/board files and does not modify this module.

### Task 10b: Port canonical curve and board projections — Astra

**Files:** Create `rs/read/src/routes/curves_canonical.rs`, `rs/read/src/market/{benchmarks,signals,curve_projection}.rs`, `rs/read/sql/curves/{orders,benchmarks,indications,stems,bands,trades}.sql`, `rs/read/tests/curves_canonical.rs`.

**Interfaces:** Consumes policy, visibility, trade evidence, and route query parser. Produces `pub fn routes() -> axum::Router<AppState>` and `pub async fn load_curve(state: &AppState, query: &Json, now: chrono::DateTime<chrono::Utc>) -> Result<Json, ApiError>`. `query` first passes the route schema; it is never SQL. Covers R09–R11. `market/benchmarks.rs` owns external/manual quotes only; Task 6a owns the distinct live-slice helper. Uses Task 6 `order_query_sql`/`bind_order_query` with `Candidates` and `trade_query_sql`/`bind_trade_query`; static tails apply surface-specific qualification, sparse keys and limits. Use `order_is_execution_qualified_at` with the captured Singapore date.

- [ ] **Step 1: Add curve tests.**

```rust
#[test]
fn all_curve_shapes_match_python() {
    for group in ["curve_table", "curve_slice", "curve_board"] {
        verdaxis_contract::compare_group(group).unwrap();
    }
}
#[test]
fn real_evidence_wins_before_focus_limit() {
    verdaxis_contract::compare_group("curve_real_before_limit").unwrap();
}
```

Include complete grid and sparse tuple requests, duplicate windows, unsupported product/lane, unknown point, no data, demo-only, mixed real/demo, unverified ingestion, stale observations, default focus fallback, ten-level depth, missing benchmark, and excluded table field. Pin the current bounds: 30-day trade lookback, seven-day freshness, ten depth levels, eight slice trades, ten indications, six stems, and `FORWARD_QUARTER_COUNT + 4` table windows (`app/services/forward_curve_market_slices.py:91-97`). Bound focus detail before serializing. Do not remove UNKNOWN metadata merely because it has no price.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group curves_canonical --rust-target curves_canonical`. Expected failure: missing curve ports.
- [ ] **Step 3: Port the board and canonical table/slice contracts as separate functions.** Leave legacy routes in Task 10a. Reuse SQL inputs and evidence policy, not one lossy common DTO. Use the aggregation core:

```rust
pub fn midpoint(bid: Option<rust_decimal::Decimal>, ask: Option<rust_decimal::Decimal>)
    -> Option<rust_decimal::Decimal>
{
    bid.zip(ask).map(|(b, a)| ((b + a) / rust_decimal::Decimal::from(2))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointNearestEven))
}
```

Export `midpoint` in `curve_projection.rs` for tests. Port label policy, evidence selection, staleness thresholds, and window normalization from `app/services/forward_curve_market_slices.py`, rather than rewriting labels from the older board. Preserve the board's fallback and unsupported-lane error (`app/routers/curves.py:623-644`). Batch requested keys with compact axis predicates only for complete grids; sparse requests require exact tuples (`ARCHITECTURE.md:193`). Use verified REAL signal evidence before limits, with disclosed DEMO fallback. Keep benchmark reads read-only; never call rebuild or ingestion.

Test round-half-even values ending in .005 and signed zero against Python. No synthetic LNG price is allowed. Preserve the separate older board field layout and canonical label policy; do not collapse their DTOs.

- [ ] **Step 4: Run** all curve corpus and projection vectors, SQL query count tests, and PostgreSQL trusted-evidence tests. Expected pass. Compare public field omission and labels, not just economics.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port public forward curve read models'` with trailer.

**Resume point:** Record R09–R11 results, zero unmapped assertions, and evidence query counts. Task 11 can reuse anonymous slice data only after privacy tests pass.

### Task 11: Port market and voyage MCP through rmcp — Astra

**Files:** Create `rs/read/src/mcp/{mod,transport,voyage,prices}.rs`, `deploy/rust/{sea-distances,voyage-properties}.json`, `rs/read/tests/mcp.rs`, `tests/rust_contract/golden/voyage/`. Reuse policy data, not product-name branches.

**Interfaces:** Consumes `AppState`, shared read semaphore, anonymous market projections, and fixed voyage data. Produces:

```rust
pub fn mcp_router(state: AppState) -> axum::Router;
pub fn voyage_plan(request: &Json, prices: &Json, today: chrono::NaiveDate) -> Result<Json, String>;
pub async fn market_slice(state: &AppState, product: &str, point: &str,
    window: &str, now: chrono::DateTime<chrono::Utc>) -> Result<Json, ApiError>;
```

Use typed internal structs generated from the recorded schemas. `Json` is the public test boundary, not a replacement for validation. `mcp_router` is empty in production and refuses any production config before construction.

- [ ] **Step 1: Add transport and calculation tests.**

```rust
#[test]
fn mcp_wire_and_tools_match_python() {
    for group in ["mcp_transport", "mcp_catalog", "mcp_slice", "mcp_voyage", "mcp_errors"] {
        verdaxis_contract::compare_group(group).unwrap();
    }
}
#[test]
fn voyage_rounding_matches_decimal_oracle() {
    verdaxis_contract::compare_group("voyage_decimal_vectors").unwrap();
}
```

Port the existing assertions for Origin, Host, body limit, privacy, cancellation, and shared REST/MCP capacity (`tests/unit/test_mcp_server.py:171-227,271-306`). Include missing prices, no EU scope, no conventional fuel, missing intensity, invalid Cape route, 365-day limit, and 30-day/730-day start bounds. Copy output schemas and tool annotations exactly; test both text and structured content.

- [ ] **Step 2: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group mcp --rust-target mcp`. Expected failure: no rmcp endpoint.
- [ ] **Step 3: Implement the transport adapter and pure arithmetic.** Reject any Origin header before invoking rmcp. Reject a nonallowlisted Host with the recorded Python status/body. Do not assume SDK defaults match the Python SDK. Configure rmcp's pinned `StreamableHttpService` for stateless operation and JSON responses. Compile a minimal initialize/list/call roundtrip against the resolved SDK first. Use its generated tool router for exactly `list_markets`, `get_market_slice`, and `plan_voyage_fuel`. Adapt protocol errors only where the golden transport proves a difference; do not create a second MCP protocol implementation.

```rust
pub fn reject_origin(headers: &axum::http::HeaderMap) -> Option<axum::response::Response> {
    use axum::response::IntoResponse;
    headers.contains_key(axum::http::header::ORIGIN).then(||
        (axum::http::StatusCode::FORBIDDEN, "Invalid Origin header").into_response())
}
pub fn half_up(value: rust_decimal::Decimal, scale: u32) -> rust_decimal::Decimal {
    value.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}
```

Export these two helpers from `transport.rs` and `voyage.rs`. `list_markets` reads policy data only and bypasses the DB gate. The other tools acquire the same two-slot semaphore used by REST. Map busy to `Verdaxis market tools are busy; retry shortly.`, database unavailability to `Market data is temporarily unavailable.`, and unexpected errors to `Internal error` (`app/mcp_server.py:74-93`). Preserve SDK success HTTP status with tool `isError` where Python does; do not substitute HTTP 503 indiscriminately.

Port the pure engine in the same operation order as `app/services/voyage_fuel_plan.py:86-126,187-225,259-369,405-462`. Use 41,000 MJ per conventional-equivalent tonne, ceil calendar days, departure-port bunkering, the existing EU shares, annual target schedule, and current missing-input behavior. Export distance and fuel-property tables as data from `app/services/sea_distances.py` and `app/services/voyage_fuel_plan.py:21-45`. Do not add LNG voyage support or new product strategies. The current engine's supported fuel set is a distinct contract from catalog visibility.

**Decimal gate:** Python uses a 28-significant-digit context; rust_decimal has different range and division behavior. Add randomized bounded decimal operation vectors and full voyage cases at rounding boundaries. If rust_decimal cannot reproduce a supported input, implement a decimal-context adapter backed by arbitrary-precision decimal arithmetic for the voyage engine. Record that dependency correction. Do not loosen tolerances, round intermediate values, or use f64 to pass the test. Output rounding is half-up only at the same points as Python (`app/services/voyage_fuel_plan.py:78-79,385-402`).

- [ ] **Step 4: Run** MCP corpus, pure engine vectors, shared-gate concurrency, and no-identifier recursive scan. Expected pass. Require initialize, notifications, GET, DELETE, unsupported versions, and body-size behavior to match; a successful tool call alone is insufficient.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: port stateless market and voyage MCP'` with trailer.

**Resume point:** Record rmcp version and protocol matrix. Task 13 registers `/mcp` exactly, with no slash redirect and no session store.

### Task 12a: Verify artifacts and gate CI publication — Astra

**Files:** Create `.github/workflows/rust-read-ci.yml`, `scripts/verify_rust_artifact.py`, `deploy/rust/artifact.schema.json`, `tests/unit/test_rust_artifact.py`, `rs/read/src/runtime/artifact.rs`. Do not edit deploy transaction, units, or ACL policy in this checkpoint.

**Interfaces:** Consumes approved full SHA, compiled binary SHA, exact migration checkpoint, artifact digest, and existing main/restart locks. Produces Rust `pub fn verify_artifact_identity(compiled: &str, runtime: &str) -> Result<(), String>` and operator artifact verification CLI `python scripts/verify_rust_artifact.py --archive PATH --source-sha SHA --target x86_64-unknown-linux-gnu --output-dir PATH`. The CLI verifies provenance before extraction and rejects symlinks, hardlinks, absolute paths, traversal, duplicate members, oversized archives, and unexpected members.

**Build recommendation:** Use a CI-built Linux artifact, not a host build. This keeps Cargo/build scripts and compiler memory off the shared staging host. It also separates compilation from live credentials. Build on a pinned Linux runner/container compatible with the host's libc, after an operator checks `uname -m` and `ldd --version`. The proposed target is `x86_64-unknown-linux-gnu`; it is not a claim that the host was inspected. Record toolchain, target, libc minimum, lockfile hash, source SHA, binary SHA-256, policy hashes, and revision allowlist hash. Use GitHub artifact attestation and an operator-approved workflow identity. Pin Actions by reviewed commit SHA during implementation. Do not use an unsigned digest from the same untrusted download as sole provenance.

- [ ] **Step 1: Add artifact tests.**

```rust
#[test]
fn wrong_binary_refuses_new_release() {
    assert!(verify_artifact_identity(&"a".repeat(40), &"b".repeat(40)).is_err());
}
```

Add archive fixtures for wrong digest/source/workflow/target, absent binary, duplicate/traversing members, links, and oversized input. Verify refusal before extraction or execution.
- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/unit/test_rust_artifact.py -q` and the pure Rust identity test. Expected failure: verifier absent.
- [ ] **Step 3: Implement identity and staged artifact validation.**

```rust
pub fn verify_artifact_identity(compiled: &str, runtime: &str) -> Result<(), String> {
    if compiled == runtime { Ok(()) } else { Err("binary release mismatch".into()) }
}
```

CI's offline job performs:

```bash
export RELEASE_SHA="$GITHUB_SHA"
cargo fetch --manifest-path rs/Cargo.toml --locked
SQLX_OFFLINE=true cargo test --manifest-path rs/Cargo.toml --locked --workspace
SQLX_OFFLINE=true cargo build --manifest-path rs/Cargo.toml --locked --release \
  --target x86_64-unknown-linux-gnu -p verdaxis-read
sha256sum rs/target/x86_64-unknown-linux-gnu/release/verdaxis-read
```

Only pure tests run in the offline job; `contract-tests` is not enabled. A separate **contract** job starts its own disposable PostgreSQL, Python, Rust, and Caddy in that same job. It must not rely on processes from another job. Run:

```bash
export RELEASE_SHA="$GITHUB_SHA"
venv/bin/python -m scripts.rust_contract.run_disposable \
  --group all --rust-target all --mixed-caddy --require-complete-ledger \
  --report docs/rust-phase1/ci-contract.json
```

The runner owns lifecycle, allocates ephemeral ports, exports `DATABASE_URL`, `MIGRATOR_DATABASE_URL`, `PYTHON_CONTRACT_URL`, `RUST_CONTRACT_URL`, `DISPOSABLE_TARGET_URL`, `DISPOSABLE_TEST_TOKEN`, and `RELEASE_SHA`, validates all identities, and runs feature-gated contract targets plus the unchanged integration suite. It fails for absent cases, zero tests, skipped contract tests, or unmapped assertions. Check SQLx metadata with `cargo sqlx prepare --check` against that job's migrated disposable database. The **publish** job has `needs: [offline, contract]`; require both results to equal success, not skipped. It attests the exact binary digest tested by the contract job. Package policy/voyage/revision data and the complete coverage report. Commit `.sqlx` metadata; CI never receives staging secrets. The binary exposes `--version-json` before serving.

- [ ] **Step 4: Run** artifact safety tests and the offline job locally. Run the contract job on the assembled Task 13 tree before publication; while assembly is absent, publication is blocked, not green. Artifact-verifier acceptance itself uses fixture archives and needs no future route code.
- [ ] **Step 5: Commit** listed files: `git commit -m 'build: verify Rust artifacts and require contract CI'` with trailer.

**Resume point:** Artifact verification is independently accepted. CI publication remains a final Task 13 gate. Task 12b consumes this verifier.

### Task 12b: Extend the canonical deploy transaction — Astra

**Files:** Modify `scripts/deploy.sh:467-565,611-712`. Create `scripts/prepare_rust_runtime.py`, `tests/unit/test_rust_release.py`, and `deploy/rust/staging-policy.json`. Do not edit installed unit bytes in this task.

**Interfaces:** Consumes artifact verifier CLI, effective budget gate, and existing main/restart locks. Produces `prepare_rust_runtime.py --environment-file PATH --release-sha SHA --output-dir PATH`. Rust consumes the Task 2 `Config` and Task 12a `verify_artifact_identity(compiled: &str, runtime: &str) -> Result<(), String>` unchanged.

- [ ] **Step 1: Add failure-injection tests.**

```python
def test_both_health_results_are_required(release_fixture):
    result = release_fixture.deploy(python_ready=True, rust_ready=False)
    assert result.returncode != 0
    assert result.state == "blocked"
    assert result.stopped_services == {"python", "rust"}


def test_dry_run_never_executes_build_code(release_fixture):
    result = release_fixture.dry_run(candidate_build_script="exit 99")
    assert result.returncode == 0
    assert result.executed_candidate_commands == []
```

Implement `release_fixture` as a temporary real subprocess/filesystem harness with command ledger. Use real child-process flocks. Test moved SHA, missing unit, wrong revision, stale authorization, opposite partial-health result, changed budget, and interruption at each publication/restart boundary. Unit commands use fixture units here; no future Task 12c file is needed.
- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/unit/test_rust_release.py -q`. Expected failure: dual-service transaction absent.
- [ ] **Step 3: Extend** `scripts/deploy.sh`, not a new competing deploy owner:

1. Dry-run still archives and validates immutable source bytes only. Add the Rust unit and artifact-policy data to attestation. It must not execute Cargo, the binary, or candidate Python. Report that artifact runtime compatibility remains untested.
2. Real deploy validates all four existing approval variables and exact checkpoint pair before source mutation. Verify the separately approved artifact against SHA, workflow identity, and digest before stopping services.
3. Acquire the existing main lock and write blocked state. Stop/drain Rust if installed before any schema change. Stop Python for a checkpoint change when the approved migration requires quiescence. Never assume start guards stop running services (`docs/runtime-hardening.md:117-120`).
4. Fast-forward the exact approved tree. Publish runtime identity and artifact selection under blocked state before executing selected-tree code. Stage the binary under `.runtime-rust/releases/<SHA>/verdaxis-read`; reject mutable/symlinked source inputs. Use a regular-file active binary installed by atomic rename, not a source-tree symlink.
5. Run the unchanged exact migration helper and ACL convergence. Prepare a minimal Rust environment containing only app database settings and runtime identity. Do not pass mail, JWT, provider, or migrator credentials into the long-running Rust process. Keep the migrator-backed prestart verifier in its short-lived control-plane process.
6. Hold both locks before `restart-authorized`. Restart both staging units inside that window. Then write readiness-pending. Validate Python's existing public readiness and Rust's direct loopback readiness against the same full SHA. Check artifact compiled identity as well. Clear state only after both pass.
7. On failure, stop both attempted services, retain the selected code/identity and blocked state, and require a forward correction. Never restore old metadata over new bytes.

Implement the restart block under the already acquired locks:

```bash
write_deploy_state restart-authorized "$CURRENT_SHA"
SERVICE_RESTART_ATTEMPTED=1
sudo systemctl restart verdaxis-backend-staging.service verdaxis-rust-staging.service
write_deploy_state readiness-pending "$CURRENT_SHA"
```

The subsequent health loop validates both services before state removal. The failure trap stops both attempted units. Trusted preparation computes the effective Task 2 budget and config digest; no fixed default total is accepted.
- [ ] **Step 4: Re-run** release failure-injection and existing deploy/start-guard tests. Expected pass; ACL policy diff empty, state retained on failure, both identities aligned.
- [ ] **Step 5: Commit** listed files: `git commit -m 'feat: extend guarded staging deploy to Rust'` with trailer.

**Resume point:** Record transaction interruption matrix and fixture-unit tests. Task 12c adds actual audited unit bytes without changing transaction ownership.

### Task 12c: Integrate unit manifest and bootstrap sequence — Astra

**Files:** Create `deploy/systemd/verdaxis-rust-staging.service`, `tests/unit/test_rust_units.py`. Modify `deploy/systemd/runtime-units.manifest`, `.gitignore`, `ARCHITECTURE.md`, and `docs/runtime-hardening.md` during implementation only.

**Interfaces:** Consumes Task 12b preparation CLI and Task 2 startup checks. Produces the manifest-selected staging unit; Rust's `connect(config: &Config) -> Result<sqlx::PgPool, ApiError>` remains unchanged.

- [ ] **Step 1: Add the unit contract test.**

```python
def test_rust_unit_keeps_runtime_credentials_separate():
    from pathlib import Path
    unit = Path("deploy/systemd/verdaxis-rust-staging.service").read_text()
    assert "--bind 127.0.0.1:8011" in unit
    assert "MemoryMax=256M" in unit
    assert "EnvironmentFile=/home/verdaxis-prod/verdaxis/staging/be/.env\n" not in unit
    assert "verify_deploy_start_guard.py" in unit
    assert "verify_migration_revision.py" in unit
```

Add immutable installer and two-release bootstrap tests: bootstrap requires no Rust unit or artifact; activation requires the installed compatible unit and verified artifact; installer HEAD must equal source-ref.
- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/unit/test_rust_units.py -q`. Expected failure: unit absent.
- [ ] **Step 3: Add the unit bytes:**

```ini
[Unit]
Description=Verdaxis Rust public reads (staging)
Wants=network-online.target
Requires=postgresql.service
After=network-online.target postgresql.service
RequiresMountsFor=/home/verdaxis-prod/verdaxis/staging/be
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
User=verdaxis-prod
Group=verdaxis-prod
WorkingDirectory=/home/verdaxis-prod/verdaxis/staging/be
EnvironmentFile=/home/verdaxis-prod/verdaxis/staging/be/.runtime-release.env
EnvironmentFile=/home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/runtime.env
ExecStartPre=/usr/bin/env -i /usr/bin/python3 -I -S -B /home/verdaxis-prod/verdaxis/staging/be/scripts/verify_deploy_start_guard.py --environment staging --expected-release-sha ${RELEASE_SHA}
ExecStartPre=/home/verdaxis-prod/verdaxis/staging/be/venv/bin/python scripts/verify_migration_revision.py --expected ${MIGRATION_REVISION}
ExecStartPre=/home/verdaxis-prod/verdaxis/staging/be/venv/bin/python scripts/prepare_rust_budget.py --check --environment-file /home/verdaxis-prod/verdaxis/staging/be/.env --release-sha ${RELEASE_SHA} --budget-file /home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/budget.json
ExecStart=/home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/verdaxis-read --bind 127.0.0.1:8011
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=30
TimeoutStartSec=120
LimitNOFILE=65536
TasksMax=256
MemoryHigh=192M
MemoryMax=256M
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RestrictSUIDSGID=true
LockPersonality=true
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM

[Install]
WantedBy=multi-user.target
```

The current verifier loads `.env` via Settings in its short-lived process (`scripts/verify_migration_revision.py:25-28`). `runtime.env` must not contain `MIGRATOR_DATABASE_URL`; the Rust HTTP process must not load `.env`. `prepare_rust_runtime.py` reads the existing environment with no-follow regular-file checks and writes the app-only file mode 0600. Add `.runtime-rust/` to the existing runtime ignore policy during implementation. Check port 8011 is free before approval; it is a proposal, not an observed free port. The memory cap is an initial safety bound, not a promised measurement. Sandbox directives match the current backend unit (`deploy/systemd/verdaxis-backend-staging.service:35-46`).

Add only `staging\tverdaxis-rust-staging.service` to the manifest. The existing installer already selects environment entries from the committed manifest; do not add a Rust-owned installer (`scripts/deploy.sh:130-155`; `README.md:119-158`).

**Bootstrap:** Use two releases. First deploy the guarded helper/artifact support with Rust inactive and Caddy unchanged. Install the reviewed unit bundle at that exact clean SHA. Then run a same-checkpoint canonical deploy with the Rust-active release policy to exercise both units and both readiness gates. The first release must not require a not-yet-installed unit. Record activation in committed staging policy, not an ambient flag that bypasses readiness. Keep relevant timers quiesced through this transition and restore only their previously active set.

- [ ] **Step 4: Run** release failure-injection tests, existing deploy/start-guard/installer tests, unit syntax verification on Linux, and artifact startup on a disposable Linux host/container with the same target ABI. Expected pass. Check `git diff -- deploy/postgres/app_acl_policy.sql` is empty.
- [ ] **Step 5: Commit** release files and documentation: `git commit -m 'feat: add attested staging Rust release integration'` with trailer.

**Resume point:** Record bootstrap SHA, activation SHA, unit digest, artifact digest, and test evidence. No live command is authorized by completion of this task. Operator checklists below require separate approval.

### Task 13: Assemble exact routing and run unchanged integration tests — Astra

**Files:** Modify `rs/read/src/runtime/http.rs` to register completed modules. Create `tests/rust_contract/Caddyfile`, `tests/rust_contract/test_routing.py`, and `deploy/caddy/rust-public-staging.caddy` from the route inventory. Extend the one `scripts/rust_contract/run_disposable.py` with `--mixed-caddy` and unchanged integration-suite execution; do not add a second lifecycle runner.

**Interfaces:** Consumes every route module's `routes()`, `mcp_router`, and the disposable identity producer. Produces `pub fn public_router(state: AppState) -> axum::Router` and a disposable Caddy target that preserves Python for every nonported route.

- [ ] **Step 1: Add a route ownership test.**

```python
def test_private_and_write_routes_stay_python(route_owner):
    for method, path in [
        ("GET", "/api/orderbook/my"),
        ("GET", "/api/orderbook/my/latest-ask-template"),
        ("POST", "/api/orderbook"),
        ("DELETE", "/api/orderbook/00000000-0000-0000-0000-000000000001"),
        ("GET", "/api/listings/my"),
        ("GET", "/api/stream/orderbook"),
    ]:
        assert route_owner(method, path) == "python"
```

`route_owner` must test the adapted Caddy configuration with two instrumented disposable upstreams, not parse path prefixes in a separate approximation. Add every inventory route, HEAD/OPTIONS/slash variants, `/api/ports/a/b`, and unknown paths. Do not send a mutation to staging to test routing.

- [ ] **Step 2: Run** `venv/bin/python -m pytest tests/rust_contract/test_routing.py -q`. Expected failure: no exact routing fixture.
- [ ] **Step 3: Compose routes and method-scoped Caddy handlers.**

```rust
pub fn public_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(crate::routes::ports::routes())
        .merge(crate::routes::catalog::routes())
        .merge(crate::routes::news::routes())
        .merge(crate::routes::orderbook_snapshot::routes())
        .merge(crate::routes::orderbook_lists::routes())
        .merge(crate::routes::orderbook_map::routes())
        .merge(crate::routes::listings::routes())
        .merge(crate::routes::prices::routes())
        .merge(crate::routes::trade_tape::routes())
        .merge(crate::routes::curves_legacy::routes())
        .merge(crate::routes::curves_canonical::routes())
        .with_state(state)
}
```

Use exact path lists for orderbook. Never use `/api/orderbook/*` or `/api/listings/*`. Port details use a one-segment anchored path regex. MCP has its own exact path and method matcher. Preserve Host forwarding and existing trusted proxy behavior. Keep public `/health/ready` routed to Python; deploy checks Rust directly. Local Rust health remains available for its unit. For disposable mixed integration, route `/.well-known/verdaxis-disposable-test` and readiness to Python so the existing suite's identity handshake remains valid.

Example group, with all orderbook paths enumerated:

```caddyfile
@rust_orderbook {
    method GET
    path /api/orderbook /api/orderbook/snapshot /api/orderbook/bids /api/orderbook/asks /api/orderbook/with-ci /api/orderbook/aggregated /api/orderbook/product-counts /api/orderbook/map-summary /api/orderbook/map-summary/compact /api/orderbook/products /api/orderbook/regions /api/orderbook/fuel-types /api/listings
}
handle @rust_orderbook {
    reverse_proxy 127.0.0.1:8011
}
```

Create independent groups for ports/catalog/news, prices/tape, curves, and MCP. OPTIONS stays on Python unless the recorded method matrix requires Rust; both must produce the same CORS contract. HEAD ownership stays Python unless explicitly ported after its golden check. Preserve existing fallback rather than replacing the site's whole Caddyfile.

Add utoipa schemas for each ported request/response type and export them from a test-only command. Compare the ported path subset against the recorded Python OpenAPI: parameter names, required/default/bounds, enum values, nullable versus omitted fields, response status/content types, and deprecated flags. Compare semantic schemas, not generated component names or documentation prose. Keep the public `/openapi.json` on Python during Phase 1; do not silently publish an incomplete Rust schema.

```rust
#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(crate::routes::news::NewsResponse)))]
pub struct ReadApi;
```

Derive `utoipa::ToSchema` on each DTO as it is ported. Register all ported operations in `ReadApi` before the subset comparison can pass; the inventory meta-test rejects a schema with only news registered. Dynamic product enums must come from the same policy data as validation, not a Rust product enum.

Run the unchanged suite with explicit disposable inputs after both services use the same seeded database:

```bash
DISPOSABLE_ITEST_PASSWORD="$FIXTURE_PASSWORD" \
venv/bin/python -m pytest tests/integration -q \
  --run-disposable-integration \
  --disposable-target-url http://127.0.0.1:59123 \
  --disposable-target-token "$DISPOSABLE_TEST_TOKEN"
```

The runner must allocate free ephemeral ports, then render them into the Caddyfile. `59123` is an example chosen only after bind succeeds; never reuse occupied ports. Start `ENVIRONMENT=test RELEASE_SHA=<full-sha> python -m tests.disposable_server --port <python-port>` or the Task 4 wrapper, then Rust, then Caddy. Test the identity handshake before launching pytest. Keep request writes on Python; do not disable failing integration cases or alter their assertions to accommodate Rust.

- [ ] **Step 4: Run** `venv/bin/python -m scripts.rust_contract.run_disposable --group all --rust-target all --mixed-caddy --require-complete-ledger --report docs/rust-phase1/ci-contract.json`, then the full Python unit suite, pure Rust workspace tests, rustfmt, and clippy. Require the CI contract job and artifact publication dependency check to pass at this assembled commit. Every route/tool has nonzero case counts, zero unpassed assertions, and AD-01 counted separately. Expected pass. Run `cargo sqlx prepare --check` against the disposable schema. Capture exact pass/fail/skip counts. A skipped integration suite is not acceptance.
- [ ] **Step 5: Commit** integration files: `git commit -m 'test: verify mixed Python Rust routing and integration parity'` with trailer.

**Resume point:** Independent Astra review checks public exposure, SQL authority, DTO privacy, MCP transport, and dual-service release failures. Resolve findings before requesting operator approval for shadow activation.

### Task 14: Shadow, measure, and prepare the founder decision — Astra

> **Superseded in part (decision 17, founder, 16 Oct):** no seven-day shadow clock. Rewrite this task at its start: the gate is the populated corpus, a seeded differential simulation at the workload sizes below, the unchanged integration suite, and one short read-only differential pass on staging data. Memory and p95 use the same simulation workload.

**Files:** Create `scripts/rust_contract/{shadow,measure,report}.py`, `tests/rust_contract/{test_shadow,test_measure}.py`, `docs/rust-phase1/go-no-go.md`, and `scripts/rust_contract/staging.example.toml` (the reviewed `--config` template: target URLs, sampling rate, duration and output paths, with no secrets; the operator copies it to `work/rust-phase1/staging.toml` and fills the URLs). Use `work/rust-phase1/` for raw benchmark samples. No changes to economic code in this task.

**Interfaces:** Consumes the approved route corpus and both readiness identities. Produces Rust `pub fn passes_gate(contract_differences: u64, python_memory: u64, rust_memory: u64, python_p95: f64, rust_p95: f64) -> bool` in `rs/contract/src/lib.rs`, and a report with raw-evidence digests. CLI: `python -m scripts.rust_contract.shadow --config PATH --duration-seconds N`; `python -m scripts.rust_contract.measure --config PATH --output PATH`. Config is an explicit reviewed file; no implicit URL defaults.

- [ ] **Step 1: Add false-pass tests.**

```rust
#[test]
fn go_no_go_needs_all_three_conditions() {
    assert!(passes_gate(0, 100, 50, 10.0, 10.0));
    assert!(!passes_gate(1, 100, 40, 10.0, 9.0));
    assert!(!passes_gate(0, 100, 51, 10.0, 9.0));
    assert!(!passes_gate(0, 100, 40, 10.0, 10.1));
    assert!(!passes_gate(0, 0, 0, 10.0, 9.0));
}
```

Add Python tests that reject real query strings, authorization/cookie headers, row IDs, difference values, and payload hashes in persisted shadow logs. Reject production URLs. Treat timeouts, data-changing sandwiches, missing coverage, and skipped cases as inconclusive or failures, never passes.

- [ ] **Step 2: Run** measure/shadow unit tests and the gate test. Expected failure: harnesses absent.
- [ ] **Step 3: Implement the gate and bounded harnesses.**

```rust
pub fn passes_gate(differences: u64, python_memory: u64, rust_memory: u64,
    python_p95: f64, rust_p95: f64) -> bool {
    differences == 0 && python_memory > 0
        && rust_memory <= python_memory / 2
        && python_p95.is_finite() && rust_p95.is_finite()
        && python_p95 > 0.0 && rust_p95 >= 0.0 && rust_p95 <= python_p95
}
```

`passes_gate` receives only **unapproved** difference count. AD-01 is a separately required report row, never an arbitrary allowlist for other failures. Golden and shadow do not compare rate-limit threshold counts or accept/reject sequences; a rate-limited pair is recorded as excluded AD-01 coverage and retried only after backoff. It is not a successful economic comparison. Rust burst/reset tests must pass independently.

**Shadow recommendation: seven consecutive full days per candidate SHA and route group.** Any code/policy/schema change resets that group's clock. Any unexplained difference resets it. Require at least 1,000 stable pairs per route, all registered groups each day, at least one nonempty case where data exists, and no missing-day gap. A route with no live nonempty data still needs its populated disposable corpus and explicit coverage note. Zero traffic is not zero differences. Track all three MCP tools separately: require successful nonempty coverage where supported and each tool's error-class coverage. A POST /mcp route count alone is insufficient. Busy/tool-error responses do not count as successful nonempty economic coverage.

Run the shadow harness on the staging host as a separately invoked, short-lived low-priority process after operator authorization. It calls Python `127.0.0.1:8001` and Rust `127.0.0.1:8011`; it opens no database connections. Use fixed synthetic public queries from the corpus. Do not mirror user requests, cookies, bearer tokens, client IPs, or arbitrary query parameters. Use a fixed shadow request ID and a safe Host header for MCP. Do not bypass application rate limits.

Limit concurrency to one. Limit total upstream calls to three per ten seconds, with a 2-second per-call deadline and 1 MiB response bound initially. Schedule groups round-robin and honor price-route minute limits. Pause for 60 seconds on any 429/503, readiness failure, or host memory-pressure signal. Do not retry immediately. Use no production endpoint and no mutating method except read-only MCP POST operations from the allowlist. Disable on deploy state presence, including a dangling symlink. Stop at the configured duration; no permanent timer is added.

Use a Python–Rust–Python sandwich. Compare the two Python results first after narrowly validated generation-time normalization. If they differ, classify the pair as `data_changed`; do not count it toward the zero-difference gate. If they agree, compare Rust. A boundary result remains unexplained until replayed against a disposable frozen fixture. Do not discard such differences merely because “the market moved.” Track inconclusive rate and require at least 99% stable pairs; otherwise reduce sample complexity or investigate, rather than claiming a pass.

Persist only candidate SHA, policy hash, route ID, case ID, status pair, field path, difference category, byte counts, durations, and aggregate counters. Field paths use schema keys and array indexes, not map keys supplied by users. Do not persist bodies, row IDs, amounts, user data, raw URLs, client IPs, or unsalted hashes of sensitive values. Rotate a bounded local file daily; retain seven days of sanitized diagnostics and the final aggregate report. A secure disposable replay, not a staging payload dump, is the debugging method.

**Workload manifest before measurement:** Commit exact fixture cardinalities, distribution seed, route/tool weights, query cases, and expected response-size bands before either run. Use representative and large datasets: proposed starting sizes are 1,000/100,000 orders, 1,000/100,000 historical trades, and 100/10,000 news rows, spread over all approved lanes/windows with disclosed evidence classes. Adjust these proposed sizes only before the run and record why. Never seed LNG demo/benchmark prices. Use an equal-weight route baseline plus a separately declared representative mix. Require at least 1,000 completed observations per measured route and MCP tool across repetitions; extend duration for rate-limited routes rather than bypassing limits. Report missing observations as inconclusive.

**Measurement method:**

1. Use a disposable Linux environment with the same CPU allocation, cgroup version, PostgreSQL/PostGIS versions, schema revision, fixture data, policy, and source SHA for both candidates. Run the load generator outside either service cgroup. Do not load-test live staging.
2. Run the normal four-worker Python service with only the selected public workload, then one Rust process with the identical workload. Do not compare a stripped Python demo app with full Rust or use a single Python worker. Keep external jobs disabled in both disposable runs. Keep Caddy configuration identical except upstream.
3. Keep database cache state comparable. Run both cold-start and warm steady-state series. Warm for five minutes, measure for fifteen minutes, and repeat five times in alternating AB/BA order. Use identical request order and fixed-rate scheduling. Report offered and completed RPS, concurrency, status distribution, timeout/error rate, and response bytes. Start at 1, 5, and 10 RPS with route-weighted public traffic; keep price-route cases below their rate limits for the primary latency series. Run saturation as a separate admission test, not part of successful-response p95.
4. Measure cgroup `memory.current`, `memory.peak`, `memory.stat` anon/file, and `cpu.stat usage_usec` once per second. Record process RSS and PSS from `/proc/<pid>/smaps_rollup` for the parent and all four Python children. Avoid double-counting shared memory. Compute CPU core-equivalents as delta usage_usec / elapsed_usec, and CPU microseconds per completed request.
5. Record end-to-end Caddy p50/p95/p99 for each route and the weighted mix, plus direct-upstream diagnostics. Use a monotonic clock and a fixed-rate generator so queuing is included. Report failed requests separately; never omit slow timeouts from the outcome. Compare every route's p95, not only the blended mix.
6. Primary memory gate uses the median steady-state cgroup memory across repetitions, for Python serving the same ported-route workload versus Rust. Require Rust <=50% of Python for each declared workload. Also report p95 memory, peak, idle baseline, anonymous memory, and measurement spread. Require no p95 regression per route and mix; if uncertainty spans a regression, mark inconclusive and collect more samples. CPU is measured and reviewed even though the founder supplied no numeric CPU threshold.
7. Separately measure the **coexistence** footprint with Python still running and Rust active. Report Python + Rust combined memory and CPU, plus staging's 642 MiB supplied baseline. Do not call route-service savings a 50% reduction in whole-host or whole-backend memory. Python retains writers/SSE and may not shrink materially.
8. Keep public-read concurrency and rate-limit ceilings in the report. The one-process Rust gate has eight slots, equal to Python's aggregate (two per worker, four workers); Python's per-worker gates can refuse earlier when requests land unevenly (AD-02). Record observed 503 rates for both. If Rust shows overload or p95 regression at the agreed load, do not increase the gate silently. Review a pool/capacity change with the 100-connection budget and re-run parity and measurement.

The measurement script reads cgroup counters directly and stores timestamped CSV samples plus request latency NDJSON under `work/rust-phase1/`. It writes only aggregate, redacted evidence and input hashes to `docs/rust-phase1/`. No measured result is prefilled by this plan.

- [ ] **Step 4: Run** local harness tests, then the authorized seven-day shadow and disposable benchmark. Expected pass only when all coverage, zero-difference, memory, and latency gates pass. Otherwise report no-go or inconclusive with exact failing conditions. Do not cut over a route group before its shadow interval passes.
- [ ] **Step 5: Commit** harnesses and final aggregate report: `git commit -m 'test: document Rust Phase 1 parity and measured go no go'` with trailer.

**Resume point:** Founder reviews the completed report and effort spent. A measured pass permits the founder's decision; it does not authorize production or the next rewrite phase automatically.

## Operator-only staging checklists

**Do not run these during plan writing or local implementation.** Obtain separate approval for bootstrap, artifact retrieval, unit apply, shadow execution, and each Caddy cutover. Commands below are for the authorized operator. Future artifact helpers and routing files must first exist, pass Tasks 12a–12c and 13 tests, and be committed.

### A. Verify target and capture inputs

- [ ] Connect only after approval: `ssh jons-openclaw@194.233.68.86`.
- [ ] Run `test "$(hostname -f)" = vmi1840561.contaboserver.net`.
- [ ] Read `/home/verdaxis-prod/verdaxis/PRODUCTION_HOST.md` and `/home/jons-openclaw/VPS_MAP.md`. Stop if either contradicts this plan.
- [ ] Run `getent ahostsv4 api-staging.verdaxis.exchange`. Confirm the approved staging host, not a production address.
- [ ] Run `systemctl show verdaxis-backend-staging.service -p User -p WorkingDirectory -p ExecStart`.
- [ ] Require user `verdaxis-prod`, working directory `/home/verdaxis-prod/verdaxis/staging/be`, and Python loopback port 8001.
- [ ] Run `uname -m; ldd --version; ss -ltn '( sport = :8011 )'`. Confirm artifact ABI compatibility and an unused Rust listener before first activation.
- [ ] Run `sudo -n -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/staging/be status --porcelain`. Require empty output.
- [ ] Record active staging timers with `systemctl list-timers --all --no-pager`. Stop only the approved relevant timers. Let active oneshots drain. Verify `systemctl list-jobs --no-pager` has no pending relevant job. Record the exact prior-active set for restoration.

### B0. Install the Python-only bootstrap first

Do this before B invokes the new verifier. `BOOTSTRAP_SHA` and `ACTIVATION_SHA` are distinct full approved commits. Bootstrap includes tested helper/verifier/unit support with committed Rust activation disabled. It needs no Rust artifact and starts no Rust service. The activation release must use identical compatible unit bytes; otherwise stop and approve a separate inactive unit-transition release first.

```bash
read -r -p 'Approved bootstrap full SHA: ' BOOTSTRAP_SHA
read -r -p 'Verified current checkpoint: ' BOOTSTRAP_REVISION
[[ "$BOOTSTRAP_SHA" =~ ^[0-9a-f]{40}$ ]] || exit 1
sudo -n -u verdaxis-prod -H bash -lc \
 'cd /home/verdaxis-prod/verdaxis/staging/be && ./scripts/deploy.sh --dry-run'
```

- [ ] Require the old helper's printed approved SHA to equal BOOTSTRAP_SHA and the exact current-to-current checkpoint pair to be allowlisted. Deploy through the existing Python-only path:

```bash
sudo -n -u verdaxis-prod -H env \
 APPROVED_RELEASE_SHA="$BOOTSTRAP_SHA" MIGRATION_APPROVED_SOURCE_SHA="$BOOTSTRAP_SHA" \
 MIGRATION_EXPECTED_CURRENT_REVISION="$BOOTSTRAP_REVISION" \
 MIGRATION_TARGET_REVISION="$BOOTSTRAP_REVISION" \
 /home/verdaxis-prod/verdaxis/staging/be/scripts/deploy.sh
sudo -n -u verdaxis-prod git -C /home/verdaxis-prod/verdaxis/staging/be rev-parse HEAD
```

- [ ] Require HEAD=BOOTSTRAP_SHA, exact Python readiness, and absent deploy state. Run the now-live dry-run for the same source. Then install units only at that current HEAD:

```bash
sudo /home/verdaxis-prod/verdaxis/staging/be/scripts/install_systemd_units.sh \
 --dry-run --environment staging --source-ref "$BOOTSTRAP_SHA"
sudo /home/verdaxis-prod/verdaxis/staging/be/scripts/install_systemd_units.sh \
 --apply --environment staging --source-ref "$BOOTSTRAP_SHA"
```

- [ ] Keep Rust stopped and Caddy unchanged. The installer never activates it. Keep timers quiesced through activation. Never pass ACTIVATION_SHA to the installer while HEAD is BOOTSTRAP_SHA (`scripts/install_systemd_units.sh:87-96`).

### B. Approve and verify the activation artifact

Precondition: B0 completed, so the reviewed verifier exists in the live bootstrap tree. The operator supplies activation values from the reviewed release record, not guessed current heads:

```bash
read -r -p 'Approved activation full SHA: ' ACTIVATION_SHA
export APPROVED_RELEASE_SHA="$ACTIVATION_SHA"
read -r -p 'Approved CI run ID: ' RUST_CI_RUN_ID
read -r -p 'Exact current Alembic revision: ' MIGRATION_EXPECTED_CURRENT_REVISION
read -r -p 'Approved literal target revision: ' MIGRATION_TARGET_REVISION
export APPROVED_RELEASE_SHA RUST_CI_RUN_ID
export MIGRATION_EXPECTED_CURRENT_REVISION MIGRATION_TARGET_REVISION
export MIGRATION_APPROVED_SOURCE_SHA="$APPROVED_RELEASE_SHA"
[[ "$APPROVED_RELEASE_SHA" =~ ^[0-9a-f]{40}$ ]] || exit 1
[[ "$RUST_CI_RUN_ID" =~ ^[0-9]+$ ]] || exit 1
```

- [ ] Run the canonical dry-run as the service user. Compare its printed SHA with the approved value:

```bash
sudo -n -u verdaxis-prod -H bash -lc \
 'cd /home/verdaxis-prod/verdaxis/staging/be && ./scripts/deploy.sh --dry-run'
```

- [ ] Confirm the committed migration policy contains the exact expected/target pair. For a no-schema-change release use the verified current revision as both values only if that pair is allowlisted. Do not assume the dated head is still current.
- [ ] Download the exact CI run's artifact into an operator-owned private directory, not the checkout. Verify attestation before extraction:

```bash
umask 077
ARTIFACT_DIR=$(mktemp -d /var/tmp/verdaxis-rust-artifact.XXXXXXXX)
gh run download "$RUST_CI_RUN_ID" --repo Verdaxis/verdaxis-backend \
  --name "verdaxis-read-$APPROVED_RELEASE_SHA" --dir "$ARTIFACT_DIR"
gh attestation verify "$ARTIFACT_DIR/verdaxis-read.tar.gz" \
  --repo Verdaxis/verdaxis-backend \
  --signer-workflow Verdaxis/verdaxis-backend/.github/workflows/rust-read-ci.yml
```

- [ ] Inspect the attestation subject digest, source commit, workflow identity, and successful CI result. The verifier must bind the subject to the approved full SHA; a successful repository-only signature check is insufficient.
- [ ] Use the reviewed bootstrap release's artifact verifier to validate target/manifest/member safety. Stage its verified output in the fixed incoming directory consumed by canonical deploy. Do not run a verifier from an unapproved downloaded archive:

```bash
sudo -n -u verdaxis-prod \
 /home/verdaxis-prod/verdaxis/staging/be/venv/bin/python \
 /home/verdaxis-prod/verdaxis/staging/be/scripts/verify_rust_artifact.py \
 --archive "$ARTIFACT_DIR/verdaxis-read.tar.gz" \
 --source-sha "$APPROVED_RELEASE_SHA" --target x86_64-unknown-linux-gnu \
 --output-dir /home/verdaxis-prod/verdaxis/staging/be/.runtime-rust/incoming
```

The operator must provision read access to the archive for `verdaxis-prod` without opening secrets to other users. The verifier refuses a preexisting nonempty incoming directory; inspect prior state rather than deleting it blindly.

### C. Deploy and install through canonical gates

- [ ] Execute the approved activation release after B0 and B. Require its committed Rust unit bytes to match the installed bootstrap unit. Keep Caddy unchanged. Use all four activation variables:

```bash
sudo -n -u verdaxis-prod -H env \
 APPROVED_RELEASE_SHA="$APPROVED_RELEASE_SHA" \
 MIGRATION_APPROVED_SOURCE_SHA="$MIGRATION_APPROVED_SOURCE_SHA" \
 MIGRATION_EXPECTED_CURRENT_REVISION="$MIGRATION_EXPECTED_CURRENT_REVISION" \
 MIGRATION_TARGET_REVISION="$MIGRATION_TARGET_REVISION" \
 /home/verdaxis-prod/verdaxis/staging/be/scripts/deploy.sh
```

- [ ] Require exact Python and Rust readiness, absent state, and checkout HEAD=ACTIVATION_SHA. Run the now-live helper dry-run again. Attest the unchanged installed unit bytes at this current SHA; never install future-ref bytes before the checkout advances:
- [ ] Run the immutable installer dry-run at the activation HEAD. Apply only after review if reconciliation is needed:

```bash
sudo /home/verdaxis-prod/verdaxis/staging/be/scripts/install_systemd_units.sh \
 --dry-run --environment staging --source-ref "$APPROVED_RELEASE_SHA"
sudo /home/verdaxis-prod/verdaxis/staging/be/scripts/install_systemd_units.sh \
 --apply --environment staging --source-ref "$APPROVED_RELEASE_SHA"
```

- [ ] Rust starts only inside the activation deploy's two-lock restart window. Do not use a manual start to bypass failed artifact, budget, revision, or readiness checks.
- [ ] Verify both loopback health responses with the canonical validator:

```bash
for port in 8001 8011; do
  curl --fail --silent --show-error --max-time 3 \
    "http://127.0.0.1:$port/health/ready" |
  sudo -n -u verdaxis-prod \
    /home/verdaxis-prod/verdaxis/staging/be/venv/bin/python \
    /home/verdaxis-prod/verdaxis/staging/be/scripts/validate_health_response.py \
    --environment staging --release-sha "$APPROVED_RELEASE_SHA" || exit 1
done
sudo test ! -e /home/verdaxis-prod/verdaxis/staging/be/.runtime-deploy/staging.state
sudo test ! -L /home/verdaxis-prod/verdaxis/staging/be/.runtime-deploy/staging.state
systemctl is-active verdaxis-backend-staging.service verdaxis-rust-staging.service
systemctl show verdaxis-rust-staging.service -p User -p WorkingDirectory -p MemoryHigh -p MemoryMax
```

- [ ] Confirm listeners remain loopback-only with `ss -ltnp`. Confirm the Rust process environment contains no migrator/provider secret names without printing secret values.
- [ ] Restore only timers recorded as active, after absent-state checks. Do not enable new timers or staging mail.
- [ ] Run the approved bounded shadow harness. Leave all user route groups on Python until each group passes seven days.

### D. Cut over one Caddy group and rehearse rollback

The installed Caddyfile path and import structure must be discovered from `systemctl cat caddy` and reviewed. No current Caddyfile is claimed by this repository. The commands below require the operator to set the actual approved paths; they do not authorize replacing unrelated site configuration.

```bash
read -r -p 'Installed Caddy config path: ' CADDY_CONFIG
read -r -p 'Reviewed candidate full config path: ' CADDY_CANDIDATE
read -r -p 'Reviewed rollback full config path: ' CADDY_ROLLBACK
: "${CADDY_CONFIG:?}" "${CADDY_CANDIDATE:?}" "${CADDY_ROLLBACK:?}"
sudo systemctl cat caddy
sudo caddy validate --config "$CADDY_CANDIDATE" --adapter caddyfile
sudo caddy validate --config "$CADDY_ROLLBACK" --adapter caddyfile
sudo diff -u "$CADDY_CONFIG" "$CADDY_CANDIDATE"
```

- [ ] Require the candidate diff to change only one approved route group's upstream from 8001 to 8011. Preserve TLS, Host, CORS, proxy trust, logging, limits, other sites, and fallback routes. Use exact method/path matchers from Task 13.
- [ ] Require rollback to change that same group's upstream back to 8001. Keep Rust's binary and schema untouched.
- [ ] Back up the inspected configuration and permissions before replacement. Use a unique timestamped backup. Install the candidate and reload:

```bash
BACKUP="${CADDY_CONFIG}.before-rust-$(date -u +%Y%m%dT%H%M%SZ)"
sudo cp -p -- "$CADDY_CONFIG" "$BACKUP"
sudo install -m 0644 -- "$CADDY_CANDIDATE" "$CADDY_CONFIG"
sudo caddy validate --config "$CADDY_CONFIG" --adapter caddyfile
sudo systemctl reload caddy
sudo systemctl is-active --quiet caddy
```

- [ ] Verify public readiness still identifies Python at the approved SHA. Probe one safe GET per moved route and one private-route unauthenticated rejection. Do not perform a live trade or registration. Check aggregate 5xx and latency against the approved baseline.
- [ ] On a route regression, perform the one-group proxy rollback:

```bash
sudo install -m 0644 -- "$CADDY_ROLLBACK" "$CADDY_CONFIG"
sudo caddy validate --config "$CADDY_CONFIG" --adapter caddyfile
sudo systemctl reload caddy
sudo systemctl is-active --quiet caddy
```

- [ ] Verify the group returns Python golden-compatible responses. Record the rollback reason. Do not reset Git, downgrade Alembic, or edit `.runtime-release.env`. If Python itself is unhealthy, a Caddy rollback is not recovery; keep the incident blocked and use the canonical forward-release procedure.

## Go/no-go report template

Write the completed report to `docs/rust-phase1/go-no-go.md`. Keep all unknown results explicitly unmeasured until evidence exists.

```markdown
# Rust Phase 1 go/no-go

- Decision requested: continue / pause / stop.
- Candidate source SHA:
- Python oracle SHA and corpus digest:
- Schema revision and policy digest:
- Rust toolchain, target, libc minimum, Cargo.lock digest, artifact digest:
- Host/CPU/cgroup/PostgreSQL/PostGIS configuration:
- Scope: 27 data GETs, 3 health GETs, 3 MCP transport pairs; 3 MCP tools.
- Contract result: total cases, passed, failed, skipped; exact difference count.
- Integration result: command, pass/fail/skip counts, evidence digest.
- Shadow: candidate/group, start/end UTC, stable pairs per route, differences,
  inconclusive rate, pause time, nonempty coverage, seven-day result.
- Workload: route weights, request seed, offered/completed RPS, concurrency,
  warmup, duration, repetitions, AB/BA order, cache state.
- Memory table: Python/Rust steady median, p95, peak, anon/file, PSS, ratio.
- CPU table: core-equivalents and microseconds/completed request.
- Latency table: per-route and mixed p50/p95/p99, errors, confidence/spread.
- Coexistence: Python + Rust combined memory/CPU; not route-only savings.
- Gate: zero unapproved differences; Rust memory <=50%; every p95 <=Python.
- Approved differences (exactly two):

| ID | What changed | Why approved | Proof |
|---|---|---|---|
| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; Python's four local counters can admit up to four times more | PM defines documented per-client limits as the contract; worker multiplication is an implementation artifact; Python unchanged | Rust exact-boundary concurrent burst/reset tests; counts excluded from golden/shadow parity |
| AD-02 | Rust uses one 8-slot public-read admission gate; Python uses 2 slots in each of 4 workers | Same total capacity; Rust admits at least every request Python's aggregate admits, while Python can return 503 earlier under uneven load; overload behavior is capacity, not an API shape | Mixed-concurrency test: 8 concurrent slow reads admitted, the 9th gets the unchanged 503 + `Retry-After`; 503 rates recorded for both in shadow/measure |
| AD-03 | Rust admits `/api/news` through the same 8-slot gate; Python's gate does not cover news | Every Rust database use holds a permit and no request waits on the pool (decision 13) | Two-direction contention test in `rs/read/tests/admission_postgres.rs` |

- Rate-limit counts excluded from parity; all other status/body/header contracts retained.
- Admission topology findings (AD-02 evidence; no other exception):
- Assertion ledger: applicable/mapped/passed/excluded counts and reviewed exclusions:
- Engineering effort: elapsed work, review effort, rework, remaining cost.
- Three largest unresolved risks and rollback readiness:
- Independent review commit and resolved findings:
- Founder decision and date:
- Production status: not authorized by this report.
```

## Production later

Rust can reach production only after Phase 0 completes and the founder reviews these measured results. Production needs its own source, schema, artifact, unit, and operator approval. This plan defines no production implementation or deployment task.

## Final resume and acceptance checks

- [ ] Read this plan, the approved design, the current progress file, and `git status`.
- [ ] Confirm the source/policy/schema/corpus identities still agree after Phase 0 work.
- [ ] Resume at the first unchecked task. Re-run that task's entry test before editing.
- [ ] Require 27 data routes, 3 health routes, 3 MCP transport pairs, and all three tools in the coverage ledger.
- [ ] Require no ACL widening, no SQLx migrations, no new runtime writer, no product literals in Rust, and no private path captured by Caddy.
- [ ] Require all golden, raw-role PostgreSQL, unchanged integration, Python unit, Rust test, formatting, and clippy gates to pass. Require zero unmapped/unpassed applicable assertions and exactly three listed exceptions, AD-01 (rate limits, with passing Rust burst/reset tests), AD-02 (admission capacity, with the passing mixed-concurrency test) and AD-03 (news admission, with the passing two-direction contention test).
- [ ] Require independent Astra review and the seven-day shadow evidence before each group switch.
- [ ] Require measured memory/CPU/p95 and an explicit founder go/no-go decision.
- [ ] Keep failed, skipped, or inconclusive checks visible. Do not replace them with a narrative claim of completion.

**Plan-writing verification:** This document is based on read-only source inspection at the pinned staging SHA. No host, database, build, benchmark, seed, agent, or background job was started to write it. The local Cargo version was checked. The worktree has no available `venv/bin/python`; Python unit tests were not run for this documentation-only change. Code blocks are implementation instructions, not claims of compiled or executed Rust.
