"""Test the ProducerProject model attributes."""
import pytest
from app.models.producer import ProducerProject, ProjectStatus


class TestProducerProjectModel:
    def test_has_required_columns(self):
        assert hasattr(ProducerProject, 'name')
        assert hasattr(ProducerProject, 'fuel_type')
        assert hasattr(ProducerProject, 'capacity_kt_per_year')
        assert hasattr(ProducerProject, 'country')
        assert hasattr(ProducerProject, 'location')
        assert hasattr(ProducerProject, 'cod_date')
        assert hasattr(ProducerProject, 'status')
        assert hasattr(ProducerProject, 'data_source')
        assert hasattr(ProducerProject, 'organization_id')

    def test_project_status_values(self):
        assert ProjectStatus.ANNOUNCED.value == "ANNOUNCED"
        assert ProjectStatus.UNDER_CONSTRUCTION.value == "UNDER_CONSTRUCTION"
        assert ProjectStatus.OPERATIONAL.value == "OPERATIONAL"
        assert ProjectStatus.CANCELLED.value == "CANCELLED"
