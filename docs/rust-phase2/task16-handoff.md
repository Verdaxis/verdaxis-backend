# Task 16: organization price-alert CRUD

## Status and scope

Component implementation is complete at this checkpoint. Full route acceptance is **pending the runner lane**, not GO. The PM's 19 October ownership update permits the marked additive module and route-export blocks. No runtime configuration, manifests, dependencies, comparator, CI workflow, Python application, or migration was changed.

The component router is `routes::alerts_router(app, verification, limits)`. The integration lane must supply its existing verification settings and shared quota authority. Runtime secret/environment projection and activation are not implemented here.

## Implementation

- A01 creates a separate organization-owned row per accepted request, counts only active rows, and preserves the five-active cap.
- A02 returns all organization rows, including inactive rows, in created-time descending order.
- A03 distinguishes missing-row 404 from foreign-organization 403, permits another member of the same organization, and returns empty 204 on success.
- Request validation replays 65 Python vectors, including direction case, UUID types, non-object bodies, threshold positivity and nonfinite values. Independent errors retain schema order.
- Threshold text is bound to PostgreSQL NUMERIC without binary floating-point or rust_decimal narrowing. Storage checks cover half-cent rounding, positive-to-zero rounding, overflow and both foreign keys.
- Product metadata includes inactive/legacy rows. Market product derives from the attested exact label tuple, not the database row UUID or activity flag.
- The shared authenticated transaction owns current-user admission, row locks and commit. No replay or organization-only lock was added. Python retains triggering and event production.

## Evidence

All builds, formatting, lints and tests ran through `verdaxis-staging-test` and `heavy run 3600`. No measurements were run. Local temporary logs are under `work/rust-phase2/` and are not committed.

| Gate | Exit | Evidence |
| --- | --- | --- |
| Initial RED attempt | 101 | `alerts-red.log`: missing required RELEASE_SHA; not a valid feature RED |
| Corrected RED | 101 | `alerts-red2.log`: missing `verdaxis_read::crud` module |
| Intermediate implementation checks | nonzero | `alerts-green1` through `green4`: syntax/compile and exact UUID diagnostic differences; fixed, not concealed |
| Python oracle freshness | 0 | `alerts-green4.log`: 65 vectors, 48 rejected |
| Focused Rust tests | 0 | `alerts-green7.log`: four offline tests plus conditional PostgreSQL driver entry; the standalone run does not qualify DB coverage |
| PostgreSQL storage + HTTP admission + Python trigger | 0 | `alerts-green7.log`: one pytest driver passed, no skips; it runs the Rust PostgreSQL test with app-role credentials on the existing disposable migrated fixture |
| Focused Clippy, warnings denied | 0 | `alerts-green7.log` |
| Owned-file formatting | 0 | `alerts-green5.log`; subsequent owned-file formatting also completed |

The PostgreSQL driver proves no-org refusal on all three routes, foreign delete refusal, same-org member list access, path validation, cap/inactive behavior, rounding, FK/overflow failures, repeated create/delete, and Python trigger compatibility. The trigger event payload, participant organization and deactivation state are checked. The only pytest warning was an existing Pydantic class-config deprecation.

## Pending runner lane

`tests/rust_contract/phase2/alerts-scenarios.json` is declarative coverage data, not a runnable or passing contract. Register it only after the shared runner interface lands. Required pending gates:

1. Paired HTTP responses and complete committed-state comparison after every action.
2. Controlled IDs and action clocks, full users/unrelated-table preservation, fault and ambiguous-commit cases.
3. Deterministic different-user/same-organization cap races in both schedule directions.
4. Mixed-writer dispatcher/outbox cell. The direct Python-trigger component check does not qualify dispatcher integration.

The source cap race has not been proven by a paired deterministic schedule. Do not silently repair it or claim equivalence. Section 0 of the plan overrides later Python-first repair instructions; a new approved Rust difference requires PM review.

## Resume point

When `feat/rust-p2-runner` lands on the integration branch, read `docs/rust-phase2-runner.md`, add only the marked Task 16 scenario registration, and run the alerts differential/race and mixed-writer gates. Preserve the current implementation and vectors. Do not qualify a harness-smoke run as alert coverage.

Codesight regeneration is deferred: the machine-local helper runs project analysis, which this brief forbids on the Mac, and it is not provisioned as part of the authorized test-host path. ARCHITECTURE.md was updated directly.
