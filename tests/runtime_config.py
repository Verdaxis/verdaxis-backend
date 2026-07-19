"""Fail-closed configuration for suites that call a running API."""

from __future__ import annotations

from ipaddress import ip_address
import re
from urllib.parse import urlsplit


MUTATION_OPT_IN = "ALLOW_TEST_MUTATIONS"
MUTATION_OPT_IN_VALUE = "I_UNDERSTAND_TEST_MUTATIONS"
RUNTIME_ENV_ATTESTATION = "TEST_RUNTIME_ENV"
DISPOSABLE_DB_NAME = "TEST_DISPOSABLE_DB_NAME"
_LOCAL_HOSTS = {"localhost", "127.0.0.1", "::1"}
_STAGING_HOSTS = {"api-staging.verdaxis.exchange"}
_PRODUCTION_HOSTS = {"verdaxis.exchange", "www.verdaxis.exchange", "api.verdaxis.exchange"}
_PRODUCTION_IPS = {"144.126.151.136"}
_LIVE_LOOPBACK_PORTS = {8000, 8001}
_DISPOSABLE_DB_NAME = re.compile(r"^[a-z0-9][a-z0-9_-]*_test$")


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
    """Return a validated explicit target for an API integration suite.

    Mutating suites require both a deliberate acknowledgement and a positive
    declaration of the runtime environment. The target class is checked too,
    so an environment variable cannot turn an arbitrary URL into a safe one.
    """
    value = environ.get("TEST_API_URL", "").strip()
    if not value:
        raise RuntimeTestConfigurationError(
            "TEST_API_URL must be set explicitly; refusing an implicit API target"
        )

    parsed = urlsplit(value)
    hostname = (parsed.hostname or "").lower()
    if (
        parsed.scheme not in {"http", "https"}
        or not hostname
        or parsed.username
        or parsed.password
        or parsed.path
        or parsed.query
        or parsed.fragment
    ):
        raise RuntimeTestConfigurationError("TEST_API_URL must be an absolute HTTP(S) URL without credentials")
    try:
        target_ip = ip_address(hostname)
    except ValueError:
        target_ip = None
    canonical_hostname = hostname.rstrip(".")
    if canonical_hostname in _PRODUCTION_HOSTS or hostname in _PRODUCTION_IPS or (
        target_ip is not None and str(target_ip) in _PRODUCTION_IPS
    ):
        raise RuntimeTestConfigurationError(
            "refusing the Verdaxis production URL/IP in integration tests"
        )
    try:
        target_port = parsed.port
    except ValueError as exc:
        raise RuntimeTestConfigurationError(
            "TEST_API_URL must use a valid TCP port"
        ) from exc
    if require_mutation_opt_in:
        if environ.get(MUTATION_OPT_IN) != MUTATION_OPT_IN_VALUE:
            raise RuntimeTestConfigurationError(
                f"{MUTATION_OPT_IN}={MUTATION_OPT_IN_VALUE} is required for mutating tests"
            )
        runtime_env = environ.get(RUNTIME_ENV_ATTESTATION, "").strip().lower()
        if runtime_env not in {"staging", "disposable"}:
            raise RuntimeTestConfigurationError(
                f"{RUNTIME_ENV_ATTESTATION} must positively attest staging or disposable"
            )
        if (
            _is_local_host(hostname)
            and target_port in _LIVE_LOOPBACK_PORTS
            and not (runtime_env == "staging" and hostname == "127.0.0.1" and target_port == 8001)
        ):
            raise RuntimeTestConfigurationError(
                "refusing live Verdaxis loopback ports 8000 and 8001"
            )
        if runtime_env == "staging":
            is_public_staging = (
                parsed.scheme == "https"
                and hostname in _STAGING_HOSTS
                and (target_port or 443) == 443
            )
            is_loopback_staging = (
                parsed.scheme == "http"
                and hostname == "127.0.0.1"
                and target_port == 8001
            )
            if not (is_public_staging or is_loopback_staging):
                raise RuntimeTestConfigurationError(
                    "staging mutation tests require exactly the approved public HTTPS target "
                    "or documented 127.0.0.1:8001 loopback target"
                )
        if runtime_env == "disposable":
            if parsed.scheme != "http" or hostname != "127.0.0.1" or target_port is None:
                raise RuntimeTestConfigurationError(
                    "disposable mutation tests require an explicit 127.0.0.1 TCP port"
                )
            db_name = environ.get(DISPOSABLE_DB_NAME, "").strip().lower()
            if not _DISPOSABLE_DB_NAME.fullmatch(db_name):
                raise RuntimeTestConfigurationError(
                    f"{DISPOSABLE_DB_NAME} must prove a disposable database ending in _test"
                )
    return value
