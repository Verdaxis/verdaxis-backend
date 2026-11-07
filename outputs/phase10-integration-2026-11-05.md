# Phase 10 integration — 5 November 2026

## Scope and decision

Local integration only. No push, CI dispatch, SSH, host read, or deployment was performed. Founder decision 204 controls release packaging: merge Phase 10 into staging once, then one CI run and one signed artifact. The later staging window combines W2 + W3; the production window combines W2-type handover + W4, including CRUD and all other groups. Python remains installed but idle until soak acceptance and separate W5 deletion. “Together” does not remove the stop/drain/fence and acceptance order within a window.

**Release disposition: integration prepared; live cutover NO_GO.** Tasks 31/32 still need executable full rehearsal, adoption, all-groups activation and complete-bundle rollback procedures. This merge does not implement those missing tasks.

### Git identity and conflicts

- Phase 10 parent: `f01acc05347c642fc2317bed2d78e55ded8a5166` (`origin/feat/rust-phase-10`).
- Staging parent: `0920be378ebbc42e88afba455d9521d60d49845a` (`origin/staging`, fetched for this integration).
- Production comparison snapshot: `85f76a091a504e60508b00243c7bb27ed52f0aa9` (`origin/prod`). These refs are source evidence, **not verification of installed host bytes**.
- The requested local branch already existed at `7829d974f`, was not attached to a worktree, and had no commits outside the Phase 10 parent. It was attached at the requested path and fast-forwarded, without reset or deletion.
- `git merge --no-commit --no-ff origin/staging`: **zero textual conflicts**. Thus there are no conflict files or manual conflict resolutions to list. Automatic compositions retained the Phase 10 CLI and the abilities exports, shared party policy, PostgreSQL runner additions, CI step and assertions. The offboarding test change is staging's existing reviewed behavior, not an integration-time assertion deletion.
- Integration-only reconciliation refreshes the runtime preparer digest, adds six new Python/Rust source dispositions, pins those memberships, and adds route/retention assertions. No existing assertion was removed.

## Re-derived pins

1. **Alembic:** no staging migration addition; single head remains `ob_20261031_offboarding`. Source: `alembic/versions/ob_20261031_individual_offboarding.py`; exact final transition and self-transition: `deploy/migration-checkpoints.tsv:63–64`. `ScriptDirectory.get_heads()` returned only that head. No migration, baseline SQL, fingerprint or checkpoint policy was changed here.
2. **Python routes:** still 188 mounted operations from 38 mounts. The abilities route is Rust-only, so it must not inflate the Python deletion inventory. AST extraction and exact membership checks remain in `tests/rust_contract/test_phase10_inventory.py:50–83,147–158`; route data remain in `tests/rust_contract/phase10/routes.json:5`.
3. **Abilities:** separate exact GET/HEAD/OPTIONS inventory at `/api/auth/me/abilities`, `tests/rust_contract/abilities/routes.json:1–8`. Runtime implementation: `rs/read/src/auth/abilities.rs:45`; shared policy: `rs/read/src/order_write/party_policy.rs`. The added Phase 10 regression checks these retained files and the inventory; it does not invent Python route ownership.
4. **Caddy composition:** the released snippet tuple already incorporates staging's `rust-abilities.caddy`: auth, abilities, offboarding, CRUD, Phase 4, Phase 5, public-staging. Market-write is the separate switched snippet; Phase 7 retirement is separately pinned. Source: `tests/rust_contract/test_phase89_switch.py:22–38`. Abilities is exact-path, not a prefix capture: `deploy/caddy/rust-abilities.caddy:1–19`. Root-health and monitor-canary remain separate Phase 10 surfaces. No Caddy file is installed or reloaded by this integration.
5. **Removal ledger:** 301 original rows become **307**, with all six newly merged `.py`/`.rs` files explicitly classified: two retained Rust runtime sources, three Rust regression sources, and the Python routing test retained only off-host. There are no new Python application files or Python routes. Runtime preparer bytes changed on staging, so both its script and helper ledger identities and `scripts.json` identity are refreshed. Script inventory remains 41; unit inventory remains 41; helper count remains 14. Membership is independently pinned in `tests/rust_contract/test_phase10_inventory.py` (`SUPPLEMENTAL_GROUPS`, `PINNED_SOURCE_IDENTITIES`); dispositions in `tests/rust_contract/phase10/removal-ledger.json` remain planned, not false installed acceptance.
6. **sqlx offline query data:** no `.sqlx/query-*.json` metadata exists or is needed for these runtime `sqlx::query` calls; the new abilities query is runtime-bound (`rs/read/src/auth/abilities.rs:192`). Offline all-target clippy compiles both feature modes. This is distinct from the sqlx **schema baseline**, `deploy/rust/sqlx-baseline.json:2–4`, which still binds the offboarding head. `scripts/rust_contract/sqlx_offline_check.py:1–6,19–35` defines the separate database-backed prepare check; that extra prepare command was not run in this integration.
7. **Production revisions:** unchanged exact eight entries: `p0_20261016_union`, `prc_20261026_changes`, `p3_20261020_auth_tokens`, `sse_20261021_replay_floor`, `p5_20261020_read_indexes`, `p89_20261022_watchlist_producer`, `p89_20261022_trade_freeze`, `ob_20261031_offboarding` (`rs/read/src/runtime/config.rs:11–20`). Do not confuse the broader deployed allowed-revisions JSON with this production list.

