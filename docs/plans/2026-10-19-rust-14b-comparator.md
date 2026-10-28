# Rust 14b Comparator Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Compare staging response content safely without changing Python or public routing.

**Architecture:** `compare.py` remains the one equality authority. `shadow.py` owns serial bounded transport, target admission, pacing, safety stops and sanitized aggregate evidence. Reviewed private cases replace disposable fixture defaults; no live run is authorized here.

**Tech Stack:** Python standard library HTTP client, pytest, existing Rust contract byte comparator.

## Review

- Architecture PASS: reuse capture and exact-token exceptions, no application coupling.
- Quality PASS: no redirects, proxies, credentials or exception text persisted; fixed metadata schema.
- Tests PASS: fake transport/clock for scheduler; local disposable sockets on remote test host for hard body/deadline checks.
- Bounds PASS: one call at a time, rolling 3/10 s plus endpoint minute buckets, hard deadline, 1 MiB response and 10 MiB evidence bounds.
- WARN: short coverage floor and live nonempty cases require PM approval. Configuration must state the approved floor explicitly; no implicit reduction of 1,000.

## Steps and resume points

1. Add failing byte-profile/comparator tests in `tests/rust_contract/test_shadow.py` and `test_compare.py`: equal-length mismatch, Python change, exact CSV/Decimal/null/order, decision 85 ties and generation clock exceptions. Run through the staging test wrapper. Add `compare.compare_responses` and make shadow use it.
2. Add failing staging admission, limits, privacy and run-loop tests. Implement exact staging hostname, numeric loopback targets, separate readiness SHA identities, private reviewed cases, safe MCP Host/Accept, no fixture endpoints. Validate config before network I/O.
3. Enforce global rolling pacing including readiness, both Python minute calls, 60-second backoff, pressure/deploy stops, absolute deadline and response cap. Stop on first unexplained difference. Data changes never count as matches. Return nonzero for missing coverage/unapproved floor/errors; retain all attempts in the usable ratio.
4. Verify all comparator/shadow tests and adjacent contract meta-tests on the staging test host; record each command's exit in `docs/rust-phase1/readiness-task14b.md`. No measurements, app/alembic edits, deploy helper edits or workflow edits.
5. Review diff, update `ARCHITECTURE.md` and staging config/CLI documentation, commit with the requested trailer. Leave work and `.claude` uncommitted. Push/PR awaits clarification of the contradictory no-push instruction.

## Test command

From this worktree only:

```bash
verdaxis-staging-test "$PWD" '/home/verdaxis-test/bin/heavy run 3600 -- bash -lc "venv/bin/python -m pytest tests/rust_contract/test_shadow.py tests/rust_contract/test_compare.py -q"'
```

Capture to `work/` with `CALL_EXIT`, wait until complete, and inspect the result before changing source or ending the turn. Expected red is an assertion failure; collection/configuration errors must be corrected and rerun. Final expected exit is 0.
