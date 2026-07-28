# Lessons — Verdaxis Backend
<!-- Self-improvement-loop: Add corrections here as Trigger → Rule → Why -->
<!-- Read at session start. Write after ANY user correction. -->

### Prioritize live onboarding blockers over strategic migrations
- **Date:** 2026-07-30
- **Trigger:** The user shelved the Supabase migration because a real Hapag-Lloyd signup could not be approved.
- **Rule:** When a real customer onboarding path is blocked, isolate strategic work and resolve the live flow end to end before resuming it.
- **Why:** Platform migration planning has lower immediate value than restoring a revenue-critical user journey.

## Format
- **Date:** YYYY-MM-DD
- **Trigger:** What happened
- **Rule:** What to do instead
- **Why:** Root cause

### Encode measured shared database budgets
- **Date:** 2026-07-20
- **Trigger:** The database architect measured the shared PostgreSQL cluster and recommended a lower per-worker pool plus explicit session timeout policies.
- **Rule:** Budget prod, staging, and maintenance together from measured capacity; configure role/session statement, lock, and idle-transaction timeouts and keep migrations on a separate longer-lived policy.
- **Why:** Per-service pool arithmetic and absent timeout policy can exhaust the shared cluster or leave market locks and idle transactions unbounded.

## Security

- **Date:** 2026-02-18
- **Trigger:** python-jose 3.5.0 is unmaintained (last release 2022) with known CVEs.
- **Rule:** Use PyJWT or joserfc instead of python-jose.
- **Why:** Unmaintained JWT libraries carry unpatched CVEs.

- **Date:** 2026-02-18
- **Trigger:** passlib 1.7.4 is unmaintained (last release 2020) and uses the deprecated crypt module.
- **Rule:** Use bcrypt directly or argon2-cffi for password hashing instead of passlib.
- **Why:** passlib's crypt dependency is removed in Python 3.13.

- **Date:** 2026-02-18
- **Trigger:** postgres_data/ directory was not in .gitignore.
- **Rule:** Gitignore data directories before they can be accidentally committed.
- **Why:** Committed database files expose data and permanently bloat the repo.

## Python

- **Date:** 2026-02-18
- **Trigger:** Multiple uses of datetime.utcnow() across the codebase.
- **Rule:** Always use datetime.now(timezone.utc) instead of datetime.utcnow().
- **Why:** Deprecated in Python 3.12, removed in 3.14; returns naive datetimes causing timezone bugs.

## Architecture

- **Date:** 2026-02-18
- **Trigger:** Two different get_current_user implementations existed in core/auth.py and routers/auth_simple.py.
- **Rule:** Maintain a single auth implementation; consolidate before adding new patterns.
- **Why:** Dual auth implementations cause inconsistent security behavior and maintenance hazards.

- **Date:** 2026-02-18
- **Trigger:** Redis container was running but no code referenced Redis anywhere.
- **Rule:** Remove unused services from docker-compose.yml.
- **Why:** Unused services consume memory, create attack surface, and confuse developers.

- **Date:** 2026-04-08
- **Trigger:** Orderbook timing was initially treated as delivery-window logistics instead of a matching-time availability bucket.
- **Rule:** For the demo orderbook, model time as mandatory `availability_window` matching metadata and keep post-match delivery logistics off-platform.
- **Why:** The product semantics are marketplace availability first; delivery scheduling belongs to later operational workflows.

- **Date:** 2026-04-08
- **Trigger:** Stale `AI_README.md` guidance was still sitting in the repo even though agent entrypoints now use `CLAUDE.md` and `.codesight`.
- **Rule:** Remove obsolete AI bootstrap docs and keep agent guidance consolidated in `CLAUDE.md`, `ARCHITECTURE.md`, and `.codesight`.
- **Why:** Multiple overlapping AI docs drift quickly and send future sessions to stale instructions.

- **Date:** 2026-04-08
- **Trigger:** Deployment work started as a direct rollout while the user intended staging branch consolidation first.
- **Rule:** When asked to "apply all changes on staging", merge all approved staging-side branches into `origin/staging` before deployment.
- **Why:** Deploying before branch consolidation can leave approved fixes out of the environment and create drift between git and runtime.

## Dependencies

