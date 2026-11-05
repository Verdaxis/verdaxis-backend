# Rust abilities verification

Source worktree: `work/rust-abilities`, branch `feat/rust-abilities`.
No push, live host access, deployment, migration changes or Python application edits.

## Tests-first evidence

- `05aacd7ee`: unauthenticated HTTP request failed 404 versus required 401.
- `0f455ace5`: migrated runtime-role acceptance failed at the same missing route.
- `f8a7f806d`: policy tests failed with missing `auth::abilities` module.
- `cd1f4069c`: route and shared enforcement implementation.
- `206eb32ae`: exact Caddy ownership, inventory and contract documentation.
- `98a0d6183`: PostgreSQL active/expired/revoked capability checks.
- `1200b3521`: generated architecture metadata and explicit constant-path addition.

## Commands and results

All cargo commands used the requested Rust 1.95.0, SQLX_OFFLINE=true,
CARGO_INCREMENTAL=0, shared `.target-phase10` target directory and
`flock $HOME/verdaxis-test/runs/.lock-target-phase10`. The disposable runner used
`$HOME/verdaxis-test/venv312/bin/python` and a Docker-assigned numeric-loopback
port, PostgreSQL 17/PostGIS, full migration and converged runtime ACLs.

- `cargo fmt --manifest-path rs/Cargo.toml --all --check`: passed.
- `cargo clippy --manifest-path rs/Cargo.toml --workspace -- -D warnings`: passed.
- Same clippy with `--features verdaxis-read/contract-tests`: passed.
- `cargo test ... -p verdaxis-read --test abilities_policy --test abilities_http --test phase4_kyc_policy`: 13 passed, no failures/ignored tests (6 policy, 1 HTTP, 6 existing KYC).
- `bash rs/read/tests/run_abilities.sh`: 1 migrated acceptance case executed,
  passed, zero ignored. Both KYC switch states, schema/order/no-store, 401/403,
  retired RFQs and active/expired/revoked staff capability states covered.
- `pytest tests/rust_contract/test_abilities_routing.py tests/rust_contract/test_phase89_switch.py -q`: 53 passed; 11 existing Pydantic deprecation warnings.
  Local Caddy 2.11.2 checked enabled/disabled/stopped ownership, exact methods,
  neighbors, slash and case variants. No deployed service was contacted.
- `git diff --check`: passed.

New SELECTs use the existing auth adapters' runtime `sqlx::query` style, not
checked query macros. Thus no new `.sqlx` cache entry exists or is needed;
real schema/type/ACL validation is supplied by migrated PostgreSQL acceptance.

## Review and differences

Shared predicates are called by enforcing admission/inventory/support and
role-specific command adapters; KYC's scalar and locked adapters use the same
truth table. No action enforcement order was changed. See `docs/user-abilities.md`
for the full list: Rust KYC-first create/publication/confirmation, organization
approval precedence, recorded-owner cleanup, REAL-only takes, exclusive profile
lock, profile-only password-change exception and retired RFQ actions.

## Tooling limitations

The automatic commit security-review hook failed with HTTP 404; it did not
produce a security verdict. Details: `~/.claude/security/log.txt`.
`refresh-codesight` generated `.codesight` and wiki successfully, then failed its
managed-bootstrap step because local `render_bootstrap` is missing. No bootstrap
configuration was changed by hand. `.claude/context-map.json` remains untracked.
The generator misses constant route paths, so `.codesight/routes.md` explicitly
records the new abilities endpoint and shared policy modules.