## Verification

Environment: `PATH=/opt/homebrew/opt/rustup/bin:/opt/homebrew/opt/util-linux/bin:$PATH`, `RUSTUP_TOOLCHAIN=1.95.0`, `SQLX_OFFLINE=true`, `CARGO_TARGET_DIR=$HOME/verdaxis-test/runs/.target-phase10`, `CARGO_INCREMENTAL=0`, `RUST_TEST_PYTHON=$HOME/verdaxis-test/venv312/bin/python`, full `RELEASE_SHA=$(git rev-parse HEAD)`. Every cargo command used `flock -w 7200 $HOME/verdaxis-test/runs/.lock-target-phase10`. Tool log confirms cargo/rustc 1.95.0. Logs are local in `work/phase10-integration/`.

- Formatting: `cargo fmt --manifest-path rs/Cargo.toml --all --check` passed. Initial invocation without `--all` reported “Failed to find targets” for the virtual workspace; it was corrected, not reported as a passing check.
- Clippy: `cargo clippy --manifest-path rs/Cargo.toml --all-targets -- -D warnings` and the same with `--features contract-tests` passed.
- Python: final Phase 10 contracts, Phase 8/9 switch contracts and runtime release regressions passed together. Counts and final result are recorded in the closing gate summary below. Baseline Phase 10 alone: **1,335 passed**.
- Abilities: policy **6 passed**; non-database HTTP **2 passed**. The disposable PostGIS runner then included all ignored database cases: **3 HTTP + 4 PostgreSQL passed**, no ignored cases in that runner. Database migrated through the exact offboarding head and was removed with its volume.
- ShellCheck: all paths from `deploy/tests/shellcheck-files.txt`, `--severity=style --external-sources`, passed with local **0.11.0**. CI pins **0.10.0** (`.github/workflows/rust-read-ci.yml:41–64`); that exact version was not installed or claimed here.
- Platform comparison: `phase10_guard` **9 passed** and `phase10_ops_env` **13 passed** on both merged source and untouched Phase 10 baseline. Current guard supports BSD stat; no GNU-stat failure was reproduced in those tests. Linux-only fixture and Caddy outcomes are recorded below, not treated as source regressions.
- Abilities Caddy: merged **1 passed, 45 setup errors**, all requiring `CADDY_BIN`. Abilities does not exist on the Phase 10 parent. The identical new routing harness, snippet and route fixture were therefore overlaid on an archived parent solely to compare the environment: **1 passed, 45 identical Caddy setup errors**. This is not a claim that parent code implements abilities.
- An optional broad `cargo test ... phase10` name filter was stopped: it mostly selected zero tests across unrelated binaries and was not meaningful full-suite evidence. The explicit guard/runtime/abilities targets above completed.
- Codesight refresh regenerated summaries but its bootstrap stage failed (`ModuleNotFoundError: render_bootstrap`). Generated output was archived under `work/phase10-integration/codesight-refresh/`; unrelated generated-doc churn was not committed. No global tool or configuration was changed.

## 4a. What an ordinary deployment changes

### Installed helper versus candidate helper

A running shell continues the installed `deploy.sh`; fast-forwarding its checkout does not switch its instructions. Using the **recorded staging helper**, a valid `ob → ob` deploy can advance source, publish matching runtime identity, verify and install the Rust artifact, converge ACLs, prepare budgets/runtime files, restart Python and Rust, verify exact readiness, enable Rust and clear deploy state. It uses retained Python tools and performs pip install/check. It does **not** install the new unit manifest, transfer timers/monitors, adopt sqlx, route all groups, stop Python permanently or delete it.

