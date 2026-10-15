# Phase 0 integrated acceptance and Rust handoff — 2026-10-16

**Decision: BLOCKED. Task 9 is not accepted. No Task 10 or deployment approval.**
The Phase 0-specific gate passes. The complete existing PostgreSQL and HTTP
suites do not. Do not turn this report into a green release by omitting them.
All runs finished; owned containers were removed with their volumes.

## Frozen candidate identities

- Backend / candidate Python oracle: `4be3fbb5ac39558fbacba6d3b6ae1f5057a2525c`.
  Application bytes equal integrated base `4105a15a2d417d74abafc01297df21beda103cdf`;
  the candidate adds runner evidence checks only. Later documentation commits do
  not change this tested identity. This is a candidate, not an accepted oracle.
- Frontend candidate: `7b7d91b33db60ce3cb2d4408ca90a7a7b2562272`.
  The measured bundle source is `d079122e1aac8599fdb505db2fe1349052e64db3`;
  only `docs/phase0/progress-task8.md` differs. Old bundle:
  `3f16cabba6424ef08250b32f8f8e4b9a621c4379`.
- Frontend gate backend: `53e1960c42a3e2373db84802a5ecfca553e0244f`.
  Subsequent Task 7 changes concern deployment identity, CORS, seed safety and
  monitoring, not the measured HTTP contracts. Reuse is limited to those
  contracts; this gate does not prove deployed CORS or systemd.
- Revision: `p0_20261016_union`, parent `kycd_20261015_staging`.
- Permanent production database: `verdaxis_prod`; old `verdaxis` stays untouched.
  Both pilot environments require UTC and `KYC_EXECUTION_GATE_ENABLED=false`.
  Production receives the encrypted vault, initially empty; no invented files.
- Founder decisions 2–6 are confirmed by the task instruction on 16 October.
  This confirmation does not authorize a host connection or operation.

## Completed local runs

| Command / evidence | Passed | Failed | Errors | Skipped |
|---|---:|---:|---:|---:|
| Runner tests, before implementation | 0 | 8 | 0 | 0 |
| `pytest tests/unit/test_phase0_runner.py -q` after implementation | 8 | 0 | 0 | 0 |
| `bash scripts/run_phase0_postgres_tests.sh` | 153 | 0 | 0 | 0 |
| Required SQLite `pytest tests/unit -q` | 2155 | 19 | 0 | 0 |
| `pytest tests/monitor -q -rs` | 290 | 0 | 0 | 1 |
| Complete existing PostgreSQL suites (Phase 0 files covered above) | 256 | 5 | 1 | 0 |
| Guarded `pytest tests/integration -q -rs` | 24 | 49 | 6 | 0 |
| Reused actual old/new frontend bundle assertions | 53 | 1 accepted exception | 0 | 0 |

Unit/monitor commands used `ENVIRONMENT=test RELEASE_SHA=test`, explicit test
JWT secret and SQLite memory URL. HTTP server and client used explicit distinct
`JWT_ISSUER=verdaxis-test-api`, `JWT_AUDIENCE=verdaxis-test-web`, the same test
secret, the full candidate SHA and an ephemeral numeric-loopback endpoint with
the required disposable token handshake. Socket guards allowed only the owned
DB for the backend and only the owned HTTP server for the client. No providers.

The PostgreSQL command was the existing `run_product_analytics_postgres_tests.sh`
through `work/task9-postgres.sh`, with only absolute root, 240-second readiness
and `docker rm -f -v` cleanup adjustments. It ran every `tests/postgres` file
except the three `test_phase0_*` files already run by the Phase 0 gate. Both
initial database `alembic check` commands reported **no new upgrade operations**.
The Phase 0 union fresh/upgraded-schema and drift checks also passed.

