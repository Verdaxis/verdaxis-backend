# Referral Links Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let any verified Verdaxis user create shareable referral links, track signups, and compete on a platform-wide leaderboard.

**Architecture:** Add `referral_code` + `referred_by_id` to User model, new `referrals` table for the relationship graph. New referral router with 5 endpoints. Hook into existing email verification and trade confirmation flows for status progression. Frontend: invite landing page + referrals tab in Settings + leaderboard.

**Tech Stack:** FastAPI, SQLAlchemy async (PostgreSQL), Alembic, Resend email, React + TypeScript + Tailwind

---

### Task 1: Referral Model + Alembic Migration

**Files:**
- Create: `app/models/referral.py`
- Modify: `app/models/__init__.py`
- Modify: `app/models/user.py`
- Create: `alembic/versions/ref_2026_03_add_referrals.py`
- Test: `tests/unit/test_referral_model.py`

**Step 1: Write the failing test**

Create `tests/unit/test_referral_model.py`:

```python
"""Unit tests for referral model and code generation."""
import pytest
from app.models.referral import Referral, ReferralStatus, generate_referral_code


class TestReferralCode:
    def test_generate_code_format(self):
        code = generate_referral_code()
        assert code.startswith("VDX-")
        assert len(code) == 10  # "VDX-" + 6 chars

    def test_generate_code_alphanumeric(self):
        code = generate_referral_code()
        suffix = code[4:]  # after "VDX-"
        assert suffix.isalnum()
        assert suffix == suffix.upper()

    def test_generate_code_unique(self):
        codes = {generate_referral_code() for _ in range(100)}
        assert len(codes) == 100  # all unique


class TestReferralStatus:
    def test_status_values(self):
        assert ReferralStatus.SIGNED_UP == "SIGNED_UP"
        assert ReferralStatus.VERIFIED == "VERIFIED"
        assert ReferralStatus.ACTIVE == "ACTIVE"


class TestReferralModel:
    def test_referral_has_required_columns(self):
        """Verify the model has all expected columns."""
        from sqlalchemy import inspect
        mapper = inspect(Referral)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "referrer_id", "referred_user_id",
            "referral_code_used", "status",
            "created_at", "verified_at", "activated_at",
        }
        assert expected.issubset(columns)
```

**Step 2: Run test to verify it fails**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_model.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'app.models.referral'`

**Step 3: Create the referral model**

Create `app/models/referral.py`:

```python
"""Referral tracking model."""
import enum
import uuid
import string
import secrets
from datetime import datetime, UTC

from sqlalchemy import ForeignKey, Enum, String, DateTime, UniqueConstraint
from sqlalchemy.dialects.postgresql import UUID
from sqlalchemy.orm import Mapped, mapped_column, relationship

from app.database import Base


class ReferralStatus(str, enum.Enum):
    SIGNED_UP = "SIGNED_UP"
    VERIFIED = "VERIFIED"
    ACTIVE = "ACTIVE"


def generate_referral_code() -> str:
    """Generate a unique referral code like VDX-7K3MX9."""
    alphabet = string.ascii_uppercase + string.digits
    suffix = "".join(secrets.choice(alphabet) for _ in range(6))
    return f"VDX-{suffix}"


class Referral(Base):
    __tablename__ = "referrals"

    id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), primary_key=True, default=uuid.uuid4
    )
    referrer_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=False
    )
    referred_user_id: Mapped[uuid.UUID] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=False, unique=True
    )
    referral_code_used: Mapped[str] = mapped_column(String, nullable=False)
    status: Mapped[ReferralStatus] = mapped_column(
        Enum(ReferralStatus, native_enum=False),
        default=ReferralStatus.SIGNED_UP,
        nullable=False,
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=lambda: datetime.now(UTC)
    )
    verified_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    activated_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    referrer: Mapped["User"] = relationship(
        foreign_keys=[referrer_id], back_populates="referrals_made"
    )
    referred_user: Mapped["User"] = relationship(
        foreign_keys=[referred_user_id], back_populates="referral_received"
    )
```

**Step 4: Add User model fields**

Modify `app/models/user.py` — add these columns to the User class (after the existing `password_reset_expires` field):

```python
    # Referrals
    referral_code: Mapped[str | None] = mapped_column(String(10), unique=True, nullable=True)
    referred_by_id: Mapped[uuid.UUID | None] = mapped_column(
        UUID(as_uuid=True), ForeignKey("users.id"), nullable=True
    )
```

And add these relationships (after the existing `organization` relationship):

```python
    referrals_made: Mapped[list["Referral"]] = relationship(
        foreign_keys="Referral.referrer_id", back_populates="referrer"
    )
    referral_received: Mapped["Referral | None"] = relationship(
        foreign_keys="Referral.referred_user_id", back_populates="referred_user", uselist=False
    )
```

Add the import at the top of `user.py`:
```python
from typing import Optional, TYPE_CHECKING
if TYPE_CHECKING:
    from app.models.referral import Referral
```

**Step 5: Update models __init__.py**

Add to `app/models/__init__.py`:

```python
from app.models.referral import Referral, ReferralStatus
```

**Step 6: Run tests to verify they pass**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_model.py -v`
Expected: 4 PASSED

