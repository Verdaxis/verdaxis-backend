"""The market admission seam must be the real security admission dependency.

The market source branch shipped a fail-closed 409 stub here. On the
integration tree the stub is replaced by a thin re-export; forking or
re-stubbing it would silently drop the admin-cannot-execute + approval gate
from every market router that depends on it. The RFQ and negotiation routers
behind the seam are indicative, so the KYC gate leaves them on the advisory
admission policy.
"""
from app.middleware.execution import require_market_admitted_user
from app.services.security_market_admission import require_security_market_admission


def test_market_admission_seam_is_the_security_admission_dependency():
    assert require_security_market_admission is require_market_admitted_user