Raw evidence: `work/task9-{unit,monitor,postgres,http}.{log,xml}` and
`work/task9-phase0.log`. Phase 0 commands, suite coverage and hashes:
`work/phase0-fixtures/ed38c87f860e49ce906263f0f2c3365b/runner-evidence.json`.
Temporary evidence is deliberately not committed. HTTP reproduction scripts:
`work/task9-http.py` and `work/task9-http-seed.py`; synthetic rows only.

## Eleven-category checklist

Signed below as an integration evidence review, **not** independent approval.

| Category | Local evidence / result | Remaining gate and owner |
|---|---|---|
| Inventory | PASS: exact 61-source/62-target classification, unknown table/column refusal | Task 10 operator: compare actual host catalog |
| Schema | PASS: supplied connection, real transaction, union, fresh/upgraded equality and drift | Integration owner: repair five stale historical migration tests without weakening guards |
| Copy | PASS: all 55 copy tables, both references, four skips, JSON/null/float/date/EWKB, cycles, sequences, six interruption boundaries | Task 10 operator: real-data blockers, restored dump, trusted host authority |
| ACL | PASS: copied-union app/backup denial and existing role policy tests | Task 10 operator: exact production roles/Settings in separate cluster |
| Products | PASS: eight IDs, scalar/typed separation, qualification and union direct takes | Existing HTTP suite remains blocked; integration owner |
| Auth | BLOCKED: unit/PostgreSQL contracts pass; legacy HTTP fixtures do not | Integration owner: explicit token claims, valid synthetic emails, login-rate isolation |
| Privacy | PASS: copied notification payload retention and response redaction; reminder concurrency | Task 10 operator: private logging and provider egress proof |
| Timezone | PASS: fresh app/migrator/backup sessions, reconnect and role override case | Task 10 operator: scratch proof; Task 11: recycle every pool/CLI |
| Vault | PASS for existing unit and PostgreSQL vault suites only | Task 10 operator: four-worker credential loading, backup/key recovery, 503/500 and proxy 504 ambiguity; frontend owner: missing browser fault coverage |
| Frontend | MEASURED GATE: 53/54, new 27/27; PM accepts old unknown ETS shown as EUR 0 | Task 11 operator/founder: verify new entry HTML and send refresh note; extra cases below not covered |
| Rollback | PASS for executable synthetic tuple/locks and outbox drain | Task 10 operator: actual Linux systemd/Caddy, full old→new→old switch and source hashes |

Integration evidence reviewer: **Task 9 integration owner (AI assistant), 2026-10-16**.
Disposition signed: **BLOCKED**. Independent reviewer / PM acceptance: **pending**.
No signature or approval from another person is implied.

## Failed checks and explicit owners

1. **Integration owner, before Task 9 acceptance:** five PostgreSQL expectations
   predate union. Three `test_cutoff_downgrade_fails_fast_behind_concurrent_user_write`
   cases encounter union's unconditional downgrade refusal before the historical
   lock guard. Preserve the old lock proof in an independently migrated predecessor
   fixture; do not replace it with a weaker generic refusal assertion. The fresh
   migration test still expects `kycd_20261015_staging` as head. The quarantine
   test expects the old downgrade message rather than union's refusal.
2. **Task 10 Linux operator:** `test_four_worker_durable_sse_dispatch` errors because
   macOS has no `/proc`. It reached server readiness but did not prove dispatch.
   The 19 unit failures are exclusively `test_deploy_health.py` and the known Bash
   3.2 `mapfile`/lowercase-expansion limitation. Monitor's single skip is
   `test_runtime_identity.py:212`: passwordless nobody execution unavailable.
   Run all three checks on supported Linux before acceptance; none is a pass here.
3. **Integration owner, before Task 9 acceptance:** HTTP suite has 49 failures and
   six setup errors. Legacy token helpers omit issuer/audience and are rejected
   with 401. Reserved `disposable.invalid` emails produce registration 422 and
   profile serialization 500. Subsequent real logins hit 5/minute and return 429.
   These causes prevent meaningful execution of many downstream assertions;
   fixing fixtures may expose additional failures. Do not disable JWT validation,
   email validation or rate limiting. No claim of HTTP acceptance is made.
