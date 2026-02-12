"""
Public producer projects endpoint.
Serves producer project data for the producer map.
No authentication required for read access.
"""
from typing import Optional, List
from fastapi import APIRouter, Depends, Query
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from geoalchemy2 import Geometry, functions as geo_func

from app.database import get_db
from app.models.producer import ProducerProject, ProjectStatus
from app.schemas.producer import ProducerProjectResponse

router = APIRouter(prefix="/producers", tags=["producers"])


@router.get("", response_model=List[ProducerProjectResponse])
async def list_producer_projects(
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    country: Optional[str] = Query(None, description="Filter by country"),
    status: Optional[str] = Query(None, description="Filter by status"),
    cod_year_min: Optional[int] = Query(None, description="Min COD year"),
    cod_year_max: Optional[int] = Query(None, description="Max COD year"),
    db: AsyncSession = Depends(get_db),
):
    """
    Public: list producer projects for the map.
    Supports filtering by fuel type, country, status, and COD timeline.
    """
    stmt = select(
        ProducerProject,
        geo_func.ST_X(ProducerProject.location.cast(Geometry)).label("lng"),
        geo_func.ST_Y(ProducerProject.location.cast(Geometry)).label("lat"),
    )

    if fuel_type:
        stmt = stmt.where(ProducerProject.fuel_type.ilike(f"%{fuel_type}%"))
    if country:
        stmt = stmt.where(ProducerProject.country.ilike(f"%{country}%"))
    if status:
        stmt = stmt.where(ProducerProject.status == status)
    if cod_year_min:
        stmt = stmt.where(ProducerProject.cod_year >= cod_year_min)
    if cod_year_max:
        stmt = stmt.where(ProducerProject.cod_year <= cod_year_max)

    stmt = stmt.order_by(ProducerProject.cod_year.asc().nullslast(), ProducerProject.name)

    result = await db.execute(stmt)
    rows = result.all()

    projects = []
    for row in rows:
        project = row[0]
        project.lng = row[1]
        project.lat = row[2]
        project.location = None  # WKBElement not serializable
        projects.append(project)

    return projects