- **Date:** 2026-02-18
- **Trigger:** Most dependencies used unpinned >= specifiers in requirements.txt.
- **Rule:** Pin all production dependencies to exact versions and use pip-compile for lockfiles.
- **Why:** Unpinned deps can break silently on incompatible upstream releases.
### Validate live data after catalog changes
- **Date:** 2026-04-13
- **Trigger:** The staging backend still served legacy marketplace rows and product naming after the green-fuels redesign because live/demo data was not fully normalized.
- **Rule:** When changing catalog semantics, verify live seeded rows and response payloads on staging, not just schema and code paths.
- **Why:** Backward-compatible models can keep serving stale data even when the new contracts compile and tests pass.
### Exercise combined marketplace filters on staging
- **Date:** 2026-04-13
- **Trigger:** A user-facing marketplace search failed after the public fuel filter was added because the route joined `products` twice when `fuel_type` was also present.
- **Rule:** After changing query-scope helpers, verify the live route with realistic combined filters such as `region + fuel_type + availability_window`, not just single-filter cases.
- **Why:** SQL join bugs can hide behind passing unit tests until the exact UI query shape hits Postgres.

### Verify public qualified rows after execution-policy changes
- **Date:** 2026-04-14
- **Trigger:** Staging looked empty even though open orders still existed, because legacy seed rows were missing execution qualifiers and the public marketplace correctly filtered them all out.
- **Rule:** After changing execution qualification rules or cleaning up smoke-test orders, verify that live staging still has execution-qualified public rows, not just open rows in the database.
- **Why:** A passing trade-path smoke test can still leave the public market blank if seeded/demo data no longer satisfies the marketplace contract.

### Keep trading delivery points on the approved port list
- **Date:** 2026-04-14
- **Trigger:** I left legacy bucket and non-approved ports like `ARA`, `Fujairah`, and `Houston` active after the user narrowed the trading surface to six specific ports.
- **Rule:** When the user defines an approved trading port set, update both the catalog/seed layer and the live dropdown/data sources together, and remove bucket ports like `ARA` from executable trading surfaces.
- **Why:** Mixed port taxonomies make the marketplace, forward curve, and seeded data disagree about what a valid market slice is.

### Treat Trade Tape As Live
- **Date:** 2026-05-19
- **Trigger:** The user corrected the Trade Tape showing `Market Closed` even though Verdaxis physical fuel trading should be presented without exchange-session delay.
- **Rule:** Do not apply exchange-session open/closed semantics to Verdaxis market surfaces unless the product owner explicitly defines a session calendar.
- **Why:** Maritime fuel trading is not being modeled as a venue with fixed UTC exchange hours, and stale session logic makes the product look unavailable.

### Separate Trade Tape Availability From History
- **Date:** 2026-05-23
- **Trigger:** The user clarified that "24" meant no market-hours delay, while trade tape history should remain 7 days.
- **Rule:** Label trade tape availability as live/no delay and keep the history lookback as a separate 7-day setting in code, docs, and UI copy.
- **Why:** "24h" can be misread as a data lookback window, which causes incorrect implementation and documentation changes.

### Keep Canary Checks Off Quota-Limited Email
- **Date:** 2026-05-25
- **Trigger:** The signup canary consumed Resend daily verification-email quota by exercising the real OTP email path every five minutes.
- **Rule:** Synthetic signup health checks must authenticate with a monitor token and suppress external email sends for narrowly scoped canary addresses only.
- **Why:** Canary traffic should verify application flow without spending provider quotas or blocking real user onboarding.

### Match the deployed runtime readiness contract
- **Date:** 2026-07-20
- **Trigger:** Integration review corrected readiness from `database=connected` with no extras to required `db=ok` fields plus bounded-compatible extras.
- **Rule:** Require `status=ok`, `db=ok`, exact environment, and matching full 40-hex release SHA; bound extra keys/values and reject nested or collection extras instead of assuming a different exact payload.
- **Why:** Local attestation must fail closed on identity and dangerous payload shapes while remaining compatible with the runtime-v2 producer contract.

### Honor Sprint-Level Test Skips Explicitly
- **Date:** 2026-06-17
- **Trigger:** User corrected the Forward Curve demo-seed sprint to skip writing and running tests for now.
- **Rule:** When the user explicitly pauses tests for a sprint, do not add new test files or run pytest; use compile/smoke/browser dogfood evidence instead and record the deferred test gap.
- **Why:** The user is prioritizing fast staging review for simple/demo-data slices, and adding tests can slow down the immediate product feedback loop.