4. **Frontend owner / PM, before expanding acceptance claims:** reused Task 8
   evidence excludes copied RFQ history, the full auth-state matrix, all supplier
   FAME/LNG snapshots, KYC browser fault cases and every order-entry UI. SSE is
   blocked and external Mapbox/CORS are untested. Backend unit proofs are not
   substitutes for browser proofs. The PM exception covers only old ETS display.

## Review finding ledger

Read every backend `progress-task*.md`, frontend Task 8 progress, and all six
`phase0-task*-review-2026-10-16.md` reports in workspace `outputs/research/`.
No review files for Tasks 4b or 6 were present; no independent review is invented.

| Review | Disposition and evidence |
|---|---|
| Task 2 M1 | Closed: `745143ed`, negative inventory/assisted routes and exact successful scalar match; current unit suite passes |
| Task 2 M2 | Closed: `a92dc44d`, literal production seed bands, CI/LCV and catalog tuples; current unit suite passes |
| Task 3 N1 | Closed: non-BUYER/SUPPLIER profile branch and gate-on/off role-null vectors |
| Task 3 N2 | Closed: `test_order_expiry_reminders_concurrency.py`, three PostgreSQL tests pass in complete run |
| Task 4a M1 | Closed: `4c29220`, physical asyncpg transaction gate; four AUTOCOMMIT vectors, mid-DDL rollback and lock lifetime pass |
| Task 4a additional suggestions | Deferred to integration owner: broader literal/stamp, disconnect and all-engine connection-count vectors; Task 10 owns production-mode exact-identity proof |
| Task 5 M1/M2/M3/N1 | Closed: `964c9cb`, `972bbf2`, `fad8a50`, `97ac741`; formatting/RLS/reference completeness/empty-history regressions pass on union |
| Task 5 final integration | Union hashes, populated scalar history, multibatch cycles and copied-union ACLs pass. Host authority, isolation and restored-data proof deferred to Task 10 operator |
| Task 7 M1/M2 | Closed: asyncpg assertion CLI and every-other-session drain tests pass |
| Task 7 M3/M4 | Local fixes closed: explicit daemon reload/loaded-manager instructions and executable tuple interruptions. Actual manager/proxy proof deferred to Task 10 operator |
| Task 8 B1 | Measured two-bundle blocker closed by union rerun, 53/54 with one PM exception; broader unmeasured cases remain explicitly assigned above |
| Task 8 M1 | Not repaired in cached old JS. Explicit PM acceptance; Task 11 operator/founder owns entry-HTML check and refresh note |
| Task 8 M2/N1 | Closed: `cc2bc5d6`, scalar map fallback/forecast guard and public demo ticker; no LNG/UCOME demo prices |
| Task 8 N2 | Correct fixture/type closed by `a20b48af`/`2c62a24b`; full browser auth-state matrix deferred to frontend owner |
| Task 8 release-test suggestions | Deferred to frontend owner: broader stubbed release execution and CI wiring verification; no release run authorized |

## Frozen digests and Rust instructions

SHA-256 values (file hashes unless labelled semantic):

