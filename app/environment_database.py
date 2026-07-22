"""Config-free environment/database identity boundary."""

from __future__ import annotations

import re
from hmac import compare_digest
from urllib.parse import unquote, urlsplit


_ENVIRONMENT_DATABASES = {
    "production": "verdaxis",
    "staging": "verdaxis_staging",
}
_DISPOSABLE_TEST_DATABASE = re.compile(r"^[a-z][a-z0-9_]*_test$")


def database_name_from_url(database_url: str) -> str:
    parsed = urlsplit(database_url.strip())
    if not parsed.scheme.startswith("postgresql"):
        raise ValueError("database URL must use PostgreSQL")
    database_name = unquote(parsed.path.removeprefix("/"))
    if not database_name or "/" in database_name:
        raise ValueError("database URL must contain one explicit database name")
    return database_name


def expected_database_for_environment(
    environment: str,
    *,
    url_database: str,
) -> str:
    normalized = environment.strip().lower()
    expected = _ENVIRONMENT_DATABASES.get(normalized)
    if expected is not None:
        if url_database != expected:
            raise ValueError(
                f"environment {normalized!r} requires database {expected!r}; "
                f"URL targets {url_database!r}"
            )
        return expected
    if normalized == "test":
        if not _DISPOSABLE_TEST_DATABASE.fullmatch(url_database):
            raise ValueError(
                "test environment requires an explicitly disposable database "
                "name ending in '_test'"
            )
        return url_database
    raise ValueError(f"unsupported environment {environment!r}")


def attestation_phrase(environment: str, database_name: str) -> str:
    return f"{environment.strip().lower()}:{database_name}"


def validate_database_target(
    *,
    environment: str,
    database_url: str,
    current_database: str,
    supplied: str | None = None,
) -> str:
    url_database = database_name_from_url(database_url)
    expected = expected_database_for_environment(
        environment,
        url_database=url_database,
    )
    if current_database != expected:
        raise ValueError(
            f"connected database {current_database!r} does not match "
            f"environment database {expected!r}"
        )
    if supplied is not None:
        required = attestation_phrase(environment, expected)
        if not compare_digest(supplied, required):
            raise ValueError(
                "environment/database attestation mismatch; supply exactly "
                f"{required!r}"
            )
    return expected
