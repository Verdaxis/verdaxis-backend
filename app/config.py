from pydantic_settings import BaseSettings
from typing import Optional

class Settings(BaseSettings):
    # Server
    PROJECT_NAME: str = "Verdaxis"
    API_V1_STR: str = "/api"
    
    # Database
    DATABASE_HOST: str
    DATABASE_PORT: int = 5432
    DATABASE_NAME: str
    DATABASE_USER: str
    DATABASE_PASSWORD: str
    
    @property
    def DATABASE_URL(self) -> str:
        return f"postgresql+asyncpg://{self.DATABASE_USER}:{self.DATABASE_PASSWORD}@{self.DATABASE_HOST}:{self.DATABASE_PORT}/{self.DATABASE_NAME}"

    # Security
    AUTHENTIK_DOMAIN: str = "http://localhost:9000"
    AUTHENTIK_CLIENT_ID: str = "verdaxis-client-id" # Will be updated after Authentik setup
    JWT_SECRET: str = "***REMOVED***" # Kept for local impersonation tokens
    JWT_ALGORITHM: str = "RS256"
    API_AUDIENCE: str = "verdaxis-client-id"

    # Gemini AI
    GEMINI_API_KEY: Optional[str] = None
    
    # CORS
    CORS_ORIGIN: str = "http://localhost:5173"

    class Config:
        env_file = ".env"
        case_sensitive = True
        extra = "ignore"

settings = Settings()
