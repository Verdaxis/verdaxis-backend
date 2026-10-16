"""Keep rate-limit state independent across explicitly attested HTTP tests."""
import httpx
import pytest


@pytest.fixture(autouse=True)
def reset_disposable_rate_limits(request):
    target = getattr(request.config, "_verdaxis_disposable_target", None)
    if target is None:
        return
    response = httpx.post(
        target.base_url + "/.well-known/verdaxis-disposable-test/reset-limits",
        headers={"X-Disposable-Token": request.config.getoption("--disposable-target-token")},
        timeout=10,
    )
    assert response.status_code == 204, response.text
