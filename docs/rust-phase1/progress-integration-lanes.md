# Merged lane harness integration — 2026-10-16

Base: `4eedbb8` (Tasks 6a, 10a and 9 merged). No route, comparison contract, golden data or live environment changes.

## Root causes

1. **Prices:** the PM log's invalid-window ValueError is the expected error-contract case; all 40 case comparisons pass. The actual target failure is `prices_oracle.py` asserting exactly three SPOT trades after corpus already supplied trades in that same slice. The probe now inserts its boundary/arithmetic prints in `2031-01` and requests that exact window for its arithmetic checks. It keeps all original value/cutoff/privacy assertions and compares the general 40-case corpus before inserting lane fixtures. An intermediate probe used the isolated window but unfiltered summary requests; exact comparison exposed summary ordering differences across multiple windows. The final arithmetic requests select their own slice, without weakening general corpus comparison.
2. **Legacy curves:** the lane called the Task 6 matrix seeder after corpus had already inserted the matrix users. It now checks for the existing matrix and asserts each required source row before reuse. Ordinary-mode runs still construct the matrix when absent. Additional curve rows retain their separate `curve:` IDs.
3. **Market matrix:** its Rust `Config` retained UTC while the matrix runner explicitly sets the database timezone to Asia/Singapore. Task 9's new startup check correctly refused that mismatch. The matrix config now uses Asia/Singapore. The generic runtime/admission test database remains UTC; its dedicated timezone test explicitly exercises both zones. The disposable HTTP runner already supplies `ORACLE_TIMEZONE` to its database and Rust config.

The merged plan file does not contain numbered decision 21, despite the handoff referring to it. The required setting, both-pool after_connect implementation and pg_db_role_setting check are present in code and documented in `progress-task9.md:146-160`. No new policy decision was invented here.

## Commits

- `17304ec`: isolate price probe slice; reuse existing corpus matrix for legacy curves.
- `fbbc215`: match market matrix's recorded Singapore zone.
- `52f97ab`: compare groups with no Rust test target through the same authoritative HTTP comparator. Empty route modules now produce real 404 differences instead of a missing Cargo-target failure before requests.

Targeted checks passed: prices 40 cases plus 17 probes twice (before/after mutable catalog changes); curves 4 cases plus 60 matrix and 8 scale comparisons; market matrix two tests, 14/46 orders and 4/6 trades. These fixes use fixture ownership/scope, not relaxed comparisons.

## Remaining observed contract issue

The intermediate unfiltered price probe over the combined corpus plus its new `2031-01` slice found 38 difference paths: summary entries for two windows appeared in opposite order. Python's summary aggregate has no final ORDER BY. This is **not** covered by the news-only tie exception and was not normalized away. The arithmetic probe now intentionally filters its private slice so its fixed expectations remain meaningful; the normal 40-case corpus still compares without this probe's added rows. Task 9/PM should decide the multi-window summary ordering contract before claiming general unfiltered parity. Evidence: `work/rust-phase1/integration-prices.txt:352`. No price route code changed.

## Full verification at `52f97ab`

- fmt and clippy passed; workspace **101 passed**.
- PostgreSQL runner **109 passed**; matrix **2 passed** (14/46 orders, 4/6 trades); Python contract suite **80 passed**.
- Corpus health: **4 differences**, existing HEAD/405 header contract for Task 13.
- Corpus echo: **2/2 cases matched**; simple reads: **29/29 matched**.
- Corpus prices: **40/40 matched**, plus both 17-probe passes.
- Corpus legacy curves: **4/4 matched**, plus 60 matrix and 8 scale comparisons.
- Ordinary live benchmarks: **4/4 matched**, plus three base and seven acceptance probes; the full Rust target passed in 130 seconds.
- Unported corpus groups: snapshot **9 difference paths/3 cases**, lists **47/15**, map **18/6**, canonical curves **18/6**. Their Rust routers are empty and return 404. These are missing implementations, not hidden passes or weakened comparisons.

Full verification logs use `work/rust-phase1/merged-*.txt`; targeted failure/repair logs use `integration-*.txt`. No push, live host or live database access. Colima and the pinned disposable PostGIS image only.
