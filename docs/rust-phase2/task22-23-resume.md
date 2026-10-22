# Tasks 22 and 23 — checkpoint, 22 October 2026

## Status

Tasks 22 and 23 are **not complete**. No staging or production deployment was
attempted. Task 24 has not started. No measurement was run (decision 110).
All calls started during this continuation have finished.

## Committed steps

- `2be99321`: exact Caddy ownership for 25 operations in six groups, independent
  rollback, Python fallback, pre-auth ingress, and real-Caddy tests.
- `0fe0fb02`: compose the Phase 2 routers, shared quota authority, and private
  runtime configuration. Existing harness state constructors carry absent
  settings. W09/W10 stay on Python.
- `dda3030a`: merge `origin/staging` at `274bd3ab`, without conflict. This takes
  PR #115's certifi change; this lane did not edit the constraint.
- Earlier `108480a4` merged PR #111. Its only conflict was ARCHITECTURE.md;
  both appended sections were retained in the order used by `a49c5963`.
- Earlier `293af2c3` records PD6-2 in the plan record and producer design:
  producer implementation/activation in Phase 8, no Python-row shadow evidence,
  and the unchanged PD4-G guard in Phase 2.

No gate/helper script, `work/`, or `.claude/` file was staged.

## Evidence from this continuation

Logs are local, ignored files under `work/`.

| Check | Result | Log |
| --- | --- | --- |
| fmt | 0 | `t22-static-resume-1.log` |
| clippy, warnings denied | 0 | same |
| offline Rust workspace | 0 | same |
| initial Caddy tests | 1: 39 failed, 1173 passed | `t22-routing-resume-1.log` |
| Rust quota checks | 0 | same |
| focused ingress RED | 1: 5 failed, 6 passed | `t22-focused-red.log` |
| focused ingress GREEN | 0: 11 passed | `t22-focused-green.log` |
| routing + disposable release fixture, first pass | 1: 1 failed, 1355 passed | `t22-t23-focused-1.log` |
| routing + disposable release fixture, corrected | 0: 1356 passed | `t22-t23-focused-2.log` |
| Rust workspace PostgreSQL | 0 | `t22-postgres-resume-1.log` |
| Phase 1 meaning, after staging merge | **124: hard deadline** | `t22-meaning-resume-1.log` |

The PostgreSQL suite executes `phase2_routes` (all 25 mounted operations),
transaction/race tests, runtime configuration, and read-pool checks. Its cleanup
printed a Podman journal cursor diagnostic; the suite itself returned 0.

The initial Caddy failures exposed a missing PATCH stub handler, a shadowed
`proxies` helper in a test, and missing passive health settings on the pre-auth
proxy. New failing-first cases showed that non-preflight HEAD/OPTIONS/unknown
admin methods were not charged and an external quota mark was forwarded. The
corrected snippet strips external marks, excludes only actual CORS preflights,
and applies the existing refused-connection fallback pattern. The final focused
run also proves exact production rollback.

The prior static logs `t22-static-1.log` and `t22-static-2.log` have no
`CALL_EXIT` marker. Remote inspection confirmed their unit inactive. They are
not acceptance evidence.

## Blocked gate

The unchanged Phase 1 meaning command ran through the wrapper and `heavy run
3600`. It hit the hard limit and returned `CALL_EXIT=124`, with no final
`MEANING_EXIT` or complete report. The last active component was the legacy
curve matrix in `rs/read/tests/legacy_curve_oracle.py`. The log shows ongoing
requests, not a demonstrated deadlock. Other lanes were using the shared host.
No causal diagnosis has been established. Do not call this green, increase the
time limit, or remove checks. Read the full log and inspect the runner's per-group
work before choosing a bounded correction. Remote process inspection after the
call found no remaining process from this lane.

## CI and remaining work

PR #88 checks were read at the previous pushed SHA `c078f1e7`: Rust offline and
contract passed; backend `postgres-analytics` failed on unreviewed private tables;
backend `test` failed pip-audit for certifi PYSEC-2026-4320. The local
`28b2c80b` addresses the table exclusions; merged PR #115 addresses certifi.
These successor commits have **not yet been pushed** in this continuation.
The prior failed CI log is `work/rust-phase2/ci-handoff-failures.log`.

Resume in this order:

1. Resolve the bounded Phase 1 gate failure. Do not overlap host calls.
2. Review the uncommitted Task 23 projection against the raw-value contract:
   the Rust parser currently trims verification settings; verify that secrets
   are not silently changed. Obtain failing-first evidence for the preparer,
   which was already drafted at handoff; current success is not RED evidence.
3. Finish Task 23 runtime preparation and disposable transaction qualification;
   commit the named paths only. The focused release fixture is green but is not
   proof of the whole task.
4. Finish CI wiring (currently uncommitted), run remaining Phase 2 scenario
   gates without a byte/twin-database acceptance requirement, Python unit and
   PostgreSQL gates, and the full merge regression checklist.
5. Push only `feat/rust-phase-2` at a green point, including `28b2c80b`, then read
   and wait for PR #88 CI. Never push staging/prod or change a PR.
6. Refresh Codesight using supported tooling, subject to the no-local-build
   rule; update this checkpoint with final evidence. Stop before Task 24.

The automated commit security-review hook failed with HTTP 404 for `0fe0fb02`.
Details: `~/.claude/security/log.txt`. This is not a completed security review.

## Plan corrections retained

The old plan says 27 operations; the active cutover has 25 because W09/W10 stay
on Python. Old direct-flock, measurement, byte-identical and twin-database gate
commands are superseded by the PM brief. Use the staging wrapper and `heavy`
helper for correctness, no local tests or builds, and no measurements.
