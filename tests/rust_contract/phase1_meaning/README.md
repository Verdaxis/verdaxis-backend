# Phase 1 meaning profile

Use only `compare.compare_responses` with explicit `profile="phase1-meaning-v1"`,
a `P1-` route ID, and ledger digest
`f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e`.
The engine pins the same digest. Phase 2's v1 ledger and the no-profile byte path
remain unchanged.

`export_rules.py` derives declared scalar paths from the pinned Python response
models, including excluded-field handling and typed-map paths. Run it only on the
isolated test host. News and compatibility listings use explicitly reviewed dict
projection paths. The source digest table identifies those Python inputs.
`routes.json` repeats reviewed paths for the immutable route inventory; a test
requires the paths to agree with the ledger.

Arrays stay ordered except news equal-`published_at` groups and the existing
price-summary collection rule. Every item and duplicate remains significant.
Tape uses strict canonical page order; its expected page must come from Task 5's
complete-relation oracle. This comparator cannot establish canonical membership
from an unchanged Python HTTP page alone.

CSV is byte-exact. Non-JSON method, redirect and auxiliary bodies remain byte-exact.
MCP success text is machine-readable tool JSON and goes through the same walker
as `structuredContent`, with payload-relative typed paths. Ordinary error text is
exact. For decision 84, a rejected tool argument request supplies
`diagnostic_context={"tool": ..., "arguments": ...}`. The pinned Python validator
provides the exact historical text and structured errors. Rust may return
`Validation failed: <JSON errors>` with exact kind, location, input and message.
No repr parser, diagnostic truncation parser, tolerance or second comparator is
used. Missing context never relaxes an error string. Changed structured diagnostic
values fail even if both responses contain the same changed values.

This profile is successor-only. It does not activate a route or qualify a release.
