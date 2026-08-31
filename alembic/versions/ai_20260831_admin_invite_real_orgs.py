"""allow admin invitations to insert real organizations

Revision ID: ai_20260831_invite_real_orgs
Revises: ae_20260811_approval_email
Create Date: 2026-08-31
"""

from __future__ import annotations

import sqlalchemy as sa
from alembic import op


revision = "ai_20260831_invite_real_orgs"
down_revision = "ae_20260811_approval_email"
branch_labels = None
depends_on = None


_DEMO_IDS = """
    '4da7b285-34ee-5443-9406-f96b4ed1a251',
    '0dbce576-2026-5925-ab66-674d505e98ad',
    '3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a',
    '277491df-cb0d-5f2d-a2cf-5746829c6da6',
    '3b302066-d65c-5c3e-8fcc-70b3da3bcafd',
    '79609f48-0a3e-560e-a1e1-63d90601d84a',
    '2c4e387e-de22-5adb-ad88-9274ba84ebe1',
    '612953c7-567a-58b3-bc42-ee817d2bbe74',
    '93ccda09-54b3-53ee-afc0-759d3048161f',
    '82426590-0963-5486-9b05-f81e97afe6ef',
    'acc3f20a-fe94-4463-9029-a55e35634eb7',
    'c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4',
    '7cc77115-0a9f-4ec4-8c74-05aa10050111',
    'd1e43e55-3fb0-4b5e-9f0b-93aa10050222'
"""

_TEST_IDS = """
    '9e63f7a1-0000-4000-8000-000000000001',
    '9e63f7a1-0000-4000-8000-000000000002',
    '9e63f7a1-0000-4000-8000-000000000003',
    '9e63f7a1-0000-4000-8000-000000000004',
    '9e63f7a1-0000-4000-8000-000000000011',
    '9e63f7a1-0000-4000-8000-000000000012',
    '9e63f7a1-0000-4000-8000-000000000013',
    '9e63f7a1-0000-4000-8000-000000000014'
"""


def _provenance_function(*, reject_real_insert: bool) -> str:
    real_guard = """
                IF NEW.provenance = 'REAL' THEN
                    RAISE EXCEPTION 'REAL organization provenance requires a later operator-only approval migration';
                ELSIF NEW.provenance = 'DEMO' AND NEW.id NOT IN (
    """ if reject_real_insert else """
                IF NEW.provenance = 'DEMO' AND NEW.id NOT IN (
    """
    return f"""
        CREATE OR REPLACE FUNCTION verdaxis_immutable_org_provenance()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF TG_OP = 'INSERT' THEN
                {real_guard}
                    SELECT value::uuid FROM unnest(ARRAY[{_DEMO_IDS}]) AS value
                ) THEN
                    RAISE EXCEPTION 'DEMO provenance is restricted to deterministic seed identities';
                ELSIF NEW.provenance = 'TEST' AND NEW.id NOT IN (
                    SELECT value::uuid FROM unnest(ARRAY[{_TEST_IDS}]) AS value
                ) THEN
                    RAISE EXCEPTION 'TEST provenance is restricted to deterministic test identities';
                ELSIF NEW.provenance = 'CANARY' THEN
                    RAISE EXCEPTION 'CANARY provenance requires an external security-owned registry';
                END IF;
            ELSIF OLD.provenance IS DISTINCT FROM NEW.provenance THEN
                IF NOT (
                    OLD.provenance = 'UNKNOWN'
                    AND NEW.provenance = 'REAL'
                    AND NEW.verification_status = 'APPROVED'
                    AND EXISTS (
                        SELECT 1 FROM organization_market_approvals
                        WHERE organization_id = NEW.id
                    )
                ) THEN
                    RAISE EXCEPTION 'organization provenance is immutable';
                END IF;
            END IF;
            RETURN NEW;
        END;
        $$
    """


def upgrade() -> None:
    op.execute(sa.text(_provenance_function(reject_real_insert=False)))


def downgrade() -> None:
    op.execute(sa.text(_provenance_function(reject_real_insert=True)))
