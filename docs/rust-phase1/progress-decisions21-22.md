# Decisions 21–22 — 2026-10-16

- `f25a9a1` records both decisions and corrects Task 2's false server-default inheritance claim. No runtime policy was changed.
- `3595445` adds the exact-item multiset exception for `/api/prices` at `/body/summaries`, configured in `tests/rust_contract/price-summary-paths.json` next to the separate news rule. Python has no final summary ordering key; reference prices and tape retain their ordering contracts.
- `aec25e1` pins configuration to that one route/path and tests recorder reuse and unrelated-array strictness.

TDD: `test_reordered_summary_passes` failed before implementation. Afterward all 15 new tests pass: reordered summaries, changed/missing/extra items, byte-level spelling, surrounding bytes, other routes/arrays, widened configuration and recorder stability. Exact item tokens are sliced from the original UTF-8 response; a Counter retains duplicate multiplicity. All bytes outside item spans are compared separately. No JSON reserialization or scalar normalization weakens item equality. The same recording comparison is used for oracle stability and Python/Rust replay.

The previously failing combined price probe is restored to unfiltered `/api/prices` and `hours=168`, while its arithmetic assertion still identifies its own `2031-01` slice. The original 38 ordering paths now pass because only the order of exact summary tokens is relaxed, not values or surrounding output. Both 17-case probe passes (before and after mutable catalog changes) pass, as do all 40 normal price cases. Simple reads retains its distinct news tie-key rule and passes all 29 cases.

Required final checks: Python contract suite **95 passed**, five existing warnings; corpus prices **40 cases plus 34 probe comparisons passed**, one Rust target; corpus simple_reads **29 cases passed**, one Rust target. Logs: `work/rust-phase1/price-order-{red,python-final,prices,simple}.txt`.

No route changes, push, staging/production access or new golden recording. Docker context was Colima and the existing runner used its pinned disposable PostGIS image. Existing golden response bytes remain intact; future captures add exact summary-token evidence to their envelope.
