"""add is_anonymous to trades

Revision ID: anon_trade_2026_03
Revises: pw_reset_2026_03
Create Date: 2026-03-12 00:00:00.000000

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "anon_trade_2026_03"
down_revision: Union[str, None] = "pw_reset_2026_03"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "trades",
        sa.Column("is_anonymous", sa.Boolean(), nullable=False, server_default="false"),
    )


def downgrade() -> None:
    op.drop_column("trades", "is_anonymous")
