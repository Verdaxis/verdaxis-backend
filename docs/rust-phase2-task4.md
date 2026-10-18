# Phase 2 Task 4 — twin-database preparation

## Status and scope

The existing Runner now has an explicit Phase 2 CLI path. Its harness-only smoke
uses the pinned PostGIS image, existing ephemeral allocator, migration verifier,
and existing finally/cleanup owner. It clones a migrated synthetic seed into two
separate databases, converges and validates the canonical ACL policy, compares
whole logical business state, and executes synthetic writes as a non-owner,
non-superuser application role. Admin connections are confined to schema setup
and committed-state inspection.

**This is partial Task 4 support, not full write differential acceptance.** Rust
CRUD routes and authenticated transaction ownership are not built. The smoke
therefore executes the same synthetic SQL transaction schedule in both database
cells; it does not claim two live application stacks or route equivalence.
Unknown scenarios and unimplemented measurement/benchmark/race modes refuse.
The report says `scope=harness-only` and `route_qualification=false`.

No Task 6 runtime changes were made. No database migrations, grants or roles
were added to deployed policy. Disposable fixture roles exist only inside the
runner-owned container, using the existing bootstrap/convergence scripts.

## Checks provided

- Same database identity, unequal seeds, absent ACL evidence, missing token and
  non-loopback target refuse before a differential action.
- Whole table/column/row membership is compared. Primary keys are retained;
  extra notifications differ even with equal response bodies.
- Whole-user-row guard allows only missing actor referral_code allocation on
  R01 or R04 with a new recipient. Other user columns and foreign users differ.
- Precommit fault rolls back the first child; no partial child remains.
- Postcommit response-loss simulation retains the complete two-child unit and
  is not replayed. Real smoke checks exact child counts after each schedule.
- Fresh connections capture committed state after each action. Dispatchers are
  not started. No email provider is contacted.
- Test-only ActionValues exposes an aware action clock and finite UUID/referral
  candidate sequences, refuses outside test, and fails when sequences exhaust.
- Cleanup now checks deletion of its own container and fails if it remains.

## Remaining Task 4 work

Do not call Task 4 complete until the application transport adapter starts each
stack against only its own database, attests both listeners, injects deterministic
action values into both, compares actual responses plus full state/effects, and
runs mixed-writer/dispatcher tests on a third database. Sequence high-water marks
and test-only database now() control also remain. The generic state comparator
is currently strict JSON/type comparison; schema-declared database Decimal/time
normalization must be tied to actual schema exports before route acceptance.
These gaps cannot be hidden by the green harness smoke or supplied to Task 3 as
complete candidate evidence.

The auth-token-matrix CLI delegates to Task 5's existing HTTP oracle check and
Rust HTTP vector test. It starts no database and makes no lock/state claims.

## Gates

All builds/tests used `heavy run 3600`; all calls finished without interruption.

| Gate | Exit | Result |
| --- | ---: | --- |
| Task 4 RED | 1 | 10 missing-module failures; existing unknown-option refusal passed |
| First unit GREEN | 0 | 18 passed |
| First smoke | 1 | Undeclared psycopg import; replaced by declared asyncpg |
| Second smoke | 1 | Incorrect organization fixture column |
| Third smoke | 1 | Incorrect user fixture column |
| Fourth smoke | 1 | Missing identifier import |
| Fifth smoke | 0 | Twin schema, ACL and app-role writes pass |
| Expanded final unit/clock/runner suite | 0 | 24 passed; five existing Pydantic warnings |
| Expanded final real PostgreSQL smoke | 0 | Child atomicity and non-replay counts pass |
| Rust format | 0 | Passed |
| Rust all-target workspace Clippy | 0 | Passed |
| Cleanup regression suite | 0 | 19 passed |
| Cleanup real PostgreSQL smoke | 0 | Container deletion verified |

Logs: `work/rust-phase2/task4-{red,green,smoke2,smoke3,smoke4,smoke5,final,cleanup}.log`.
Task 3 CI at `7181a471`: test, both PostgreSQL jobs, offline and contract passed;
publish skipped; watch exit 0.

## Resume

PM must review the partial Task 4 boundary. Keep Task 6 blocked until the PM
hands off the final Phase 1 runtime. Preserve the existing lifecycle owner when
adding actual route adapters; do not create a second container/server runner.
