# Browser retry and support-context headers implementation plan

Goal: browser clients can read early rate-limit responses and the server's actual support-context invalidation header.

Architecture: keep the existing FastAPI, middleware and API adapter. Move only CORS registration and expose an explicit three-header list. No origin, authentication, permission, limiter, market or database policy changes.

Tech stack: FastAPI/Starlette, HTTPX/Pytest, TypeScript/Vitest.

## Backend task

Files: app/main.py and existing tests/unit/test_idempotency_cors.py. Apply the same small change independently to production and staging; preserve branch differences.

Move the existing CORSMiddleware registration after preauth middleware registration and before request_logging_middleware registration. Requests must run logging -> CORS -> preauth -> support scope -> routes. Preserve settings.BACKEND_CORS_ORIGINS, allow_credentials=True, allow_methods=["*"], allow_headers=["*"]. Add only expose_headers=["Retry-After", "X-Request-ID", MARKET_SUPPORT_CONTEXT_INVALID_HEADER]. Do not expose an unused Context-Code header or add a Context-Expired alias.

Extend existing real-app ASGI tests: an exhausted preauth bucket returns429 to an allowed origin with exact credentialed CORS, Retry-After and echoed fixed requestID, and explicit exposed headers; disallowed origins remain without ACAO. A valid sensitive-path preflight does not consume a preauth bucket. Existing support mutation rejection and missing-auth behavior remain403/401 with allowed-origin CORS. Reuse current fixtures and ensure bucket state is restored. No database/network/lifespan startup.

Run the affected existing test module once per changed branch through the owner backend venv; broader exact-SHA CI is the final authority. Add one architecture sentence about the middleware order and browser-readable retry/invalidation contract.

## Frontend task

Files: src/services/api.ts and existing src/tests/api-market-support-context.test.ts. Change the two response header lookups from X-Verdaxis-Market-Support-Context-Expired to X-Verdaxis-Market-Support-Context-Invalid. Keep structured error-body invalidation, auth-generation/stale request checks and cache/context cleanup. No new alias, event, API surface or abstraction. Add or adjust one behavior-bearing existing test that invalidates a context when only the server header signals invalidity; cover a fresh request and the existing refresh-retry path only if each has distinct behavior. Run the affected existing suite once per branch with the branch's own Vitest4 dependency installation. Do not reuse mixed/shared node_modules.

## Review and release

Specification review reads the actual diff, then correctness/security review; the independent complexity reviewer can require deletion/reuse but cannot weaken a security boundary. Commit reviewed branch changes. Run required existing full CI at exact source SHAs, use protected PR merges and verify merged push checks. Release staging through existing guards/publisher, then production through EU backend guard and protected Vercel workflow with notify_release=false. Reuse existing backup and smoke paths. No load test is needed for this CORS fix. Record exact installed identities and operator handoff.

This first repair does not claim CPU reservation or command responsiveness under read saturation. Read admission and query candidates need separate measured proof before implementation.
