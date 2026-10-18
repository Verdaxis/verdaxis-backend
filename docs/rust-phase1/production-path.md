# Production path source acceptance

19 October 2026. Plan: `docs/plans/2026-10-19-rust-phase-1-production.md`.

## Built boundary

- Production bootstrap policy remains false. New unit is `verdaxis-rust.service`, loopback 8010, app-only projected runtime credentials, existing start/revision/budget prestarts, 192/256 MiB memory limits, one-CPU quota and lower CPU weight.
- Production configuration requires `verdaxis_app`, `127.0.0.1:5432/verdaxis_prod`, union checkpoint and prepared budget. Production CORS uses the current Python production origin set. MCP remains non-production.
- Shared deploy selects the environment policy and binds production artifact provenance to `refs/heads/prod`. Unit absence and wrong provenance refuse before source mutation. Both readiness documents are required before state clears.
- Offline Caddy candidate generation changes only one upstream token and retains exact rollback bytes; production MCP changes refuse. The existing Task 13 matcher snippet is unchanged.
- No changes under `app/` or `alembic/`, and no changes to staging policy, staging unit or comparator. No host install/deploy/Caddy action, signing, live database access or measurements were performed.

## Test-first evidence (isolated staging test account only)

Each command used `verdaxis-staging-test` and `/home/verdaxis-test/bin/heavy run 3600`. Local logs are untracked under `work/`.

| Gate | Exit | Result |
| --- | --- | --- |
| Initial production Python authority red | 1 | Missing policy/unit and environment selector |
| Initial Rust authority red | 101 | Production unsupported |
| Production deploy red | 1 | Existing production path ignores Rust activation and refusal inputs |
| CORS red | 101 | Production origin policy absent |
| Caddy candidate red | 1 | Generator absent |
| Production prepared-budget red | 101 | Missing budget accepted before guard |
| Deployment/runtime/unit/budget/Caddy/health regressions | 0 | 216 passed, one existing warning |
| Final Rust runtime + HTTP | 0 | 33 passed |
| Final production/Caddy/monitor source ownership | 0 | 16 passed |
| Pinned Rust formatting | 0 | Passed after retrieving remote formatting |
| Workspace all-target clippy, warnings denied | 0 | Passed |

Intermediate runs found obsolete staging-only assertions, a fixture import error (collection exit 2), fixture production database mismatch, and local formatting drift; all were corrected and rerun. One staging fixture readiness attempt failed once and passed in the complete rerun. One wrapper transport call returned 255; the isolated unit was confirmed absent before rerun. These were not suppressed or reported as PASS. All test calls finished.

GitHub PR CI is the full build/offline/contract/PostgreSQL authority and is recorded in the delivery report. A PR does not publish a deployable production artifact; actual `prod` push offline, contract and publish success remains a later operator gate.

`refresh-codesight` completed locally as documentation generation, not a build/test. Its broad generated changes (including unrelated schema/wiki rewrites) were retained in `work/` and excluded from the production patch; `ARCHITECTURE.md` carries the scoped relationship update.

## Operator gates still open

Private runbooks are in the workspace-level `outputs/runbooks/`, not this public repository:
- `2026-10-19-rust-staging-caddy-rehearsal.md`
- `2026-10-19-rust-production-phase1.md`

Live acceptance requires staging 14b PASS, checklist D rehearsal, reviewed full promotion payload, production B' deploy/install, separate prod-push S' artifact and activation, then simple/orderbook/prices/curves windows. Production MCP stays Python. R12–R14 quotas are process-local; the PM must accept the bounded one-minute quota reset or stop prices for a separately approved shared limiter.

Host questions and exact read-only commands are in the plan: confirmed EU Caddy import/site paths, unit/drop-in authority, live source/revision, PostgreSQL maximum/timezone/connection owners, and usable passive latency/5xx monitoring thresholds. No source test substitutes for those answers. Never fill missing host configuration with guessed Caddy bytes.
