# Meaning Oracle Retry Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fail meaning gates separately as `ORACLE_UNAVAILABLE` after bounded retries of proven transient Python-oracle failures.

**Architecture:** Keep comparison and reviewed rules unchanged. Designate Python recording legs explicitly in `simulate.py`; `record.py` retries only those calls and preserves typed timeout/reset evidence. `run_disposable.py` owns bounded Python process startup restarts and their evidence. Reports retain all retry events, including failures before capture.

**Tech Stack:** Python, urllib, pytest, disposable PostGIS, existing remote Linux test hosts.

## Review

- Architecture: PASS — retry capture, not comparison; no Rust retry or whole-group replay.
- Quality: PASS — distinguish transport timeout/reset from ordinary HTTP errors; terminate failed oracle children before restart.
- Tests: PASS — startup success/exhaustion, narrow busy detection, timeout/reset, Rust/nontransient controls, first complete answer, complete report evidence.
- Performance: PASS — three capture retries and two process restarts maximum. Default Python startup wait is 180 seconds per attempt, configurable with a positive finite setting. Honor valid Retry-After delay/date; malformed values do not authorize retry.

## Task 1: Regression tests (before implementation)

Create `tests/rust_contract/test_oracle_retry.py`. Exercise the existing Simulation and Runner interfaces with fake process/transport boundaries. Assert normal comparison after busy then success; timeout/reset retries; no retry of Rust or nontransient Python statuses; exhaustion yields ORACLE_UNAVAILABLE and zero meaning differences; startup restart cleanup and reporting. Use remote `venv/bin/python -m pytest tests/rust_contract/test_oracle_retry.py -q`; verify RED assertions, not collection/import errors. Do not run tests/builds on the Mac.

## Task 2: Small implementation

- `scripts/rust_contract/record.py`: typed capture failure classification, strict busy response detection, opt-in bounded retries and structured event callback/list. Preserve complete error response bytes and Retry-After without changing comparator.
- `scripts/rust_contract/run_disposable.py`: configurable 180-second Python startup timeout, at most two restarts; retain each event and raise a distinct unavailable error on exhaustion. No retry of Rust startup.
- `scripts/rust_contract/simulate.py`: Python-only retry opt-in, startup failure handling inside cleanup, group and aggregate reports with retry/unavailable evidence. Preserve sandwich and first complete answer comparison. Fail CLI when gate fails.
- Re-pin only harness attestation digests using the PR #122 pattern if covered. Never change comparator, normalization, or rules digests.

## Task 3: Remote verification and publication

1. Transfer exact candidate bytes to a private task path on the preferred production test host, not the live checkout. Start each test call detached; wait for `CALL_EXIT=` within this turn.
2. Run new unit tests, then `tests/rust_contract`; record exact command/result and any existing skips.
3. On staging, wait for 1-minute load below 8, up to two hours. Start one disposable meaning-gate call only if admitted. Reuse existing build artifacts; no Rust source change/build on this branch. Skip and document if load remains high.
4. Save final evidence in `outputs/meaning-oracle-retry.md`. Review diff, run required quality checks, commit, push branch, and open PR to `staging`. Check and report CI.

## Resume point

Initial source investigation complete: `wait_for_port` defaults to 60 seconds; Simulation calls unqualified record three times; record transport errors lose type and HTTP error reads can discard incomplete bodies. Tests are next. Host/digest evidence lookup is running in the `gate-evidence` agent. Resume from the first unchecked gate above; never start a second waiter or duplicate meaning run.
