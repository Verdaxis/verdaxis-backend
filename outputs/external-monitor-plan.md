# External monitor implementation plan

**Goal:** Switch only the shared-host monitor and verification units to a pinned CI Rust binary. No host access or merge.

**Architecture:** Keep the existing Rust external monitor behavior. Install its release binary at a dedicated path, separate from backend deployment. Preserve the environment, state, timer, masked recovery, diagnosis, and unrelated software.

**Review:** Architecture PASS: reuse existing command. Quality PASS: validate digest before changes, serialize installation, timestamp backups, atomic file replacement, explicit rollback. Tests PASS: exercise install, repeat install, refusal, and rollback with disposable fixtures in CI. Performance PASS: no new recurring process.

1. Update `deploy/external_monitor/systemd/verdaxis-monitor{,-verify}.service`; refresh their hashes in `deploy/monitor/artifact-manifest.json`.
2. Add `deploy/external_monitor/install.sh`, a pinned CI release record, and an operator procedure in `outputs/external-monitor-install.md`. Preserve the timer state and recovery mask. Do not change environment values.
3. Remove `deploy/external_monitor/verdaxis_monitor.py` and `tests/monitor/test_external_monitor.py`; retain Python recovery and diagnosis (no implemented Rust equivalents).
4. Add installer behavior tests in `tests/tooling/` and Rust configuration/unit contract coverage in `rs/read/tests/phase10_external.rs`. Add the installer to the ShellCheck list. Update ownership documentation.
5. Commit and push `chore/external-monitor-rust`, open one PR to `staging`, inspect all CI results and failed logs, amend through normal commits until green. No local Cargo build or tests.
6. Report exact pinned install, verify, and rollback commands. Delete only the requested temporary clone after the pushed PR is green.

**Resume point:** Implementation pushed as PR #172. Pin selected from successful staging CI run 42775104768 and its attested binary. Next: finish CI review, push final documentation and any CI-driven changes, then remove the requested clone after all checks pass.
