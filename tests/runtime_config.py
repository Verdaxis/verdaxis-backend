"""Fail-closed configuration for suites that call a running API."""

from __future__ import annotations

from ipaddress import ip_address
from urllib.parse import urlsplit


REMOTE_MUTATION_OPT_IN = "ALLOW_REMOTE_TEST_MUTATIONS"
REMOTE_MUTATION_OPT_IN_VALUE = "I_UNDERSTAND_REMOTE_TEST_MUTATIONS"
_LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1"}
_PRODUCTION_HOSTS = {"verdaxis.exchange", "www.verdaxis.exchange", "api.verdaxis.exchange"}


class RuntimeTestConfigurationError(RuntimeError):
    """Raised when an API test target is absent or unsafe."""


def _is_local_host(hostname: str) -> bool:
    if hostname in _LOCAL_HOSTS:
        return True
    try:
        return ip_address(hostname).is_loopback
    except ValueError:
        return False


def resolve_test_api_url(
    environ: dict[str, str], *, require_mutation_opt_in: bool = False
) -> str:
    """Return a validated explicit target for an API integration suite."""
    value = environ.get("TEST_API_URL", "").strip()
    if not value:
        raise RuntimeTestConfigurationError(
            "TEST_API_URL must be set explicitly; refusing an implicit API target"
        )

    parsed = urlsplit(value)
    hostname = (parsed.hostname or "").lower().rstrip(".")
    if parsed.scheme not in {"http", "https"} or not hostname or parsed.username:
        raise RuntimeTestConfigurationError("TEST_API_URL must be an absolute HTTP(S) URL without credentials")
    if hostname in _PRODUCTION_HOSTS:
        raise RuntimeTestConfigurationError(
            "refusing production verdaxis.exchange host in integration tests"
        )
    if require_mutation_opt_in and not _is_local_host(hostname):
        if environ.get(REMOTE_MUTATION_OPT_IN) != REMOTE_MUTATION_OPT_IN_VALUE:
            raise RuntimeTestConfigurationError(
                f"{REMOTE_MUTATION_OPT_IN}={REMOTE_MUTATION_OPT_IN_VALUE} is required "
                "for mutating remote integration tests"
            )
    return value.rstrip("/")
