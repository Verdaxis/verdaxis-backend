# Rust read connection budget

Founder decision 88 (2026-10-18) supersedes AD-02's Python-derived limit.

- `READ_ADMISSION_SLOTS` in the deployed environment, or
  `read_admission_slots` in `RUST_CONFIG`, defaults to **16**.
- The read pool equals slots. Readiness has one separate connection.
- JSON `pool_max` must equal slots + 1. Zero slots and overflow refuse startup.
- The budget helper reads the same deployed environment value and retains its
  full environment digest. A changed value requires a new prepared artifact.
- Runtime verifies release binding, artifact arithmetic, and the configured
  maximum before connecting, then verifies PostgreSQL's observed maximum.
- Python admission remains unchanged. It does not size the Rust gate.

| Read slots | Rust total | Python services | Reserve | Full budget |
| --- | --- | --- | --- | --- |
| 8 | 9 | 32 | 20 | 61 |
| 16 (default) | 17 | 32 | 20 | 69 |
| 24 | 25 | 32 | 20 | 77 |

Python services = `2 × 4 × (2 + 1 + 1)`. These are example effective settings,
not authority to ignore the configured or observed `max_connections`.
At these settings and maximum 100, the largest valid read limit is 47.

Overload remains HTTP 503 with `Retry-After: 1`. A cancelled caller does not
release capacity before its query-owning task finishes and returns or closes
the pooled connection. See `docs/rust-phase1/admission-capacity.md` for the
exclusive test-host probe and verification results. No activation is authorized.