### Synthetic Browser Checks Must Fail Closed Without Crashing
- **Date:** 2026-06-22
- **Trigger:** The Verdaxis monitor reported `rendered page checks crashed` because headless Chromium timed out while dumping the login page DOM.
- **Rule:** Wrap browser-render timeouts as ordinary monitor failures with clear context, and set render timeouts for VPS/browser cold-start latency rather than assuming fast local Chrome startup.
- **Why:** A synthetic UI check is useful only if it reports the failing condition; uncaught subprocess timeouts make the monitor itself look broken and obscure the actual site state.

### Treat Vercel Bot Challenges As Monitor-Client Blocks
- **Date:** 2026-06-23
- **Trigger:** The Verdaxis monitor reported the login page was missing `Sign In` because Vercel returned a Security Checkpoint page to the VPS/headless monitor client.
- **Rule:** Synthetic frontend checks against Vercel-hosted sites must detect Vercel Security Checkpoint responses and report them as monitor-client challenges, not application-render failures.
- **Why:** Repeated headless checks can trigger Vercel mitigation for the monitor IP; the app can be healthy for normal users while the synthetic browser sees only the challenge DOM.

### Create Named User Accounts In The Requested Environment
- **Date:** 2026-07-08
- **Trigger:** The user clarified that `belinda@verdaxis.exchange` should be created/updated on prod, not treated as a staging mirror task.
- **Rule:** For named operational user accounts, default to the explicitly requested environment and avoid mirroring to staging unless asked.
- **Why:** User provisioning affects real access; environment drift is safer than unintentionally creating extra login surfaces.

### Keep Test Helpers On Declared Dependencies
- **Date:** 2026-07-20
- **Trigger:** A touched test helper imported `requests`, which is not declared in `requirements.txt`.
- **Rule:** Prefer an already-declared HTTP client such as `httpx` before adding a dependency to a test helper.
- **Why:** Test-only dependency drift makes clean CI/bootstrap environments fail unnecessarily.

### Validate Exact Runtime Authority And Artifact Provenance
- **Date:** 2026-07-20
- **Trigger:** Final runtime review found that unrelated database/schema grantees survived bootstrap, systemd units could come from an unapproved invoking tree, news refresh had no external owner, and migrator placeholder passwords were accepted.
- **Rule:** Security validators must compare complete ACL/provenance sets, operational jobs must have one source-controlled owner per environment, and every deployed credential class must reject placeholders consistently.
- **Why:** Checking only named roles or expected live checkouts leaves unexamined authority and artifact paths that can pass validation while violating the deployment contract.

### Normalize Every Inherited Authority And Freeze Install Inputs
- **Date:** 2026-07-20
- **Trigger:** Follow-up review found global default ACLs, delegated grants, decoded blank passwords, unbound prune units, and a verify-then-reopen systemd unit race.
- **Rule:** Validate every scope that composes into effective authority, require destructive jobs to attest deployed identity, and materialize install candidates once from immutable commit objects before verification or copy.
- **Why:** Exact checks at only the obvious scope and hashes followed by mutable path reuse leave inheritance and TOCTOU paths outside the claimed security boundary.

### Remove Obsolete Harnesses Identified By Audit
- **Date:** 2026-07-20
- **Trigger:** Ponytail audit found the purchase-flow helper had no references and targeted obsolete endpoints.
- **Rule:** Remove zero-reference legacy harnesses instead of preserving direct invocation compatibility when no authoritative use remains.
- **Why:** Compatibility code for dead endpoints increases maintenance and can mislead operators about supported runtime workflows.

### Keep Repair And Release Identity Exact Through Failure
- **Date:** 2026-07-20
- **Trigger:** Review found object ACLs that validation rejected but bootstrap could not repair, dry-run checks that were skipped, and deploy failures that could leave new source bytes paired with stale release identity.
- **Rule:** Every exact validator needs an idempotent repair for the same authority scope, and deployment must publish identity before executing selected-tree code while dry-run executes every genuinely non-mutating gate.
- **Why:** A validator-only policy and a success-only identity update both fail closed too late, leaving operators unable to converge or runtime metadata inconsistent with executable bytes.

### Keep Readiness Vocabulary Canonical Across Owners
- **Date:** 2026-07-20
- **Trigger:** Integration review found runtime readiness reported `db=connected` while the approved monitor contract requires `db=ok`.
- **Rule:** Treat readiness JSON as a cross-component API and assert its complete canonical success payload in runtime, deploy, and monitor-facing tests.
- **Why:** Semantically similar status words still break strict health gates and can cause a healthy release to be rejected after integration.

