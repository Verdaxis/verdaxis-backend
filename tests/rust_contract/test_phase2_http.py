"""Full FastAPI middleware/validation oracle without application mutations."""
import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client():
    from app.main import app
    # No lifespan or database request is needed for these pre-handler refusals.
    return TestClient(app, raise_server_exceptions=False)


@pytest.mark.parametrize("path", ["/api/watchlists/me", "/api/referrals/my-code"])
@pytest.mark.parametrize("slash", ["", "/"])
def test_known_get_refusal_is_before_authentication(client, path, slash):
    response = client.get(path + slash, follow_redirects=False)
    assert response.status_code == 405
    assert response.json()["detail"]["code"] == "STATE_CHANGING_GET_NOT_ALLOWED"


@pytest.mark.parametrize("path,method", [("/api/feedback", "POST"), ("/api/alerts", "POST"),
    ("/api/users/me/preferences/tutorial", "PUT"), ("/api/watchlists", "POST"), ("/api/referrals/invite", "POST")])
@pytest.mark.parametrize("support", ["context", " "])
def test_support_refusal_precedes_body_and_auth(client, path, method, support):
    response = client.request(method, path, content="{", headers={"content-type": "application/json",
        "authorization": "Bearer invalid", "x-verdaxis-market-support-context": support})
    assert response.status_code == 403
    assert response.json()["detail"]["code"] == "MARKET_SUPPORT_MUTATION_NOT_ALLOWED"


def test_malformed_json_precedes_bearer_but_schema_errors_follow_it(client):
    headers = {"content-type": "application/json", "authorization": "Bearer invalid"}
    syntax = client.post("/api/feedback", content="{", headers=headers)
    assert syntax.status_code == 422
    assert syntax.json()["detail"][0]["type"] == "json_invalid"
    schema = client.post("/api/feedback", json={"message": ""}, headers=headers)
    assert schema.status_code == 401
    assert schema.headers["www-authenticate"] == "Bearer"
    path = client.patch("/api/notifications/not-a-uuid/read", headers=headers)
    assert path.status_code == 401


def test_cors_wraps_early_refusals_and_untrusted_origin_has_no_allow_origin(client):
    from app.config import settings
    allowed = settings.BACKEND_CORS_ORIGINS[0]
    for origin in (allowed, "https://untrusted.invalid"):
        response = client.get("/api/watchlists/me", headers={"origin": origin})
        assert response.status_code == 405
        assert response.headers.get("access-control-allow-origin") == (origin if origin == allowed else None)


def test_preauth_admin_bucket_is_one_prefix_not_per_path(client):
    from app.middleware import preauth_rate_limit
    preauth_rate_limit._buckets.clear()
    for index in range(300):
        path = "/api/admin/feedback" if index % 2 else "/api/admin/unknown-python-path"
        response = client.get(path, headers={"authorization": "Bearer invalid"})
        assert response.status_code != 429
    response = client.get("/api/admin/feedback", headers={"authorization": "Bearer invalid"})
    assert response.status_code == 429
    assert response.headers["retry-after"] == "60"
    preauth_rate_limit._buckets.clear()
