from pydantic_settings import BaseSettings
from pydantic import model_validator
from typing import Optional

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
    
    # Helper to allow overriding the URL (e.g. for testing with sqlite)
    # The actual DATABASE_URL used by the app
    DATABASE_URL: Optional[str] = None

    @model_validator(mode='after')
    def assemble_db_connection(self) -> 'Settings':
        if self.DATABASE_URL is None:
            self.DATABASE_URL = f"postgresql+asyncpg://{self.DATABASE_USER}:{self.DATABASE_PASSWORD}@{self.DATABASE_HOST}:{self.DATABASE_PORT}/{self.DATABASE_NAME}"
        return self

    # Security
    # Authentik Removed
    
    # Dev/Test Auth Bypass
    ENABLE_AUTH_BYPASS: bool = False
    
    JWT_SECRET: str = "***REMOVED***" 
    JWT_ALGORITHM: str = "HS256" # Changed to HS256 for simple auth
    API_AUDIENCE: str = "verdaxis-client-id"

    # Gemini AI
    GEMINI_API_KEY: Optional[str] = None
    
    # CORS
    BACKEND_CORS_ORIGINS: list[str] = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://144.126.151.136:5173",
        "https://app.verdaxis.exchange"
    ]

    class Config:
        env_file = ".env"
        case_sensitive = True
        extra = "ignore"

settings = Settings()
