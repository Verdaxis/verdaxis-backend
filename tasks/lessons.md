# Lessons — Verdaxis Backend
<!-- Self-improvement-loop: Add corrections here as Trigger → Rule → Why -->
<!-- Read at session start. Write after ANY user correction. -->

## Format
- **Date:** YYYY-MM-DD
- **Trigger:** What happened
- **Rule:** What to do instead
- **Why:** Root cause

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
