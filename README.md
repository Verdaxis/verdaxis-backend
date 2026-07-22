# Verdaxis Exchange — Backend API

Maritime fuel trading exchange platform backend.

## Quick Start

```bash
cd /home/verdaxis-prod/verdaxis/staging/be
source venv/bin/activate
ENVIRONMENT=development uvicorn app.main:app --host 127.0.0.1 --port 8000  # local only
```

## Live Deployment

The VPS runs the backend via systemd, not Docker Compose.

```bash
# Staging
cd /home/verdaxis-prod/verdaxis/staging/be
./scripts/deploy.sh --dry-run
APPROVED_RELEASE_SHA=<sha> \
MIGRATION_APPROVED_SOURCE_SHA=<same-sha> \
MIGRATION_EXPECTED_CURRENT_REVISION=<exact-current> \
MIGRATION_TARGET_REVISION=<allowlisted-target> \
  ./scripts/deploy.sh

# Production
cd /home/verdaxis-prod/verdaxis/prod/be
./scripts/deploy.sh --dry-run
APPROVED_RELEASE_SHA=<sha> \
MIGRATION_APPROVED_SOURCE_SHA=<same-sha> \
MIGRATION_EXPECTED_CURRENT_REVISION=<exact-current> \
MIGRATION_TARGET_REVISION=<allowlisted-target> \
  ./scripts/deploy.sh
```

The deploy helper infers and fixes the branch, service, and canonical guard path from the live checkout and always refuses dirty worktrees. It uses fixed trusted tool paths with inherited Git/database/Python routing controls removed; database-bearing Python helpers run with a minimal environment, and health retry controls are bounded integers. Dry-run resolves one remote full SHA, archives only that pinned commit, and verifies its unit manifest, migration-checkpoint policy, ACL policy/convergence bundle, and systemd bytes with trusted tooling. It never clones a mutable branch, links `.env`, imports candidate Python, runs pip/Alembic, opens the live database, or supplies candidate code with operator secrets/home/network. It prints `APPROVED_RELEASE_SHA=<sha>`; real deploy additionally requires the same SHA as `MIGRATION_APPROVED_SOURCE_SHA`, an exact expected current revision, and a literal target paired with it in that SHA's committed `deploy/migration-checkpoints.tsv`. Missing approval, symbolic targets such as `head`, moved source, unexpected live state, and non-allowlisted transitions refuse before source mutation. A deploy applies only the approved literal checkpoint with explicit application/migrator URLs on the same host/port and database but distinct exact roles; checkpoint application pins Alembic to that migrator URL, and startup revision verification never falls back to application credentials. After migration and before restart, the exact archived `scripts/converge_runtime_acls.py` plus its SQL policy rebuild runtime object/column ACLs transactionally as the migrator. The helper refuses ambient URL overrides, a symlinked/non-regular `.env`, a different endpoint, wrong database/role, or application credential. It verifies the exact migration result and publishes it with the release SHA so backend preflight can enforce the pause even when later revisions exist in source. Real deploys use a durable per-environment flock and `.runtime-deploy/<environment>.state`; live revision verification occurs under that lock, then the state is fail-closed before source mutation and remains through restart/readiness, including crashes and interruption. Services allow a start only during the explicit restart-authorized phase. After restart, parsed `/health/ready` JSON must report exact `status="ok"`, `db="ok"`, environment, and full release SHA before state is cleared. The application does not invoke Git. Off-host monitoring must use readiness; `/health/live` proves only that a process responds. The legacy `/health` path is a readiness alias.

`./scripts/install_systemd_units.sh --dry-run --environment <production|staging> --source-ref <approved-40-hex-sha>` is the no-change unit-install check. It attests the fixed checkout with trusted Git commands, archives the exact committed unit allowlist and unit blobs, verifies a SHA-256 digest manifest, and runs `systemd-analyze verify` only on private staged bytes. Unit filenames are globally unique and bound to production/non-`-staging` or staging/`-staging` destinations. It executes no checkout Python, preflight, Alembic, or candidate build backend and never reopens mutable unit paths. `--apply` is serialized by an environment-specific `flock` whose marker is accepted only by its root re-exec, records durable pending state in a fixed root-owned directory, prepares every replacement before changing destinations, replaces rather than trusts destination symlinks, rolls back a partial replacement, always runs `systemctl daemon-reload`, and removes pending state only after successful reload; a reload failure remains retryable. It never enables, starts, restarts, or deploys a service. The guard-aware units must be installed before relying on the deploy failure contract. Enabling each environment's news and prune timers is a separate operator-held action. Rollbacks publish a clean forward revert commit as a new release and use the same health gate; never automatically downgrade schema or roll release metadata back independently of code.

