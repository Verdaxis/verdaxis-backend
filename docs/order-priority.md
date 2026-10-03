# Order acceptance priority

Executable candidates are selected by best price, then the persisted
`acceptance_ordinal`, then order UUID. PostgreSQL allocates the ordinal from
`orderbook_acceptance_ordinal_seq` while the transaction holds the canonical
market-slice advisory lock. `created_at` remains the immutable audit timestamp.

A price change, quantity increase, expiry extension, or changed execution
eligibility receives a new ordinal. A quantity reduction retains its ordinal.
Fills and changes that do not alter price, available quantity, expiry, or
execution eligibility also retain it. Staging UCOME B100 structured-term
changes are execution-eligibility changes.

One match transaction examines at most 100 crossing candidates. If a new or
amended order still has quantity after those candidates and more candidates
were eligible, the API returns HTTP 409 and rolls back the complete
transaction. It never commits a partial prefix. The participant can retry
after the book changes; this release does not provide a continuation worker.
