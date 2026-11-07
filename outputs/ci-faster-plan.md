# Rust-only backend and faster CI implementation plan

## Goal and authority
Remove the Python API and comparison stack from baseline `61bf722dc` on `ci/faster-pipelines`. Commit locally; never push or access hosts. The PM assigned this whole lane and authorized replacement of its earlier partial CI edits. Those edits are saved in ignored `work/ci-faster/`.

## Compatibility boundaries
- Keep `requirements.txt` and `constraints.txt` byte-identical, and keep the installed M helper's dependency-change check unchanged.
- Keep `deploy/scripts/pre-m-bundle.sh` unchanged: rollback restores an earlier release containing Python.
- Preserve any other kept-tooling dependency whose removal could change or block the next deploy. List exact paths and reasons in the final report.
- Preserve native release inputs, package digests, signatures, independent Rust assertions, host guards, monitors and tooling tests.

## Review
- Architecture: PASS. Rust is the sole application backend; Python remains only for host tooling and explicit release compatibility.
- Maintainability: PASS. Remove comparison infrastructure rather than retain an unused second stack.
- Coverage: WARN. Retain fixed expected Rust assertions and move tooling tests before deleting old directories.
- Performance: WARN. The cited PostgreSQL job timed only run_postgres.sh (23m14s); later runners were skipped. A four-cell runner matrix is provisional, not measured-balanced.

## Execution checkpoints
1. Inventory and preserve tooling closure; move its tests to `tests/tooling`. Keep monitors. Remove Python service unit files and update inventory.
2. Remove app, migrations, backend-only scripts, tests and generated FastAPI assets. Keep dependency manifests for the next release. Replace obsolete primary documentation with Rust instructions.
3. Remove Python-reference Rust tests and adapt PostgreSQL fixtures to the sqlx baseline. Keep fixed-value checks and independent Rust tests.
4. Replace backend CI with reporting jobs named `test` and `postgres-analytics`. Add a small tooling job, four PostgreSQL cells, stable pinned caches and a parallel release producer. Publication waits for every remaining gate and signs identical subjects.
5. Run fmt, both clippy variants, whole workspace tests, all remaining PostgreSQL runners, tooling tests, ShellCheck and actionlint (or YAML parse/review). Record exact exits and failed names/first errors.
6. Review changes, scan live imports and prohibited public terms, verify compatibility bytes, record counts and host actions. Commit small coherent changes. Change shared Phase 10 JSON only in the final commit alone; register new paths there.

## Resume point
Step 1–4 in progress. Three isolated workers own Rust harnesses, kept tooling/tests, and workflows respectively. Main lane owns removal and final integration/gates. Shared Cargo commands use only the existing target and lock from the common rules. No tests or host changes have yet been performed by this lane.
