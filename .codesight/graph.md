# Dependency Graph

## Most Imported Files (change these carefully)

- `app/models/user.py` — imported by **174** files
- `app/database.py` — imported by **129** files
- `app/models/orderbook.py` — imported by **113** files
- `app/market_catalog.py` — imported by **93** files
- `app/config.py` — imported by **72** files
- `app/models/catalog.py` — imported by **69** files
- `app/routers/auth_simple.py` — imported by **65** files
- `app/main.py` — imported by **41** files
- `app/services/availability_windows.py` — imported by **38** files
- `scripts/rust_contract/run_disposable.py` — imported by **35** files
- `app/model_base.py` — imported by **33** files
- `app/schemas/market_activity.py` — imported by **33** files
- `app/core/security.py` — imported by **32** files
- `rs/read/tests/market_database.rs` — imported by **30** files
- `app/services/execution_policy.py` — imported by **29** files
- `app/schemas/orderbook.py` — imported by **28** files
- `app/rate_limit.py` — imported by **26** files
- `app/services/market_data_eligibility.py` — imported by **25** files
- `scripts/rust_contract/compare.py` — imported by **25** files
- `app/routers/orderbook.py` — imported by **24** files

## Import Map (who imports what)

- `app/models/user.py` ← `app/admin.py`, `app/cli/send_kyc_grace_notices.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/models/__init__.py` +169 more
- `app/database.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/auth_maintenance.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py` +124 more
- `app/models/orderbook.py` ← `app/models/__init__.py`, `app/models/live_slice_benchmark.py`, `app/models/user.py`, `app/routers/admin_analytics.py`, `app/routers/compliance_api.py` +108 more
- `app/market_catalog.py` ← `app/market_constraints.py`, `app/mcp_server.py`, `app/models/catalog.py`, `app/models/orderbook.py`, `app/models/supplier_offer.py` +88 more
- `app/config.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py`, `app/cli/send_kyc_grace_notices.py` +67 more
- `app/models/catalog.py` ← `app/models/__init__.py`, `app/routers/admin_analytics.py`, `app/routers/alerts.py`, `app/routers/availability.py`, `app/routers/catalog.py` +64 more
- `app/routers/auth_simple.py` ← `app/main.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/middleware/subscription.py`, `app/routers/activity.py` +60 more
- `app/main.py` ← `app/services/audit_service.py`, `rs/read/tests/fixtures/offboarding_acceptance_harness.py`, `rs/read/tests/fixtures/offboarding_python_harness.py`, `rs/read/tests/fixtures/offboarding_reassignment_harness.py`, `scripts/export_openapi.py` +36 more
- `app/services/availability_windows.py` ← `app/mcp_server.py`, `app/models/orderbook.py`, `app/models/rfq.py`, `app/routers/admin_analytics.py`, `app/routers/curves.py` +33 more
- `scripts/rust_contract/run_disposable.py` ← `rs/read/tests/orderbook_lists_oracle.py`, `rs/read/tests/orderbook_map_oracle.py`, `rs/read/tests/orderbook_snapshot_oracle.py`, `rs/read/tests/public_read_latency.py`, `rs/read/tests/run_curves_canonical.py` +30 more