Cross-branch integration remains gated: runtime metadata stays directly after `pa_20260715_analytics_facts`, security integration reparents `sec_20260720_identity` onto runtime and preserves its chain through `sec_20260720_device`, and market revisions follow the reviewed security chain. Integration must add each reviewed pause as an explicit migration-checkpoint transition; operators approve and execute one checkpoint at a time rather than traversing to `head`. At the security checkpoint, the ACL policy must gain exact table/column entries for pending registration, organization joins, refresh sessions, and the admission/KYC columns emitted by combined signup/admin flows; unknown columns stay denied. The combined immutable unit manifest adds the production/staging auth-maintenance service/timer pairs and their audited bytes without installer code changes. Protected seed, quarantine, and market-evidence/provenance writes require integration-owned trusted writers; the legacy signal-ingestion CLI needs a separately approved operator identity rather than broader app DML. Audit and status history remain app-append-only. The four-worker runtime contract remains unchanged. This runtime-only branch is not a complete combined runtime+security release.

## API Endpoints

### Authentication
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| POST | `/api/auth/login` | No | Login (returns access + refresh tokens) |
| POST | `/api/auth/refresh` | No | Refresh tokens |
| POST | `/api/auth/register` | No | Register new user |
| GET | `/api/auth/me` | Yes | Get current user profile |
| PUT | `/api/auth/me/password` | Yes | Change password (returns fresh tokens) |

### Order Book
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/orderbook` | No | List all open orders |
| GET | `/api/orderbook/bids` | No | List open BIDs |
| GET | `/api/orderbook/asks` | No | List open ASKs |
| POST | `/api/orderbook` | Yes | Place order (auto-matches crossing orders) |
| GET | `/api/orderbook/my` | Yes | List user's own orders |
| DELETE | `/api/orderbook/{id}` | Yes | Cancel own order |

### Trades
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| POST | `/api/trades` | Yes | Hit an order to create a trade |
| GET | `/api/trades/my` | Yes | List user's trades |
| PUT | `/api/trades/{id}/confirm` | Yes | Confirm trade (counterparty only) |
| PUT | `/api/trades/{id}/decline` | Yes | Decline trade (counterparty only) |
| PUT | `/api/trades/{id}/deliver` | Yes | Mark delivered (with final qty/price) |
| POST | `/api/trades/{id}/pay` | Yes | Mark paid (seller only) |

### Price Discovery (Public)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/prices` | No | Aggregated trade prices (24h) with source/scope/demo provenance |
| GET | `/api/prices/reference` | No | Daily VWAP reference prices |
| GET | `/api/trade-tape` | No | Anonymized 7-day confirmed trade tape; filterable by `market_product`, `delivery_point_id`, `region`, and `availability_window` |

Market-data surfaces use a shared provenance vocabulary:

- `source_kind`: `CONFIRMED_TRADE`, `LIVE_ORDER`, `DEMO_SEED`, `BENCHMARK_REFERENCE`, `MIXED_SOURCE`, `NO_DATA`, or `UNKNOWN`.
- `scope`: `DELIVERY_POINT`, `REGION`, `PRODUCT`, or `UNKNOWN`.
- `demo_status`: `REAL_ONLY`, `DEMO_ONLY`, `MIXED`, `UNKNOWN`, or `NOT_APPLICABLE`.

Aggregate responses expose real/demo/unknown counts. Unknown contributors remain `UNKNOWN` rather than being reported as real activity.

