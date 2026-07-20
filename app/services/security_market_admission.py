"""Integration seam: the market branch's admission dependency, now wired to
the real security-owned execution-eligibility predicate.

The market source branch deliberately did not duplicate account, KYC, or
organization-approval truth; it shipped a fail-closed 409 stub named
``require_security_market_admission``. Final integration replaces that stub
with a thin re-export of ``require_execution_eligible_user`` so market routers
that ``Depends(require_security_market_admission)`` enforce the genuine
admin-cannot-execute + KYC/approval eligibility gate without forking it.
"""
from app.middleware.execution import (
    require_execution_eligible_user as require_security_market_admission,
)

__all__ = ["require_security_market_admission"]