**Step 7: Create Alembic migration**

Create `alembic/versions/ref_2026_03_add_referrals.py`:

```python
"""add referral_code to users and referrals table

Revision ID: ref_2026_03
Revises: sub_2026_03
Create Date: 2026-03-14
"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import UUID

revision = "ref_2026_03"
down_revision = "sub_2026_03"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Add referral columns to users
    op.add_column("users", sa.Column("referral_code", sa.String(10), nullable=True))
    op.add_column(
        "users",
        sa.Column("referred_by_id", UUID(as_uuid=True), sa.ForeignKey("users.id"), nullable=True),
    )
    op.create_unique_constraint("uq_users_referral_code", "users", ["referral_code"])
    op.create_index("ix_users_referral_code", "users", ["referral_code"])

    # Create referrals table
    op.create_table(
        "referrals",
        sa.Column("id", UUID(as_uuid=True), nullable=False),
        sa.Column("referrer_id", UUID(as_uuid=True), nullable=False),
        sa.Column("referred_user_id", UUID(as_uuid=True), nullable=False),
        sa.Column("referral_code_used", sa.String(), nullable=False),
        sa.Column("status", sa.String(), nullable=False, server_default="SIGNED_UP"),
        sa.Column(
            "created_at", sa.DateTime(timezone=True),
            server_default=sa.text("now()"), nullable=False,
        ),
        sa.Column("verified_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("activated_at", sa.DateTime(timezone=True), nullable=True),
        sa.ForeignKeyConstraint(["referrer_id"], ["users.id"]),
        sa.ForeignKeyConstraint(["referred_user_id"], ["users.id"]),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("referred_user_id", name="uq_referrals_referred_user"),
    )
    op.create_index("ix_referrals_referrer_id", "referrals", ["referrer_id"])


def downgrade() -> None:
    op.drop_index("ix_referrals_referrer_id", table_name="referrals")
    op.drop_table("referrals")
    op.drop_index("ix_users_referral_code", table_name="users")
    op.drop_constraint("uq_users_referral_code", "users", type_="unique")
    op.drop_column("users", "referred_by_id")
    op.drop_column("users", "referral_code")
```

**Step 8: Run migration**

Run: `cd /home/verdaxis-prod/verdaxis-backend && alembic upgrade head`
Expected: `INFO  [alembic.runtime.migration] Running upgrade sub_2026_03 -> ref_2026_03, add referral_code to users and referrals table`

**Step 9: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/models/referral.py app/models/user.py app/models/__init__.py \
    alembic/versions/ref_2026_03_add_referrals.py tests/unit/test_referral_model.py
git commit -m "feat: referral model, user fields, and Alembic migration"
```

---

### Task 2: Referral Schemas

**Files:**
- Create: `app/schemas/referral.py`
- Test: `tests/unit/test_referral_schemas.py`

**Step 1: Write the failing test**

Create `tests/unit/test_referral_schemas.py`:

```python
"""Unit tests for referral Pydantic schemas."""
import pytest
from uuid import uuid4
from datetime import datetime, UTC
from pydantic import ValidationError

from app.schemas.referral import (
    ReferralCodeResponse,
    ReferralInviteRequest,
    ReferralListItem,
    LeaderboardEntry,
    ResolveCodeResponse,
)


class TestReferralInviteRequest:
    def test_valid_email(self):
        req = ReferralInviteRequest(email="test@example.com")
        assert req.email == "test@example.com"

    def test_invalid_email_rejected(self):
        with pytest.raises(ValidationError):
            ReferralInviteRequest(email="not-an-email")


class TestReferralCodeResponse:
    def test_has_code_and_link(self):
        resp = ReferralCodeResponse(referral_code="VDX-ABC123", referral_link="https://app.verdaxis.exchange/invite/VDX-ABC123")
        assert resp.referral_code == "VDX-ABC123"
        assert "invite" in resp.referral_link


class TestLeaderboardEntry:
    def test_fields(self):
        entry = LeaderboardEntry(
            rank=1,
            user_name="John Doe",
            organization_name="Acme Corp",
            referral_count=15,
        )
        assert entry.rank == 1
        assert entry.referral_count == 15
```

**Step 2: Run test to verify it fails**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_schemas.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'app.schemas.referral'`

**Step 3: Create schemas**

Create `app/schemas/referral.py`:

```python
"""Pydantic schemas for referral endpoints."""
from datetime import datetime
from typing import Optional
from uuid import UUID

from pydantic import BaseModel, EmailStr


class ReferralCodeResponse(BaseModel):
    referral_code: str
    referral_link: str


class ReferralInviteRequest(BaseModel):
    email: EmailStr


class ReferralListItem(BaseModel):
    organization_name: Optional[str] = None
    role: Optional[str] = None
    status: str
    signed_up_at: datetime

    class Config:
        from_attributes = True


class ReferralStatsResponse(BaseModel):
    total: int
    verified: int
    active: int
    referrals: list[ReferralListItem]


class LeaderboardEntry(BaseModel):
    rank: int
    user_name: str
    organization_name: Optional[str] = None
    referral_count: int


class ResolveCodeResponse(BaseModel):
    valid: bool
    organization_name: Optional[str] = None
    organization_type: Optional[str] = None
    referrer_name: Optional[str] = None
```

