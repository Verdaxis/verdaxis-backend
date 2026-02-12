"""Unit tests for matchmaking router."""
import pytest
from app.models.notification import NotificationType


class TestMatchNotificationType:
    def test_match_suggestion_type_exists(self):
        assert NotificationType.MATCH_SUGGESTION.value == "MATCH_SUGGESTION"
