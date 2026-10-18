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