**Step 4: Run tests to verify they pass**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_schemas.py -v`
Expected: 4 PASSED

**Step 5: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/schemas/referral.py tests/unit/test_referral_schemas.py
git commit -m "feat: referral Pydantic schemas"
```

---

### Task 3: Referral Router (5 Endpoints)

**Files:**
- Create: `app/routers/referrals.py`
- Modify: `app/main.py` (add router include)
- Test: `tests/unit/test_referral_endpoints.py`

**Step 1: Write the failing test**

Create `tests/unit/test_referral_endpoints.py`:

```python
"""Unit tests for referral router logic — no DB required."""
import pytest
import uuid
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import datetime, UTC

from app.models.referral import generate_referral_code


class TestGenerateReferralCode:
    def test_format(self):
        code = generate_referral_code()
        assert code.startswith("VDX-")
        assert len(code) == 10

    def test_uniqueness(self):
        codes = {generate_referral_code() for _ in range(200)}
        assert len(codes) == 200


class TestResolveCodeLogic:
    """Tests for the public resolve endpoint logic."""

    def test_invalid_code_format(self):
        """Codes must be VDX- prefix + 6 alphanumeric."""
        from app.routers.referrals import _validate_code_format
        assert _validate_code_format("VDX-ABC123") is True
        assert _validate_code_format("VDX-abc123") is False  # lowercase
        assert _validate_code_format("ABC123") is False  # no prefix
        assert _validate_code_format("VDX-AB") is False  # too short
        assert _validate_code_format("") is False
```

**Step 2: Run test to verify it fails**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_endpoints.py -v`
Expected: FAIL with `ModuleNotFoundError: No module named 'app.routers.referrals'`

**Step 3: Create the referral router**

Create `app/routers/referrals.py`:

```python
"""Referral link endpoints — generate, share, track, leaderboard."""
import re
from datetime import datetime, UTC
from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Request as _Request
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.referral import Referral, ReferralStatus, generate_referral_code
from app.models.user import User, Organization
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.config import settings
from app.schemas.referral import (
    ReferralCodeResponse,
    ReferralInviteRequest,
    ReferralListItem,
    ReferralStatsResponse,
    LeaderboardEntry,
    ResolveCodeResponse,
)

router = APIRouter(prefix="/referrals", tags=["referrals"])

_CODE_PATTERN = re.compile(r"^VDX-[A-Z0-9]{6}$")


def _validate_code_format(code: str) -> bool:
    """Check if a referral code matches the expected format."""
    return bool(_CODE_PATTERN.match(code))


