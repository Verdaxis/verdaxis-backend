"""Integration seam: the market branch's admission dependency, wired to the
real security-owned market admission predicate.

The market source branch deliberately did not duplicate account, KYC, or
organization-approval truth; it shipped a fail-closed 409 stub named
``require_security_market_admission``. Final integration replaces that stub
with a thin re-export so market routers that
``Depends(require_security_market_admission)`` enforce the genuine
admin-cannot-execute + approval gate without forking it.

The routers behind this seam (RFQ and negotiation) are indicative and cannot
execute, so they keep the advisory KYC policy. The KYC execution rule applies
only to executable routes, which use ``require_execution_eligible_user``.
"""
from app.middleware.execution import (
    require_market_admitted_user as require_security_market_admission,
)

__all__ = ["require_security_market_admission"]