### Preserve The Branch-Owned Migration Edge During Linearization
- **Date:** 2026-07-20
- **Trigger:** Integration clarified that runtime metadata remains immediately after product analytics and security's first revision must be reparented onto runtime, not the reverse.
- **Rule:** Keep branch-owned migration ancestry unchanged unless explicitly assigned; express cross-branch linearization as a precise integration reparenting gate on the downstream branch.
- **Why:** Reparenting the wrong branch changes ownership boundaries and can make an isolated remediation conflict with the intended combined migration order.

### Bind Cutovers And Runtime Authority To Explicit Policy
- **Date:** 2026-07-20
- **Trigger:** Integration review arrived after the runtime pass and found that deploy still traversed to Alembic head while the app role inherited blanket current/future DML.
- **Rule:** Production cutovers must bind an exact source, expected revision, and allowlisted checkpoint; database bootstrap must reconstruct app authority only from explicit table, privilege, and column declarations.
- **Why:** Branch heads and default grants silently absorb later security or market changes, bypassing staged review boundaries and giving ordinary runtime code control-plane authority.

### Never Fall Back Across Database Authority Boundaries
- **Date:** 2026-07-20
- **Trigger:** Parent integration review found the checkpoint reader could substitute the application URL when the migrator URL was absent.
- **Rule:** Migration and revision-verification paths must require an explicit, distinct migrator URL and role before opening an engine; never reuse application credentials for control-plane work.
- **Why:** An availability fallback collapses the least-privilege role split and can run schema control operations with runtime authority.

### Preserve Real Write Paths When Narrowing Column ACLs
- **Date:** 2026-07-20
- **Trigger:** ACL review found that omitting `organizations.verification_status` would break signup plus existing demo/admin organization flows.
- **Rule:** Before narrowing a table to column grants, inventory ORM defaults and every signup, system, and administrator write path; grant only the exact current columns and prove them through the raw runtime role.
- **Why:** A syntactically exact allowlist can still deny required application transactions when it models desired sensitivity but not actual emitted SQL.

### Model Append-Only Runtime Authority Explicitly
- **Date:** 2026-07-20
- **Trigger:** ACL review found `audit_logs` and `user_status_transitions` were read-only even though normal request transactions append both.
- **Rule:** Give runtime event/history tables explicit `SELECT, INSERT` authority with `UPDATE, DELETE` denied, and test the business mutation and its append in one raw-role transaction.
- **Why:** Least privilege is not synonymous with read-only; denying required append authority breaks auditability and atomic status history.
### Keep Periodic Jobs Out Of Web Workers
- **Date:** 2026-07-20
- **Trigger:** News refresh scheduling in the API lifespan ran once per worker, multiplying refreshes across the environment.
- **Rule:** Keep web workers request-driven and expose periodic work as a process-independent one-shot CLI invoked by exactly one environment timer.
- **Why:** Worker counts are an implementation detail and are not a safe coordination mechanism for singleton jobs.

### Do Not Hold Request Sessions Across SSE Streams
- **Date:** 2026-07-20
- **Trigger:** Security review found private SSE handlers retained request-scoped database sessions for the entire stream and treated backend failures as anonymous access.
- **Rule:** Resolve SSE authorization with short-lived sessions, close/rollback immediately, and catch only expected authentication errors; propagate database/runtime failures.
- **Why:** Long-lived streams can exhaust small connection pools, and fail-open anonymous fallback hides authorization outages.

### Make Every Security Transition Explicit And Independently Revalidated
- **Date:** 2026-07-20
- **Trigger:** A second security review found implicit tenant-join approval, ambiguous SSE credentials, incomplete party provenance, and revocation races after an initial hardening pass.
- **Rule:** Model each approval as its own audited operation, bind long-lived/private flows to one credential and immutable scope, and revalidate every persisted actor inside the transaction that changes executable state.
- **Why:** Reusing adjacent approval transitions or inferred ownership creates authorization coupling that happy-path tests do not expose.

### Treat Auth Error Codes As A Cross-Client Contract
- **Date:** 2026-07-20
- **Trigger:** Frontend review found refresh failures exposed only human strings, forcing unsafe string matching to decide whether authentication was terminal.
- **Rule:** Every terminal refresh 401 must use an allowlisted stable machine code with separate human detail; transient rotation races remain a distinct 409 code.
- **Why:** Client token preservation and logout behavior are security decisions and cannot depend on mutable prose.

