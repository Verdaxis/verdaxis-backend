# Phase 2 shared route-scenario runner

## Scope

The runner owns one disposable PostGIS container, migrated seed, cloned cells,
application-role credentials, child processes, and a loopback email provider.
No deployed target is accepted. No measurement runs are part of this change.
`harness-smoke`, `http-boundaries`, and the auth component scenarios remain
explicit selections. A group without a registered **real route** scenario fails;
it never substitutes the smoke or the synthetic example.

## Register a lane

1. Put the lane's fixture and schedules in a module under `scripts/rust_contract/`.
2. Return a `Scenario` from that module. Add one import and `register(...)` call
   inside the marked block in `phase2_scenarios.py`.
3. Add the scenario ID, group, kind and action names to
   `tests/rust_contract/phase2/scenarios.json`. This is the CLI inventory, not an
   executable plugin loader. The Python registry owns execution.
4. Set `route_qualification=False` for harness-only examples. Real groups are
   selected with `--phase2-group notifications` (or the lane's group). An explicit
   `--phase2-scenario NAME` selects only that scenario. Do not use component or
   harness results as route acceptance.

A `Scenario` has these fields:

| Field | Contract |
| --- | --- |
| `name`, `group` | Unique CLI ID and route group |
| `setup(connection)` | Seed the scenario template, once, before cloning. This is fixture-owner authority, not request authority. |
| `servers(context)` | Return exactly two `Server(command, environment)` values, Python first. `{port}` is replaced in command arguments. |
| `build` | Optional argv tuple for a test-only Rust server build |
| `actions(context)` | Ordered `Action` objects, including expected status and state/effect assertions |
| `races(context)` | Optional `Race` schedules; required when `--phase2-races` is selected |
| `mixed(context)` | Optional `(writer_index, Action)` sequence on a third database |
| `dispatch(context)` | Run the existing Python producer/dispatcher against `context.database_url` |
| `assert_mixed(state, result)` | Verify committed mixed-writer state and dispatcher evidence |

`context` provides `root`, `python_bin`, `environment`, and the runner's `mail`.
The mixed dispatcher also gets the third cell's application-role `database_url`.
The runner starts both writers on that same third cell before dispatching.
Dispatchers do not run during twin comparisons.

### Adapter contract

The runner overrides `DATABASE_URL` (SQLAlchemy async form),
`PHASE2_DATABASE_URL` (driver form), `PHASE2_BIND`, `ENVIRONMENT=test`,
`RELEASE_SHA`, `DISPOSABLE_TEST_TOKEN`, and `PHASE2_MAIL_URL` for each child.
Both children must expose the existing disposable identity endpoint. Each child
must use only its assigned application-role database, not the owner seed URL.
Never put a live credential in a `Server.environment` dictionary.

Use `python -m scripts.rust_contract.phase2_oracle --port {port}` for the Python
adapter. It starts the unchanged app through the existing test wrapper, replaces
`app.services.email.RESEND_API_URL` with the runner's loopback URL, uses a dummy
provider key, and rejects non-loopback HTTPX sends. This is test-only code; there
is no new application or runtime Config setting. A Rust mail lane must wire its
**test-only** provider client to `PHASE2_MAIL_URL`. The runner cannot add this seam
to Rust Config or route code: that remains the lane's ownership.

Generated IDs and action clocks belong in lane-controlled test adapters. Keep
those values in response and state comparisons. Do not discard business
timestamps to obtain equality. The synthetic writer's `x-database-now` header
is checked against its captured request interval and retained as probe evidence;
this does not exempt a real route header or column.

## Requests, state and no replay

`Action` specifies `name`, comparator `route_id`, HTTP `method`, local absolute
`path`, `expected_status`, optional JSON `body`, and `headers` (including bearer
and supplied request ID when needed).

After **each** action the executor opens fresh database connections, captures
all business tables and columns through Task 4's snapshot function, and compares
both states. HTTP uses only `compare.compare_responses` with the digest-pinned
`phase2-meaning-v1` profile. State comparison is strict: no undeclared timestamp,
Decimal, UUID or foreign-key removal. Whole-user-row guards permit only the
conditional referral-code allocation allowed by R01/R04.

Use `assert_after(before, after, response, mail)` to assert the expected write,
unchanged unrelated tables, and effect count. Pairwise equality alone does not
prove that both implementations made the correct change.

- `outcome="rolled-back"` requires unchanged full business state.
- `outcome="ambiguous"` records a retained commit; add an exact unit/count
  assertion. `allow_transport_error=True` permits a lost response only for a
  scenario that proves its committed state.
- The executor sends every action once. It has no retry loop. An explicit second
  Action is a deliberate source-contract duplicate, not transport replay.
- Redirects and ambient HTTP proxies are disabled. Requests have a 20-second
  timeout and an 8 MiB capture limit.

## Races

Return `Race(actors=(...), invariant=callback, repeats=3, seed=20261018)`.
Actor names must be distinct. A thread barrier releases all N actors for each
cell, with fixed-seed submission order. There are 2–32 actors and 1–20 repeats.
The report records the seed and actor order for each repeat. No sleeps determine
release. Add schedules for both action directions and same-user, same-org and
cross-org cases where the route contract requires them.

The barrier controls client release, **not** a database lock-acquisition order.
A lane that must prove a specific database interleaving must add an explicit
barrier/lock observation seam in its test adapter. Do not claim deterministic
SQL scheduling from simultaneous HTTP alone.

The invariant receives `(before, after, actor_results)` separately for each
runtime. It must assert the organization cap, candidate uniqueness, atomic unit
or other lane rule. Responses remain paired by actor name, not sorted into an
unordered success bucket. State/mail differences fail even if each invariant
passes. Repeats use the prior committed state; seed independent actor IDs or
use rollback schedules if repeated creation must not accumulate.

## Email assertions

`context.mail` is one loopback HTTP provider per run, closed on every exit.
Each writer has its own ledger namespace, including in the mixed cell.
`Action.mail_status` sets the provider response for both twins (default 200).
For malformed provider responses, set `Action.mail_body=b"..."`.
Direct schedules can also call `context.mail.respond(status=..., body=b"...")`.
It accepts POST `<namespace>/emails`, records every attempt including failures,
and never forwards traffic. Bodies are bounded to 1 MiB. The ledger retains
payload, provider status and idempotency key; it does not retain authorization.
Payloads remain in memory. Reports contain only counts/digests, not addresses,
provider bodies, bearer values or HTML.

An action's assertion gets the cumulative cell ledger. Check exact recipient,
sender, reply address, subject, escaped content, link and attempt count. A second
accepted invite must have a second attempt if that is the source behavior.
Do not infer exactly-once delivery. The stub can return 200/201, 202, 4xx/5xx or
malformed bytes; provider timeout injection remains a lane adapter seam.

## Complete example

`scripts/rust_contract/phase2_example.py` is the executable complete example.
It registers `runner-example`, uses the existing Python and Rust synthetic
`/__contract__/phase2/write` probes, and supplies:

- one successful two-child commit;
- one failure between child writes with unchanged committed state;
- one postcommit response-loss classification with no automatic replay;
- three actors released by a barrier, repeated three times with rollback
  invariants;
- both writers on a third database, followed by the real Python
  `MarketEventDispatcher` through the existing `mixed_dispatch` helper;
- a no-email assertion and exact retained-child checks.

Run it only through the authorized staging wrapper:

```bash
verdaxis-staging-test "$PWD" '/home/verdaxis-test/bin/heavy run 3600 -- bash -lc "venv/bin/python -m scripts.rust_contract.run_disposable --phase2 --phase2-scenario runner-example --phase2-races --report work/rust-phase2/runner-example.json"'
```

This example deliberately reports `scope=harness-only` and
`route_qualification=false`. It does not qualify any CRUD route or candidate.

## Verification record

All focused checks ran on staging through `heavy run 3600`. Every call finished.
No local build/test, measurement, deployment or live database action ran.

| Gate | Own exit | Result |
| --- | ---: | --- |
| New shared-runner RED | 1 | Seven expected missing-runner failures |
| Initial focused GREEN | 0 | 20 passed |
| Disposable example attempt 1 | 1 | Probe database-clock header differed |
| Disposable example attempt 2 | 1 | Comparator correctly refused extra evidence key in raw HTTP envelope |
| Disposable example attempt 3 | 0 | Twins, rollback, no replay, repeated barriers and mixed dispatcher passed |
| Regression unit invocation | 4 | Wrong comparator test filename; no tests collected |
| Existing harness smoke | 0 | Live twin writers and mixed dispatcher passed |
| Existing HTTP boundaries | 0 | 18 Python tests, 16 Rust boundary tests, 7 Rust HTTP tests |
| Corrected focused suite | 0 | 86 passed; 11 existing Pydantic warnings |
| Final focused suite | 0 | 86 passed; 11 existing Pydantic warnings |
| Final disposable example | 0 | Full example with races and mixed dispatcher passed |

Temporary command output is under `work/runner-*.log` and is not committed.
The existing CI workflows do not select the new Phase 2 Python tests or example;
the focused host gates above cover them. Workflow changes are outside this
lane's ownership. The CI-only draft PR runs the existing full jobs, not a
Phase 2 candidate-qualification gate.

Remaining lane-owned seams: real route registrations, deterministic generated
values in each route adapter, Rust test-only provider wiring, and database-level
interleaving observations for races that need them. The generic runner does not
claim these route-specific gates are complete. Structural refresh was checked
with `refresh-codesight --dry-run`; full regeneration was not run because its
bootstrap can update managed instruction files outside this lane's ownership.
