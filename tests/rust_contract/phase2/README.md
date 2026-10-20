# Phase 2 contract fixtures

- `routes.json`: 27 mounted operations, request and response schema exports,
  effects, Python oracle SHA and source digests.
- `export_inventory.py`: test-host-only, reproducible full HTTP OpenAPI export.
- `evidence-manifest.json`: required evidence, not observed results. Task 3 owns
  its evaluator. No candidate or production GO exists from Tasks 1–2.
- `normalization.json`: the reviewed `phase2-meaning-v1` path ledger. Paths are
  relative to the response body. `*` means an array element, never an arbitrary
  object field. Error bodies do not receive successful DTO rules.

## Comparison entry point

Use `scripts.rust_contract.compare.compare_responses` with raw response mappings:

```python
{"status": 200, "headers": {"content-type": "application/json"}, "body": b'{}'}
```

Pass `profile="phase2-meaning-v1"`, a stable `route_id`, and `rules_sha256` equal
both to the committed ledger's digest and to the implementation's reviewed pin.
Pass `request_id` when the harness supplied one. Do not pass recorded parsed
bodies or a caller's proposed rules without their reviewed pin. Unknown profiles,
unknown routes, absent or changed digests, invalid JSON and invalid typed values
raise `ContractError` (invalid evidence). Valid responses with different meaning
return a nonempty list of schema-path/category diagnostics. Diagnostics contain
no response values. No profile retains the Phase 1 byte comparison.

All body IDs, ranks, private fields, stored timestamps and page members remain
in the comparison. The only request-ID substitution is a validated generated
UUIDv4 when the harness supplied no ID. Status and non-transport headers remain
exact, including absence. Nonempty 204 bodies are invalid evidence.

Decimal strings normalize only A01 `/threshold_usd` and A02
`/*/threshold_usd`. Typed timestamps use aware ISO text with optional 1–6 digit
fractions and `Z` or a numeric `HH:MM` offset. They compare exact UTC instants,
without rounding or tolerance. Naive or excess-precision timestamps are invalid.
Preference values, notification data, event payloads and opaque cursors have no
typed-string normalization. Normal JSON numbers use exact Decimal values; bool
remains a separate kind.

Approved ties preserve the ordered descending key sequence and a multiset of
complete canonical items inside each contiguous equal-key group. W01 entries
alone have an unordered multiset with an empty tuple key. Rank reassignment,
changed multiplicity, a new page member, cross-key movement and changed item
fields all fail. W07 feed order has no exception. Cross-stack cursor execution
belongs to the later runner; these tests prove only that cursor text is opaque.

## Refresh boundary

An approved oracle repair requires source review and inventory regeneration.
A normalization change requires review of its exact DTO paths, a new ledger
digest in `phase2_meaning.py`, and adversarial tests. A caller-provided digest
alone never approves a wider rule. Keep the Phase 1 default unchanged.
