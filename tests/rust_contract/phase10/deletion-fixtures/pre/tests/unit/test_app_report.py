"""Synthetic Python test that imports the application (fixture data only)."""
from app.routers.activity import router


def test_router_is_registered():
    assert router is not None
