import os
from decimal import Decimal
from pydantic_settings import BaseSettings
from pydantic import Field, SecretStr, model_validator, field_validator
from typing import Optional
from urllib.parse import urlparse

class Settings(BaseSettings):
    # Server
    PROJECT_NAME: str = "Verdaxis"
    API_V1_STR: str = "/api"

    # Database
    DATABASE_HOST: str = "localhost"
    DATABASE_PORT: int = 5432
    DATABASE_NAME: str = "verdaxis"
    DATABASE_USER: str = "postgres"
    DATABASE_PASSWORD: str = "postgres"
    DATABASE_URL: Optional[str] = None

    @model_validator(mode='after')
    def assemble_db_connection(self) -> 'Settings':
        if self.DATABASE_URL is None:
            self.DATABASE_URL = (
                f"postgresql+asyncpg://{self.DATABASE_USER}:{self.DATABASE_PASSWORD}"
                f"@{self.DATABASE_HOST}:{self.DATABASE_PORT}/{self.DATABASE_NAME}"
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

    @field_validator('JWT_SECRET')
    @classmethod
    def validate_jwt_secret(cls, v: str) -> str:
        if len(v) < 32:
            env = os.environ.get('ENVIRONMENT', 'production')
            if env != 'test':
                raise ValueError(
                    'JWT_SECRET must be at least 32 characters. '
                    'Generate one with: python3 -c "import secrets; print(secrets.token_hex(32))"'
                )
        return v

    @field_validator('ENABLE_AUTH_BYPASS')
    @classmethod
    def validate_auth_bypass(cls, v: bool) -> bool:
        env = os.environ.get('ENVIRONMENT', 'production')
        if v and env == 'production':
            raise ValueError('ENABLE_AUTH_BYPASS must be false in production')
        return v


    @field_validator('DATABASE_PASSWORD')
    @classmethod
    def validate_db_password(cls, v: str) -> str:
        env = os.environ.get('ENVIRONMENT', 'production')
        if v == 'postgres' and env == 'production':
            raise ValueError('DATABASE_PASSWORD must not be "postgres" in production')
        return v

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
    BACKEND_CORS_ORIGINS: list[str] = [
        "http://localhost:5173",
        "http://localhost:5174",
        "http://127.0.0.1:5173",
        "http://127.0.0.1:5174",
        "https://app.verdaxis.exchange",
        "https://verdaxis-frontend.vercel.app",
        "https://staging.verdaxis.exchange",
    ]

    class Config:
        env_file = ".env"
        case_sensitive = True
        extra = "ignore"

settings = Settings()
