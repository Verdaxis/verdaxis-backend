"""Configuration-free SQLAlchemy declarative metadata.

Alembic imports this module for graph and drift commands, so model metadata is
available without constructing runtime settings, engines, or secrets.
"""
from sqlalchemy.orm import DeclarativeBase


class Base(DeclarativeBase):
    pass
