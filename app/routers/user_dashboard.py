from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from sqlalchemy.orm import selectinload
from uuid import UUID
from typing import Annotated

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User
from app.models.dashboard import Dashboard, DashboardWidget
from app.schemas.dashboard import (
    DashboardCreate,
    DashboardUpdate,
    DashboardResponse,
    WidgetCreate,
    WidgetUpdate,
    WidgetResponse,
)

router = APIRouter(prefix="/dashboards", tags=["dashboards"])


async def _get_widget_or_404(
    widget_id: UUID,
    dashboard_id: UUID,
    db: AsyncSession,
) -> DashboardWidget:
    """Fetch a widget belonging to the given dashboard. Raises 404 if not found."""
    result = await db.execute(
        select(DashboardWidget).where(
            DashboardWidget.id == widget_id,
            DashboardWidget.dashboard_id == dashboard_id,
        )
    )
    widget = result.scalar_one_or_none()
    if widget is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Widget not found")
    return widget


async def _get_user_dashboard(
    dashboard_id: UUID,
    current_user: User,
    db: AsyncSession,
    with_widgets: bool = False,
) -> Dashboard:
    """Fetch a dashboard belonging to the current user. Raises 404 for missing or other-user dashboards."""
    query = select(Dashboard).where(
        Dashboard.id == dashboard_id,
        Dashboard.user_id == current_user.id,
    )
    if with_widgets:
        query = query.options(selectinload(Dashboard.widgets))
    result = await db.execute(query)
    dashboard = result.scalar_one_or_none()
    if dashboard is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Dashboard not found")
    return dashboard


@router.post("", response_model=DashboardResponse, status_code=status.HTTP_201_CREATED)
async def create_dashboard(
    body: DashboardCreate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Create a new dashboard for the current user."""
    dashboard = Dashboard(
        user_id=current_user.id,
        name=body.name,
        layout={},
        is_default=False,
    )
    db.add(dashboard)
    await db.commit()
    await db.refresh(dashboard, attribute_names=["widgets"])
    return dashboard


@router.get("", response_model=list[DashboardResponse])
async def list_dashboards(
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """List all dashboards owned by the current user."""
    result = await db.execute(
        select(Dashboard)
        .options(selectinload(Dashboard.widgets))
        .where(Dashboard.user_id == current_user.id)
        .order_by(Dashboard.created_at.asc())
    )
    return result.scalars().all()


@router.get("/{dashboard_id}", response_model=DashboardResponse)
async def get_dashboard(
    dashboard_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Get a single dashboard with its widgets. Returns 404 for other users' dashboards."""
    return await _get_user_dashboard(dashboard_id, current_user, db, with_widgets=True)


@router.patch("/{dashboard_id}", response_model=DashboardResponse)
async def update_dashboard(
    dashboard_id: UUID,
    body: DashboardUpdate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Update a dashboard's name and/or layout."""
    dashboard = await _get_user_dashboard(dashboard_id, current_user, db, with_widgets=True)

    if body.name is not None:
        dashboard.name = body.name
    if body.layout is not None:
        dashboard.layout = body.layout

    await db.commit()
    await db.refresh(dashboard, attribute_names=["widgets"])
    return dashboard


@router.delete("/{dashboard_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_dashboard(
    dashboard_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Delete a dashboard. Cascade deletes all its widgets."""
    dashboard = await _get_user_dashboard(dashboard_id, current_user, db)
    await db.delete(dashboard)
    await db.commit()
    return None


@router.post("/{dashboard_id}/widgets", response_model=WidgetResponse, status_code=status.HTTP_201_CREATED)
async def add_widget(
    dashboard_id: UUID,
    body: WidgetCreate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Add a widget to a dashboard."""
    await _get_user_dashboard(dashboard_id, current_user, db)

    widget = DashboardWidget(
        dashboard_id=dashboard_id,
        widget_type=body.widget_type,
        config=body.config or {},
        position=body.position or {},
    )
    db.add(widget)
    await db.commit()
    await db.refresh(widget)
    return widget


@router.patch("/{dashboard_id}/widgets/{widget_id}", response_model=WidgetResponse)
async def update_widget(
    dashboard_id: UUID,
    widget_id: UUID,
    body: WidgetUpdate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Update a widget's config and/or position."""
    # Verify dashboard ownership
    await _get_user_dashboard(dashboard_id, current_user, db)
    widget = await _get_widget_or_404(widget_id, dashboard_id, db)

    if body.config is not None:
        widget.config = body.config
    if body.position is not None:
        widget.position = body.position

    await db.commit()
    await db.refresh(widget)
    return widget


@router.delete("/{dashboard_id}/widgets/{widget_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_widget(
    dashboard_id: UUID,
    widget_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Remove a widget from a dashboard."""
    # Verify dashboard ownership
    await _get_user_dashboard(dashboard_id, current_user, db)
    widget = await _get_widget_or_404(widget_id, dashboard_id, db)

    await db.delete(widget)
    await db.commit()
    return None
