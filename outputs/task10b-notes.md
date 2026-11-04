# Phase 10 Task 10b — local handoff

## Status: blocked acceptance

No remote host was contacted. No Cargo command, push, live unit activation or `.env` read ran.

Implemented the host installer, canonical caller, external monitor unit ABI,
render identity/home policy, artifact inventory, retirement counts and named
Rust/shell tests. The installer checks all source digests before destination
writes and installs each file with a checked temporary copy and rename. It
runs named sysusers policies and one tmpfiles invocation, then compares full
handoff ACLs. It does not activate or remove units.

## Checks

- `bash deploy/tests/units-contract.sh`: **29 cases passed; 2 failed**.
  - `installer_refuses_a_manifest_digest_mismatch`: passed for the binary and
    each of the 25 pinned artifacts; destination root stayed absent.
  - `installer_restart_and_revert_preserve_admitted_alert_state`: failed,
    `VERDAXIS_PREVIOUS_HOST_RELEASE` is absent. Task 10a has no host installer;
    a copied candidate is not valid prior-release evidence.
  - `installer_enforces_per_check_handoff_acl_and_identity_boundary`: disposable
    Docker install and exact ACL inspection passed; the mandatory Task 28 suite
    failed because `VERDAXIS_READ_BIN` does not name an available Linux binary.
    Colima supplies Linux; the old client-OS restriction was removed.
- ShellCheck: all paths in `deploy/tests/shellcheck-files.txt` passed.
- Requested Python command: **1609 passed, 1 skipped**. The existing skip is not
  one of the six mandatory new scenarios. Both Python files that directly read
  the changed external units are included in this command.
- `rustfmt --edition 2024 rs/read/tests/phase10_units.rs`: passed.
- Rust compilation and execution: **not run**, as instructed.

## PM actions / resume point

1. Bind `deploy/monitor/artifact-manifest.json`'s `binary.sha256` to the verified
   release binary. The source value `UNBOUND_RELEASE_BINARY` deliberately refuses
   installation. No compiled release was available to derive a valid digest.
   Integrate this release-pack binding with packaging before deployment. The
   canonical installer currently stages the source manifest, so it also refuses
   until an approved release has a bound digest; this is not deployment-ready.
2. Supply a Linux binary with `VERDAXIS_READ_BIN` and a real previous native
   release pack with `VERDAXIS_PREVIOUS_HOST_RELEASE`. Run all 31 shell cases;
   missing inputs remain failures, not skips. The previous-release path cannot
   be invented from Task 10a, which did not have this installer.
3. Compile/run all `phase10_units` tests, especially:
   `manifest_counts_are_16_canonical_11_local_5_external`,
   `old_unit_can_start_the_new_binary`,
   `timer_catch_up_after_restart_runs_once`, and
   `monitor_units_exec_the_digest_pinned_host_copy`.
   Old ABI fixtures are exact local Git `1ceafc83a` bytes, 16 canonical units
   plus seven Task 27/28 native local units. No Git is used at test time.
4. Perform real systemd stop/start/boot and admitted-journal forward-revert
   verification. The installer-only shell fixture tests file preservation, not
   actual admission or Python reactivation; the mandatory Task 28 suite owns the
   real process/identity boundary. Do not infer those results from a fake binary.
5. Check uncached external timing against the deployed settings and live verify
   mode in a separately authorized window. Unit timeout is 1800 s, with a
   conservative default-check budget of 1122 s documented beside it. Increased
   environment retries/timeouts require a fresh budget review.

No deployment authority is granted by this handoff.
