# Dependency Graph

## Most Imported Files (change these carefully)

- `app/models/user.py` — imported by **170** files
- `app/database.py` — imported by **125** files
- `app/models/orderbook.py` — imported by **112** files
- `app/market_catalog.py` — imported by **92** files
- `app/config.py` — imported by **70** files
- `app/models/catalog.py` — imported by **68** files
- `app/routers/auth_simple.py` — imported by **61** files
- `app/services/availability_windows.py` — imported by **38** files
- `app/main.py` — imported by **36** files
- `app/schemas/market_activity.py` — imported by **33** files
- `app/model_base.py` — imported by **32** files
- `app/services/execution_policy.py` — imported by **30** files
- `scripts/rust_contract/run_disposable.py` — imported by **30** files
- `app/core/security.py` — imported by **29** files
- `app/schemas/orderbook.py` — imported by **28** files
- `app/rate_limit.py` — imported by **25** files
- `app/services/market_data_eligibility.py` — imported by **25** files
- `app/routers/orderbook.py` — imported by **24** files
- `app/services/audit_actions.py` — imported by **24** files
- `scripts/rust_contract/compare.py` — imported by **24** files

## Import Map (who imports what)

- `app/models/user.py` ← `app/admin.py`, `app/cli/send_kyc_grace_notices.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/models/__init__.py` +165 more
- `app/database.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/auth_maintenance.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py` +120 more
- `app/models/orderbook.py` ← `app/models/__init__.py`, `app/models/live_slice_benchmark.py`, `app/models/user.py`, `app/routers/admin_analytics.py`, `app/routers/compliance_api.py` +107 more
- `app/market_catalog.py` ← `app/market_constraints.py`, `app/mcp_server.py`, `app/models/catalog.py`, `app/models/orderbook.py`, `app/models/supplier_offer.py` +87 more
- `app/config.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py`, `app/cli/send_kyc_grace_notices.py` +65 more
- `app/models/catalog.py` ← `app/models/__init__.py`, `app/routers/admin_analytics.py`, `app/routers/alerts.py`, `app/routers/availability.py`, `app/routers/catalog.py` +63 more
- `app/routers/auth_simple.py` ← `app/main.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/middleware/subscription.py`, `app/routers/activity.py` +56 more
- `app/services/availability_windows.py` ← `app/mcp_server.py`, `app/models/orderbook.py`, `app/models/rfq.py`, `app/routers/admin_analytics.py`, `app/routers/curves.py` +33 more
- `app/main.py` ← `app/services/audit_service.py`, `scripts/export_openapi.py`, `scripts/rust_contract/feedback_oracle.py`, `scripts/rust_contract/phase2_alert_oracle.py`, `scripts/rust_contract/phase2_auth_vectors.py` +31 more
- `app/schemas/market_activity.py` ← `app/routers/availability.py`, `app/routers/curves.py`, `app/routers/demand.py`, `app/routers/orderbook.py`, `app/routers/price_discovery.py` +28 more
