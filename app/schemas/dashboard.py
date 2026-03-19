from pydantic import BaseModel
from typing import Optional
from uuid import UUID
from datetime import datetime


class WidgetCreate(BaseModel):
    widget_type: str
    config: Optional[dict] = None
    position: Optional[dict] = None


class WidgetUpdate(BaseModel):
    config: Optional[dict] = None
    position: Optional[dict] = None


class WidgetResponse(BaseModel):
    id: UUID
    widget_type: str
    config: Optional[dict] = None
    position: Optional[dict] = None

    class Config:
        from_attributes = True


class DashboardCreate(BaseModel):
    name: str


class DashboardUpdate(BaseModel):
    name: Optional[str] = None
    layout: Optional[dict] = None


class DashboardResponse(BaseModel):
    id: UUID
    name: str
    layout: Optional[dict] = None
    is_default: bool
    widgets: list[WidgetResponse] = []
    created_at: datetime

    class Config:
        from_attributes = True