### Keep Operational Surfaces Out Of The Public API
- **Date:** 2026-07-20
- **Trigger:** Endpoint inventory found log reading, host metrics, a state-changing news refresh, and dead role mutation exposed through web routes.
- **Rule:** Operational logs, detailed host telemetry, and scheduled maintenance belong in authenticated infrastructure tooling or one-shot CLIs; remove dead token-mutation routes and assert their absence from the application route table.
- **Why:** Even nominally protected operational endpoints expand data-exposure and state-mutation attack surface, while duplicate scheduler entry points undermine singleton controls.

### Derive Mutable Account Identity From Authentication
- **Date:** 2026-07-20
- **Trigger:** The onboarding survey accepted a body email and let an unauthenticated caller win a write-once update for any verified account.
- **Rule:** Account-mutating endpoints must derive the target user from an authenticated principal or a narrowly scoped server-verified token; body identity fields are forbidden, even when the write is idempotent.
- **Why:** Rate limits and write-once semantics do not establish ownership and can turn first-write behavior into a cross-account race.

### Reject Unsafe External Links Before Persistence
- **Date:** 2026-07-20
- **Trigger:** RSS ingestion persisted publisher-controlled links that the frontend later opened directly without validating scheme, host, or size.
- **Rule:** Validate and bound externally supplied titles and URLs at ingestion; persist only absolute HTTP(S) links without credentials or local/private destinations.
- **Why:** Output rendering is too late to make unsafe stored links harmless, and feed data is untrusted even when fetched from a known publisher.

### Remove Nonfunctional Upload Surfaces
- **Date:** 2026-07-20
- **Trigger:** A legacy compliance upload endpoint accepted files but performed no verification, while an overlapping active compliance router already served the supported product.
- **Rule:** Do not register placeholder upload or mutation routes; remove obsolete routers from the application and assert dead paths are absent.
- **Why:** Authentication alone does not justify parser/upload attack surface, and stubs create a misleading security contract.

### Serialize Cookie Authentication By Browser Device
- **Date:** 2026-07-20
- **Trigger:** A follow-up review showed that cross-account login and refresh responses could arrive out of order, allowing a delayed cookie response to restore an older refresh family.
- **Rule:** Bind refresh families to an opaque HttpOnly device identifier and serialize login, refresh, and logout with the same device-scoped PostgreSQL advisory lock; revoke every superseded device family before issuing a replacement.
- **Why:** Token rotation alone orders database writes, not browser `Set-Cookie` application, so response reordering must leave every delayed token cryptographically and server-side unusable.
### Require Attested Disposable Integration Targets
- **Date:** 2026-07-20
- **Trigger:** A review run inherited the test suite's implicit localhost API target and created a pending production registration while deploy identity, backup producer wiring, and retirement proof were still only partially integrated.
- **Rule:** Integration and E2E tests must require an explicit disposable opt-in, loopback URL, non-live port, and matching disposable-server attestation; operational remediations must test the complete producer/deploy/cutover wiring rather than isolated consumers alone.
- **Why:** Safe-looking defaults and source-only helpers can cross a live boundary or leave critical invariants unenforced when the caller, producer, and retirement path are not mechanically gated end to end.

### Exercise Process and Promotion Boundaries End to End
- **Date:** 2026-07-20
- **Trigger:** Review found that a mocked backup writer concealed `pg_dump` bypassing `gzip.GzipFile`, while deploy and installer tests did not prove checkout/identity alignment or byte-safe rollback.
- **Rule:** For subprocess streams and artifact promotion, add regression tests using real child processes and real staged filesystem transactions; never infer correctness from callback writes, self-reported contracts, or metadata-only rollback.
- **Why:** File-descriptor inheritance and partial promotion behave below the mocked API boundary, so unit-shaped tests can report green while publishing corrupt or provenance-ambiguous artifacts.

### Keep Operational Branches Inside Their Owner Boundary
- **Date:** 2026-07-20
- **Trigger:** Source review rejected the local-monitor branch for taking ownership of runtime deploy identity, a competing root installer, and an incomplete backup producer replacement.
- **Rule:** A monitor branch may publish read-only contracts, readers, proof gates, and a static immutable-install inventory; runtime deploy, activation, and full producer responsibility stay with their canonical owners and are documented as integration seams.
- **Why:** Duplicated partial ownership creates competing activation paths and can falsely claim rollback, backup success, or cutover safety without preserving the complete runtime responsibility graph.

