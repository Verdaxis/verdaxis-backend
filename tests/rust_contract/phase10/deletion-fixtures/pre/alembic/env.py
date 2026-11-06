"""Synthetic Alembic environment (fixture data only; never run)."""
from alembic import context
from app.config import settings

config = context.config
config.set_main_option("sqlalchemy.url", str(settings.DATABASE_URL))
