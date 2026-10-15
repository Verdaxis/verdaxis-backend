# Task 7b — Blocked on legacy list ordering

Date: 2026-10-16. Base: `77842676`, branch `feat/rust-phase-1-lane-7b`.
Scope: R16–R18 and R26 only. No implementation acceptance is claimed.

## Required stop: unordered ties

The task brief requires a stop and PM decision if Python leaves a list tie
unordered. Source inspection confirms this condition inside Task 7b:

- R18 `/api/orderbook/with-ci` orders only by `created_at DESC`
  (`app/routers/orderbook.py:1066-1070`).
- R26 `/api/orderbook` orders only by `created_at DESC`, then applies the
  effective offset and limit (`app/routers/orderbook.py:1790-1794`).
- Corpus construction sets every order's `created_at` and `updated_at` to the
  fixed clock (`scripts/rust_contract/seed_corpus.py:261-265`).
- R16/R17 do have complete tie-breakers: price sorts use acceptance ordinal
  and ID; quantity uses created time and ID; newest uses created time and ID
  (`app/routers/orderbook.py:246-266`). These routes are not the blocker.

A local, read-only Python inspection of the accepted golden response bodies at
oracle `a127672ee4c82fafbb4c633eb84f66bbdc676e75` confirmed:

| Case | KYC enabled rows | KYC disabled rows | Created time of every row |
| --- | ---: | ---: | --- |
| R18-with-ci | 17 | 21 | 2026-10-15T12:00:00Z |
| R26-orderbook | 17 | 21 | 2026-10-15T12:00:00Z |
| R26b-orderbook-page | 2 | 2 | 2026-10-15T12:00:00Z |

These are six inspected responses, not six executed contract tests. PostgreSQL
has no guaranteed order among these equal keys. A different query plan can
change both array order and which tied rows fall on a page. A comparison rule
that only ignores order within a returned page does not resolve page membership.
Decisions 18 and 22 apply only to news and price summaries, not these routes.

## Decision needed

The PM must define R18/R26 tie ordering and pagination acceptance before this
lane proceeds. Do not add a Rust-only ID tie-breaker, change Python, remove ties
from the corpus, or widen comparator exceptions without that decision.
An approved deterministic Python tie-breaker would require coordinated oracle
and golden updates; an alternative comparison rule must address page membership,
not only permutations within the returned array.

## Checks and changes

- Source and golden inspection found the required stop condition before Step 1.
- No failing test, route implementation, SQL, fixture, golden or comparator edit.
- No new shared files. Only this task progress file was added.
- No Cargo, PostgreSQL, disposable HTTP, Python contract or Python unit suites
  ran: implementation stopped at the explicit PM gate. No passing gate is claimed.
- No Docker container, live host, live database, skill or subagent was used.
- No push. Documentation-only commit; `git diff --check` checked before commit.

## Resume point

Read the PM's ordering decision and any resulting oracle update first. Then
finish the Task 7a code review and start Task 7b Step 1 (failing test first).
Preserve Task 7a's separate public response projection, shared visibility rules,
one captured instant, Singapore date, batched benchmarks and explicit recorded
database time zone. Run all requested gates after implementation. This stop
neither completes Task 7b nor authorizes work on another task.
