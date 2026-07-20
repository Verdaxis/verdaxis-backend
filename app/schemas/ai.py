"""Bounded request schemas for AI-assisted endpoints."""

from pydantic import BaseModel, Field, field_validator

MAX_AI_MESSAGE_CHARS = 4_000
MAX_AI_HISTORY_ITEMS = 20
MAX_AI_HISTORY_VALUE_CHARS = 2_000
MAX_AI_HISTORY_TOTAL_CHARS = 12_000


class AIChatRequest(BaseModel):
    message: str = Field(min_length=1, max_length=MAX_AI_MESSAGE_CHARS)
    history: list[dict[str, str]] = Field(default_factory=list, max_length=MAX_AI_HISTORY_ITEMS)

    @field_validator("history")
    @classmethod
    def validate_history_size(cls, history: list[dict[str, str]]) -> list[dict[str, str]]:
        total_chars = 0
        for item in history:
            if len(item) > 4:
                raise ValueError("history items may contain at most four fields")
            for key, value in item.items():
                if len(key) > 32:
                    raise ValueError("history field names may contain at most 32 characters")
                if len(value) > MAX_AI_HISTORY_VALUE_CHARS:
                    raise ValueError(
                        f"history field values may contain at most {MAX_AI_HISTORY_VALUE_CHARS} characters"
                    )
                total_chars += len(key) + len(value)
        if total_chars > MAX_AI_HISTORY_TOTAL_CHARS:
            raise ValueError(f"history may contain at most {MAX_AI_HISTORY_TOTAL_CHARS} characters in total")
        return history
