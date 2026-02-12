"""Test the MatchSuggestion model attributes exist."""
import pytest
from app.models.matchmaking import MatchSuggestion, MatchStatus


class TestMatchSuggestionModel:
    def test_has_required_columns(self):
        assert hasattr(MatchSuggestion, 'bid_order_id')
        assert hasattr(MatchSuggestion, 'ask_order_id')
        assert hasattr(MatchSuggestion, 'score')
        assert hasattr(MatchSuggestion, 'match_reasons')
        assert hasattr(MatchSuggestion, 'status')

    def test_match_status_values(self):
        assert MatchStatus.SUGGESTED.value == "SUGGESTED"
        assert MatchStatus.VIEWED.value == "VIEWED"
        assert MatchStatus.ACTED.value == "ACTED"
        assert MatchStatus.DISMISSED.value == "DISMISSED"
