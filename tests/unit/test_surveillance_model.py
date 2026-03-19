"""TDD tests for the SurveillanceEvent model and related schemas — STORY-S6-001."""
import uuid
from datetime import datetime, timezone


from app.models.surveillance import (
    SurveillanceEvent,
    SurveillanceSeverity,
    SurveillanceStatus,
    SurveillanceType,
)
from app.schemas.surveillance import (
    SurveillanceEventResponse,
    SurveillanceEventUpdate,
    SurveillanceSeverity as SchemaSeverity,
    SurveillanceStatus as SchemaStatus,
    SurveillanceType as SchemaType,
)


class TestSurveillanceTypeEnum:
    def test_enum_values(self):
        assert SurveillanceType.FRONT_RUNNING.value == "FRONT_RUNNING"
        assert SurveillanceType.SPOOFING.value == "SPOOFING"
        assert SurveillanceType.WASH_TRADING.value == "WASH_TRADING"
        assert SurveillanceType.LAYERING.value == "LAYERING"
        assert SurveillanceType.MARKING_THE_CLOSE.value == "MARKING_THE_CLOSE"

    def test_all_five_types_present(self):
        assert len(SurveillanceType) == 5


class TestSurveillanceSeverityEnum:
    def test_enum_values(self):
        assert SurveillanceSeverity.LOW.value == "LOW"
        assert SurveillanceSeverity.MEDIUM.value == "MEDIUM"
        assert SurveillanceSeverity.HIGH.value == "HIGH"
        assert SurveillanceSeverity.CRITICAL.value == "CRITICAL"

    def test_all_four_severities_present(self):
        assert len(SurveillanceSeverity) == 4


class TestSurveillanceStatusEnum:
    def test_enum_values(self):
        assert SurveillanceStatus.OPEN.value == "OPEN"
        assert SurveillanceStatus.REVIEWING.value == "REVIEWING"
        assert SurveillanceStatus.ACTIONED.value == "ACTIONED"
        assert SurveillanceStatus.CLOSED.value == "CLOSED"

    def test_all_four_statuses_present(self):
        assert len(SurveillanceStatus) == 4


class TestSurveillanceEventModel:
    def test_model_has_all_fields(self):
        for field in (
            "id",
            "type",
            "severity",
            "status",
            "participants",
            "related_orders",
            "related_trades",
            "description",
            "notes",
            "window_start",
            "window_end",
            "auto_detected",
            "created_at",
            "updated_at",
        ):
            assert hasattr(SurveillanceEvent, field), f"Missing field: {field}"

    def test_tablename(self):
        assert SurveillanceEvent.__tablename__ == "surveillance_events"

    def test_default_status_is_open(self):
        # Default status value on the column should be OPEN
        col = SurveillanceEvent.__table__.c["status"]
        assert col.default.arg == SurveillanceStatus.OPEN

    def test_auto_detected_default_true(self):
        col = SurveillanceEvent.__table__.c["auto_detected"]
        assert col.default.arg is True


class TestSurveillanceEventResponseSchema:
    def _make_event_dict(self, **overrides):
        base = {
            "id": uuid.uuid4(),
            "type": "SPOOFING",
            "severity": "HIGH",
            "status": "OPEN",
            "participants": ["user-uuid-1"],
            "related_orders": ["order-uuid-1"],
            "related_trades": [],
            "description": "Suspicious order pattern detected",
            "notes": None,
            "window_start": datetime(2026, 3, 19, 10, 0, 0, tzinfo=timezone.utc),
            "window_end": datetime(2026, 3, 19, 10, 30, 0, tzinfo=timezone.utc),
            "auto_detected": True,
            "created_at": datetime(2026, 3, 19, 10, 31, 0, tzinfo=timezone.utc),
            "updated_at": datetime(2026, 3, 19, 10, 31, 0, tzinfo=timezone.utc),
        }
        base.update(overrides)
        return base

    def test_schema_serialization(self):
        data = self._make_event_dict()
        response = SurveillanceEventResponse(**data)
        assert response.type == SchemaType.SPOOFING
        assert response.severity == SchemaSeverity.HIGH
        assert response.status == SchemaStatus.OPEN
        assert response.auto_detected is True
        assert response.participants == ["user-uuid-1"]

    def test_schema_accepts_all_enum_variants(self):
        for t in SchemaType:
            for sev in SchemaSeverity:
                for st in SchemaStatus:
                    data = self._make_event_dict(type=t.value, severity=sev.value, status=st.value)
                    r = SurveillanceEventResponse(**data)
                    assert r.type == t
                    assert r.severity == sev
                    assert r.status == st

    def test_optional_fields_default_none(self):
        data = self._make_event_dict(
            participants=None,
            related_orders=None,
            related_trades=None,
            description=None,
            notes=None,
            window_start=None,
            window_end=None,
        )
        response = SurveillanceEventResponse(**data)
        assert response.description is None
        assert response.notes is None
        assert response.window_start is None
        assert response.window_end is None


class TestSurveillanceEventUpdateSchema:
    def test_update_schema_only_allows_status_and_notes(self):
        allowed = set(SurveillanceEventUpdate.model_fields.keys())
        assert allowed == {"status", "notes"}

    def test_update_with_status_only(self):
        update = SurveillanceEventUpdate(status=SchemaStatus.REVIEWING)
        assert update.status == SchemaStatus.REVIEWING
        assert update.notes is None

    def test_update_with_notes_only(self):
        update = SurveillanceEventUpdate(notes="Escalated to compliance team")
        assert update.notes == "Escalated to compliance team"
        assert update.status is None

    def test_empty_update_is_valid(self):
        # Both fields are optional — an empty payload is valid
        update = SurveillanceEventUpdate()
        assert update.status is None
        assert update.notes is None

    def test_update_does_not_accept_type_field(self):
        # Pydantic v2 ignores extra fields by default; ensure type is not a model field
        assert "type" not in SurveillanceEventUpdate.model_fields
        assert "severity" not in SurveillanceEventUpdate.model_fields
        assert "participants" not in SurveillanceEventUpdate.model_fields