| Artifact | Digest |
|---|---|
| Structural schema (semantic) | `54a40050c8af02f0132fe3c8516089212cf0e7494c9b1e24eee6b42885d85ffa` |
| Migration graph (policy algorithm) | `748dbcdfba75e5b9b2b563c814a5d42808c05a7c4139218fee23402cc7742df9` |
| Reference values (semantic) | `5b522ebaed9732c8576f00bebf9100d7c36c7f67079add01f0234e3445bf1c58` |
| Copy manifest | `a5e27570803d5d00c43f83ebbee734acf97bea9891cf51ce8347b596ba958b1c` |
| Bootstrap policy | `e794312c20819ad342a4bbd083a1bbe073b29ec96aefe804e154899c035472c5` |
| Revision checkpoint allowlist | `b840a03ce47add08fe3e89e40aeb7fd92ceb17985a15aa43c7eb16984d0b4713` |
| Synthetic history corpus (`tests/phase0/synthetic.py`) | `33998481d3ad3c8a63e1fa7d390035590a67c5143240b1d0f8c5b5bcbeecf619` |
| Exact readiness corpus | `07172b9315750135599c7bae16d05754e718b09d1cb4bc87281e4a7fa526f50e` |
| Product/delivery policy (Task 2 algorithm) | `fbe6fb695a983267d0f7db973e3aac8b8b0ba803543ef7327756bde7e953bb20` |
| Phase 0 JUnit | `152c17ce8faeb370005cbe5022f5bedaa5a165d9b90bd5fa27c5f2e1c4e289cc` |
| Unit JUnit | `341729c5006621a84e8f7afaab6d1c74103b0558d82bf2e3e85f8497267dbe68` |
| Monitor JUnit | `5200883e7e6390c8bae29361113c70aca9f2521852319bc22e1c59354482c7d5` |
| PostgreSQL JUnit | `2edf50829dd8ff5037568efa918c4e2ff50b5d3a4486a10b8f0d5a0a799e8afd` |
| HTTP JUnit | `f731596aa70024cd295d0c7f767691918879b0355f1b037f350e9c78d67e5207` |
| Frontend raw two-bundle result | `287f546bfc9622c5c240c42735a0266bf124483a36ff64604482b6d798e3a70c` |

Rust owner: use the candidate SHA above for investigation, but **do not label it
an accepted final oracle yet**. After the blocked gates pass, regenerate catalog
policy, affected goldens and SQLx metadata from the accepted SHA. Target union,
UTC (`DB_SESSION_TIME_ZONE` too), pilot KYC off, all eight distinct product IDs,
absent-liquidity null prices, historical snapshots, notification redaction and
copied reference IDs. Replace the Phase 1 plan's old staging revision and
Singapore-session assumptions after compatibility proof. Keep migrator-backed
prestart and the compiled revision allowlist; no runtime SELECT on
`alembic_version`. Phase 0 does not authorize a Rust production rollout.

Rust plan decision 23 carries `created_at DESC, id DESC` on two legacy list
routes. That Python tie-break fix is not in this Phase 0 candidate. It does not
change Phase 0's schema, copy or economic contracts and does not explain these
acceptance failures. Rust must record that intentional tie-order delta when
comparing goldens; do not quietly treat its branch as this canonical oracle.

## Task 10 requirements and resume point

First close the local failures above and obtain independent acceptance. Then
obtain separate host approval and complete reviewed host commands. Host apply is
still disabled in bootstrap/copy; editing policy JSON is not authorization or
implementation. The integration owner must resolve that source gate before any
host apply attempt. No host mode was implemented in Task 9.

Linux rehearsal must prove exact production Settings/roles/names in a separate
cluster; live-target collision refusal; trusted OID/cluster bindings; blocked
provider/live-DB egress; complete restored-backup hashes; effective cached systemd
fence/drop-in loading; four workers; ordinary deploy two-lock and exact readiness
checks including union-to-union; stopped timers/cron and all writers; outbox drain;
fresh UTC sessions; encrypted key-ring recovery and vault fault semantics; and
full old→new→old tuple restoration with unchanged frozen source hashes. Keep
public mutations closed. After reopening, rollback needs reconciliation, never
automatic loss of new writes. Host backup producer and monitor wiring remain
operator-owned. Preserve Python feature freeze and do not port Rust work here.

No packages, skills, subagents, pushes, host operations or real data were used.
No frontend files were changed. Reused frontend build evidence: both environment
builds/artifact checks pass, 918 Vitest tests, 14 Node checks, 16 helper tests;
synthetic-token bundles are not publishable. See frontend progress for exact
build digests. Any later code change invalidates its affected evidence.