### Delete Non-Owned Reference Implementations
- **Date:** 2026-07-20
- **Trigger:** Parent source-ownership review rejected backup producer code retained as a non-installable reference after the branch became monitor-only.
- **Rule:** Once an operational responsibility is assigned to an external owner, delete local executable reference implementations and their tests instead of excluding them from promotion and calling them documentation.
- **Why:** Dead executable references still create maintenance and authority ambiguity; a narrow data contract documents the integration seam without competing code ownership.

### Pin Frameworks That Security Tests Introspect
- **Date:** 2026-07-22
- **Trigger:** An unpinned `fastapi>=0.100.0` resolved to 0.139.2 in a fresh environment, whose lazy `_IncludedRouter` objects broke `app.routes` introspection and the route-surface security tests while HTTP behavior stayed green.
- **Rule:** Dependencies whose internal object model is asserted by tests (route tables, middleware stacks, schema internals) must be pinned or bounded, with the breakage documented next to the pin.
- **Why:** Loose bounds turn a routine dependency resolution into a silent contract change that only surfaces in fresh environments, making CI results depend on install date rather than source.

### Pin Content Hashes, Not Commit Hashes, For Same-Commit Tripwires
- **Date:** 2026-07-22
- **Trigger:** The source-ownership tripwire pinned runtime-owned files to a commit hash; the next stage legitimately changed a pinned file and could not reference its own unborn commit in the same single-commit change.
- **Rule:** Ownership/tamper tripwires that must survive reviewed changes to the guarded files should pin per-file content hashes (git blob SHAs), updated deliberately in the same commit as the guarded change.
- **Why:** A commit-hash pin creates an unresolvable self-reference for single-commit stages, forcing either a two-commit dance or silent disabling of the guard.

### Assign Outbox Stream Sequences After Commit Via One Leader
- **Date:** 2026-07-22
- **Trigger:** Designing Last-Event-ID replay for the shared SSE transport: sequences assigned inside producing transactions become visible out of order (a lower sequence can commit after a higher one), so a subscriber cursor would silently skip events.
- **Rule:** Durable stream cursors require sequence assignment that is serialized after the producing commit (single advisory-lock leader assigning from a database sequence); treat NOTIFY strictly as a lossy wake with a poll fallback, never as the delivery channel.
- **Why:** Producer-assigned monotonic IDs plus `seq > cursor` reads form a classic visibility race; the failure is unobservable in single-process tests and loses committed events under real concurrency.
### Distinguish Legacy Token Compatibility From Link Validity
- **Date:** 2026-07-22
- **Trigger:** I described the identity cutover as necessarily invalidating active verification links before checking the hash-only verifier and migration behavior.
- **Rule:** Before claiming an auth migration invalidates links, inspect both the migration and the post-cutover verifier; distinguish link validity from old-worker and rollback compatibility guards.
- **Why:** `sec_boundaries` retains token hashes that the hardened verifier accepts, while its expiry guard protects the plaintext-writing legacy release and rollback path rather than proving the links are unusable.

### Keep Assisted Operations In The Customer Product Context
- **Date:** 2026-07-23
- **Trigger:** Market Support was implemented as a separate administrative authorization console, but the requested workflow was for an administrator to enter a customer's organization and use the normal customer-facing product on its behalf.
- **Rule:** Assisted-operation features should reuse the customer workflow under an explicit, audited organization context; keep authorization machinery behind the workflow instead of exposing it as the primary staff interface.
- **Why:** A parallel operations console duplicates product behavior, exposes internal controls, and makes staff learn a different workflow from the customer whose experience they are supporting.
### Activate Split Demo Schedulers During Cutover
- **Date:** 2026-07-28
- **Trigger:** The user found only one production listing after the shared demo scheduler was retired.
- **Rule:** A cutover that retires a shared synthetic-data scheduler is incomplete until each approved environment-specific replacement is installed, started, enabled, and verified against live listing counts.
- **Why:** The legacy timer was correctly disabled, but its split production replacement was never activated, so every demo listing eventually expired.

### Restore Demo Coverage, Not Just Scheduler Activity
- **Date:** 2026-07-28
- **Trigger:** The user clarified that 17 generated listings did not restore the much broader demo book used in earlier demonstrations.
- **Rule:** When recovering demo liquidity, compare against the prior visible coverage and restore canonical product-port-window depth rather than stopping at a small active count.
- **Why:** Restarting a one-listing-per-tick generator repairs ongoing activity but does not recreate the seeded breadth users expect during a demo.
