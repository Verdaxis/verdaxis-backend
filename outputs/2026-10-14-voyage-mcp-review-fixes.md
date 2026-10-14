# Voyage MCP review fixes

Date: 2026-10-14
Branch: `feat/voyage-mcp-t4`
Base: `67bf733ded76a2580d8112fa23025449b8ee6165`

## Changes

- Public market GETs and MCP database tools share one limiter created in `app/main.py`. Capacity retains the existing pool-minus-one formula. Zero capacity rejects database reads; `list_markets` bypasses admission.
- The planner uses the shared ETS phase-in schedule without intermediate EUR rounding. The exact regression returns USD 16.76 and EUR 1.58.
- Speed is bounded from 1 to 30 knots. Calendar voyage duration, including final berth, is checked against 365 days before date arithmetic. MCP returns the planning error before opening a database session.
- Green-only vessels generate FULL strategies without EU scope. Missing prices remain explicit in the recommendation reason.
- Updated transport documentation, implementation plan, architecture relationship and project lessons.

## Verification

- New regression run before fixes: 12 failed, 1 passed (the valid 365-day boundary), 64 deselected.
- Requested focused suite: **212 passed**, 16 warnings.
- Full `tests/unit/` suite: **1,833 passed, 19 failed**, 483 warnings. All failures are in `test_deploy_health.py`, the expected macOS failures. Output includes `scripts/deploy.sh: line 479: mapfile: command not found`. No other failures.
- Raw logs: `work/review-regressions-before.log`, `work/review-focused.log`, `work/review-full-unit.log` (local, not committed).
- `git diff --check`: clean.

## Scope

All work was done in this session and the existing worktree. No agents, new worktrees, push or deployment. The MCP integration tests create a fresh app per case because the SDK session manager permits only one lifespan run per instance. No implementation deviations. Codesight was checked; its existing summaries contain none of the changed MCP/voyage/admission relationships, so no generated refresh was made.