Evidence in the recorded helper (read with `git show origin/staging:scripts/deploy.sh`): checkpoint/current check `:887–902,1011–1013`; source fast-forward `:1020–1043`; pip `:1061–1063`; migration/ACL/runtime `:1112–1145`; Python restart `:1187–1191`; Rust restart and acceptance `:1216–1241`. Matching-current checkpoint helper skips Alembic upgrade (`scripts/apply_migration_checkpoint.py:301–317`).

The **candidate helper**, when separately run, replaces those consumers with shell/native operations (`scripts/deploy.sh:1068–1078,1125–1212`) and refuses dependency-file changes (`:959–968`), but still restarts Python then Rust (`:1238–1281`). Therefore even a second candidate-helper pass is not W3/W4 acceptance.

### Is it refused just because the helper changed?

**No demonstrated blanket helper-change refusal.** The old staging ACL bundle archives policy SQL, convergence SQL and the Python convergence helper only (`origin/staging:scripts/deploy.sh:695–714`). Its post-checkout call arguments (`:1120–1124`) match the unchanged `scripts/converge_runtime_acls.py:197–216`. The unchanged Python start guard accepts the old helper's three-key restart-authorized state while both locks are held (`scripts/verify_deploy_start_guard.py:142–154`). There is no deploy-helper digest comparison that inherently blocks this first pass.

Normal gates still refuse dirty source, wrong branch/ref, mismatched approvals, moved target, unexpected revision, unlisted transition, invalid artifact, or stale/invalid deploy state. Candidate deploy also requires enabled Rust policy and loaded Rust unit (`scripts/deploy.sh:897–899,990–1039`). Do not bypass a refusal by deleting state or fabricating authorization.

**Production caveat:** actual installed bytes were not read. Recorded `origin/prod` omits `ob` from its producer/listener classifiers. With explicit WATCHLIST producer inputs it refuses at `origin/prod:scripts/deploy.sh:540–543` (called `:903–904`); without them it clears carried producer keys (`:545–547`) and selects listener `0` (`:1098`). Its own missing checkpoint row is not the issue: it reads the candidate's archived policy. Thus the production old-helper path is not approved merely because staging's path can complete. Check and stage a reviewed bootstrap in the later authorized window; do not run this audit's templates blindly.

### Approval interface

From the fixed, clean environment checkout, a later operator uses dry-run's full SHA, not this local branch name:

```bash
./scripts/deploy.sh --dry-run
APPROVED_RELEASE_SHA="$SHA" \
MIGRATION_APPROVED_SOURCE_SHA="$SHA" \
MIGRATION_EXPECTED_CURRENT_REVISION=ob_20261031_offboarding \
MIGRATION_TARGET_REVISION=ob_20261031_offboarding \
RUST_ARTIFACT_ARCHIVE="$ARCHIVE" \
RUST_PROVENANCE_RECORD="$RECORD" \
RUST_PROVENANCE_SIGNATURE="$SIGNATURE" \
./scripts/deploy.sh
```

Names remain unchanged (`scripts/deploy.sh:114–116,131–134,912–957`). Both environments consume staging-ref provenance (`:103–106`). This command is **ordinary release deployment**, not window authority.

**No W2/W3/W4/W5 happens automatically.** More strongly, this branch has no complete window helper to invoke. Unit installation explicitly does not activate units (`scripts/install_systemd_units.sh:3`). There is no allowlisted sqlx transition (`deploy/migration-checkpoints.tsv:63–64`); native migration treats a non-sqlx self-transition as already-at-target, not baseline adoption (`rs/read/src/cli/migrate.rs:472–535`). Baseline application also needs blocked deploy state, nonce and both locks (`:538–598,692–707`). A standalone baseline invocation is not a workaround.

## 4b. Ordered later operator windows — requirements and executable gaps

Plan reference below means the external approved `outputs/Verdaxis Rust Backend Rewrite - Phase 10 Plan - 2026-10-22.md`. Window table `:615–625`, standing gates `:627–629`, Task 31 `:1925–1961`, Task 32 `:1963–1998`. Decision 204 changes packaging to bulk windows, not the guard requirements.

### Common entry checks

