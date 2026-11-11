#!/usr/bin/env bash
# Pure turn contract and prompt checks; no database or provider is required.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../../.." && pwd)
log=$(mktemp)
trap 'rm -f "$log"' EXIT
cargo test --manifest-path "$repo/rs/Cargo.toml" --locked -p verdaxis-read \
  --lib agent::turn::tests:: -- --test-threads=1 2>&1 | tee "$log"
if ! grep -Eq '^test result: ok\. 6 passed; 0 failed; 0 ignored;' "$log"; then
  printf 'agent turn unit test count mismatch: expected 6 passing cases\n' >&2
  exit 1
fi