@router.get("/my-code", response_model=ReferralCodeResponse)
async def get_my_referral_code(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Get the current user's referral code. Generates one if missing."""
    if not current_user.email_verified:
        raise HTTPException(status_code=403, detail="Email must be verified to get a referral code")

    if not current_user.referral_code:
        # Generate and persist a unique code
        for _ in range(10):  # retry on collision
            code = generate_referral_code()
            existing = await db.execute(
                select(User.id).where(User.referral_code == code)
            )
            if not existing.scalar_one_or_none():
                current_user.referral_code = code
                await db.commit()
                break
        else:
            raise HTTPException(status_code=500, detail="Failed to generate unique code")

    return ReferralCodeResponse(
        referral_code=current_user.referral_code,
        referral_link=f"{settings.FRONTEND_URL}/invite/{current_user.referral_code}",
    )


@router.get("/my-referrals", response_model=ReferralStatsResponse)
async def get_my_referrals(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """List all users referred by the current user."""
    stmt = (
        select(Referral)
        .where(Referral.referrer_id == current_user.id)
        .order_by(Referral.created_at.desc())
    )
    result = await db.execute(stmt)
    referrals = result.scalars().all()

    items = []
    for ref in referrals:
        # Load referred user's org info
        user_stmt = (
            select(User)
            .options(selectinload(User.organization))
            .where(User.id == ref.referred_user_id)
        )
        user_result = await db.execute(user_stmt)
        referred_user = user_result.scalar_one_or_none()

        items.append(ReferralListItem(
            organization_name=referred_user.organization.name if referred_user and referred_user.organization else None,
            role=referred_user.role.value if referred_user and referred_user.role else None,
            status=ref.status.value,
            signed_up_at=ref.created_at,
        ))

    total = len(referrals)
    verified = sum(1 for r in referrals if r.status in (ReferralStatus.VERIFIED, ReferralStatus.ACTIVE))
    active = sum(1 for r in referrals if r.status == ReferralStatus.ACTIVE)

    return ReferralStatsResponse(
        total=total, verified=verified, active=active, referrals=items,
    )


@router.get("/leaderboard", response_model=list[LeaderboardEntry])
async def get_leaderboard(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Top 10 referrers platform-wide."""
    stmt = (
        select(
            Referral.referrer_id,
            func.count(Referral.id).label("ref_count"),
        )
        .group_by(Referral.referrer_id)
        .order_by(func.count(Referral.id).desc())
        .limit(10)
    )
    result = await db.execute(stmt)
    rows = result.all()

    entries = []
    for rank, (referrer_id, count) in enumerate(rows, 1):
        user_stmt = (
            select(User)
            .options(selectinload(User.organization))
            .where(User.id == referrer_id)
        )
        user_result = await db.execute(user_stmt)
        user = user_result.scalar_one_or_none()
        if not user:
            continue

        entries.append(LeaderboardEntry(
            rank=rank,
            user_name=f"{user.first_name or ''} {user.last_name or ''}".strip() or "Anonymous",
            organization_name=user.organization.name if user.organization else None,
            referral_count=count,
        ))

    return entries


@router.post("/invite")
@limiter.limit("10/hour")
async def send_referral_invite(
    request: _Request,
    body: ReferralInviteRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Send a referral invite email."""
    if not current_user.email_verified:
        raise HTTPException(status_code=403, detail="Email must be verified to send invites")

    # Check if email is already registered
    existing = await db.execute(
        select(User.id).where(User.email == body.email)
    )
    if existing.scalar_one_or_none():
        raise HTTPException(status_code=400, detail="This email is already registered on Verdaxis")

    # Self-referral check
    if body.email == current_user.email:
        raise HTTPException(status_code=400, detail="Cannot invite yourself")

    # Ensure user has a referral code
    if not current_user.referral_code:
        current_user.referral_code = generate_referral_code()
        await db.commit()

    from app.services.email import send_referral_invite_email
    sent = await send_referral_invite_email(
        to_email=body.email,
        referrer_name=f"{current_user.first_name or ''} {current_user.last_name or ''}".strip(),
        referral_code=current_user.referral_code,
    )

    if not sent:
        raise HTTPException(status_code=502, detail="Failed to send invite email")

    return {"message": "Invitation sent", "email": body.email}


@router.get("/resolve/{code}", response_model=ResolveCodeResponse)
async def resolve_referral_code(
    code: str,
    db: AsyncSession = Depends(get_db),
):
    """Public endpoint — validate a referral code and return referrer info."""
    if not _validate_code_format(code):
        return ResolveCodeResponse(valid=False)

    stmt = (
        select(User)
        .options(selectinload(User.organization))
        .where(User.referral_code == code)
    )
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user:
        return ResolveCodeResponse(valid=False)

    return ResolveCodeResponse(
        valid=True,
        organization_name=user.organization.name if user.organization else None,
        organization_type=user.organization.type.value if user.organization and user.organization.type else None,
        referrer_name=f"{user.first_name or ''} {user.last_name or ''}".strip() or None,
    )
```

**Step 4: Mount the router**

Add to `app/main.py` alongside other router includes:

```python
from app.routers.referrals import router as referrals_router
# ...
app.include_router(referrals_router, prefix=settings.API_V1_STR)
```

**Step 5: Run tests to verify they pass**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_endpoints.py -v`
Expected: 3 PASSED

**Step 6: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/routers/referrals.py app/main.py tests/unit/test_referral_endpoints.py
git commit -m "feat: referral router with 5 endpoints"
```

---

### Task 4: Registration + Verification + Trade Hooks

**Files:**
- Modify: `app/routers/auth_simple.py` (register + verify-email)
- Modify: `app/routers/trades.py` (trade confirmation)
- Test: `tests/unit/test_referral_hooks.py`

**Step 1: Write the failing test**

Create `tests/unit/test_referral_hooks.py`:

```python
"""Unit tests for referral attribution and status progression logic."""
import pytest
from app.models.referral import ReferralStatus


class TestReferralStatusProgression:
    def test_signed_up_to_verified(self):
        assert ReferralStatus.SIGNED_UP.value == "SIGNED_UP"
        assert ReferralStatus.VERIFIED.value == "VERIFIED"

    def test_verified_to_active(self):
        assert ReferralStatus.ACTIVE.value == "ACTIVE"

    def test_all_statuses(self):
        statuses = [s.value for s in ReferralStatus]
        assert statuses == ["SIGNED_UP", "VERIFIED", "ACTIVE"]
```

**Step 2: Run test to verify it passes** (this is a sanity check)

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_hooks.py -v`
Expected: 3 PASSED

**Step 3: Hook into registration**

Modify `app/routers/auth_simple.py` — in the `register` endpoint, after `db.add(new_user)` and before `await db.commit()`, add referral attribution.

Find the line `return RegistrationResponse(status="created", user=new_user)` in the `existing_org` branch (~line 275) and add before it:

```python
        # Referral attribution
        if hasattr(user_in, 'referral_code') and user_in.referral_code:
            from app.models.referral import Referral
            referrer_stmt = select(User).where(User.referral_code == user_in.referral_code)
            referrer_result = await db.execute(referrer_stmt)
            referrer = referrer_result.scalar_one_or_none()
            if referrer and referrer.id != new_user.id:
                new_user.referred_by_id = referrer.id
                db.add(Referral(
                    referrer_id=referrer.id,
                    referred_user_id=new_user.id,
                    referral_code_used=user_in.referral_code,
                ))
                await db.commit()
                await db.refresh(new_user)
```

Also add `referral_code: str | None = None` field to `UserCreate` schema in `app/schemas/user.py`.

Do the same in the `register_with_org` endpoint — after `db.add(new_user)` and before `await db.commit()`, add the same referral attribution block. The referral code comes from the expiring server-side pending-registration row addressed by the opaque one-time token; it is never stored in a registration JWT.

In the `register` endpoint's `requires_org` branch, add `referral_code` to `token_data`:

```python
        token_data = {
            # ... existing fields ...
            "referral_code": user_in.referral_code,
        }
```

**Step 4: Hook into email verification**

Modify the `verify_email` endpoint in `app/routers/auth_simple.py`. After `user.status = UserStatus.APPROVED`, add:

```python
    # Progress referral status if this user was referred
    if user.referred_by_id:
        from app.models.referral import Referral, ReferralStatus
        ref_stmt = select(Referral).where(Referral.referred_user_id == user.id)
        ref_result = await db.execute(ref_stmt)
        referral = ref_result.scalar_one_or_none()
        if referral and referral.status == ReferralStatus.SIGNED_UP:
            referral.status = ReferralStatus.VERIFIED
            referral.verified_at = datetime.now(UTC)
    
    # Generate referral code for the newly verified user
    from app.models.referral import generate_referral_code
    for _ in range(10):
        code = generate_referral_code()
        existing_code = await db.execute(select(User.id).where(User.referral_code == code))
        if not existing_code.scalar_one_or_none():
            user.referral_code = code
            break
```

**Step 5: Hook into trade confirmation**

Modify `app/routers/trades.py` — in the `confirm_trade` endpoint, after `trade.status = TradeStatus.CONFIRMED`, add:

```python
    # Progress referral to ACTIVE on first confirmed trade for both parties
    from app.models.referral import Referral, ReferralStatus
    for party_id in (trade.buyer_id, trade.seller_id):
        # Find users in the org who were referred
        party_users_stmt = select(User).where(User.organization_id == party_id)
        party_users_result = await db.execute(party_users_stmt)
        for party_user in party_users_result.scalars():
            if party_user.referred_by_id:
                ref_stmt = select(Referral).where(
                    Referral.referred_user_id == party_user.id,
                    Referral.status == ReferralStatus.VERIFIED,
                )
                ref_result = await db.execute(ref_stmt)
                referral = ref_result.scalar_one_or_none()
                if referral:
                    referral.status = ReferralStatus.ACTIVE
                    referral.activated_at = datetime.now(UTC)
```

**Step 6: Run all unit tests**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/ -v`
Expected: All PASSED

**Step 7: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/routers/auth_simple.py app/routers/trades.py app/schemas/user.py \
    tests/unit/test_referral_hooks.py
git commit -m "feat: referral hooks — registration attribution, verification, trade activation"
```

---

### Task 5: Referral Invite Email Template

**Files:**
- Modify: `app/services/email.py`
- Test: `tests/unit/test_referral_email.py`

**Step 1: Write the failing test**

Create `tests/unit/test_referral_email.py`:

```python
"""Unit tests for referral invite email."""
import pytest
from unittest.mock import patch, AsyncMock


class TestSendReferralInviteEmail:
    @pytest.mark.asyncio
    async def test_sends_email(self):
        with patch("app.services.email._send_email", new_callable=AsyncMock, return_value=True) as mock_send:
            from app.services.email import send_referral_invite_email
            result = await send_referral_invite_email(
                to_email="new@example.com",
                referrer_name="John Doe",
                referral_code="VDX-ABC123",
            )
            assert result is True
            mock_send.assert_called_once()
            call_args = mock_send.call_args
            assert call_args[1]["to_email"] == "new@example.com"
            assert "John Doe" in call_args[1]["subject"]
            assert "VDX-ABC123" in call_args[1]["html"]

    @pytest.mark.asyncio
    async def test_invite_link_in_html(self):
        with patch("app.services.email._send_email", new_callable=AsyncMock, return_value=True) as mock_send:
            from app.services.email import send_referral_invite_email
            await send_referral_invite_email(
                to_email="new@example.com",
                referrer_name="Jane",
                referral_code="VDX-XYZ789",
            )
            html = mock_send.call_args[1]["html"]
            assert "/invite/VDX-XYZ789" in html
```

**Step 2: Run test to verify it fails**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_email.py -v`
Expected: FAIL with `ImportError: cannot import name 'send_referral_invite_email'`

**Step 3: Add the email function**

Add to `app/services/email.py`:

```python
async def send_referral_invite_email(to_email: str, referrer_name: str, referral_code: str) -> bool:
    """Send a referral invite email with a link to the invite landing page."""
    invite_url = f"{settings.FRONTEND_URL}/invite/{referral_code}"
    html = f"""
    <div style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
      <h2 style="color: #1a1a2e;">{referrer_name} invited you to Verdaxis</h2>
      <p>You've been invited to join <strong>Verdaxis Exchange</strong> — the maritime fuel marketplace
         where buyers and suppliers trade bunker fuel transparently.</p>
      <p style="margin: 24px 0;">
        <a href="{invite_url}"
           style="background-color: #059669; color: white; padding: 12px 24px;
                  text-decoration: none; border-radius: 4px; font-weight: bold;">
          Accept Invitation
        </a>
      </p>
      <p style="color: #666; font-size: 14px;">
        Or copy this link: <a href="{invite_url}">{invite_url}</a>
      </p>
      <hr style="border: none; border-top: 1px solid #eee; margin: 24px 0;" />
      <p style="color: #999; font-size: 12px;">
        Verdaxis — Maritime Fuel Marketplace
      </p>
    </div>
    """
    return await _send_email(to_email=to_email, subject=f"{referrer_name} invited you to Verdaxis", html=html)
```

**Step 4: Run tests to verify they pass**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/test_referral_email.py -v`
Expected: 2 PASSED

**Step 5: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-backend
git add app/services/email.py tests/unit/test_referral_email.py
git commit -m "feat: referral invite email template via Resend"
```

---

### Task 6: Frontend — Invite Landing Page

**Files:**
- Create: `/home/verdaxis-prod/verdaxis-frontend/src/pages/InvitePage.tsx`
- Modify: `/home/verdaxis-prod/verdaxis-frontend/src/App.tsx` (add route)

**Step 1: Create the invite landing page**

Create `/home/verdaxis-prod/verdaxis-frontend/src/pages/InvitePage.tsx`:

```tsx
import { useEffect, useState } from 'react';
import { useParams, useNavigate } from 'react-router-dom';
import { API_URL } from '../services/config';

interface ResolveResponse {
  valid: boolean;
  organization_name?: string;
  organization_type?: string;
  referrer_name?: string;
}

export const InvitePage = () => {
  const { code } = useParams<{ code: string }>();
  const navigate = useNavigate();
  const [data, setData] = useState<ResolveResponse | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!code) {
      setData({ valid: false });
      setLoading(false);
      return;
    }
    fetch(`${API_URL}/referrals/resolve/${code}`)
      .then(r => r.json())
      .then(setData)
      .catch(() => setData({ valid: false }))
      .finally(() => setLoading(false));
  }, [code]);

  const handleJoin = () => {
    navigate(data?.valid ? `/register?ref=${code}` : '/register');
  };

  if (loading) {
    return (
      <div className="min-h-screen bg-slate-900 flex items-center justify-center">
        <div className="text-emerald-400 text-lg">Loading...</div>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-slate-900 flex items-center justify-center px-4">
      <div className="max-w-lg w-full text-center">
        <div className="mb-8">
          <h1 className="text-4xl font-bold text-white mb-2">Verdaxis Exchange</h1>
          <div className="h-1 w-16 bg-emerald-500 mx-auto rounded" />
        </div>

        {data?.valid ? (
          <>
            <p className="text-slate-300 text-lg mb-2">You've been invited by</p>
            <p className="text-white text-2xl font-semibold mb-1">
              {data.organization_name || data.referrer_name || 'a Verdaxis member'}
            </p>
            {data.organization_type && (
              <p className="text-emerald-400 text-sm mb-8 capitalize">
                {data.organization_type.replace(/_/g, ' ').toLowerCase()}
              </p>
            )}
            <p className="text-slate-400 mb-8">
              Join the maritime fuel marketplace where buyers and suppliers trade transparently.
            </p>
          </>
        ) : (
          <>
            <p className="text-slate-300 text-lg mb-4">
              Join the maritime fuel marketplace
            </p>
            <p className="text-slate-400 mb-8">
              Trade bunker fuel transparently with verified counterparties.
            </p>
          </>
        )}

        <button
          onClick={handleJoin}
          className="bg-emerald-600 hover:bg-emerald-500 text-white font-semibold py-3 px-8 rounded-lg transition-colors text-lg"
        >
          Create Account
        </button>

        <p className="text-slate-500 text-sm mt-6">
          Already have an account?{' '}
          <a href="/login" className="text-emerald-400 hover:text-emerald-300">
            Log in
          </a>
        </p>
      </div>
    </div>
  );
};
```

**Step 2: Add route to App.tsx**

Add to the routes in `App.tsx` (alongside other public routes like `/register`):

```tsx
import { InvitePage } from './pages/InvitePage';
// ...
<Route path="/invite/:code" element={<InvitePage />} />
```

**Step 3: Wire referral code from URL into RegisterPage**

Modify `/home/verdaxis-prod/verdaxis-frontend/src/pages/RegisterPage.tsx` — read `ref` from URL query params and include it in the registration API call:

```tsx
// At the top of the component:
const [searchParams] = useSearchParams();
const referralCode = searchParams.get('ref');

// In the registration fetch body, add:
body: JSON.stringify({
  ...existingFields,
  referral_code: referralCode || undefined,
}),
```

**Step 4: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-frontend
git add src/pages/InvitePage.tsx src/App.tsx src/pages/RegisterPage.tsx
git commit -m "feat: invite landing page + referral code on registration"
```

---

### Task 7: Frontend — Referrals Tab in Settings

**Files:**
- Create: `/home/verdaxis-prod/verdaxis-frontend/src/components/ReferralsTab.tsx`
- Modify: `/home/verdaxis-prod/verdaxis-frontend/src/components/Settings.tsx` (add tab)

**Step 1: Create the ReferralsTab component**

Create `/home/verdaxis-prod/verdaxis-frontend/src/components/ReferralsTab.tsx`:

```tsx
import { useEffect, useState } from 'react';
import { Copy, Check, Send, Trophy, Users, UserCheck, Zap } from 'lucide-react';
import { API_URL } from '../services/config';
import { useAuth } from '../contexts/AuthContext';

interface ReferralItem {
  organization_name: string | null;
  role: string | null;
  status: string;
  signed_up_at: string;
}

interface ReferralStats {
  total: number;
  verified: number;
  active: number;
  referrals: ReferralItem[];
}

interface LeaderboardEntry {
  rank: int;
  user_name: string;
  organization_name: string | null;
  referral_count: number;
}

const statusBadge = (status: string) => {
  const colors: Record<string, string> = {
    SIGNED_UP: 'bg-yellow-500/20 text-yellow-400',
    VERIFIED: 'bg-blue-500/20 text-blue-400',
    ACTIVE: 'bg-emerald-500/20 text-emerald-400',
  };
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${colors[status] || 'bg-slate-600 text-slate-300'}`}>
      {status}
    </span>
  );
};

