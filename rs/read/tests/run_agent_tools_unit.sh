#!/usr/bin/env bash
# Pure typed-tool checks; no database or provider credentials are required.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../../.." && pwd)
log=$(mktemp)
trap 'rm -f "$log"' EXIT
check_count() {
  local count=$1
  if ! grep -Eq "^test result: ok\. ${count} passed; 0 failed; 0 ignored;" "$log"; then
    printf 'agent tool test count mismatch: expected %s passing cases\n' "$count" >&2
    exit 1
  fi
}
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --lib agent::tools::tests:: -- --test-threads=1 2>&1 | tee "$log"
check_count 1
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --test group_crud agent_tools:: -- --test-threads=1 2>&1 | tee "$log"
check_count 8