### Compliance
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/compliance/fleet` | Yes | Fleet-wide compliance summary |
| GET | `/api/compliance/vessels/{id}/score` | Yes | Single vessel score |
| POST | `/api/compliance/scenario` | Yes | What-if fuel mix scenario |
| GET | `/api/compliance/fuels` | No | Fuel GHG intensity reference |

### Real-Time (SSE)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/stream/prices` | No | Price update events |
| GET | `/api/stream/orderbook` | No | Order events (created/cancelled/matched) with append-only provenance fields |
| GET | `/api/stream/trades` | No | Trade lifecycle events with append-only provenance fields |

### Admin (ADMIN role only)
| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/api/admin/analytics/overview` | Admin | Platform stats |
| GET | `/api/admin/analytics/daily` | Admin | Daily breakdown |
| GET | `/api/admin/analytics/product-usage?days=7\|30\|90` | Admin | Aggregated behavioral usage plus authoritative registrations, logins, and order-placing organizations |
| GET | `/api/admin/audit-logs` | Admin | Audit trail |

Behavioral analytics is optional. Configure the server-only
`ANALYTICS_ENABLED`, `UMAMI_BASE_URL`, `UMAMI_WEBSITE_ID`,
`UMAMI_API_USERNAME`, `UMAMI_API_PASSWORD`, and bounded
`ANALYTICS_REQUEST_TIMEOUT_SECONDS` values to enable it. Missing or unavailable
Umami returns a degraded behavioral section with HTTP 200 while preserving the
Verdaxis database counts. See `docs/behavioral-analytics-contract.md` for the
privacy and event contracts.

## Test Accounts

| Email | Password | Role |
|-------|----------|------|
| buyer@buy.com | password | BUYER |
| seller@sell.com | password | SUPPLIER |

## Tests

```bash
ENVIRONMENT=test RELEASE_SHA=test \
  JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \
  DATABASE_URL=sqlite+aiosqlite:///:memory: \
  python -m pytest tests/unit/ -v
```

Integration/E2E suites additionally require explicit pytest opt-in and an
attested disposable server on a numeric-loopback ephemeral port. Production,
staging, `localhost`, and ports 8000/8001 are refused by collection guards.
Start the source-tree producer with `ENVIRONMENT=test`, a matching
`DISPOSABLE_TEST_TOKEN`, and
`python -m tests.disposable_server --port <ephemeral-port>`.

Run the mutating suites only with the explicit opt-in flags; collection
skips them otherwise:

```bash
DISPOSABLE_ITEST_PASSWORD=... \
  python -m pytest tests/integration -v \
  --run-disposable-integration \
  --disposable-target-url http://127.0.0.1:59123 \
  --disposable-target-token <token-provisioned-into-the-disposable-server>
```

There is no staging or production integration target: mutating suites accept
only an attested `127.0.0.1` ephemeral-port disposable server. (The former
`ALLOW_TEST_MUTATIONS`/`TEST_RUNTIME_ENV` resolver in `tests/runtime_config.py`
remains for staging-scoped tooling but no suite consumes it.)

Use `scripts/run_product_analytics_postgres_tests.sh` for a disposable
PostGIS container. It binds a unique loopback port, runs migrations, checks
Alembic drift, applies and validates the idempotent app/migrator/backup role
policy, exercises the runtime upgrade/downgrade roundtrip, and removes only
the container it created.

Credentialed API CORS is fail-closed by environment: production allows only
the canonical production landing/app origins, staging only its staging origin,
and development/test only enumerated localhost origins. Wildcards are never
used with credentials.

## Seed safety

Every executable seeder requires `SEED_DATABASE_URL`,
`ALLOW_SEED_MUTATIONS=I_UNDERSTAND_SEED_MUTATIONS`, `SEED_RUNTIME_ENV`, and an
exact matching `SEED_TARGET_DATABASE`. Production/system databases,
superuser URLs, non-loopback targets, and non-canonical availability windows
are denied. Disposable database names end in `_test`; staging is exactly
`verdaxis_staging`. Seeders never inherit an implicit application URL.

Historical repository versions contained a live database credential in seed
scripts. Current-tree source cleanup is insufficient and does not revoke it;
rewriting repository history is not a substitute for rotation. An operator
must create and verify a replacement least-privilege credential, update the
secret store and deployed environment, attest the replacement role, then
revoke the exposed credential. This repository change performs none of those
external actions.

## Feature Branches

- `feature/oauth-integration` — Google + Microsoft SSO (169 tests, ready for merge)