1. Obtain separate live authorization; read current host instructions and installed state. Attest the actual helper, branch, SHA, database head, timers and source/runtime authority. This integration grants none of that access.
2. Finish W1 independently: all retained databases at final offboarding head through approved exact predecessor releases. Do not combine W1 migration with W3.
3. Close Tasks 1–30, 35 and 36; fresh signed W0 inventories and owner acceptance, selected Phase 7 disposition, schema handover, full Task 31 rehearsal, actual-old-binary rollback proof and first independent review. Current fixture inventory is not installed acceptance.
4. Verify one signed integration artifact and protect the complete `pre-M-<environment>-<full-sha>` bundle: old binary/source, units, runtime/revision/budget identity, guards and shared state. Keep Alembic audit rows. No schema downgrade or identity-only rollback (plan `:532–536,629`).

### Staging window: W2 + W3, all groups

1. **W2 installation, then activation:** separately stage and install the exact manifest using the verified binary digest:

   ```bash
   sudo -n ./scripts/install_systemd_units.sh --dry-run \
     --environment staging --source-ref "$SHA" --binary-sha256 "$VERIFIED_BINARY_SHA256"
   sudo -n ./scripts/install_systemd_units.sh --apply \
     --environment staging --source-ref "$SHA" --binary-sha256 "$VERIFIED_BINARY_SHA256"
   ```

   Installer argument contract: `scripts/install_systemd_units.sh:23–25,61–67`. Neither command activates the window.
2. Record active timers, stop them, let active oneshots drain, fence Python producers/direct launchers, then transfer jobs in plan order: G3 news; G4/G5 auth/prune; G6 reminders; G7 onboarding with G4; G8 demo only after market gates; G9 local monitoring before G10 external/recovery. Reconcile alert journal ownership before reversal. Require one successful run per unit, readiness, and no duplicate effect (plan `:407–420,619,1943–1949`). Rollback each failed group by restoring its prior command and shared state under the same fences.
3. **W3 in the same authorized staging window:** verify pre-M bundle, adopt sqlx under deploy ownership/fingerprint gates, perform a genuinely Python-free deploy, and move **all** route groups to reviewed Rust owners, including abilities and CRUD. Keep Python installed but idle. Check exact readiness, passive harm checks, no Python traffic/process restart, and every group's acceptance. This is the founder's bulk target, not a command available in this tree.
4. On W3 failure, restore the **entire** pre-M bundle under both locks and prove the actual previous binary starts. Preserve adoption and Alembic audit records; do not restart Alembic writers. Keep timers quiesced until the recorded accepted or rollback state permits restoration.

### Production window: W2-type handover + W4, all groups including CRUD

1. After staging acceptance, recheck production-specific identity, authority, inventories, pre-M bundle, readiness and rollback. Do not infer CRUD ownership from staging: production must move CRUD too.
2. Run the installer pair above with `--environment production`, using its independent approved SHA and verified artifact digest.
3. Execute the same stop/drain/fence and ordered job/monitor transfer, then W4 fingerprint/adoption, Python-free deploy and all-groups Caddy ownership switch. Require production readiness, passive checks, no duplicate effects and **one observed timer run per job** (plan `:621`).
4. Roll back to production's complete pre-M bundle and actual old binary. Per-group Python rollback is available only while Python still exists. Soak follows acceptance; W5 is a later separate deletion window, staging first, production second. W5 has **no route rollback** (plan `:622–625,1991`).

### Why there is no exact runnable all-groups command list yet

Do **not** substitute an invented helper for missing implementation:

- `deploy/tests/full-rehearsal.sh` (Task 31) and `deploy/tests/deletion-contract.sh` (Task 32) are absent. Task 32's deletion Python contract is also absent.
- No reviewed sqlx-adoption checkpoint is allowlisted, and the current deploy requires/restarts Python (`scripts/deploy.sh:1008–1011,1238–1258`; Python units `deploy/systemd/verdaxis-backend.service:26–28` and staging equivalent).
- Existing offline candidate commands are only partial tools:

  ```bash
  python3 -m scripts.rust_caddy_candidate --site "$SAVED_SITE" \
    --environment "$ENVIRONMENT" --group "$GROUP" --output-dir "$NEW_PRIVATE_DIR"
  python3 -m scripts.rust_phase5_candidate --site "$SAVED_SITE" \
    --environment "$ENVIRONMENT" --target rust --output-dir "$NEW_PRIVATE_DIR"
  ```

  First helper covers simple/orderbook/prices/curves/mcp, six CRUD groups, preauth/access-log/fallback; preauth precedes CRUD (`scripts/rust_caddy_candidate.py:10–15,118–127,436–439`). Phase 5 writes the original-byte rollback (`scripts/rust_phase5_candidate.py:6–12,110–123`). Neither reloads Caddy or supplies complete all-groups activation, ownership acceptance, sqlx adoption or pre-M bundle restore.

