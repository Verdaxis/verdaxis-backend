"""Test producer Pydantic schemas."""
import pytest
from decimal import Decimal
from uuid import uuid4
from datetime import datetime, date

from app.schemas.producer import ProducerProjectResponse, ProducerProjectCreate


class TestProducerProjectCreate:
    def test_valid_create(self):
        p = ProducerProjectCreate(
            name="Jurong Green Methanol",
            fuel_type="Methanol",
            capacity_kt_per_year=Decimal("500"),
            country="Singapore",
            lat=1.29,
            lng=103.85,
            cod_year=2027,
            status="ANNOUNCED",
            data_source="GENA",
        )
        assert p.name == "Jurong Green Methanol"
        assert p.fuel_type == "Methanol"


class TestProducerProjectResponse:
    def test_response_shape(self):
        p = ProducerProjectResponse(
            id=uuid4(),
            name="Test Project",
            fuel_type="Ethanol",
            capacity_kt_per_year=Decimal("200"),
            country="Brazil",
            region="South America",
            lat=-23.55,
            lng=-46.63,
            cod_date=date(2027, 6, 1),
            cod_year=2027,
            status="UNDER_CONSTRUCTION",
            data_source="GENA",
            feedstock="Sugarcane",
            technology="Fermentation",
            carbon_intensity_gco2_mj=Decimal("25.0"),
            created_at=datetime.utcnow(),
        )
        assert p.fuel_type == "Ethanol"
        assert p.cod_year == 2027
