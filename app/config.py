from decimal import Decimal
import re
from pydantic_settings import BaseSettings
from pydantic import Field, SecretStr, model_validator, field_validator
from typing import Optional
from urllib.parse import urlparse
from sqlalchemy.engine import make_url


_LOCAL_CORS_ORIGINS = (
    "http://localhost:5173",
    "http://localhost:5174",
    "http://127.0.0.1:5173",
    "http://127.0.0.1:5174",
)

_CORS_ORIGINS_BY_ENVIRONMENT = {
    "production": (
        "https://verdaxis.exchange",
        "https://app.verdaxis.exchange",
    ),
    "staging": ("https://staging.verdaxis.exchange",),
    "development": _LOCAL_CORS_ORIGINS,
    "test": _LOCAL_CORS_ORIGINS,
}

_DEPLOYED_DATABASE_IDENTITIES = {
    "production": {
        "database": "verdaxis",
        "app_role": "verdaxis_app",
        "migrator_role": "verdaxis_migrator",
    },
    "staging": {
        "database": "verdaxis_staging",
        "app_role": "verdaxis_app_staging",
        "migrator_role": "verdaxis_migrator_staging",
    },
}
_DEFAULT_JWT_SECRETS = {"", "change-me-in-production", "CHANGE_ME_MIN_32_CHARS"}
_DEFAULT_DATABASE_PASSWORDS = {"postgres", "change_me"}


def _database_password_is_placeholder(password: str | None) -> bool:
    normalized_password = (password or "").strip().lower()
    return not normalized_password or normalized_password in _DEFAULT_DATABASE_PASSWORDS


