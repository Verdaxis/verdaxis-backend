"""Market-support router assembly.

Focused modules register routes on the shared admin/customer routers without
introducing an ambient acting-organization context.
"""
from app.routers.market_support_common import admin_router, customer_router
from app.routers import market_support_capabilities as _capabilities  # noqa: F401
from app.routers import market_support_authorizations as _authorizations  # noqa: F401
from app.routers import market_support_listing_create as _listing_create  # noqa: F401
from app.routers import market_support_listing_manage as _listing_manage  # noqa: F401

__all__ = ["admin_router", "customer_router"]
