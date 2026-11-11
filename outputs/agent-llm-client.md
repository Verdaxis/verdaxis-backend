# Phase A Task A2: model transport

PR: https://github.com/Verdaxis/verdaxis-backend/pull/176

## Layout

- `rs/read/src/agent/mod.rs`: agent namespace, no route mounting.
- `rs/read/src/agent/llm/mod.rs`: async client, bounded I/O, retries and typed attempt receipts.
- `rs/read/src/agent/llm/credentials.rs`: environment slots, usage meters and secret checks.
- `rs/read/src/agent/llm/models.rs`: single model allow-list and reasoning budget.
- `rs/read/src/agent/llm/tests.rs`: local mock HTTP tests.
- `rs/read/tests/agent_llm.rs`: public allow-list test in `group_runtime`.

Keep one `GoClient` for the service lifetime. Its mutable API serializes calls;
callers that share it must retain the same instance so its cache and disabled
slots survive between turns. A `Failure` retains all model-attempt receipts.
No raw provider error body is returned or logged. Unknown cost is null.
The client checks transport output, not the later decision/tool contract.

Optional serve-only configuration: `OPENCODE_API_KEY`, `OPENCODE_API_KEY_2`,
`OPENCODE_API_KEY_3`, `AGENT_ENABLED`, `AGENT_MODEL`. Absent settings are not
projected. No route or feature switch is activated by this task.

## Verification and resume

- Local pinned Rust formatting and test-group registration checked.
- No local Cargo build or test suite: disk-limited execution uses CI.
- Local tooling test command could not start: system Python has no pytest.
  The PR CI tooling job runs the full command with its pinned packages.
- Local Codesight scan regenerated summaries; bootstrap refresh stopped because
  the machine helper could not import `render_bootstrap`. No generated files
  are included in this PR.
- Resume by reading the latest PR CI run, addressing any failed check, and
  confirming all required checks pass. Do not merge.