class Settings(BaseSettings):
    # Server
    PROJECT_NAME: str = "Verdaxis"
    API_V1_STR: str = "/api"
    ENVIRONMENT: str = "development"
    RELEASE_SHA: str = "development"
    UVICORN_WORKERS: int = Field(default=4, ge=1, le=100)
    HEALTH_READINESS_TIMEOUT_SECONDS: float = Field(default=2.0, gt=0, le=30)

    # Database
    DATABASE_HOST: str = "localhost"
    DATABASE_PORT: int = 5432
    DATABASE_NAME: str = "verdaxis_dev"
    DATABASE_USER: str = "verdaxis_app"
    DATABASE_PASSWORD: str = ""
    DATABASE_URL: Optional[str] = None

    # SQLAlchemy pool settings are per application worker.  The aggregate
    # validator below budgets production, staging, and a maintenance reserve
    # against the shared PostgreSQL max_connections.
    DB_POOL_SIZE: int = Field(default=2, ge=1, le=100)
    DB_MAX_OVERFLOW: int = Field(default=1, ge=0, le=100)
    DB_POOL_TIMEOUT: float = Field(default=30.0, gt=0, le=300)
    DB_POOL_RECYCLE: int = Field(default=1800, ge=0, le=86400)
    DB_SERVICE_COUNT: int = Field(default=2, ge=1, le=100)
    DB_MAX_CONNECTIONS: int = Field(default=100, ge=1, le=1000)
    DB_RESERVED_CONNECTIONS: int = Field(default=20, ge=0, le=999)
    DB_STATEMENT_TIMEOUT_MS: int = Field(default=30_000, gt=0, le=3_600_000)
    DB_LOCK_TIMEOUT_MS: int = Field(default=3_000, gt=0, le=600_000)
    DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS: int = Field(
        default=60_000, gt=0, le=3_600_000
    )
    MIGRATOR_DATABASE_URL: Optional[str] = None
    MIGRATOR_STATEMENT_TIMEOUT_MS: int = Field(default=300_000, gt=0, le=7_200_000)
    MIGRATOR_LOCK_TIMEOUT_MS: int = Field(default=30_000, gt=0, le=600_000)
    MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS: int = Field(
        default=300_000, gt=0, le=7_200_000
    )

    # KYC uploads are held in memory only while sent to the verifier. Keep
    # both individual and aggregate requests bounded before reading them.
    KYC_MAX_FILE_BYTES: int = Field(default=10 * 1024 * 1024, ge=1, le=100 * 1024 * 1024)
    KYC_MAX_TOTAL_BYTES: int = Field(default=20 * 1024 * 1024, ge=1, le=200 * 1024 * 1024)

    @model_validator(mode='after')
    def validate_db_pool_capacity(self) -> 'Settings':
        if self.DB_RESERVED_CONNECTIONS >= self.DB_MAX_CONNECTIONS:
            raise ValueError(
                'DB_RESERVED_CONNECTIONS must be less than DB_MAX_CONNECTIONS'
            )
        configured_connections = (
            self.DB_SERVICE_COUNT
            * self.UVICORN_WORKERS
            * (self.DB_POOL_SIZE + self.DB_MAX_OVERFLOW)
        )
        available_connections = self.DB_MAX_CONNECTIONS - self.DB_RESERVED_CONNECTIONS
        if configured_connections > available_connections:
            raise ValueError(
                'DB_SERVICE_COUNT * UVICORN_WORKERS * '
                '(DB_POOL_SIZE + DB_MAX_OVERFLOW) must be '
                'less than or equal to DB_MAX_CONNECTIONS - DB_RESERVED_CONNECTIONS'
            )
        return self

    @model_validator(mode='after')
    def validate_db_timeout_policies(self) -> 'Settings':
        if self.MIGRATOR_STATEMENT_TIMEOUT_MS < self.DB_STATEMENT_TIMEOUT_MS:
            raise ValueError(
                'MIGRATOR_STATEMENT_TIMEOUT_MS must be at least DB_STATEMENT_TIMEOUT_MS'
            )
        if self.MIGRATOR_LOCK_TIMEOUT_MS < self.DB_LOCK_TIMEOUT_MS:
            raise ValueError(
                'MIGRATOR_LOCK_TIMEOUT_MS must be at least DB_LOCK_TIMEOUT_MS'
            )
        if (
            self.MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS
            < self.DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS
        ):
            raise ValueError(
                'MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS must be at least '
                'DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS'
            )
        return self

    @model_validator(mode='after')
    def validate_kyc_upload_capacity(self) -> 'Settings':
        if self.KYC_MAX_TOTAL_BYTES < self.KYC_MAX_FILE_BYTES:
            raise ValueError('KYC_MAX_TOTAL_BYTES must be at least KYC_MAX_FILE_BYTES')
        return self

    @model_validator(mode='after')
    def assemble_db_connection(self) -> 'Settings':
        if self.DATABASE_URL is None:
            self.DATABASE_URL = (
                f"postgresql+asyncpg://{self.DATABASE_USER}:{self.DATABASE_PASSWORD}"
                f"@{self.DATABASE_HOST}:{self.DATABASE_PORT}/{self.DATABASE_NAME}"
            )
        return self

    @model_validator(mode="after")
    def validate_production_boundaries(self) -> "Settings":
        environment = self.ENVIRONMENT.strip().lower()
        if environment not in {"development", "test", "staging", "production"}:
            raise ValueError("ENVIRONMENT must be development, test, staging, or production")
        self.ENVIRONMENT = environment
        release_sha = self.RELEASE_SHA.strip().lower()
        full_release_sha = re.fullmatch(r"[0-9a-f]{40}", release_sha) is not None
        if environment in {"staging", "production"}:
            if not full_release_sha:
                raise ValueError(
                    "RELEASE_SHA must be a full 40-hex commit SHA in staging and production"
                )
        elif release_sha != environment and not full_release_sha:
            raise ValueError(
                f"RELEASE_SHA must be {environment!r} or a full 40-hex commit SHA"
            )
        self.RELEASE_SHA = release_sha

        migration_url = make_url(self.MIGRATOR_DATABASE_URL or self.DATABASE_URL)
        if migration_url.query:
            raise ValueError(
                "migration database URLs must not contain query parameters"
            )

        if environment in _DEPLOYED_DATABASE_IDENTITIES:
            identity = _DEPLOYED_DATABASE_IDENTITIES[environment]
            if len(self.JWT_SECRET) < 32 or self.JWT_SECRET in _DEFAULT_JWT_SECRETS:
                raise ValueError(
                    "JWT_SECRET must be a non-default value of at least 32 characters "
                    "in staging and production"
                )
            if self.ENABLE_AUTH_BYPASS:
                raise ValueError(
                    "ENABLE_AUTH_BYPASS must be false in staging and production"
                )
            if self.DB_SERVICE_COUNT != 2:
                raise ValueError(
                    "DB_SERVICE_COUNT must be exactly 2 for the deployed production "
                    "and staging topology"
                )
            if self.DB_RESERVED_CONNECTIONS < 20:
                raise ValueError(
                    "DB_RESERVED_CONNECTIONS must reserve at least 20 maintenance "
                    "connections in deployed environments"
                )

            app_url = make_url(self.DATABASE_URL)
            if not app_url.drivername.startswith("postgresql"):
                raise ValueError(
                    "DATABASE_URL must use PostgreSQL in staging and production"
                )
            if self.DATABASE_NAME != identity["database"]:
                raise ValueError(
                    f"{environment} database identity must be exactly "
                    f"{identity['database']}"
                )
            if self.DATABASE_USER != identity["app_role"]:
                raise ValueError(
                    f"{environment} application role must be exactly "
                    f"{identity['app_role']}"
                )
            if app_url.database != identity["database"]:
                raise ValueError(
                    f"effective DATABASE_URL database must be exactly "
                    f"{identity['database']} in {environment}"
                )
            if app_url.username != identity["app_role"]:
                raise ValueError(
                    f"effective DATABASE_URL application role must be exactly "
                    f"{identity['app_role']} in {environment}"
                )
            if app_url.query:
                raise ValueError(
                    "deployed database URLs must not contain query parameters"
                )
            if _database_password_is_placeholder(app_url.password):
                raise ValueError(
                    "DATABASE_URL password must be explicitly configured in deployed environments"
                )

            if not self.MIGRATOR_DATABASE_URL:
                raise ValueError(
                    "MIGRATOR_DATABASE_URL is required in staging and production"
                )
            if not migration_url.drivername.startswith("postgresql"):
                raise ValueError(
                    "MIGRATOR_DATABASE_URL must use PostgreSQL in staging and production"
                )
            if migration_url.database != identity["database"]:
                raise ValueError(
                    f"effective MIGRATOR_DATABASE_URL database must be exactly "
                    f"{identity['database']} in {environment}"
                )
            if migration_url.username != identity["migrator_role"]:
                raise ValueError(
                    f"effective MIGRATOR_DATABASE_URL migrator role must be exactly "
                    f"{identity['migrator_role']} in {environment}"
                )
            if app_url.username == migration_url.username:
                raise ValueError(
                    "runtime application and migrator database roles must be distinct"
                )
            if _database_password_is_placeholder(migration_url.password):
                raise ValueError(
                    "MIGRATOR_DATABASE_URL password must be explicitly configured in deployed environments"
                )
        return self

    # Security
    ENABLE_AUTH_BYPASS: bool = False

    # Admin UI credentials
    ADMIN_USERNAME: Optional[str] = None
    ADMIN_PASSWORD: Optional[str] = None
    ADMIN_SESSION_SECRET: Optional[str] = None

    # JWT
    JWT_SECRET: str = "change-me-in-production"
    JWT_ALGORITHM: str = "HS256"
    ACCESS_TOKEN_EXPIRE_MINUTES: int = 15  # Short-lived access tokens
    REFRESH_TOKEN_EXPIRE_DAYS: int = 7     # Long-lived refresh tokens
    API_AUDIENCE: str = "verdaxis-client-id"

    # Order Matching Engine
    AUTO_MATCHING_ENABLED: bool = True  # Set to False to disable match-on-insert

    # Compliance pricing overlay: EUR/USD conversion override (defaults to
    # the ASSUMED rate in app/services/compliance_pricing.py when unset)
    COMPLIANCE_EUR_USD_RATE: Optional[Decimal] = None

    # Gemini AI
    GEMINI_API_KEY: Optional[str] = None

    # Email (Resend)
    RESEND_API_KEY: Optional[str] = None
    EMAIL_FROM: str = "Verdaxis <noreply@verdaxis.exchange>"
    FRONTEND_URL: str = "https://app.verdaxis.exchange"

    # Internal monitoring
    MONITOR_TOKEN: Optional[str] = None

    # Optional behavioral analytics (Umami, server-side credentials only)
    ANALYTICS_ENABLED: bool = False
    UMAMI_BASE_URL: Optional[str] = None
    UMAMI_WEBSITE_ID: Optional[str] = None
    UMAMI_API_USERNAME: Optional[str] = None
    UMAMI_API_PASSWORD: Optional[SecretStr] = None
    ANALYTICS_REQUEST_TIMEOUT_SECONDS: float = Field(default=2.0, gt=0, le=10.0)

    @field_validator("UMAMI_BASE_URL")
    @classmethod
    def validate_umami_base_url(cls, value: Optional[str]) -> Optional[str]:
        if value is None or not value.strip():
            return None
        normalized = value.strip().rstrip("/")
        parsed = urlparse(normalized)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            raise ValueError("UMAMI_BASE_URL must be an absolute HTTP(S) URL")
        if parsed.username or parsed.password:
            raise ValueError("UMAMI_BASE_URL must not contain credentials")
        return normalized

    @field_validator("UMAMI_WEBSITE_ID", "UMAMI_API_USERNAME")
    @classmethod
    def normalize_optional_analytics_value(cls, value: Optional[str]) -> Optional[str]:
        if value is None or not value.strip():
            return None
        normalized = value.strip()
        if len(normalized) > 200:
            raise ValueError("Analytics configuration value is too long")
        return normalized

    # CORS
    BACKEND_CORS_ORIGINS: Optional[list[str]] = None

    @model_validator(mode="after")
    def validate_cors_origins(self) -> "Settings":
        allowed = _CORS_ORIGINS_BY_ENVIRONMENT[self.ENVIRONMENT]
        origins = list(allowed) if self.BACKEND_CORS_ORIGINS is None else self.BACKEND_CORS_ORIGINS
        for origin in origins:
            parsed = urlparse(origin)
            if (
                "*" in origin
                or parsed.scheme not in {"http", "https"}
                or not parsed.netloc
                or parsed.username
                or parsed.password
                or parsed.path not in {"", "/"}
                or parsed.params
                or parsed.query
                or parsed.fragment
                or origin.rstrip("/") not in allowed
            ):
                raise ValueError(
                    f"BACKEND_CORS_ORIGINS contains an origin incompatible with {self.ENVIRONMENT}"
                )
        self.BACKEND_CORS_ORIGINS = [origin.rstrip("/") for origin in origins]
        return self

    class Config:
        env_file = ".env"
        case_sensitive = True
        extra = "ignore"

settings = Settings()