export const ReferralsTab = () => {
  const { token, user } = useAuth();
  const [referralCode, setReferralCode] = useState('');
  const [referralLink, setReferralLink] = useState('');
  const [stats, setStats] = useState<ReferralStats | null>(null);
  const [leaderboard, setLeaderboard] = useState<LeaderboardEntry[]>([]);
  const [copied, setCopied] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteMsg, setInviteMsg] = useState<{ type: string; text: string } | null>(null);
  const [inviteLoading, setInviteLoading] = useState(false);

  const headers = { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` };

  useEffect(() => {
    // Fetch referral code
    fetch(`${API_URL}/referrals/my-code`, { headers })
      .then(r => r.json())
      .then(d => { setReferralCode(d.referral_code); setReferralLink(d.referral_link); })
      .catch(() => {});

    // Fetch stats
    fetch(`${API_URL}/referrals/my-referrals`, { headers })
      .then(r => r.json())
      .then(setStats)
      .catch(() => {});

    // Fetch leaderboard
    fetch(`${API_URL}/referrals/leaderboard`, { headers })
      .then(r => r.json())
      .then(setLeaderboard)
      .catch(() => {});
  }, []);

  const handleCopy = () => {
    navigator.clipboard.writeText(referralLink);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleInvite = async (e: React.FormEvent) => {
    e.preventDefault();
    setInviteMsg(null);
    setInviteLoading(true);
    try {
      const res = await fetch(`${API_URL}/referrals/invite`, {
        method: 'POST', headers, body: JSON.stringify({ email: inviteEmail }),
      });
      if (res.ok) {
        setInviteMsg({ type: 'success', text: 'Invitation sent!' });
        setInviteEmail('');
      } else {
        const err = await res.json().catch(() => null);
        setInviteMsg({ type: 'error', text: err?.detail || 'Failed to send invite' });
      }
    } catch {
      setInviteMsg({ type: 'error', text: 'Network error' });
    } finally {
      setInviteLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      {/* Referral Link */}
      <div className="bg-slate-800 rounded-lg p-5">
        <h3 className="text-white font-semibold mb-3">Your Referral Link</h3>
        <div className="flex gap-2 mb-4">
          <input readOnly value={referralLink} className="flex-1 bg-slate-700 text-slate-200 rounded px-3 py-2 text-sm" />
          <button onClick={handleCopy} className="bg-emerald-600 hover:bg-emerald-500 text-white px-3 py-2 rounded transition-colors">
            {copied ? <Check size={18} /> : <Copy size={18} />}
          </button>
        </div>

        {/* Invite by email */}
        <form onSubmit={handleInvite} className="flex gap-2">
          <input
            type="email" required placeholder="colleague@company.com" value={inviteEmail}
            onChange={e => setInviteEmail(e.target.value)}
            className="flex-1 bg-slate-700 text-slate-200 rounded px-3 py-2 text-sm placeholder-slate-400"
          />
          <button type="submit" disabled={inviteLoading}
            className="bg-slate-600 hover:bg-slate-500 text-white px-4 py-2 rounded transition-colors flex items-center gap-1 text-sm disabled:opacity-50">
            <Send size={14} /> Send
          </button>
        </form>
        {inviteMsg && (
          <p className={`mt-2 text-sm ${inviteMsg.type === 'success' ? 'text-emerald-400' : 'text-red-400'}`}>
            {inviteMsg.text}
          </p>
        )}
      </div>

      {/* Stats */}
      {stats && (
        <div className="grid grid-cols-3 gap-3">
          <div className="bg-slate-800 rounded-lg p-4 text-center">
            <Users size={20} className="text-slate-400 mx-auto mb-1" />
            <p className="text-2xl font-bold text-white">{stats.total}</p>
            <p className="text-xs text-slate-400">Total Referrals</p>
          </div>
          <div className="bg-slate-800 rounded-lg p-4 text-center">
            <UserCheck size={20} className="text-blue-400 mx-auto mb-1" />
            <p className="text-2xl font-bold text-white">{stats.verified}</p>
            <p className="text-xs text-slate-400">Verified</p>
          </div>
          <div className="bg-slate-800 rounded-lg p-4 text-center">
            <Zap size={20} className="text-emerald-400 mx-auto mb-1" />
            <p className="text-2xl font-bold text-white">{stats.active}</p>
            <p className="text-xs text-slate-400">Active</p>
          </div>
        </div>
      )}

      {/* Referrals Table */}
      {stats && stats.referrals.length > 0 && (
        <div className="bg-slate-800 rounded-lg overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-slate-700 text-slate-400">
                <th className="text-left px-4 py-3">Organization</th>
                <th className="text-left px-4 py-3">Role</th>
                <th className="text-left px-4 py-3">Status</th>
                <th className="text-left px-4 py-3">Signed Up</th>
              </tr>
            </thead>
            <tbody>
              {stats.referrals.map((r, i) => (
                <tr key={i} className="border-b border-slate-700/50 text-slate-300">
                  <td className="px-4 py-3">{r.organization_name || '—'}</td>
                  <td className="px-4 py-3 capitalize">{r.role?.toLowerCase() || '—'}</td>
                  <td className="px-4 py-3">{statusBadge(r.status)}</td>
                  <td className="px-4 py-3 text-slate-400">{new Date(r.signed_up_at).toLocaleDateString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Leaderboard */}
      {leaderboard.length > 0 && (
        <div className="bg-slate-800 rounded-lg p-5">
          <h3 className="text-white font-semibold mb-3 flex items-center gap-2">
            <Trophy size={18} className="text-amber-400" /> Top Referrers
          </h3>
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-slate-700 text-slate-400">
                <th className="text-left px-4 py-2 w-12">#</th>
                <th className="text-left px-4 py-2">Name</th>
                <th className="text-left px-4 py-2">Organization</th>
                <th className="text-right px-4 py-2">Referrals</th>
              </tr>
            </thead>
            <tbody>
              {leaderboard.map(entry => {
                const isMe = entry.user_name === `${(user as any)?.first_name || ''} ${(user as any)?.last_name || ''}`.trim();
                return (
                  <tr key={entry.rank} className={`border-b border-slate-700/50 ${isMe ? 'bg-emerald-500/10' : ''}`}>
                    <td className="px-4 py-2 text-slate-400 font-mono">{entry.rank}</td>
                    <td className="px-4 py-2 text-slate-200">{entry.user_name}{isMe && <span className="text-emerald-400 ml-1 text-xs">(you)</span>}</td>
                    <td className="px-4 py-2 text-slate-400">{entry.organization_name || '—'}</td>
                    <td className="px-4 py-2 text-right text-white font-semibold">{entry.referral_count}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
};
```

**Step 2: Add tab to Settings.tsx**

Modify `/home/verdaxis-prod/verdaxis-frontend/src/components/Settings.tsx`:

Add to imports:
```tsx
import { Share2 } from 'lucide-react';
import { ReferralsTab } from './ReferralsTab';
```

Update `SettingsTab` type:
```tsx
type SettingsTab = 'profile' | 'notifications' | 'security' | 'billing' | 'referrals';
```

Add to `tabConfig` array (after `billing`):
```tsx
{ key: 'referrals', label: 'Referrals', icon: <Share2 size={18} /> },
```

Add the tab content render (alongside other tab cases):
```tsx
{activeTab === 'referrals' && <ReferralsTab />}
```

**Step 3: Commit**

```bash
cd /home/verdaxis-prod/verdaxis-frontend
git add src/components/ReferralsTab.tsx src/components/Settings.tsx
git commit -m "feat: referrals tab in Settings — stats, table, leaderboard, invite"
```

---

### Task 8: Run Full Test Suite + Verify

**Step 1: Run backend unit tests**

Run: `cd /home/verdaxis-prod/verdaxis-backend && python -m pytest tests/unit/ -v`
Expected: All PASSED (existing + 9 new referral tests)

**Step 2: Run Alembic migration on production**

Run: `cd /home/verdaxis-prod/verdaxis-backend && alembic upgrade head`
Expected: Successfully applies `ref_2026_03`

**Step 3: Restart backend service**

Run: `sudo systemctl restart verdaxis-api`

**Step 4: Verify endpoints manually**

```bash
# Health check
curl -s https://api.verdaxis.exchange/api/referrals/resolve/VDX-INVALID | jq
# Expected: {"valid": false}

# Get referral code (authenticated)
curl -s -H "Authorization: Bearer $TOKEN" https://api.verdaxis.exchange/api/referrals/my-code | jq
# Expected: {"referral_code": "VDX-XXXXXX", "referral_link": "https://app.verdaxis.exchange/invite/VDX-XXXXXX"}
```

**Step 5: Build and deploy frontend**

Run: `cd /home/verdaxis-prod/verdaxis-frontend && npm run build`
Expected: Build succeeds

**Step 6: Commit any final fixes**

```bash
git add -A
git commit -m "chore: verify full referral system integration"
```