**Resume point:** implement and review Tasks 31/32's exact installed-path commands, checkpoint policy, native Python-free transaction and rollback before authorizing either live window. Do not call the ordinary release deploy a cutover.

## 4c. Task 31 full rehearsal: prerequisites and duration

The plan requires the whole disposable W2–W5 lifecycle, every unit/timer/monitor, baseline adoption, Python-free deploy, deletion/no restart, group rollbacks, actual old-binary startup and interruption at each publication boundary. Tasks 1–30, 35/36, fresh W0/owner acceptance and privately signed off-host evidence are entry requirements (plan `:1927–1961`).

Planned command:

```bash
bash deploy/tests/full-rehearsal.sh --disposable &&
bash deploy/tests/verify-host-inventory.sh --fixture
```

It cannot run now: the first file is absent, and the current verifier actually requires `--fixture INVENTORY.json` and disables installed acceptance (`deploy/tests/verify-host-inventory.sh:2–10`). Fixture validation is not proof that an installed host is clean.

- **On this Mac:** Colima/Docker can run existing subsets, but a systemd fixture needs an executable **Linux ELF** artifact, not the native Mac binary. Example existing Task 10 subset: `VERDAXIS_READ_BIN=/absolute/path/to/linux/verdaxis-read bash deploy/tests/units-contract.sh`. It needs Docker, disposable systemd identities and ACL tooling (`deploy/tests/units-contract.sh:252–257,270–315`). Full sandbox/boot acceptance also needs an isolated root-systemd VM, not only rootless Docker. The current Mac alone plus `cargo test` is insufficient.
- **On the production test host:** `verdaxis-test-postgres` supplies only disposable database capability, not an entire isolated service manager/filesystem. Historical inventory shows its PostgreSQL 17 service exists but was disabled; current status is unverified (`../outputs/research/rust-phase10/t36/production-deps.txt:14,69–70`, relative to workspace root). A separately authorized capped test lane and isolated root-systemd VM/container environment, signed Linux artifact, old recovery artifact, test credentials/mail/monitor stubs, Caddy and ACL tools are still needed. No live database or production units are test targets.
- **Duration:** plan's **20–28 hours is Task 31 implementation/acceptance effort**, not measured run duration (`:1927`). Task 32 is 24–34 hours of effort (`:1965`). No defensible full-rehearsal wall-clock estimate exists before the missing driver is implemented and timed. Historical Task 28 subset took 8m28s, not a Task 31 measurement. Budget Task 31's stated effort and measure one complete isolated run before scheduling the live windows; do not promise an unsupported Mac/host runtime.

## Closing gate summary

- Final combined Python gate: **1,391 passed**, 11 existing Pydantic deprecation warnings (Phase 10 + switch + runtime release regressions). Untouched Phase 10 parent gate: **1,335 passed**. Additional tests include the new supplemental-membership mutations; none were dropped.
- Formatting and both all-target clippy feature modes: **passed** with Rust 1.95.0.
- Abilities policy/HTTP/PostgreSQL: **passed** as detailed above. Running-Caddy cases: **45 environment setup errors**, reproduced with the same overlaid harness on the parent; not passed or silently skipped.
- Installer static fixture: **32 passed, 2 disposable cases explicitly skipped**, identically on merged and baseline trees. GNU-stat failures were **not reproduced** in static mode. Linux full installer/ACL checks were not claimed on macOS.
- Deploy transaction fixture: both trees stop with `deploy-transaction.sh needs Linux (the deploy reads /proc/self/fd)`. This is a local platform limitation, not a completed Linux gate.
- ShellCheck **0.11.0 passed** on the complete reviewed list; exact CI-pinned 0.10.0 and Linux CI remain unrun.
- A final independent pin-review attempt was blocked by the review agent's worktree boundary; it produced no verified review result. The integration author checked the diff and complete six-file ledger coverage locally. This does not replace the plan's later independent release reviews.
- `git diff --check` passed. Local free space after gates: 47 GiB. All raw logs remain under ignored `work/phase10-integration/`; generated Codesight output is archived there. No unrelated cache or user work was removed.
- The merge commit containing this report is the integration identity; obtain it with `git log -1 --format=%H -- outputs/phase10-integration-2026-11-05.md`. Neither this report nor local gates assert CI, installed-host acceptance, or live cutover readiness.
