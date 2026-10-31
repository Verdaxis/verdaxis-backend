# Rust Phase 2 Task 9 — preferences

## Scope

P01 GET and P02 PUT are implemented in `rs/read/src/crud/preferences.rs`.
The marked route export is an integration seam, not deployed activation.
No Python application, migration, runtime setting, dependency, comparator,
workflow or deploy file changed.

The existing disposable runner now dispatches `--phase2-group preferences`
to the preferences scenario, never harness smoke. The scenario uses the existing
container/process lifecycle and independently ACL-converged twin databases.
The full Python application remains the HTTP oracle. Disposable adapters control
preference UUIDs and UTC timestamps; release composition does not accept those
headers. The scenario compares HTTP through `phase2-meaning-v1`, and all logical
business tables through fresh connections after each action. Redirects have
explicit empty-body/Location assertions because the JSON comparator rejects them.

## Checks

All developer builds/tests ran on staging through `heavy run 3600`. No project
code or test ran on the Mac. No measurements ran.

| Gate | Exit | Evidence |
| --- | --- | --- |
| Initial build attempt (missing RELEASE_SHA; not functional RED) | 101 | work/preferences-red.log |
| Missing module RED | 101 | work/preferences-red2.log |
| Executable validator RED (two failing assertions) | 101 | work/preferences-red3.log |
| Missing preferences server scenario RED | 1 | work/preferences-red3.log |
| Final focused `crud_preferences` (2 tests) | 0 | work/preferences-final4.log |
| Final Rust formatting check | 0 | work/preferences-final4.log |
| Final workspace/all-target strict Clippy | 0 | work/preferences-final4.log |
| Final preferences differential (54 actions) | 0 | work/preferences-final4.log; work/rust-phase2/preferences.json |

The 54 actions cover all three namespaces, strict field validation, unknown
products, extra/missing fields, non-object and missing bodies, 8192/8193-byte
ASCII boundaries, multibyte UTF-8 size, raw whitespace, unknown namespace before
size, repeated updates, timestamp advancement, user isolation, unknown stored
namespace and concurrent same-user PUTs. Concurrency holds the user row and
observes both requests waiting in PostgreSQL before releasing it. Both calls
succeed and the final twin state matches. All unrelated tables remain unchanged
through the sequential differential actions. A known SQLAlchemy warning about
rfq_quotes/rfqs cyclic sorting appeared during disposable migrations.

## Review and integration boundaries

Author self-review only; no subagents used, per the brief. The shared
transaction owner supplies current-account checks, exclusive mutation locks,
commit/cancellation ownership and no automatic retry. Composition must supply
verification settings and shared quota authority. Test controls are feature-gated
at construction and require environment=test.

This is task-local correctness evidence, not a complete artifact or cutover GO.
The current JSON parser still owns broader diagnostic equivalence outside the
recorded malformed-object vectors; no comparator exception was introduced here.
Canonical-size evidence covers schema-valid strings/booleans; malformed numeric
values near the size threshold need broader Python floating-point encoding
vectors before claiming exhaustive arbitrary-JSON size equivalence.
Full suites are delegated to the authorized draft PR's GitHub Actions.

## Shared registry follow-up (19 October)

Merged `origin/feat/rust-p2-runner` through `808f4fa8` in `52c27aac`.
Removed the Task 9 dispatch from `run_disposable.py`. The only preferences
registration is the marked block in `phase2_scenarios.py`; the lane module now
returns a `Scenario` and owns no database cloning, server lifecycle, HTTP
transport, comparator dispatch or report writer.

The shared runner compares all business tables after every action and checks
whole users rows and the mail ledger. Lane assertions additionally require the
exact stored value, one user/namespace row, controlled update timestamp,
unchanged other-user rows and no mail. A fixture-owned trigger injects a real
precommit SQL failure (500 with full-state rollback). A test-only response-loss
adapter replaces a completed 200 with 503 after commit; the retained exact row
and a subsequent read prove no replay. This is controlled response loss, not a
claim to have injected a PostgreSQL commit-acknowledgement failure.

The registered race uses seed 20261019 and three repeats. Two named callers
write identical values to the same existing user/namespace row. Both must return
200, preserve the row count and all unrelated tables, and produce equal twin
state. Client release order is not claimed as SQL lock order. The accepted
component evidence separately observed both requests waiting on the user row.
Task 9 does not require a mixed-writer dispatcher cell (preferences emits no
events) or invite delivery. The shared mail ledgers are asserted empty.

Raw syntax, whitespace and redirect component cases remain in the accepted
54-action evidence; shared Action serializes JSON and does not accept raw bytes.
Malformed JSON wording is the approved Task 7 difference, not a Task 9 blocker.

### Recorded remaining numeric-size gap

The gap is explicitly retained, not silently treated as passing: serde_json
preserves numeric token spelling while Python parses a float and re-encodes it.
For example, a disallowed extra field holding `1e0` is three bytes in Rust but
four bytes (`1.0`) after Python encoding. With padding at the canonical limit,
Rust can return schema 422 while Python returns size 413. This cannot persist a
malformed payload, but it is response-contract drift. No comparator relaxation
or Python fix was made. PM requested that this gap be closed **or recorded**;
this record chooses the latter. Valid preference schemas contain no numeric
leaves, so the validated-persistence size boundary remains covered.

### Follow-up gates

- Registry RED: own exit 1, missing preferences registry entry.
- Registry/scenario unit GREEN: own exit 0.
- Shared preferences group with races: own exit 0.
- Final formatting, focused Rust tests, workspace Clippy, registry tests and
  preferences group each returned own exit 0 (`work/preferences-registry-final.log`).
- Shared evidence contains 51 sequential actions and 3 race repeats;
  `work/rust-phase2/preferences-registry.json`. CI is PR #106.

No measurements, deployments, new dependencies, runtime settings or comparator
changes were made. All verification uses the staging `heavy run` wrapper.
